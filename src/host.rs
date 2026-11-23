//! The live capabilities — Kaze v0.2.
//!
//! [`LiveHost`] routes to explicitly configured backends:
//!
//! * `drone` → a live MAVLink link ([`crate::link`])
//! * `device` → Linux GPIO lines ([`crate::gpio`])
//! * `drone` / `rover` → simulator only when explicitly selected for testing
//! * `log` / `policy` → local logging and detection
//! * other unavailable families → explicit refusal
//!
//! The R7 gate in [`crate::hardware`] has already judged every call before it
//! arrives here (declared, granted, in range). This module only decides *which*
//! backend answers, and refuses to invent a device that was not named.

use std::io::Write;

use crate::caps::{render_value, Action, CapHost, SimHost, SimState};
use crate::gpio::GpioBus;
use crate::link::{
    command_name, copter_mode_name, DroneLink, Telemetry, COPTER_MODE_GUIDED,
    SAFETY_FRESHNESS_MS,
};
use crate::mavlink;
use crate::value::{Heap, Value};
use crate::arena::Arena;

/// The ceiling one alert record's message may occupy, in bytes. Shared with
/// `caps::MAX_ALERT_BYTES` so the language-level constant and the sink cannot
/// drift apart.
pub const MAX_ALERT_BYTES: usize = crate::caps::MAX_ALERT_BYTES;

/// The families with a real in-process implementation. Everything else must
/// have a configured live backend or be refused; nothing falls through to the
/// simulator by accident.
const IN_PROCESS_FAMILIES: &[&str] = &["log", "policy"];

/// A real alert destination selected with `--cap alert=stdout|stderr|file:<path>`.
///
/// Every record is one line: a UTC timestamp, the word `alert`, then the
/// message with its credentials redacted and its length capped at
/// [`MAX_ALERT_BYTES`]. A message that does not fit is truncated and marked,
/// never dropped: an alert that vanishes is worse than a long one.
#[derive(Debug)]
pub struct AlertSink {
    target: AlertTarget,
    label: String,
}

#[derive(Debug)]
enum AlertTarget {
    Stdout,
    Stderr,
    File(std::fs::File),
}

impl AlertSink {
    /// Interpret the device half of `--cap alert=<spec>`. A sink that cannot be
    /// opened is refused here, before the script runs, with the fix named.
    pub fn parse(spec: &str) -> Result<AlertSink, String> {
        let spec = spec.trim();
        if spec.eq_ignore_ascii_case("stdout") {
            return Ok(AlertSink { target: AlertTarget::Stdout, label: "stdout".to_string() });
        }
        if spec.eq_ignore_ascii_case("stderr") {
            return Ok(AlertSink { target: AlertTarget::Stderr, label: "stderr".to_string() });
        }
        if let Some(path) = spec.strip_prefix("file:") {
            let path = path.trim();
            if path.is_empty() {
                return Err(
                    "E_CAP_MISSING: 'alert=file:' needs a path, for example --cap alert=file:alerts.log"
                        .to_string(),
                );
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| {
                    format!(
                        "E_CAP_MISSING: cannot open the alert file '{}': {} (fix the path, or use --cap alert=stdout)",
                        path, e
                    )
                })?;
            return Ok(AlertSink {
                target: AlertTarget::File(file),
                label: format!("file:{}", path),
            });
        }
        Err(format!(
            "E_CAP_MISSING: '{}' is not an alert sink; use --cap alert=stdout, alert=stderr, or alert=file:<path>",
            spec
        ))
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// Write one record and return once the bytes are flushed. A sink that
    /// cannot accept the record is a failure, never a silent success.
    pub fn send(&mut self, message: &str) -> Result<(), String> {
        let line = format!("{} alert {}\n", crate::caps::utc_timestamp(), capped_alert_body(message));
        let label = self.label.clone();
        match &mut self.target {
            AlertTarget::Stdout => {
                let mut out = std::io::stdout().lock();
                out.write_all(line.as_bytes())
                    .and_then(|_| out.flush())
                    .map_err(|e| format!("E_KAZE_HOST: cannot write the alert to stdout: {}", e))
            }
            AlertTarget::Stderr => {
                let mut out = std::io::stderr().lock();
                out.write_all(line.as_bytes())
                    .and_then(|_| out.flush())
                    .map_err(|e| format!("E_KAZE_HOST: cannot write the alert to stderr: {}", e))
            }
            AlertTarget::File(file) => file
                .write_all(line.as_bytes())
                .and_then(|_| file.flush())
                .map_err(|e| format!("E_KAZE_HOST: cannot write the alert to {}: {}", label, e)),
        }
    }
}

/// The redacted, size-capped body of one alert record. Redaction runs first so
/// truncation can never expose the tail of a credential the redactor removed.
pub fn capped_alert_body(message: &str) -> String {
    let redacted = crate::caps::redact_secrets(message);
    if redacted.len() <= MAX_ALERT_BYTES {
        return redacted;
    }
    const MARKER: &str = " …[truncated]";
    let (head, _) = crate::caps::truncate_utf8(&redacted, MAX_ALERT_BYTES - MARKER.len());
    let mut body = String::with_capacity(MAX_ALERT_BYTES);
    body.push_str(head);
    body.push_str(MARKER);
    body
}

/// A host with live backends.
pub struct LiveHost {
    sim: SimHost,
    link: Option<DroneLink>,
    drone_device: Option<String>,
    sim_families: Vec<String>,
    gpio: Option<GpioBus>,
    pwm: Option<crate::pwm::PwmBus>,
    alerts: Option<AlertSink>,
}

impl LiveHost {
    /// `backends` is `family -> device` from `--cap`; `pins` is `name -> line`
    /// from `--pin`. A simulator is available only for an explicit robotics
    /// selection. Real links are opened lazily, after the VM authorises a call.
    pub fn new(
        sim: SimState,
        backends: &[(String, String)],
        pins: &[(String, u16)],
        pwm: &[(String, u16, u16)],
    ) -> Result<LiveHost, String> {
        let mut drone_device = None;
        let mut sim_families = Vec::new();
        let mut alerts = None;
        let mut configured = Vec::new();
        for (family, device) in backends {
            let device = device.trim();
            let family = crate::caps::family_of(family);
            if configured.contains(&family) {
                return Err(format!("E_CAP_DENIED: more than one backend was specified for '{}'", family));
            }
            configured.push(family);
            if device.is_empty() {
                return Err(format!("E_CAP_MISSING: no backend was specified for '{}'", family));
            }
            if device == "sim" {
                if !matches!(family, "drone" | "rover") {
                    return Err(format!(
                        "E_CAP_MISSING: simulation is only available for explicit drone or rover tests, not '{}'",
                        family
                    ));
                }
                sim_families.push(family.to_string());
                continue;
            }
            match family {
                "drone" => {
                    crate::link::parse_device(device)?;
                    drone_device = Some(device.to_string());
                }
                // A device family with no named pin or PWM channel has nothing to drive,
                // and falling back to the simulator would let a script report
                // success for a relay or fan that does not exist.
                "device" => {
                    if !matches!(device, "gpio" | "pwm") {
                        return Err(format!("E_CAP_MISSING: unknown device backend '{}'; use gpio or pwm with named mappings", device));
                    }
                    if pins.is_empty() && pwm.is_empty() {
                        return Err(format!(
                            "E_CAP_MISSING: --cap device={} needs at least one --pin name=<line> or --pwm name=<chip>:<channel>",
                            device
                        ));
                    }
                }
                // An alert sink is a real output channel: stdout, stderr, or a
                // file. It is opened here so a bad path fails before the script
                // runs rather than on the first emergency it has to report.
                "alert" => {
                    alerts = Some(AlertSink::parse(device)?);
                }
                other => {
                    return Err(format!(
                        "E_CAP_MISSING: the '{}' family has no configured live backend (drone, device and alert do)",
                        other
                    ));
                }
            }
        }
        let gpio = if pins.is_empty() { None } else { Some(GpioBus::new(pins, None)) };
        let pwm_bus = if pwm.is_empty() { None } else { Some(crate::pwm::PwmBus::new(pwm, None)) };
        Ok(LiveHost {
            sim: SimHost::new(sim),
            link: None,
            drone_device,
            sim_families,
            gpio,
            pwm: pwm_bus,
            alerts,
        })
    }

    /// A host around an already-open link (tests drive the real command path
    /// without a socket).
    pub fn with_link(sim: SimState, link: DroneLink) -> LiveHost {
        LiveHost {
            sim: SimHost::new(sim),
            link: Some(link),
            drone_device: None,
            sim_families: Vec::new(),
            gpio: None,
            pwm: None,
            alerts: None,
        }
    }

    pub fn is_live(&self) -> bool {
        self.link.is_some() || self.drone_device.is_some() || self.gpio.is_some() || self.pwm.is_some()
    }

    pub fn device(&self) -> Option<&str> {
        self.link.as_ref().map(|l| l.device.as_str()).or(self.drone_device.as_deref())
    }

    pub fn gpio(&self) -> Option<&GpioBus> {
        self.gpio.as_ref()
    }

    fn drain_notes(&mut self) {
        if let Some(link) = &self.link {
            for note in link.take_notes() {
                self.sim.logs.push((note.level, note.text));
            }
        }
    }

    fn ensure_drone_link(&mut self) -> Result<(), String> {
        if self.link.is_none() {
            let device = self.drone_device.as_deref().ok_or_else(|| {
                "E_CAP_MISSING: no live drone backend is configured; select a device or explicitly select drone=sim for testing".to_string()
            })?;
            self.link = Some(DroneLink::connect(device)?);
        }
        Ok(())
    }
}

impl CapHost for LiveHost {
    fn call(
        &mut self,
        family: &str,
        op: &str,
        args: &[Value],
        arena: &mut Arena,
        heap: &mut Heap,
    ) -> Result<Value, String> {
        if family == "drone" {
            if self.link.is_none() && self.drone_device.is_none() {
                if self.sim_families.iter().any(|f| f == "drone") {
                    return self.sim.call(family, op, args, arena, heap);
                }
                return Err("E_CAP_MISSING: no drone backend is configured; select a live device or drone=sim for testing".to_string());
            }
            self.ensure_drone_link()?;
            if let Some(mut link) = self.link.take() {
                let shown: Vec<String> =
                    args.iter().map(|v| render_value(*v, arena, heap)).collect();
                // The 1500 ms freshness veto runs before every verb: a command
                // computed from a stale world model is never put on the wire.
                // `E_SAFETY_VIOLATION` is not catchable, so a script cannot
                // recover from a stale link and retry with the same stale model.
                let result = if op == "telemetry" {
                    drone_op(&mut link, op, args, arena, heap)
                } else {
                    match live_freshness_veto(&link.telemetry(), op, args, heap) {
                        Ok(()) => drone_op(&mut link, op, args, arena, heap),
                        Err(veto) => {
                            let _ = crate::hardware::record_safety_veto("drone", op, &veto);
                            Err(veto)
                        }
                    }
                };
                self.link = Some(link);
                let rendered = match &result {
                    Ok(value) => render_value(*value, arena, heap),
                    Err(e) => format!("error: {}", e),
                };
                self.sim.note("drone", op, &shown, rendered);
                self.drain_notes();
                return result;
            }
            return Err("E_KAZE_HOST_UNCLASSIFIED: the authorised drone link is unavailable".to_string());
        }

        // `send alert`: one redacted, size-capped line to the sink the operator
        // selected. With no sink the call fails closed rather than pretending to
        // have told anyone.
        if family == "alert" {
            let shown: Vec<String> = args.iter().map(|v| render_value(*v, arena, heap)).collect();
            let message = match args.first() {
                Some(Value::Str(s)) => arena.get(*s).to_string(),
                Some(other) => render_value(*other, arena, heap),
                None => String::new(),
            };
            let result = match self.alerts.as_mut() {
                Some(sink) => sink.send(&message).map(|_| Value::Unit),
                None => Err(
                    "E_CAP_MISSING: no alert sink is configured; add --cap alert=stdout, --cap alert=stderr, or --cap alert=file:<path>"
                        .to_string(),
                ),
            };
            let rendered = match &result {
                Ok(value) => render_value(*value, arena, heap),
                Err(e) => format!("error: {}", e),
            };
            // The record is redacted, so the trace is too: an alert's text is
            // the payload that leaves the process, and the run report is no
            // place to print the credential the sink just removed.
            let trace_args: Vec<String> =
                shown.iter().map(|arg| crate::caps::redact_secrets(arg)).collect();
            self.sim.note("alert", op, &trace_args, rendered);
            return result;
        }

        // PWM belongs to the device family but has its own backend, so it is
        // matched before the GPIO bus.
        if family == "device" && op == "set_duty" {
            let shown: Vec<String> = args.iter().map(|v| render_value(*v, arena, heap)).collect();
            let name = args
                .first()
                .map(|v| render_value(*v, arena, heap))
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            let percent = args.get(1).and_then(|v| v.as_num()).unwrap_or(f64::NAN);
            let result = if let Some(mut bus) = self.pwm.take() {
                let res = bus.set_duty(&name, percent).map(|_| Value::Unit);
                self.pwm = Some(bus);
                res
            } else {
                Err(format!(
                    "E_CAP_MISSING: no PWM channel is configured for '{}' (--pwm {}=<chip>:<channel>)",
                    name, name
                ))
            };
            let rendered = match &result {
                Ok(value) => render_value(*value, arena, heap),
                Err(e) => format!("error: {}", e),
            };
            self.sim.note("device", op, &shown, rendered);
            return result;
        }

        if family == "device" {
            if let Some(mut bus) = self.gpio.take() {
                let shown: Vec<String> =
                    args.iter().map(|v| render_value(*v, arena, heap)).collect();
                let result = device_op(&mut bus, op, args, arena, heap);
                self.gpio = Some(bus);
                let rendered = match &result {
                    Ok(value) => render_value(*value, arena, heap),
                    Err(e) => format!("error: {}", e),
                };
                self.sim.note("device", op, &shown, rendered);
                return result;
            }
            return Err("E_CAP_MISSING: no GPIO backend is configured; provide a named --pin".to_string());
        }

        if family == "rover" {
            if self.sim_families.iter().any(|f| f == "rover") {
                return self.sim.call(family, op, args, arena, heap);
            }
            return Err("E_CAP_MISSING: no live rover backend is available; rover=sim is only for explicit testing".to_string());
        }
        // Only families with a real in-process implementation may use the
        // in-process host. Everything else is refused here by name, so a family
        // that later gains a simulator arm cannot start fabricating answers.
        if !IN_PROCESS_FAMILIES.contains(&family) {
            return Err(format!(
                "E_CAP_MISSING: the '{}' family has no configured live backend; {}.{} is refused rather than fabricated",
                family, family, op
            ));
        }
        self.sim.call(family, op, args, arena, heap)
    }

    fn actions(&self) -> &[Action] {
        self.sim.actions()
    }

    fn logs(&self) -> &[(String, String)] {
        self.sim.logs()
    }

    fn advance(&mut self, now_ms: u64) {
        // A live link measures silence from its receive thread in wall-clock
        // time; the simulated link needs the virtual tick clock.
        self.sim.advance(now_ms);
    }

    fn link_silent_ms(&self) -> Option<u64> {
        match &self.link {
            Some(link) => Some(link.silence_ms()),
            None if self.sim_families.iter().any(|f| f == "drone") => self.sim.link_silent_ms(),
            None => None,
        }
    }

    fn read_link_silence_ms(&mut self) -> Result<Option<u64>, String> {
        if self.sim_families.iter().any(|f| f == "drone") {
            return self.sim.read_link_silence_ms();
        }
        self.ensure_drone_link()?;
        let link = self.link.as_ref().ok_or_else(|| {
            "E_KAZE_HOST_UNCLASSIFIED: the authorised drone link is unavailable".to_string()
        })?;
        Ok(Some(link.silence_ms()))
    }
}

/// The telemetry a live actuation command's decision depends on, beyond the
/// HEARTBEAT. Kept in step with [`drone_op`] and [`resolve_drone_target`],
/// which read exactly these fields to fill a partial target; the freshness
/// unit tests walk the same table so the two cannot drift.
pub fn dependent_fields(op: &str, args: &[Value], heap: &Heap) -> &'static [&'static str] {
    match op {
        // A GUIDED takeoff starts from the position estimate the vehicle will
        // fly from; without one the autopilot either refuses or flies blind.
        "takeoff" => &["latitude", "longitude", "altitude"],
        "goto" => match args.first() {
            None => &[],
            Some(Value::Vec(v)) => {
                if args.len() >= 2 {
                    &[]
                } else {
                    match heap.vec3(*v) {
                        Some(point) if point[2] != 0.0 => &[],
                        _ => &["altitude"],
                    }
                }
            }
            Some(_) => match args.len() {
                1 => &["longitude", "altitude"],
                2 => &["longitude"],
                _ => &[],
            },
        },
        // arm, land, rtl, hold and stop are judged on the HEARTBEAT alone.
        _ => &[],
    }
}

/// The 1500 ms physical-effector gate.
///
/// Returns `E_SAFETY_VIOLATION` when the vehicle's HEARTBEAT is older than
/// [`SAFETY_FRESHNESS_MS`] or a field the command depends on is stale or was
/// never reported. The code is deliberately one the VM refuses to catch: a
/// script may not recover from a stale link and retry with the same stale
/// world model.
pub fn live_freshness_veto(
    tele: &Telemetry,
    op: &str,
    args: &[Value],
    heap: &Heap,
) -> Result<(), String> {
    match tele.heartbeat_age_ms() {
        Some(age) if age <= SAFETY_FRESHNESS_MS => {}
        Some(age) => {
            return Err(format!(
                "E_SAFETY_VIOLATION: drone.{} refused: the last HEARTBEAT is {} ms old (limit {} ms); the vehicle is not under control",
                op, age, SAFETY_FRESHNESS_MS
            ))
        }
        None => {
            return Err(format!(
                "E_SAFETY_VIOLATION: drone.{} refused: no HEARTBEAT has been received (limit {} ms); nothing confirms the vehicle is alive",
                op, SAFETY_FRESHNESS_MS
            ))
        }
    }
    for field in dependent_fields(op, args, heap) {
        match tele.field_age_ms(field) {
            Some(age) if age <= SAFETY_FRESHNESS_MS => {}
            Some(age) => {
                return Err(format!(
                    "E_SAFETY_VIOLATION: drone.{} refused: '{}' telemetry is {} ms old (limit {} ms); the command would act on a stale world model",
                    op, field, age, SAFETY_FRESHNESS_MS
                ))
            }
            None => {
                return Err(format!(
                    "E_SAFETY_VIOLATION: drone.{} refused: '{}' telemetry has not been reported (limit {} ms); the command would act on a stale world model",
                    op, field, SAFETY_FRESHNESS_MS
                ))
            }
        }
    }
    Ok(())
}

/// One Kaze drone verb, one gated MAVLink command.
fn drone_op(
    link: &mut DroneLink,
    op: &str,
    args: &[Value],
    arena: &mut Arena,
    heap: &Heap,
) -> Result<Value, String> {
    let num = |i: usize| -> Result<f64, String> {
        args.get(i).and_then(Value::as_num).ok_or_else(|| {
            format!("E_KAZE_TYPE: drone.{} needs a number for argument {}", op, i + 1)
        })
    };
    match op {
        "arm" => {
            // A refusal is the autopilot's own word, so pass it on with the
            // STATUSTEXT that usually explains it.
            if let Err(why) =
                link.command(mavlink::cmd::COMPONENT_ARM_DISARM, [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
            {
                let why = match link.take_notes().pop() {
                    Some(note) => format!("{} ({})", why, note.text),
                    None => why,
                };
                return Err(why);
            }
            // ACCEPTED is the autopilot agreeing to try, not the motors turning:
            // wait for the heartbeat to say the vehicle is actually armed, and
            // pass on the vehicle's own reason when it is not.
            for _ in 0..60 {
                if link.telemetry().armed == Some(true) {
                    return Ok(Value::Unit);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let why = link
                .take_notes()
                .pop()
                .map(|note| format!(" ({})", note.text))
                .unwrap_or_default();
            Err(format!(
                "E_ARMING_DENIED: the vehicle accepted the arm command but never armed{}",
                why
            ))
        }
        "takeoff" => {
            let alt = num(0)?;
            ensure_guided(link)?;
            link.command(
                mavlink::cmd::NAV_TAKEOFF,
                [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, alt as f32],
            )
            .map(|_| Value::Unit)
        }
        // goto(lat, lon, alt) is the capability signature; goto(point, alt) is the
        // sentence form. A point index has no meaning to a real vehicle, so a
        // two-argument call treats the first as latitude — the same reading the
        // simulator uses.
        //
        // The target is a position setpoint, not MAV_CMD_NAV_WAYPOINT: measured
        // against ArduCopter, NAV_WAYPOINT is answered UNSUPPORTED outside AUTO
        // mode, while a GUIDED position target flies the same route. A mission
        // variant (AUTO + NAV_WAYPOINT) belongs with mission upload, later.
        "goto" => {
            let tele = link.telemetry();
            let (lat, lon, alt) = resolve_drone_target(args, &tele, heap)?;
            ensure_guided(link)?;
            let frame = link.position_frame(lat, lon, alt as f32);
            link.send_target(frame).map(|_| Value::Unit)
        }
        "rtl" => link.command(mavlink::cmd::NAV_RETURN_TO_LAUNCH, [0.0; 7]).map(|_| Value::Unit),
        "land" => link.command(mavlink::cmd::NAV_LAND, [0.0; 7]).map(|_| Value::Unit),
        "hold" | "stop" => link.command(mavlink::cmd::NAV_LOITER_UNLIM, [0.0; 7]).map(|_| Value::Unit),
        "telemetry" => {
            let field = args
                .first()
                .map(|v| render_value(*v, arena, heap))
                .unwrap_or_default();
            let field = field.trim_matches('"').to_string();
            // A reading the vehicle has not sent yet is not a zero, and a
            // reading it sent a while ago is not the present: wait for a value
            // inside the 1500 ms window, then say so instead of inventing one.
            for _ in 0..40 {
                if let Some(value) = link.fresh_field(&field, SAFETY_FRESHNESS_MS) {
                    return Ok(Value::Num(value));
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(format!(
                "E_KAZE_LINK: the vehicle has not reported '{}' within {} ms",
                field, SAFETY_FRESHNESS_MS
            ))
        }
        other => Err(format!("E_KAZE_LINK: the live drone link has no '{}'", other)),
    }
}

/// Resolve partial targets only from actual telemetry, never invented values.
fn resolve_drone_target(
    args: &[Value],
    tele: &crate::link::Telemetry,
    heap: &Heap,
) -> Result<(f64, f64, f64), String> {
    let number = |i: usize| -> Result<f64, String> {
        args.get(i).and_then(Value::as_num).ok_or_else(|| {
            format!("E_KAZE_TYPE: drone.goto needs a number for argument {}", i + 1)
        })
    };
    let live = |value: Option<f64>, field: &str| -> Result<f64, String> {
        value.ok_or_else(|| format!("E_KAZE_LINK: drone.goto needs current {} telemetry", field))
    };
    let target = if let Some(Value::Vec(v)) = args.first() {
        let point = heap.vec3(*v).ok_or_else(|| "E_KAZE_BADCODE: target point is missing".to_string())?;
        let altitude = if args.len() >= 2 {
            number(1)?
        } else if point[2] != 0.0 {
            point[2]
        } else {
            live(tele.rel_alt_m, "altitude")?
        };
        (point[0], point[1], altitude)
    } else if args.len() >= 3 {
        (number(0)?, number(1)?, number(2)?)
    } else if args.len() == 2 {
        (number(0)?, live(tele.lon, "longitude")?, number(1)?)
    } else {
        (number(0)?, live(tele.lon, "longitude")?, live(tele.rel_alt_m, "altitude")?)
    };
    let values = [Value::Num(target.0), Value::Num(target.1), Value::Num(target.2)];
    crate::hardware::validate_arguments("drone", "goto", &values, heap)
        .map_err(|d| format!("{}: {} ({})", d.code, d.reason, d.rule))?;
    Ok(target)
}

/// GUIDED is a precondition for takeoff and waypoints in ArduCopter, so switch
/// the vehicle there when it is somewhere else — and wait for the vehicle to
/// confirm before the flight command follows it.
fn ensure_guided(link: &mut DroneLink) -> Result<(), String> {
    if let Some(mode) = link.telemetry().mode {
        if mode.eq_ignore_ascii_case(copter_mode_name(COPTER_MODE_GUIDED).as_str()) {
            return Ok(());
        }
    }
    // A mode switch is refused while the vehicle is still coming up (EKF, home
    // position), and the same request a moment later is accepted. Retrying an
    // idempotent mode change is what a ground station does; anything else would
    // surface as a mysterious failure to a script that just asked to fly.
    let mut last = String::from("E_KAZE_LINK: the vehicle never answered the mode change");
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        match link.command(
            mavlink::cmd::DO_SET_MODE,
            [1.0, COPTER_MODE_GUIDED as f32, 0.0, 0.0, 0.0, 0.0, 0.0],
        ) {
            Ok(()) => {
                for _ in 0..40 {
                    if let Some(mode) = link.telemetry().mode {
                        if mode.eq_ignore_ascii_case("guided") {
                            return Ok(());
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                last = "E_KAZE_LINK: the vehicle accepted GUIDED but is not reporting it".to_string();
            }
            Err(why) => last = why,
        }
    }
    if let Some(note) = link.take_notes().pop() {
        last = format!("{} ({})", last, note.text);
    }
    Err(last)
}

/// One Kaze device verb, one GPIO line.
fn device_op(
    bus: &mut GpioBus,
    op: &str,
    args: &[Value],
    arena: &mut Arena,
    heap: &mut Heap,
) -> Result<Value, String> {
    let name_of = |args: &[Value]| -> String {
        args.first()
            .map(|v| render_value(*v, arena, heap))
            .unwrap_or_default()
            .trim_matches('"')
            .to_string()
    };
    match op {
        "set_relay" => {
            let name = name_of(args);
            let on = match args.get(1) {
                Some(Value::Bool(flag)) => *flag,
                Some(value) => value.as_num().map(|n| n != 0.0).unwrap_or(false),
                None => false,
            };
            bus.set(&name, on)?;
            Ok(Value::Unit)
        }
        "pulse" => {
            let name = name_of(args);
            let seconds = args.get(1).and_then(|v| v.as_num()).unwrap_or(1.0);
            bus.pulse(&name, seconds)?;
            Ok(Value::Unit)
        }
        "read_gpio" => {
            let name = name_of(args);
            Ok(Value::Bool(bus.read(&name)?))
        }
        other => Err(format!("E_KAZE_LINK: the live device bus has no '{}'", other)),
    }
}

/// The printable name a refusal should carry, for hosts that want it.
pub fn op_name(command: u16) -> &'static str {
    command_name(command)
}

#[cfg(test)]
mod safety_tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A heartbeat and every field a command could depend on, all heard now.
    fn fresh_telemetry() -> Telemetry {
        let mut tele = heartbeat_only_telemetry();
        let now = Instant::now();
        tele.heard.lat = Some(now);
        tele.heard.lon = Some(now);
        tele.heard.alt_m = Some(now);
        tele.heard.rel_alt_m = Some(now);
        tele
    }

    /// A fresh HEARTBEAT, but no position has ever been reported.
    fn heartbeat_only_telemetry() -> Telemetry {
        let mut tele = Telemetry::default();
        let now = Instant::now();
        tele.heard.heartbeat = Some(now);
        tele.heard.armed = Some(now);
        tele.heard.mode = Some(now);
        tele
    }

    #[test]
    fn the_freshness_veto_refuses_a_stale_heartbeat_and_a_stale_field() {
        let heap = Heap::default();

        // Nothing has been heard: no heartbeat is not a healthy link.
        let tele = Telemetry::default();
        let err = live_freshness_veto(&tele, "arm", &[], &heap).expect_err("silence is not consent");
        assert!(err.starts_with("E_SAFETY_VIOLATION:"), "{}", err);
        assert!(err.contains("HEARTBEAT"), "{}", err);
        assert!(err.contains("1500"), "the limit is named: {}", err);

        // A heartbeat older than the limit is refused even if a field is fresh.
        let mut tele = fresh_telemetry();
        tele.heard.heartbeat =
            Some(Instant::now() - Duration::from_millis(SAFETY_FRESHNESS_MS + 1));
        let err = live_freshness_veto(&tele, "land", &[], &heap).expect_err("stale heartbeat");
        assert!(err.contains("HEARTBEAT is 1501 ms old") || err.contains("HEARTBEAT is 1502 ms old"), "{}", err);
        assert!(err.starts_with("E_SAFETY_VIOLATION:"), "{}", err);

        // A fresh heartbeat is not enough for a command that reads a field.
        let mut tele = heartbeat_only_telemetry();
        let err = live_freshness_veto(&tele, "takeoff", &[Value::Num(10.0)], &heap)
            .expect_err("takeoff needs a position");
        assert!(err.contains("'latitude' telemetry has not been reported"), "{}", err);
        tele.heard.lat = Some(Instant::now() - Duration::from_millis(SAFETY_FRESHNESS_MS + 1));
        let err = live_freshness_veto(&tele, "takeoff", &[Value::Num(10.0)], &heap)
            .expect_err("stale latitude");
        assert!(err.contains("'latitude' telemetry is"), "{}", err);

        // Everything fresh: the same command passes.
        let tele = fresh_telemetry();
        assert!(live_freshness_veto(&tele, "takeoff", &[Value::Num(10.0)], &heap).is_ok());
        assert!(live_freshness_veto(&tele, "arm", &[], &heap).is_ok());
        assert!(
            live_freshness_veto(&tele, "rtl", &[], &heap).is_ok(),
            "a failsafe command with a fresh heartbeat passes"
        );
    }

    #[test]
    fn the_veto_depends_on_exactly_the_fields_a_goto_reads() {
        let mut heap = Heap::default();
        let three = heap.push_vec([47.6, 8.5, 20.0]).expect("point");
        let ground = heap.push_vec([47.6, 8.5, 0.0]).expect("point without altitude");

        assert_eq!(dependent_fields("arm", &[], &heap), &[] as &[&str]);
        assert_eq!(dependent_fields("takeoff", &[], &heap), &["latitude", "longitude", "altitude"]);
        assert_eq!(dependent_fields("goto", &[Value::Num(47.6)], &heap), &["longitude", "altitude"]);
        assert_eq!(dependent_fields("goto", &[Value::Num(47.6), Value::Num(20.0)], &heap), &["longitude"]);
        assert_eq!(dependent_fields("goto", &[Value::Num(47.6), Value::Num(8.5), Value::Num(20.0)], &heap), &[] as &[&str]);
        assert_eq!(dependent_fields("goto", &[three], &heap), &[] as &[&str]);
        assert_eq!(dependent_fields("goto", &[ground], &heap), &["altitude"]);
        assert_eq!(
            dependent_fields("goto", &[ground, Value::Num(20.0)], &heap),
            &[] as &[&str]
        );
        for op in ["land", "rtl", "hold", "stop"] {
            assert_eq!(dependent_fields(op, &[], &heap), &[] as &[&str], "{}", op);
        }
    }

    #[test]
    fn an_alert_record_is_redacted_and_capped() {
        let body = capped_alert_body("contact ops with key sk-live-0123456789 and standby");
        assert!(body.contains("***"), "{}", body);
        assert!(!body.contains("0123456789"), "{}", body);

        let long = format!("{} password=hunter2", "x".repeat(4_000));
        let body = capped_alert_body(&long);
        assert!(body.len() <= MAX_ALERT_BYTES, "{} bytes", body.len());
        assert!(!body.contains("hunter2"), "redaction runs before truncation");
        let tail: String = body.chars().rev().take(12).collect::<String>().chars().rev().collect();
        assert!(tail.contains("[truncated]"), "the reader is told it was cut: {}", tail);
    }

    #[test]
    fn an_alert_sink_needs_a_known_destination() {
        assert!(AlertSink::parse("stdout").is_ok());
        assert!(AlertSink::parse("stderr").is_ok());
        let err = AlertSink::parse("telegram").expect_err("not a sink");
        assert!(err.contains("alert=stdout"), "the fix is named: {}", err);
        let err = AlertSink::parse("file:").expect_err("no path");
        assert!(err.contains("needs a path"), "{}", err);
        let err = AlertSink::parse("file:/kaze-no-such-directory/alerts.log").expect_err("unwritable");
        assert!(err.contains("cannot open the alert file"), "{}", err);
    }

    #[test]
    fn a_file_alert_sink_appends_one_timestamped_line_per_alert() {
        let path = std::env::temp_dir().join(format!(
            "kaze-alert-sink-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&path);
        let spec = format!("file:{}", path.display());
        {
            let mut sink = AlertSink::parse(&spec).expect("a fresh file opens");
            sink.send("gate opened with password=hunter2").expect("first alert");
            sink.send("gate closed").expect("second alert");
        }
        let text = std::fs::read_to_string(&path).expect("the sink wrote a file");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "one line per alert: {}", text);
        for line in &lines {
            assert!(line.contains(" alert "), "{}", line);
            assert!(line.len() >= 24, "a timestamp and a body: {}", line);
            assert_eq!(line.as_bytes().get(4), Some(&b'-'), "YYYY-MM-DD: {}", line);
            assert_eq!(line.as_bytes().get(10), Some(&b'T'), "date and time split: {}", line);
        }
        assert!(lines[0].contains("password=***"), "{}", lines[0]);
        assert!(!text.contains("hunter2"), "{}", text);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn partial_drone_targets_require_real_valid_telemetry() {
        let heap = Heap::default();
        let mut tele = crate::link::Telemetry::default();
        let args = [Value::Num(10.0)];
        assert!(resolve_drone_target(&args, &tele, &heap).unwrap_err().contains("longitude"));
        tele.lon = Some(20.0);
        assert!(resolve_drone_target(&args, &tele, &heap).unwrap_err().contains("altitude"));
        tele.rel_alt_m = Some(30.0);
        assert_eq!(resolve_drone_target(&args, &tele, &heap).unwrap(), (10.0, 20.0, 30.0));
        tele.lon = Some(181.0);
        assert!(resolve_drone_target(&args, &tele, &heap).unwrap_err().contains("E_KAZE_RANGE"));
    }
}
