// The live backends, end to end: a Kaze script drives a real socket and real
// sysfs files. No autopilot and no hardware needed — the stand-ins below speak
// the real wire format.

use std::net::UdpSocket;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use kaze::mavlink::{encode, msg_heartbeat, Frame, Parser};
use kaze::{run_source, RunOptions};

const COMMAND_LONG: u32 = 76;
const COMMAND_ACK: u32 = 77;
const GLOBAL_POSITION_INT: u32 = 33;
const ARM: u16 = 400;

/// A stand-in autopilot: says hello, reports a position, ACKs every command.
fn fake_autopilot(kaze_port: u16) -> (Arc<AtomicBool>, thread::JoinHandle<Vec<u16>>) {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        let sock = UdpSocket::bind("127.0.0.1:0").expect("the stand-in binds");
        sock.set_read_timeout(Some(Duration::from_millis(30)))
            .expect("read timeout");
        let gateway = format!("127.0.0.1:{}", kaze_port);
        let mut parser = Parser::new();
        let mut seen: Vec<u16> = Vec::new();
        let mut seq = 0u8;
        // A real vehicle repeats its position several times a second; the link
        // must not depend on catching the first one during connect.
        let mut peer: Option<std::net::SocketAddr> = None;
        let mut armed = false;
        while !flag.load(Ordering::SeqCst) {
            // A UDP link has no peer until the vehicle speaks, so the vehicle
            // speaks first. Once it accepts an arm, its heartbeat says so.
            let mut hb = msg_heartbeat(seq, 7, 1);
            if armed {
                hb.set_u8_at(6, 0x80);
            }
            seq = seq.wrapping_add(1);
            let _ = sock.send_to(&encode(&hb).expect("heartbeat encodes"), &gateway);

            let mut buf = [0u8; 2048];
            let Ok((n, from)) = sock.recv_from(&mut buf) else {
                if let Some(peer) = peer {
                    let mut pos = Frame::new(GLOBAL_POSITION_INT, seq, 7, 1).expect("position builds");
                    seq = seq.wrapping_add(1);
                    pos.set_i32_at(4, 476_000_000);
                    pos.set_i32_at(8, 85_000_000);
                    pos.set_i32_at(12, 500_000);
                    pos.set_i32_at(16, 120_000);
                    let _ = sock.send_to(&encode(&pos).expect("position encodes"), peer);
                }
                continue;
            };
            peer = Some(from);
            parser.push(&buf[..n]);
            while let Some(frame) = parser.next() {
                if frame.msgid == COMMAND_LONG {
                    let command = frame.u16_at(28);
                    seen.push(command);
                    let mut ack = Frame::new(COMMAND_ACK, seq, 7, 1).expect("ack builds");
                    seq = seq.wrapping_add(1);
                    ack.set_u16_at(0, command);
                    ack.set_u8_at(2, 0);
                    let _ = sock.send_to(&encode(&ack).expect("ack encodes"), from);
                    if command == ARM {
                        armed = true;
                    }
                }
            }
            let mut pos = Frame::new(GLOBAL_POSITION_INT, seq, 7, 1).expect("position builds");
            seq = seq.wrapping_add(1);
            pos.set_i32_at(4, 476_000_000);
            pos.set_i32_at(8, 85_000_000);
            pos.set_i32_at(12, 500_000);
            pos.set_i32_at(16, 120_000);
            let _ = sock.send_to(&encode(&pos).expect("position encodes"), from);
        }
        seen
    });
    (stop, handle)
}

fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .expect("a free port")
        .local_addr()
        .expect("local addr")
        .port()
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
    let (stop, fc) = fake_autopilot(port);

    let mut opts = RunOptions::default();
    opts.grants = vec!["drone".to_string()];
    opts.backend = vec![("drone".to_string(), format!("udp:127.0.0.1:{}", port))];
    opts.fuel = 50_000;
    let source = "needs: drone\naction main():\n  arm drone\n";
    let outcome = run_source(source, "live.kaze", &opts);

    stop.store(true, Ordering::SeqCst);
    let seen = fc.join().expect("the stand-in stops");

    let report = outcome.expect("the arm succeeds against a real socket");
    assert_eq!(report.actions.len(), 1, "one live drone call: {:?}", report.actions);
    assert!(
        report.actions[0].contains("drone.arm"),
        "the trace names the call: {:?}",
        report.actions
    );
    assert!(
        seen.contains(&ARM),
        "the autopilot received COMPONENT_ARM_DISARM, saw {:?}",
        seen
    );
}

#[test]
fn a_script_drives_a_real_gpio_line() {
    let root = gpio_root();
    let mut opts = RunOptions::default();
    opts.grants = vec!["device".to_string()];
    opts.pins = vec![("fan".to_string(), 17u16)];
    opts.fuel = 50_000;
    let source = "needs: fan\n\naction main():\n    turn on fan\n";
    let report = run_source(source, "gpio.kaze", &opts).expect("the pin is driven");
    assert_eq!(report.actions.len(), 1);
    let value = std::fs::read_to_string(root.join("gpio17").join("value")).expect("pin file");
    assert_eq!(value.trim(), "1", "the line is high");
}

#[test]
fn hardware_named_without_a_pin_is_refused_not_simulated() {
    let mut opts = RunOptions::default();
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
    let mut opts = RunOptions::default();
    opts.grants = vec!["sensor".to_string()];
    opts.backend = vec![("sensor".to_string(), "i2c".to_string())];
    opts.fuel = 50_000;
    let source = "needs: sensor, log\n\naction main():\n    log sensor.temperature()\n";
    let err = run_source(source, "sensor.kaze", &opts).expect_err("no live sensor");
    assert!(
        err.message.contains("has no live backend in v0.2"),
        "{}",
        err.message
    );
}

#[test]
fn a_pin_the_operator_never_named_is_refused() {
    let _ = gpio_root();
    let mut opts = RunOptions::default();
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
    let mut opts = RunOptions::default();
    opts.grants = vec!["drone".to_string()];
    opts.fuel = 50_000;
    let source = "needs: drone\n\naction main():\n    arm drone\n    takeoff to 10 meters\n";
    let report = run_source(source, "sim.kaze", &opts).expect("the simulator answers");
    assert_eq!(report.actions.len(), 2, "both calls are traced: {:?}", report.actions);
    assert!(report.actions[1].contains("drone.takeoff"), "{:?}", report.actions);
}

#[test]
fn a_missing_link_is_reported_before_the_script_runs() {
    let port = free_udp_port();
    let mut opts = RunOptions::default();
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
    let (stop, fc) = fake_autopilot(port);
    let mut opts = RunOptions::default();
    opts.grants = vec!["drone".to_string()];
    opts.backend = vec![("drone".to_string(), format!("udp:127.0.0.1:{}", port))];
    opts.fuel = 50_000;
    let source = "needs: drone, log\n\naction main():\n    log drone.telemetry(\"altitude\")\n";
    let outcome = run_source(source, "live.kaze", &opts);
    stop.store(true, Ordering::SeqCst);
    let _ = fc.join();
    let report = outcome.expect("telemetry is readable");
    assert!(
        report.logs.iter().any(|(_, line)| line.contains("120")),
        "the altitude the vehicle reported reaches the script: {:?}",
        report.logs
    );
}

// Keep the Mutex import meaningful for future shared-state tests.
#[allow(dead_code)]
fn unused(_: Arc<Mutex<()>>) {}

