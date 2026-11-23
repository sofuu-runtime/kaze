#![allow(clippy::field_reassign_with_default)]

// The live backends, end to end: a Kaze script drives a real socket and real
// sysfs files. No autopilot and no hardware needed — the stand-ins below speak
// the real wire format.

use std::net::UdpSocket;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use kaze::link::COPTER_MODE_GUIDED;
use kaze::mavlink::{encode, msg_heartbeat, Frame, Parser};
use kaze::{run_source, RunOptions};

const COMMAND_LONG: u32 = 76;
const COMMAND_ACK: u32 = 77;
const GLOBAL_POSITION_INT: u32 = 33;
const SATELLITES: u32 = 24;
const POSITION_TARGET: u32 = 86;
const ARM: u16 = 400;

fn live_opts() -> RunOptions {
    RunOptions {
        policy: Some(serde_json::json!({ "deny": [] })),
        ..Default::default()
    }
}

/// What a stand-in autopilot is willing to send. The 1500 ms safety gate makes
/// each of these a distinct scenario: a vehicle that speaks, one that never
/// heartbeats, and one that never reports a position.
#[derive(Clone, Copy)]
struct AutopilotProfile {
    /// Send HEARTBEAT frames. The safety gate watches these.
    heartbeat: bool,
    /// Send GLOBAL_POSITION_INT frames.
    positions: bool,
    /// Send a SATELLITES frame, which is enough for `connect` to finish when
    /// positions are deliberately withheld.
    satellites: bool,
    /// Drop every n-th position frame (0 keeps all of them).
    drop_position_every: u32,
}

impl AutopilotProfile {
    /// A healthy vehicle: heartbeats, positions, acknowledgements.
    const FRESH: AutopilotProfile =
        AutopilotProfile { heartbeat: true, positions: true, satellites: false, drop_position_every: 0 };

    fn without_heartbeat() -> AutopilotProfile {
        AutopilotProfile { heartbeat: false, positions: true, satellites: false, drop_position_every: 0 }
    }

    fn without_positions() -> AutopilotProfile {
        AutopilotProfile { heartbeat: true, positions: false, satellites: true, drop_position_every: 0 }
    }

    fn lossy(drop_position_every: u32) -> AutopilotProfile {
        AutopilotProfile { drop_position_every, ..AutopilotProfile::FRESH }
    }
}

/// What the stand-in vehicle saw on the wire.
#[derive(Clone, Default)]
struct Seen {
    /// Every MAV_CMD the link sent, in order.
    commands: Vec<u16>,
    /// How many SET_POSITION_TARGET_GLOBAL_INT frames the link sent.
    position_targets: u32,
}

/// A stand-in autopilot: says hello (or deliberately does not), reports what its
/// profile allows, ACKs every command, and records what it received.
struct FakeAutopilot {
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<Seen>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl FakeAutopilot {
    fn start(kaze_port: u16, profile: AutopilotProfile) -> FakeAutopilot {
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Seen::default()));
        let flag = Arc::clone(&stop);
        let log = Arc::clone(&seen);
        let handle = thread::spawn(move || {
            let sock = UdpSocket::bind("127.0.0.1:0").expect("the stand-in binds");
            sock.set_read_timeout(Some(Duration::from_millis(30)))
                .expect("read timeout");
            let gateway = format!("127.0.0.1:{}", kaze_port);
            let mut parser = Parser::new();
            let mut seq = 0u8;
            let mut positions = 0u32;
            // A real vehicle repeats its position several times a second; the
            // link must not depend on catching the first one during connect.
            let mut peer: Option<std::net::SocketAddr> = None;
            let mut armed = false;
            macro_rules! send_position {
                ($to:expr) => {{
                    positions += 1;
                    if profile.drop_position_every == 0
                        || positions % profile.drop_position_every != 0
                    {
                        let mut pos =
                            Frame::new(GLOBAL_POSITION_INT, seq, 7, 1).expect("position builds");
                        seq = seq.wrapping_add(1);
                        pos.set_i32_at(4, 476_000_000);
                        pos.set_i32_at(8, 85_000_000);
                        pos.set_i32_at(12, 500_000);
                        pos.set_i32_at(16, 120_000);
                        let _ = sock.send_to(&encode(&pos).expect("position encodes"), $to);
                    }
                }};
            }
            while !flag.load(Ordering::SeqCst) {
                // A UDP link has no peer until the vehicle speaks, so the
                // vehicle speaks first. It reports GUIDED so takeoff/goto are
                // testable.
                if profile.heartbeat {
                    let mut hb = msg_heartbeat(seq, 7, 1);
                    hb.set_u32_at(0, COPTER_MODE_GUIDED as u32);
                    if armed {
                        hb.set_u8_at(6, 0x80);
                    }
                    seq = seq.wrapping_add(1);
                    let _ = sock.send_to(&encode(&hb).expect("heartbeat encodes"), &gateway);
                }
                if profile.satellites {
                    let mut sat = Frame::new(SATELLITES, seq, 7, 1).expect("satellites builds");
                    seq = seq.wrapping_add(1);
                    sat.set_u8_at(29, 12);
                    let _ = sock.send_to(&encode(&sat).expect("satellites encodes"), &gateway);
                }

                // A UDP link has no peer until the vehicle speaks, so a vehicle
                // without heartbeats still has to open the conversation with a
                // position — otherwise the link could not even come up and the
                // safety gate would never be reached.
                if peer.is_none() && profile.positions {
                    send_position!(gateway.as_str());
                }

                let mut buf = [0u8; 2048];
                let Ok((n, from)) = sock.recv_from(&mut buf) else {
                    if profile.positions {
                        if let Some(peer) = peer {
                            send_position!(peer);
                        }
                    }
                    continue;
                };
                peer = Some(from);
                parser.push(&buf[..n]);
                while let Some(frame) = parser.next() {
                    if frame.msgid == COMMAND_LONG {
                        let command = frame.u16_at(28);
                        if let Ok(mut log) = log.lock() {
                            log.commands.push(command);
                        }
                        let mut ack = Frame::new(COMMAND_ACK, seq, 7, 1).expect("ack builds");
                        seq = seq.wrapping_add(1);
                        ack.set_u16_at(0, command);
                        ack.set_u8_at(2, 0);
                        let _ = sock.send_to(&encode(&ack).expect("ack encodes"), from);
                        if command == ARM {
                            armed = true;
                        }
                    } else if frame.msgid == POSITION_TARGET {
                        if let Ok(mut log) = log.lock() {
                            log.position_targets += 1;
                        }
                    }
                }
                if profile.positions {
                    send_position!(from);
                }
            }
        });
        FakeAutopilot { stop, seen, handle: Some(handle) }
    }

    /// The wire evidence so far.
    fn seen(&self) -> Seen {
        self.seen.lock().map(|log| log.clone()).unwrap_or_default()
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    /// Stop the stand-in and return everything it received.
    fn stop(mut self) -> Seen {
        self.shutdown();
        self.seen()
    }
}

impl Drop for FakeAutopilot {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .expect("a free port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// One fake PWM sysfs root for the whole test binary (the variable is global).
fn pwm_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("kaze-live-pwm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("pwmchip0")).expect("chip dir");
        std::fs::write(root.join("pwmchip0").join("export"), "").expect("export");
        for ch in ["pwm0", "pwm1"] {
            let channel = root.join("pwmchip0").join(ch);
            std::fs::create_dir_all(&channel).expect("fake pwm channel");
            for file in ["period", "duty_cycle", "enable"] {
                std::fs::write(channel.join(file), "0").expect("channel file");
            }
        }
        std::env::set_var("KAZE_PWM_ROOT", &root);
        root
    })
    .clone()
}

/// One fake sysfs root for the whole test binary (the variable is global).
fn gpio_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("kaze-live-gpio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fake sysfs root");
        std::fs::write(root.join("export"), "").expect("export file");
        for pin in [17u16, 27u16] {
            let dir = root.join(format!("gpio{}", pin));
            std::fs::create_dir_all(&dir).expect("pin dir");
            std::fs::write(dir.join("value"), "0").expect("value file");
        }
        std::env::set_var("KAZE_GPIO_ROOT", &root);
        root
    })
    .clone()
}

#[test]
fn a_script_arms_a_vehicle_over_a_real_socket() {
    let port = free_udp_port();
    let autopilot = FakeAutopilot::start(port, AutopilotProfile::FRESH);

    let mut opts = live_opts();
    opts.grants = vec!["drone".to_string()];
    opts.backend = vec![("drone".to_string(), format!("udp:127.0.0.1:{}", port))];
    opts.fuel = 50_000;
    let source = "needs: drone\naction main():\n  arm drone\n";
    let outcome = run_source(source, "live.kaze", &opts);

    let seen = autopilot.stop();

    let report = outcome.expect("the arm succeeds against a real socket");
    assert_eq!(report.actions.len(), 1, "one live drone call: {:?}", report.actions);
    assert!(
        report.actions[0].contains("drone.arm"),
        "the trace names the call: {:?}",
        report.actions
    );
    assert!(
        seen.commands.contains(&ARM),
        "the autopilot received COMPONENT_ARM_DISARM, saw {:?}",
        seen.commands
    );
}

#[test]
fn a_script_drives_a_real_gpio_line() {
    let root = gpio_root();
    let mut opts = live_opts();
    opts.grants = vec!["device".to_string(), "log".to_string()];
    opts.pins = vec![("fan".to_string(), 17u16)];
    opts.fuel = 50_000;
    // The line is read back through the real sysfs file while it is high. The
    // run then ends normally, and a normal exit de-asserts the line (Phase 5),
    // so the file is low once `run_source` returns.
    let source = "needs: fan, log\n\naction main():\n    turn on fan\n    let is_on = device.read_gpio(\"fan\") otherwise 0\n    log is_on\n";
    let report = run_source(source, "gpio.kaze", &opts).expect("the pin is driven and read back");
    assert_eq!(report.actions.len(), 3, "drive, read back, log: {:?}", report.actions);
    assert!(
        report.logs.iter().any(|(_, line)| line == "true"),
        "the line read back high while the run was live: {:?}",
        report.logs
    );
    let value = std::fs::read_to_string(root.join("gpio17").join("value")).expect("pin file");
    assert_eq!(value.trim(), "0", "a normal exit leaves the line de-asserted");
}

#[test]
fn a_script_sets_a_duty_cycle_on_a_real_pwm_channel() {
    let root = pwm_root();
    let mut opts = live_opts();
    opts.grants = vec!["device".to_string()];
    opts.pwm = vec![("fan".to_string(), 0u16, 0u16)];
    opts.fuel = 50_000;
    let source = "needs: fan\n\naction main():\n    device.set_duty(\"fan\", 60)\n";
    let report = run_source(source, "pwm.kaze", &opts).expect("the channel is driven");
    assert_eq!(report.actions.len(), 1, "{:?}", report.actions);
    let duty = std::fs::read_to_string(root.join("pwmchip0").join("pwm0").join("duty_cycle"))
        .expect("duty file");
    assert_eq!(duty.trim(), "12000000", "60% of a 20 ms period");
    // The channel was switched on during the run; a normal exit switches it
    // back off (Phase 5), which is the state left on disk.
    let enable = std::fs::read_to_string(root.join("pwmchip0").join("pwm0").join("enable"))
        .expect("enable file");
    assert_eq!(enable.trim(), "0", "a normal exit disables the channel");
}

#[test]
fn a_non_numeric_duty_is_a_type_refusal_not_a_link_failure() {
    let _ = pwm_root();
    let mut opts = live_opts();
    opts.grants = vec!["device".to_string(), "log".to_string()];
    opts.pwm = vec![("fan".to_string(), 0u16, 0u16)];
    opts.fuel = 50_000;
    // A type refusal must keep its own code: `try` may not recover from it.
    let source = "needs: fan, log\naction main():\n    try:\n        device.set_duty(\"fan\", \"hot\")\n    otherwise:\n        log warning \"caught\"\n";
    let err = run_source(source, "pwm.kaze", &opts)
        .expect_err("a non-numeric duty is a type refusal, not a link failure");
    assert_eq!(err.code, "E_KAZE_TYPE", "{}", err.message);
    assert!(
        err.message.contains("needs a number for argument 2"),
        "{}",
        err.message
    );
}

#[test]
fn a_live_link_reports_silence_after_the_vehicle_stops() {
    use kaze::caps::{CapHost, SimState};
    use kaze::host::LiveHost;
    use kaze::link::{Core, DroneLink, Io};

    /// A transport that accepts writes and never delivers a byte.
    struct SilentIo;
    impl Io for SilentIo {
        fn write(&mut self, _bytes: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }

    let link = DroneLink::with_io(
        Core::new("silent"),
        Box::new(SilentIo),
        "silent".to_string(),
        1,
        1_000,
    )
    .expect("a silent link starts");
    let host = LiveHost::with_link(SimState::default(), link);

    let fresh = host.link_silent_ms().expect("a live host always answers");
    assert!(fresh < 1_000, "a fresh link starts near zero: {}", fresh);

    thread::sleep(Duration::from_millis(120));
    let silent = host.link_silent_ms().expect("a live host always answers");
    assert!(silent >= 100, "silence grows without frames: {}", silent);
}

#[test]
fn a_lossy_position_stream_still_completes_a_mission() {
    let port = free_udp_port();
    let autopilot = FakeAutopilot::start(port, AutopilotProfile::lossy(2));

    let mut opts = live_opts();
    opts.grants = vec!["drone".to_string(), "log".to_string()];
    opts.backend = vec![("drone".to_string(), format!("udp:127.0.0.1:{}", port))];
    opts.fuel = 50_000;
    let source = "needs: drone, log\naction main():\n    arm drone\n    takeoff to 10 meters\n    fly to vector(47.6002, 8.5002, 20) at 20 meters\n    log drone.telemetry(\"altitude\")\n";
    let outcome = run_source(source, "lossy.kaze", &opts);

    let _seen = autopilot.stop();

    let report = outcome.expect("half the position frames may be lost; the mission still runs");
    for verb in ["drone.arm", "drone.takeoff", "drone.goto"] {
        assert!(
            report.actions.iter().any(|a| a.contains(verb)),
            "{} missing from {:?}",
            verb,
            report.actions
        );
    }
    assert!(!report.logs.is_empty(), "a telemetry read survives the gaps");
}

#[test]
fn a_duty_cycle_out_of_range_is_refused_by_the_gate() {
    let _ = pwm_root();
    let mut opts = live_opts();
    opts.grants = vec!["device".to_string()];
    opts.pwm = vec![("fan".to_string(), 0u16, 0u16)];
    opts.fuel = 50_000;
    let source = "needs: fan\n\naction main():\n    device.set_duty(\"fan\", 140)\n";
    let err = run_source(source, "pwm.kaze", &opts).expect_err("140% is not a duty cycle");
    assert_eq!(err.code, "E_KAZE_RANGE", "{}", err.message);
    assert!(err.message.contains("outside the permitted range 0 to 100 percent"), "{}", err.message);
    assert!(err.message.contains("rejected, not clamped"), "{}", err.message);
}

#[test]
fn an_unnamed_pwm_channel_is_refused() {
    let _ = pwm_root();
    let mut opts = live_opts();
    opts.grants = vec!["device".to_string()];
    opts.pwm = vec![("fan".to_string(), 0u16, 0u16)];
    opts.fuel = 50_000;
    let source = "needs: pump\n\naction main():\n    device.set_duty(\"pump\", 50)\n";
    let err = run_source(source, "pwm.kaze", &opts).expect_err("no channel for pump");
    assert!(
        err.message.contains("no PWM channel is configured for 'pump'"),
        "{}",
        err.message
    );
}

#[test]
fn hardware_named_without_a_pin_is_refused_not_simulated() {
    let mut opts = live_opts();
    opts.grants = vec!["device".to_string()];
    opts.backend = vec![("device".to_string(), "gpio".to_string())];
    opts.fuel = 50_000;
    let source = "needs: fan\n\naction main():\n    turn on fan\n";
    let err = run_source(source, "gpio.kaze", &opts).expect_err("nothing to drive");
    assert!(
        err.message.contains("needs at least one --pin"),
        "a script must not get a simulated success: {}",
        err.message
    );
}

#[test]
fn a_family_with_no_live_backend_says_so() {
    let mut opts = live_opts();
    opts.grants = vec!["sensor".to_string(), "log".to_string()];
    opts.backend = vec![("sensor".to_string(), "i2c".to_string())];
    opts.fuel = 50_000;
    let source = "needs: sensor, log\n\naction main():\n    let temperature = sensor.temperature() otherwise 0 Celsius\n    log temperature\n";
    let err = run_source(source, "sensor.kaze", &opts).expect_err("no live sensor");
    assert!(
        err.message.contains("has no configured live backend") || err.message.contains("has no live backend"),
        "{}",
        err.message
    );
}

#[test]
fn a_pin_the_operator_never_named_is_refused() {
    let _ = gpio_root();
    let mut opts = live_opts();
    opts.grants = vec!["device".to_string()];
    opts.pins = vec![("fan".to_string(), 27u16)];
    opts.fuel = 50_000;
    let source = "needs: mister\n\naction main():\n    pulse mister for 1 seconds\n";
    let err = run_source(source, "gpio.kaze", &opts).expect_err("an unnamed pin is refused");
    assert!(
        err.message.contains("no GPIO pin is configured for 'mister'"),
        "the refusal names the pin: {}",
        err.message
    );
}

#[test]
fn a_device_with_no_backend_named_stays_simulated() {
    let mut opts = live_opts();
    opts.grants = vec!["drone".to_string()];
    opts.backend = vec![("drone".to_string(), "sim".to_string())];
    opts.fuel = 50_000;
    let source = "needs: drone\n\naction main():\n    arm drone\n    takeoff to 10 meters\n";
    let report = run_source(source, "sim.kaze", &opts).expect("the simulator answers");
    assert_eq!(report.actions.len(), 2, "both calls are traced: {:?}", report.actions);
    assert!(report.actions[1].contains("drone.takeoff"), "{:?}", report.actions);
}

#[test]
fn a_missing_link_is_reported_before_the_script_runs() {
    let port = free_udp_port();
    let mut opts = live_opts();
    opts.grants = vec!["drone".to_string()];
    opts.backend = vec![("drone".to_string(), format!("udp:127.0.0.1:{}", port))];
    opts.fuel = 50_000;
    let source = "needs: drone\naction main():\n  arm drone\n";
    let err = run_source(source, "live.kaze", &opts).expect_err("nothing answers");
    assert!(
        err.message.contains("nothing is streaming"),
        "the refusal says what is missing: {}",
        err.message
    );
}

#[test]
fn the_telemetry_a_script_reads_comes_from_the_vehicle() {
    let port = free_udp_port();
    let autopilot = FakeAutopilot::start(port, AutopilotProfile::FRESH);
    let mut opts = live_opts();
    opts.grants = vec!["drone".to_string(), "log".to_string()];
    opts.backend = vec![("drone".to_string(), format!("udp:127.0.0.1:{}", port))];
    opts.fuel = 50_000;
    let source = "needs: drone, log\n\naction main():\n    log drone.telemetry(\"altitude\")\n";
    let outcome = run_source(source, "live.kaze", &opts);
    let _seen = autopilot.stop();
    let report = outcome.expect("telemetry is readable");
    assert!(
        report.logs.iter().any(|(_, line)| line.contains("120")),
        "the altitude the vehicle reported reaches the script: {:?}",
        report.logs
    );
}

#[test]
fn a_script_runs_with_only_pwm_and_no_pins_under_live_device() {
    let root = pwm_root();
    let mut opts = live_opts();
    opts.grants = vec!["device".to_string()];
    opts.backend = vec![("device".to_string(), "gpio".to_string())];
    opts.pwm = vec![("mister".to_string(), 0u16, 1u16)];
    opts.fuel = 50_000;
    let source = "needs: mister\n\naction main():\n    device.set_duty(\"mister\", 40)\n";
    let report = run_source(source, "pwm_only.kaze", &opts).expect("runs with only pwm");
    assert_eq!(report.actions.len(), 1);
    let duty = std::fs::read_to_string(root.join("pwmchip0").join("pwm1").join("duty_cycle"))
        .expect("duty file");
    assert_eq!(duty.trim(), "8000000");
}

#[test]
fn set_duty_without_pwm_configured_hints_at_the_fix() {
    let _ = gpio_root();
    let mut opts = live_opts();
    opts.grants = vec!["device".to_string()];
    opts.pins = vec![("fan".to_string(), 17u16)];
    opts.fuel = 50_000;
    let source = "needs: fan\n\naction main():\n    device.set_duty(\"fan\", 50)\n";
    let err = run_source(source, "no_pwm.kaze", &opts).expect_err("no pwm channel");
    assert!(
        err.message.contains("no PWM channel is configured for 'fan'"),
        "{}",
        err.message
    );
    assert!(err.message.contains("--pwm fan=<chip>:<channel>"), "{}", err.message);
}

/// A vehicle that is talking but never heartbeating: the link connects, and
/// every actuation command is refused with the uncatchable safety code.
#[test]
fn a_vehicle_that_never_sends_a_heartbeat_is_refused_uncatchably() {
    let port = free_udp_port();
    let autopilot = FakeAutopilot::start(port, AutopilotProfile::without_heartbeat());

    let mut opts = live_opts();
    opts.grants = vec!["drone".to_string(), "log".to_string()];
    opts.backend = vec![("drone".to_string(), format!("udp:127.0.0.1:{}", port))];
    opts.fuel = 50_000;
    // A `try` may not recover from a safety violation: a script cannot retry
    // with the same stale world model.
    let source = "needs: drone, log\naction main():\n    try:\n        arm drone\n    otherwise:\n        log warning \"caught\"\n";
    let outcome = run_source(source, "freshness.kaze", &opts);
    let seen = autopilot.stop();

    let err = outcome.expect_err("no heartbeat is not consent");
    assert_eq!(err.code, "E_SAFETY_VIOLATION", "{}", err.message);
    assert!(err.message.contains("HEARTBEAT"), "{}", err.message);
    assert!(err.message.contains("1500"), "{}", err.message);
    assert!(
        !seen.commands.contains(&ARM),
        "the refused arm was not sent: {:?}",
        seen.commands
    );
}

/// A link that is alive but has never reported a position: a partial target
/// has no real value to fill from, so it is refused rather than invented.
#[test]
fn a_field_the_vehicle_never_reported_refuses_a_partial_target() {
    let port = free_udp_port();
    let autopilot = FakeAutopilot::start(port, AutopilotProfile::without_positions());

    let mut opts = live_opts();
    opts.grants = vec!["drone".to_string()];
    opts.backend = vec![("drone".to_string(), format!("udp:127.0.0.1:{}", port))];
    opts.fuel = 50_000;
    let source = "needs: drone\naction main():\n    drone.goto(47.6)\n";
    let outcome = run_source(source, "partial.kaze", &opts);
    let seen = autopilot.stop();

    let err = outcome.expect_err("a target with no position fix is refused");
    assert_eq!(err.code, "E_SAFETY_VIOLATION", "{}", err.message);
    assert!(err.message.contains("longitude"), "{}", err.message);
    assert_eq!(seen.position_targets, 0, "no target frame was sent");
}

/// A link that spoke and then went silent past the 1500 ms limit: the exact
/// watchdog boundary, measured on a real clock.
#[test]
fn a_silent_link_past_the_freshness_limit_refuses_actuation() {
    use kaze::arena::Arena;
    use kaze::caps::{CapHost, SimState};
    use kaze::host::LiveHost;
    use kaze::link::{Core, DroneLink, Io, SAFETY_FRESHNESS_MS};
    use kaze::value::Heap;

    /// Says hello exactly once, then never again; records every byte written.
    struct SpeakOnce {
        frames: Option<Vec<u8>>,
        wrote: Arc<Mutex<Vec<Vec<u8>>>>,
    }
    impl Io for SpeakOnce {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            if let Ok(mut log) = self.wrote.lock() {
                log.push(bytes.to_vec());
            }
            Ok(())
        }
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.frames.take() {
                Some(bytes) => {
                    let n = bytes.len().min(buf.len());
                    buf[..n].copy_from_slice(&bytes[..n]);
                    Ok(n)
                }
                None => Ok(0),
            }
        }
    }

    let mut hb = msg_heartbeat(0, 7, 1);
    hb.set_u32_at(0, COPTER_MODE_GUIDED as u32);
    hb.set_u8_at(6, 0x80);
    let mut pos = Frame::new(GLOBAL_POSITION_INT, 1, 7, 1).expect("position builds");
    pos.set_i32_at(4, 476_000_000);
    pos.set_i32_at(8, 85_000_000);
    pos.set_i32_at(12, 500_000);
    pos.set_i32_at(16, 120_000);
    let mut frames = encode(&hb).expect("heartbeat encodes");
    frames.extend(encode(&pos).expect("position encodes"));

    let wrote = Arc::new(Mutex::new(Vec::new()));
    let link = DroneLink::with_io(
        Core::new("scripted"),
        Box::new(SpeakOnce { frames: Some(frames), wrote: Arc::clone(&wrote) }),
        "scripted".to_string(),
        5,
        200,
    )
    .expect("the link thread starts");
    let mut host = LiveHost::with_link(SimState::default(), link);

    // The vehicle spoke a heartbeat and a position a moment ago, then went
    // silent. Wait past the limit before asking it to actuate.
    thread::sleep(Duration::from_millis(SAFETY_FRESHNESS_MS + 250));

    let mut arena = Arena::new(1024);
    let mut heap = Heap::new();
    let err = host
        .call("drone", "arm", &[], &mut arena, &mut heap)
        .expect_err("a silent vehicle must not be armed");
    assert!(err.starts_with("E_SAFETY_VIOLATION:"), "{}", err);
    assert!(err.contains("HEARTBEAT"), "{}", err);
    assert!(err.contains("1500"), "{}", err);

    // Nothing but the link's own heartbeats left the process.
    let mut parser = Parser::new();
    if let Ok(log) = wrote.lock() {
        for bytes in log.iter() {
            parser.push(bytes);
        }
    }
    let mut commands = Vec::new();
    while let Some(frame) = parser.next() {
        if frame.msgid == COMMAND_LONG {
            commands.push(frame.u16_at(28));
        }
    }
    assert!(!commands.contains(&ARM), "the refused arm was not sent: {:?}", commands);
}

