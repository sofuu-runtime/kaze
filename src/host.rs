//! The live capabilities — Kaze v0.2.
//!
//! [`LiveHost`] is a [`CapHost`] with real hardware behind two families and the
//! deterministic simulator behind the rest:
//!
//! * `drone` → a live MAVLink link ([`crate::link`])
//! * `device` → Linux GPIO lines ([`crate::gpio`])
//! * `log`, `memory`, `policy`, `sensor`, `rover`, `alert` → unchanged simulation
//!
//! The R7 gate in [`crate::hardware`] has already judged every call before it
//! arrives here (declared, granted, in range). This module only decides *which*
//! backend answers, and refuses to invent a device that was not named.

use crate::caps::{render_value, Action, CapHost, SimHost, SimState};
use crate::gpio::GpioBus;
use crate::link::{command_name, copter_mode_name, DroneLink, COPTER_MODE_GUIDED};
use crate::mavlink;
use crate::value::{Heap, Value};
use crate::arena::Arena;

/// A host with live backends.
pub struct LiveHost {
    sim: SimHost,
    link: Option<DroneLink>,
    gpio: Option<GpioBus>,
    pwm: Option<crate::pwm::PwmBus>,
}

impl LiveHost {
    /// `backends` is `family -> device` from `--cap`; `pins` is `name -> line`
    /// from `--pin`. A family with no device, or with the device `sim`, keeps
    /// the simulator.
    pub fn new(
        sim: SimState,
        backends: &[(String, String)],
        pins: &[(String, u16)],
        pwm: &[(String, u16, u16)],
    ) -> Result<LiveHost, String> {
        let mut link = None;
        for (family, device) in backends {
            let device = device.trim();
            if device.is_empty() || device == "sim" {
                continue;
            }
            match family.as_str() {
                "drone" => link = Some(DroneLink::connect(device)?),
                // A device family with no named pin or PWM channel has nothing to drive,
                // and falling back to the simulator would let a script report
                // success for a relay or fan that does not exist.
                "device" => {
                    if pins.is_empty() && pwm.is_empty() {
                        return Err(format!(
                            "E_KAZE_LINK: --cap device={} needs at least one --pin name=<line> or --pwm name=<chip>:<channel>",
                            device
                        ));
                    }
                }
                other => {
                    return Err(format!(
                        "E_KAZE_LINK: the '{}' family has no live backend in v0.2 (drone and device do)",
                        other
                    ));
                }
            }
        }
        let gpio = if pins.is_empty() { None } else { Some(GpioBus::new(pins, None)) };
        let pwm_bus = if pwm.is_empty() { None } else { Some(crate::pwm::PwmBus::new(pwm, None)) };
        Ok(LiveHost { sim: SimHost::new(sim), link, gpio, pwm: pwm_bus })
    }

    /// A host around an already-open link (tests drive the real command path
    /// without a socket).
    pub fn with_link(sim: SimState, link: DroneLink) -> LiveHost {
        LiveHost { sim: SimHost::new(sim), link: Some(link), gpio: None, pwm: None }
    }

    pub fn is_live(&self) -> bool {
        self.link.is_some() || self.gpio.is_some() || self.pwm.is_some()
    }

    pub fn device(&self) -> Option<&str> {
        self.link.as_ref().map(|l| l.device.as_str())
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
            if let Some(mut link) = self.link.take() {
                let shown: Vec<String> =
                    args.iter().map(|v| render_value(*v, arena, heap)).collect();
                let result = drone_op(&mut link, op, args, arena, heap);
                self.link = Some(link);
                let rendered = match &result {
                    Ok(value) => render_value(*value, arena, heap),
                    Err(e) => format!("error: {}", e),
                };
                self.sim.note("drone", op, &shown, rendered);
                self.drain_notes();
                return result;
            }
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
            } else if self.gpio.is_some() {
                Err(format!(
                    "E_KAZE_LINK: no PWM channel is configured for '{}' (--pwm {}=<chip>:<channel>)",
                    name, name
                ))
            } else {
                self.sim.call(family, op, args, arena, heap)
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
            // `--cap drone=sim` (or a device-only live run) still has the
            // simulator's link, so a script can exercise the hook offline.
            None => self.sim.link_silent_ms(),
        }
    }
}

/// One Kaze drone verb, one gated MAVLink command.
fn drone_op(
    link: &mut DroneLink,
    op: &str,
    args: &[Value],
    arena: &mut Arena,
    heap: &Heap,
) -> Result<Value, String> {
    let num = |i: usize, fallback: f64| args.get(i).and_then(|v| v.as_num()).unwrap_or(fallback);
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
            ensure_guided(link)?;
            let alt = num(0, 10.0);
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
            ensure_guided(link)?;
            let tele = link.telemetry();
            let (lat, lon, alt) = if let Some(Value::Vec(v)) = args.first() {
                let p = heap.vec3(*v).unwrap_or([tele.lat.unwrap_or(0.0), tele.lon.unwrap_or(0.0), 0.0]);
                let alt = if args.len() >= 2 {
                    num(1, if p[2] != 0.0 { p[2] } else { tele.rel_alt_m.unwrap_or(10.0) })
                } else if p[2] != 0.0 {
                    p[2]
                } else {
                    tele.rel_alt_m.unwrap_or(10.0)
                };
                (p[0], p[1], alt)
            } else if args.len() >= 3 {
                (num(0, tele.lat.unwrap_or(0.0)), num(1, tele.lon.unwrap_or(0.0)), num(2, tele.rel_alt_m.unwrap_or(10.0)))
            } else if args.len() == 2 {
                (num(0, tele.lat.unwrap_or(0.0)), tele.lon.unwrap_or(0.0), num(1, 10.0))
            } else {
                (num(0, tele.lat.unwrap_or(0.0)), tele.lon.unwrap_or(0.0), tele.rel_alt_m.unwrap_or(10.0))
            };
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
            // A reading the vehicle has not sent yet is not a zero: wait a
            // moment for it, then say so instead of inventing a number.
            for _ in 0..40 {
                if let Some(value) = link.telemetry().read(&field) {
                    return Ok(Value::Num(value));
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(format!(
                "E_KAZE_LINK: the vehicle has not reported '{}' yet",
                field
            ))
        }
        other => Err(format!("E_KAZE_LINK: the live drone link has no '{}'", other)),
    }
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
