//! The live MAVLink link — Kaze v0.2.
//!
//! Everything here is `std` plus [`crate::mavlink`]. The link owns a transport,
//! sends a 1 Hz HEARTBEAT, correlates COMMAND_LONGs with their COMMAND_ACKs, and
//! keeps the telemetry a script may read. It runs on its own thread so a script
//! that only computes a value still hears the vehicle.
//!
//! Two rules the code enforces:
//!
//! 1. **Only the commands the language can produce leave this process.**
//!    [`SENT_COMMANDS`] is a closed list; the R7 gate in [`crate::hardware`] has
//!    already judged the call above it, and this is the second lock.
//! 2. **Nothing is assumed about the vehicle.** `telemetry()` returns zeros until
//!    the autopilot has actually reported those fields, and `connect()` does not
//!    return until the vehicle has said hello and sent a position.

use std::collections::HashMap;
#[cfg(feature = "uart")]
use std::collections::VecDeque;
use std::io;
#[cfg(feature = "uart")]
use std::io::Write;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::mavlink::{self, Frame, Parser};

/// How often the receiver thread drains the transport (20 Hz).
pub const POLL_MS: u64 = 50;
/// How long a command waits for its COMMAND_ACK.
pub const ACK_TIMEOUT_MS: u64 = 3_000;
/// How long `connect` waits for the vehicle's first datagram.
pub const PEER_WAIT_MS: u64 = 1_500;
/// How long `connect` waits for the first position report after saying hello.
pub const WARMUP_MS: u64 = 15_000;
/// How long silence is tolerated before the link calls the vehicle gone. A
/// real autopilot streams at 1-4 Hz, so two seconds is generous.
pub const STALE_MS: u64 = 2_000;

/// The commands a Kaze script can put on the wire. Anything else is a bug, and
/// the link refuses it rather than trusting the caller.
pub const SENT_COMMANDS: &[u16] = &[
    mavlink::cmd::COMPONENT_ARM_DISARM,
    mavlink::cmd::NAV_TAKEOFF,
    mavlink::cmd::NAV_LAND,
    mavlink::cmd::NAV_RETURN_TO_LAUNCH,
    mavlink::cmd::NAV_LOITER_UNLIM,
    mavlink::cmd::DO_SET_MODE,
    mavlink::cmd::SET_MESSAGE_INTERVAL,
];

/// A byte pipe. `UdpIo` is a socket; `UartIo` is a serial device.
pub trait Io: Send {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

/// A UDP endpoint named by the operator. The device string names **our** endpoint:
/// the socket binds it and replies go to the source of the first datagram.
pub struct UdpIo {
    sock: UdpSocket,
    peer: Option<SocketAddr>,
}

impl UdpIo {
    pub fn connect(device: &str) -> io::Result<UdpIo> {
        let addr = device.trim().strip_prefix("udp:").unwrap_or(device.trim());
        let sock = UdpSocket::bind(addr)?;
        sock.set_nonblocking(true)?;
        Ok(UdpIo { sock, peer: None })
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.sock.local_addr().ok()
    }
}

impl Io for UdpIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self.peer {
            Some(peer) => self.sock.send_to(bytes, peer).map(|_| ()),
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "no peer yet: the vehicle has not sent anything to this endpoint",
            )),
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.sock.recv_from(buf) {
            Ok((n, from)) => {
                self.peer = Some(from);
                Ok(n)
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(e),
        }
    }
}

/// A serial device. The line discipline is set once with `stty` (raw, no echo at
/// the requested baud) and the device is then read on its own thread, because a
/// blocking read must never park the flight loop.
#[cfg(feature = "uart")]
pub struct UartIo {
    writer: std::fs::File,
    incoming: Receiver<Vec<u8>>,
    queued: VecDeque<u8>,
}

#[cfg(feature = "uart")]
impl UartIo {
    pub fn open(path: &str, baud: u32) -> io::Result<UartIo> {
        let status = std::process::Command::new("stty")
            .args(["-F", path, &baud.to_string(), "raw", "-echo", "-echoe", "-echok"])
            .status()?;
        if !status.success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("stty could not configure {} at {} baud", path, baud),
            ));
        }
        let writer = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        let mut reader = writer.try_clone()?;
        let (tx, rx) = channel::<Vec<u8>>();
        thread::Builder::new()
            .name(format!("kaze-uart-{}", path))
            .spawn(move || {
                let mut buf = [0u8; 2048];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => thread::sleep(Duration::from_millis(10)),
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                return;
                            }
                        }
                        Err(_) => return,
                    }
                }
            })?;
        Ok(UartIo { writer, incoming: rx, queued: VecDeque::new() })
    }
}

#[cfg(feature = "uart")]
use std::io::Read as _;

#[cfg(feature = "uart")]
impl Io for UartIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.queued.is_empty() {
            match self.incoming.recv_timeout(Duration::from_millis(5)) {
                Ok(chunk) => self.queued.extend(chunk),
                Err(_) => return Ok(0),
            }
        }
        let n = self.queued.len().min(buf.len());
        for slot in buf.iter_mut().take(n) {
            *slot = self.queued.pop_front().expect("length counted");
        }
        Ok(n)
    }
}

/// What the autopilot last reported. Every field starts at `None`: a script must
/// be able to tell "the vehicle says 0" from "the vehicle has not said".
#[derive(Clone, Debug, Default)]
pub struct Telemetry {
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub alt_m: Option<f64>,
    pub rel_alt_m: Option<f64>,
    pub heading_deg: Option<f64>,
    pub vx_ms: Option<f64>,
    pub vy_ms: Option<f64>,
    pub battery_pct: Option<f64>,
    pub satellites: Option<f64>,
    pub armed: Option<bool>,
    pub mode: Option<String>,
}

impl Telemetry {
    pub fn read(&self, field: &str) -> Option<f64> {
        match field {
            "battery" => self.battery_pct,
            "altitude" => self.rel_alt_m,
            "altitude_msl" => self.alt_m,
            "heading" => self.heading_deg,
            "lat" | "latitude" => self.lat,
            "lon" | "longitude" => self.lon,
            "speed" => match (self.vx_ms, self.vy_ms) {
                (Some(vx), Some(vy)) => Some((vx * vx + vy * vy).sqrt()),
                _ => None,
            },
            "satellites" => self.satellites,
            "armed" => self.armed.map(|a| if a { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn apply(&mut self, frame: &Frame) {
        match frame.msgid {
            0 => {
                let base = frame.u8_at(6);
                self.armed = Some(base & 0x80 != 0);
                self.mode = Some(copter_mode_name((frame.u32_at(0) & 0xFF) as u8));
            }
            1 => self.battery_pct = Some(frame.u8_at(30) as i8 as f64),
            24 => self.satellites = Some(frame.u8_at(29) as f64),
            30 => self.heading_deg = Some((frame.f32_at(12) as f64).to_degrees()),
            33 => {
                self.lat = Some(frame.i32_at(4) as f64 / 1e7);
                self.lon = Some(frame.i32_at(8) as f64 / 1e7);
                self.alt_m = Some(frame.i32_at(12) as f64 / 1000.0);
                self.rel_alt_m = Some(frame.i32_at(16) as f64 / 1000.0);
                self.vx_ms = Some(frame.i16_at(20) as f64 / 100.0);
                self.vy_ms = Some(frame.i16_at(22) as f64 / 100.0);
            }
            _ => {}
        }
    }
}

/// ArduCopter's custom-mode numbers, for the modes a script may see.
pub fn copter_mode_name(custom: u8) -> String {
    match custom {
        0 => "Stabilize",
        1 => "Acro",
        2 => "AltHold",
        3 => "Auto",
        4 => "Guided",
        5 => "Loiter",
        6 => "RTL",
        7 => "Circle",
        9 => "Land",
        11 => "Drift",
        16 => "PosHold",
        17 => "Brake",
        21 => "SmartRTL",
        22 => "FlowHold",
        23 => "Follow",
        24 => "ZigZag",
        25 => "SystemID",
        26 => "AutoTune",
        _ => "Unknown",
    }
    .to_string()
}

/// The GUIDED custom mode in ArduCopter.
pub const COPTER_MODE_GUIDED: u8 = 4;

/// Something the vehicle did, worth putting in the run's log.
#[derive(Clone, Debug)]
pub struct Note {
    pub level: String,
    pub text: String,
}

struct Request {
    command: u16,
    params: [f32; 7],
    frame: Option<Frame>,
    reply: Sender<Result<(), String>>,
}

/// One live link, owned by its receiver thread.
pub struct DroneLink {
    tx: Sender<Request>,
    tele: Arc<Mutex<Telemetry>>,
    notes: Arc<Mutex<Vec<Note>>>,
    stop: Arc<AtomicBool>,
    /// When the vehicle last said anything. Silence means the link is gone, not
    /// that commands should queue up behind an answer that will never come.
    last_rx: Arc<Mutex<Instant>>,
    handle: Option<thread::JoinHandle<()>>,
    ack_timeout_ms: u64,
    pub device: String,
}

impl DroneLink {
    pub fn connect(device: &str) -> Result<DroneLink, String> {
        let spec = parse_device(device)?;
        let is_udp = matches!(spec, DeviceSpec::Udp(_));
        let mut io = open_device(device)?;
        let mut core = Core::new(device);
        if is_udp {
            wait_for_first_bytes(io.as_mut(), PEER_WAIT_MS).map_err(|_| {
                format!("E_KAZE_LINK: nothing is streaming to {} — is the autopilot sending there?", device)
            })?;
        }
        if let Some(bytes) = core.tick(0) {
            io.write(&bytes)
                .map_err(|e| format!("E_KAZE_LINK: cannot write to {}: {}", device, e))?;
        }
        if is_udp {
            core.request_streams(io.as_mut())?;
            core.warm_up(io.as_mut(), WARMUP_MS).map_err(|why| {
                format!("E_KAZE_LINK: {} at {}", why, device)
            })?;
        }
        DroneLink::with_io(core, io, device.to_string(), POLL_MS, ACK_TIMEOUT_MS)
    }

    /// The seam tests use: any transport, any clock speed.
    pub fn with_io(
        mut core: Core,
        mut io: Box<dyn Io>,
        device: String,
        poll_ms: u64,
        ack_timeout_ms: u64,
    ) -> Result<DroneLink, String> {
        if let Some(bytes) = core.tick(0) {
            io.write(&bytes)
                .map_err(|e| format!("E_KAZE_LINK: cannot write to {}: {}", device, e))?;
        }
        let (tx, rx) = channel::<Request>();
        let tele = Arc::new(Mutex::new(core.tele.clone()));
        let notes = Arc::new(Mutex::new(Vec::<Note>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let last_rx = Arc::new(Mutex::new(Instant::now()));
        let handle = {
            let tele = Arc::clone(&tele);
            let last_rx = Arc::clone(&last_rx);
            let notes = Arc::clone(&notes);
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name(format!("kaze-link-{}", device))
                .spawn(move || {
                    run_link(core, io, rx, tele, notes, stop, last_rx, poll_ms, ack_timeout_ms)
                })
                .map_err(|e| format!("E_KAZE_LINK: cannot start the receiver thread: {}", e))?
        };
        Ok(DroneLink {
            tx,
            tele,
            notes,
            stop,
            last_rx,
            handle: Some(handle),
            ack_timeout_ms,
            device,
        })
    }

    /// Send a command and wait for the autopilot's answer.
    pub fn command(&self, command: u16, params: [f32; 7]) -> Result<(), String> {
        self.require_vehicle()?;
        if !SENT_COMMANDS.contains(&command) {
            return Err(format!("E_KAZE_LINK: command {} is not one Kaze sends", command));
        }
        let (reply_tx, reply_rx) = channel();
        self.tx
            .send(Request { command, params, frame: None, reply: reply_tx })
            .map_err(|_| "E_KAZE_LINK: the link thread is gone".to_string())?;
        let wait = Duration::from_millis(self.ack_timeout_ms.saturating_add(1_000));
        match reply_rx.recv_timeout(wait) {
            Ok(result) => result,
            Err(_) => Err(format!(
                "E_KAZE_LINK_TIMEOUT: the link never answered for {}",
                command_name(command)
            )),
        }
    }

    /// Send a frame the vehicle does not acknowledge (a position target).
    pub fn send_target(&self, frame: Frame) -> Result<(), String> {
        self.require_vehicle()?;
        let (reply_tx, reply_rx) = channel();
        self.tx
            .send(Request { command: 0, params: [0.0; 7], frame: Some(frame), reply: reply_tx })
            .map_err(|_| "E_KAZE_LINK: the link thread is gone".to_string())?;
        match reply_rx.recv_timeout(Duration::from_millis(self.ack_timeout_ms)) {
            Ok(result) => result,
            Err(_) => Err("E_KAZE_LINK_TIMEOUT: the link did not send the target".to_string()),
        }
    }

    pub fn position_frame(&self, lat: f64, lon: f64, alt: f32) -> Frame {
        mavlink::msg_set_position_target_global_int(
            0, 255, 191, 1, 1,
            (lat * 1e7) as i32,
            (lon * 1e7) as i32,
            alt,
            [0.0, 0.0, 0.0],
            0.0,
            0x0DF8,
            6,
        )
    }

    pub fn telemetry(&self) -> Telemetry {
        self.tele.clone().lock().map(|t| t.clone()).unwrap_or_default()
    }

    pub fn take_notes(&self) -> Vec<Note> {
        self.notes.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default()
    }

    /// Is the receiving thread still running?
    pub fn is_alive(&self) -> bool {
        self.handle.as_ref().map(|h| !h.is_finished()).unwrap_or(false)
    }

    /// Milliseconds since the vehicle last said anything.
    pub fn silence_ms(&self) -> u64 {
        self.last_rx
            .lock()
            .map(|at| at.elapsed().as_millis() as u64)
            .unwrap_or(u64::MAX)
    }

    /// Has the vehicle been heard from recently enough to be worth talking to?
    pub fn vehicle_alive(&self) -> bool {
        self.silence_ms() < STALE_MS
    }

    fn require_vehicle(&self) -> Result<(), String> {
        if self.vehicle_alive() {
            return Ok(());
        }
        Err(format!(
            "E_KAZE_LINK: nothing heard from the vehicle for {} ms",
            self.silence_ms()
        ))
    }
}

impl Drop for DroneLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The protocol core: sequence numbers, the heartbeat clock, the parser.
pub struct Core {
    pub device: String,
    pub parser: Parser,
    pub seq: u8,
    pub sysid: u8,
    pub compid: u8,
    pub heartbeat_period_ms: u64,
    pub last_heartbeat_ms: Option<u64>,
    /// What the vehicle said while `connect` was still waiting. Handed to the
    /// receiver thread, so a script that reads telemetry on its first line sees
    /// the position the vehicle already sent.
    pub tele: Telemetry,
    pending: HashMap<u16, Instant>,
}

impl Core {
    pub fn new(device: &str) -> Core {
        Core {
            device: device.to_string(),
            parser: Parser::new(),
            seq: 0,
            sysid: 255,
            compid: 191,
            heartbeat_period_ms: 1_000,
            last_heartbeat_ms: None,
            tele: Telemetry::default(),
            pending: HashMap::new(),
        }
    }

    fn next_seq(&mut self) -> u8 {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        seq
    }

    /// The heartbeat due at `now_ms`, if any. Runtime-scheduled, never from a script.
    pub fn tick(&mut self, now_ms: u64) -> Option<Vec<u8>> {
        let due = match self.last_heartbeat_ms {
            Some(t) => now_ms.saturating_sub(t) >= self.heartbeat_period_ms,
            None => true,
        };
        if !due {
            return None;
        }
        self.last_heartbeat_ms = Some(now_ms);
        let seq = self.next_seq();
        let hb = mavlink::msg_heartbeat(seq, self.sysid, self.compid);
        mavlink::encode(&hb).ok()
    }

    /// Encode one outgoing command. The caller has already decided it may be sent.
    pub fn command_frame(&mut self, command: u16, params: [f32; 7]) -> Frame {
        let frame = mavlink::msg_command_long(
            self.seq, self.sysid, self.compid, 1, 1, command, params, 0,
        );
        self.pending.insert(command, Instant::now());
        frame
    }

    /// Feed bytes and hand back the frames that decoded.
    pub fn on_bytes(&mut self, bytes: &[u8]) -> Vec<Frame> {
        self.parser.push(bytes);
        let mut out = Vec::new();
        while let Some(frame) = self.parser.next() {
            out.push(frame);
        }
        out
    }

    fn request_streams(&mut self, io: &mut dyn Io) -> Result<(), String> {
        // ArduPilot 4.6 removed the SRx_* stream parameters: a link that never
        // asks receives little more than HEARTBEAT.
        for (msgid, hz) in [(0u16, 1.0f32), (1, 2.0), (24, 4.0), (30, 4.0), (33, 4.0)] {
            let frame = self.command_frame(
                mavlink::cmd::SET_MESSAGE_INTERVAL,
                [msgid as f32, 1_000_000.0 / hz, 0.0, 0.0, 0.0, 0.0, 0.0],
            );
            let bytes = mavlink::encode(&frame)
                .map_err(|e| format!("E_KAZE_LINK: cannot encode the stream request: {:?}", e))?;
            io.write(&bytes)
                .map_err(|e| format!("E_KAZE_LINK: cannot write to the link: {}", e))?;
        }
        Ok(())
    }

    fn warm_up(&mut self, io: &mut dyn Io, timeout_ms: u64) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut buf = [0u8; 2048];
        let mut heard = false;
        while Instant::now() < deadline {
            match io.read(&mut buf) {
                Ok(0) => thread::sleep(Duration::from_millis(20)),
                Ok(n) => {
                    heard = true;
                    let frames = self.on_bytes(&buf[..n]);
                    for frame in &frames {
                        self.tele.apply(frame);
                    }
                    if frames.iter().any(|f| f.msgid == 33 || f.msgid == 24) {
                        return Ok(());
                    }
                }
                Err(_) => break,
            }
        }
        if heard {
            Err("the vehicle is talking but has not reported a position yet".to_string())
        } else {
            Err("nothing is streaming".to_string())
        }
    }
}

fn run_link(
    mut core: Core,
    mut io: Box<dyn Io>,
    rx: Receiver<Request>,
    tele: Arc<Mutex<Telemetry>>,
    notes: Arc<Mutex<Vec<Note>>>,
    stop: Arc<AtomicBool>,
    last_rx: Arc<Mutex<Instant>>,
    poll_ms: u64,
    ack_timeout_ms: u64,
) {
    let start = Instant::now();
    let mut pending: Option<(u16, Sender<Result<(), String>>, Instant)> = None;

    while !stop.load(Ordering::SeqCst) {
        let now = start.elapsed().as_millis() as u64;

        if let Some(bytes) = core.tick(now) {
            if io.write(&bytes).is_err() {
                break;
            }
        }

        let mut buf = [0u8; 2048];
        match io.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                if let Ok(mut at) = last_rx.lock() {
                    *at = Instant::now();
                }
                for frame in core.on_bytes(&buf[..n]) {
                    if frame.msgid == 77 {
                        let command = frame.u16_at(0);
                        let result = frame.u8_at(2);
                        if let Some((want, _, _)) = &pending {
                            if *want == command {
                                let (_, reply, _) = pending.take().expect("checked above");
                                let verdict = if result == 0 {
                                    Ok(())
                                } else {
                                    Err(format!(
                                        "E_COMMAND_REJECTED: the autopilot answered {} for {}",
                                        mav_result_name(result),
                                        command_name(command)
                                    ))
                                };
                                let _ = reply.send(verdict);
                            }
                        }
                    } else if frame.msgid == 253 {
                        let severity = frame.u8_at(0);
                        let text = statustext(&frame);
                        if !text.is_empty() {
                            let level = if severity <= 3 { "error" } else if severity <= 4 { "warning" } else { "info" };
                            if let Ok(mut notes) = notes.lock() {
                                notes.push(Note { level: level.to_string(), text });
                            }
                        }
                    } else if let Ok(mut t) = tele.lock() {
                        t.apply(&frame);
                    }
                }
            }
            Err(_) => break,
        }

        if pending.is_none() {
            if let Ok(request) = rx.try_recv() {
                if let Some(frame) = request.frame {
                    let sent = mavlink::encode(&frame)
                        .map_err(|e| format!("E_KAZE_LINK: cannot encode the target: {:?}", e));
                    let result = match sent {
                        Ok(bytes) => io.write(&bytes).map_err(|e| {
                            format!("E_KAZE_LINK: cannot write to the link: {}", e)
                        }),
                        Err(e) => Err(e),
                    };
                    let _ = request.reply.send(result);
                    continue;
                }
                let frame = core.command_frame(request.command, request.params);
                match mavlink::encode(&frame) {
                    Ok(bytes) => match io.write(&bytes) {
                        Ok(()) => pending = Some((request.command, request.reply, Instant::now())),
                        Err(e) => {
                            let _ = request.reply.send(Err(format!(
                                "E_KAZE_LINK: cannot write to the link: {}", e
                            )));
                        }
                    },
                    Err(e) => {
                        let _ = request.reply.send(Err(format!(
                            "E_KAZE_LINK: cannot encode the command: {:?}", e
                        )));
                    }
                }
            }
        }

        if let Some((_, _, sent_at)) = &pending {
            if sent_at.elapsed().as_millis() as u64 > ack_timeout_ms {
                let (command, reply, _) = pending.take().expect("checked above");
                let _ = reply.send(Err(format!(
                    "E_KAZE_LINK_TIMEOUT: no COMMAND_ACK for {} within {} ms",
                    command_name(command),
                    ack_timeout_ms
                )));
            }
        }

        thread::sleep(Duration::from_millis(poll_ms));
    }

    if let Some((_, reply, _)) = pending {
        let _ = reply.send(Err("E_KAZE_LINK: the link closed before the autopilot answered".to_string()));
    }
}

/// The printable name of a MAV_CMD the link may send.
pub fn command_name(command: u16) -> &'static str {
    match command {
        mavlink::cmd::COMPONENT_ARM_DISARM => "MAV_CMD_COMPONENT_ARM_DISARM",
        mavlink::cmd::NAV_TAKEOFF => "MAV_CMD_NAV_TAKEOFF",
        mavlink::cmd::NAV_LAND => "MAV_CMD_NAV_LAND",
        mavlink::cmd::NAV_RETURN_TO_LAUNCH => "MAV_CMD_NAV_RETURN_TO_LAUNCH",
        mavlink::cmd::NAV_LOITER_UNLIM => "MAV_CMD_NAV_LOITER_UNLIM",
        mavlink::cmd::DO_SET_MODE => "MAV_CMD_DO_SET_MODE",
        mavlink::cmd::SET_MESSAGE_INTERVAL => "MAV_CMD_SET_MESSAGE_INTERVAL",
        _ => "the command",
    }
}

/// MAV_RESULT names, for the refusal a script gets to read.
pub fn mav_result_name(result: u8) -> &'static str {
    match result {
        0 => "ACCEPTED",
        1 => "TEMPORARILY_REJECTED",
        2 => "DENIED",
        3 => "UNSUPPORTED",
        4 => "FAILED",
        5 => "IN_PROGRESS",
        6 => "CANCELLED",
        _ => "UNKNOWN",
    }
}

fn statustext(frame: &Frame) -> String {
    let raw = &frame.payload[1..51.min(frame.payload.len())];
    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).trim().to_string()
}

/// What a `--cap drone=<device>` string asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceSpec {
    /// `udp:HOST:PORT` — the local endpoint the link binds.
    Udp(String),
    /// `uart:/dev/ttyAMA0@57600` — a serial device and its baud rate.
    Uart { path: String, baud: u32 },
}

/// The one place a device string is interpreted, so a typo is a named error
/// rather than a socket that never connects.
pub fn parse_device(device: &str) -> Result<DeviceSpec, String> {
    let trimmed = device.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(addr) = lower.strip_prefix("udp:") {
        let _ = addr;
        let addr = trimmed[4..].trim().to_string();
        if addr.is_empty() || !addr.contains(':') {
            return Err(format!(
                "E_KAZE_LINK: '{}' is not udp:HOST:PORT (for example udp:127.0.0.1:14550)",
                trimmed
            ));
        }
        return Ok(DeviceSpec::Udp(addr));
    }
    if lower.starts_with("uart:") {
        let rest = trimmed[5..].trim();
        let (path, baud) = match rest.rsplit_once('@') {
            Some((path, baud)) => {
                let baud: u32 = baud.trim().parse().map_err(|_| {
                    format!("E_KAZE_LINK: '{}' does not end in a baud rate", trimmed)
                })?;
                (path.trim().to_string(), baud)
            }
            None => (rest.to_string(), 57_600),
        };
        if path.is_empty() {
            return Err(
                "E_KAZE_LINK: uart: needs a device path, for example uart:/dev/ttyAMA0@57600"
                    .to_string(),
            );
        }
        return Ok(DeviceSpec::Uart { path, baud });
    }
    Err(format!(
        "E_KAZE_LINK: '{}' is neither udp:HOST:PORT nor uart:/dev/device@baud",
        trimmed
    ))
}

fn open_device(device: &str) -> Result<Box<dyn Io>, String> {
    match parse_device(device)? {
        DeviceSpec::Udp(addr) => UdpIo::connect(&format!("udp:{}", addr))
            .map(|io| Box::new(io) as Box<dyn Io>)
            .map_err(|e| format!("E_KAZE_LINK: cannot open udp:{}: {}", addr, e)),
        DeviceSpec::Uart { path, baud } => {
            #[cfg(feature = "uart")]
            {
                UartIo::open(&path, baud)
                    .map(|io| Box::new(io) as Box<dyn Io>)
                    .map_err(|e| {
                        format!("E_KAZE_LINK: cannot open {} at {} baud: {}", path, baud, e)
                    })
            }
            #[cfg(not(feature = "uart"))]
            {
                let _ = baud;
                Err(format!(
                    "E_KAZE_LINK: {} needs serial support — rebuild with --features uart (UDP works as built)",
                    path
                ))
            }
        }
    }
}

fn wait_for_first_bytes(io: &mut dyn Io, timeout_ms: u64) -> Result<(), ()> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        match io.read(&mut buf) {
            Ok(0) => thread::sleep(Duration::from_millis(20)),
            Ok(_) => return Ok(()),
            Err(_) => return Err(()),
        }
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::Frame;

    #[test]
    fn a_udp_device_string_is_its_bind_address() {
        assert_eq!(
            parse_device("udp:127.0.0.1:14550").expect("parses"),
            DeviceSpec::Udp("127.0.0.1:14550".to_string())
        );
        assert_eq!(
            parse_device("  UDP:0.0.0.0:14550 ").expect("case and spaces are forgiven"),
            DeviceSpec::Udp("0.0.0.0:14550".to_string())
        );
    }

    #[test]
    fn a_udp_string_without_a_port_is_refused_by_name() {
        let err = parse_device("udp:127.0.0.1").expect_err("no port");
        assert!(err.contains("is not udp:HOST:PORT"), "{err}");
        let err = parse_device("udp:").expect_err("no address");
        assert!(err.contains("is not udp:HOST:PORT"), "{err}");
    }

    #[test]
    fn a_uart_device_string_carries_its_baud() {
        assert_eq!(
            parse_device("uart:/dev/ttyAMA0@57600").expect("parses"),
            DeviceSpec::Uart { path: "/dev/ttyAMA0".to_string(), baud: 57_600 }
        );
        assert_eq!(
            parse_device("uart:/dev/ttyUSB0").expect("the baud has a default"),
            DeviceSpec::Uart { path: "/dev/ttyUSB0".to_string(), baud: 57_600 }
        );
    }

    #[test]
    fn a_uart_string_with_nonsense_is_refused_by_name() {
        let err = parse_device("uart:/dev/ttyUSB0@fast").expect_err("not a baud");
        assert!(err.contains("does not end in a baud rate"), "{err}");
        let err = parse_device("uart:@57600").expect_err("no path");
        assert!(err.contains("needs a device path"), "{err}");
    }

    #[test]
    fn an_unknown_transport_is_refused_instead_of_guessed() {
        let err = parse_device("can:0").expect_err("not a transport");
        assert!(err.contains("neither udp:HOST:PORT nor uart"), "{err}");
    }

    #[test]
    fn telemetry_only_reports_what_the_vehicle_sent() {
        let mut tele = Telemetry::default();
        assert_eq!(tele.read("altitude"), None, "silence is not zero");
        assert_eq!(tele.read("armed"), None);

        let mut hb = Frame::new(0, 0, 7, 1).expect("heartbeat");
        hb.set_u32_at(0, 4);
        hb.set_u8_at(6, 0x80 | 0x51);
        tele.apply(&hb);
        assert_eq!(tele.read("armed"), Some(1.0));
        assert_eq!(tele.mode.as_deref(), Some("Guided"));

        let mut pos = Frame::new(33, 1, 7, 1).expect("position");
        pos.set_i32_at(4, 476_000_000);
        pos.set_i32_at(8, 85_000_000);
        pos.set_i32_at(12, 500_000);
        pos.set_i32_at(16, 120_000);
        pos.set_i16_at(20, 300);
        pos.set_i16_at(22, 400);
        tele.apply(&pos);
        assert!((tele.read("lat").expect("lat") - 47.6).abs() < 1e-9);
        assert!((tele.read("altitude").expect("alt") - 120.0).abs() < 1e-6);
        assert!((tele.read("speed").expect("speed") - 5.0).abs() < 1e-6);
    }

    #[test]
    fn a_statustext_is_read_as_a_string() {
        let mut frame = Frame::new(253, 0, 7, 1).expect("statustext");
        frame.set_u8_at(0, 4);
        for (i, byte) in b"PreArm: hardware safety switch".iter().enumerate() {
            frame.set_u8_at(1 + i, *byte);
        }
        assert_eq!(statustext(&frame), "PreArm: hardware safety switch");
    }

    struct Silent;

    impl Io for Silent {
        fn write(&mut self, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    #[test]
    fn a_silent_vehicle_makes_the_link_refuse_instead_of_hanging() {
        let link = DroneLink::with_io(
            Core::new("silent"),
            Box::new(Silent),
            "silent".to_string(),
            10,
            100,
        )
        .expect("the thread starts");
        assert!(link.vehicle_alive(), "a fresh link assumes nothing yet is wrong");

        std::thread::sleep(Duration::from_millis(STALE_MS + 300));

        let err = link
            .command(crate::mavlink::cmd::COMPONENT_ARM_DISARM, [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
            .expect_err("silence is not consent");
        assert!(err.contains("nothing heard from the vehicle"), "{err}");
        assert!(!link.vehicle_alive());
    }

    #[test]
    fn the_command_allowlist_is_closed() {
        assert!(SENT_COMMANDS.contains(&crate::mavlink::cmd::COMPONENT_ARM_DISARM));
        assert!(!SENT_COMMANDS.contains(&crate::mavlink::cmd::PARAM_SET));
        assert!(!SENT_COMMANDS.contains(&crate::mavlink::cmd::DO_MOTOR_TEST));
    }
}

