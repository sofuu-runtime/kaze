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
    drone_device: Option<String>,
    sim_families: Vec<String>,
    gpio: Option<GpioBus>,
    pwm: Option<crate::pwm::PwmBus>,
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
                other => {
                    return Err(format!(
                        "E_CAP_MISSING: the '{}' family has no configured live backend (drone and device do)",
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
            // The live link currently tracks the age of received bytes, not
            // heartbeat and individual telemetry fields. Until those safety
            // checks exist, even an authorised command must not actuate a
            // physical vehicle. Read-only telemetry remains available.
            if op != "telemetry" {
                return Err("E_CAP_DENIED: live drone actuation is disabled until heartbeat and telemetry freshness safety checks are implemented".to_string());
            }
            self.ensure_drone_link()?;
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
            return Err("E_KAZE_HOST_UNCLASSIFIED: the authorised drone link is unavailable".to_string());
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

        if family == "rover" && !self.sim_families.iter().any(|f| f == "rover") {
            return Err("E_CAP_MISSING: no live rover backend is available; rover=sim is only for explicit testing".to_string());
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
