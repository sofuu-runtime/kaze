//! Capability-boundary checks. The only simulated execution is an explicitly
//! selected and labelled drone test; all other checks use real local behavior
//! or assert that an unavailable backend refuses to execute.

use std::fs;
use std::io::Write;
use std::net::UdpSocket;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("kaze-cap-safety-{}-{}", std::process::id(), id));
        fs::create_dir(&path).expect("create isolated test directory");
        Self(path)
    }

    fn policy(&self, contents: &str) {
        fs::write(self.0.join("policy.json"), contents).expect("write policy");
    }

    fn clear_policy(&self) {
        let path = self.0.join("policy.json");
        if path.exists() {
            fs::remove_file(path).expect("remove test policy");
        }
    }

    fn run(&self, source: &str, args: &[&str]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kaze"))
            .current_dir(&self.0)
            .env_remove("KAZE_GPIO_ROOT")
            .env_remove("KAZE_PWM_ROOT")
            .arg("run")
            .arg("-")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start Kaze");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(source.as_bytes())
            .expect("write source");
        child.wait_with_output().expect("wait for Kaze")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn check_error(output: &Output, code: &str) {
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "expected {}, got success: {}",
        code,
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(err.contains(code), "expected {}, got {}", code, err);
}

const LOG: &str = "needs: log\naction main():\n    log \"actual log\"\n";
const DRONE: &str = "needs: drone\naction main():\n    arm drone\n";
const LINK_HOOK: &str = "needs: drone\nwhen link lost for 1 seconds:\n    return to launch\n";

#[test]
fn declarations_grants_and_a_valid_policy_gate_actual_local_logging() {
    let fixture = Fixture::new();
    fixture.policy(r#"{"deny":[]}"#);
    check_error(&fixture.run(LOG, &[]), "E_CAP_MISSING");
    fixture.clear_policy();
    check_error(&fixture.run(LOG, &["--cap", "log"]), "E_CAP_DENIED");

    fixture.policy(r#"{"deny":[]}"#);
    check_error(
        &fixture.run(LOG, &["--cap", "log", "--no-policy"]),
        "E_CAP_DENIED",
    );
    let output = fixture.run(LOG, &["--cap", "log", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON run report");
    assert_eq!(report["logs"][0][1], "actual log");
    assert_eq!(report["simulated_families"], serde_json::json!([]));

    fixture.policy(r#"{"deny":["log.info"]}"#);
    check_error(&fixture.run(LOG, &["--cap", "log"]), "E_CAP_DENIED");
}

#[test]
fn malformed_missing_and_unknown_policy_rules_fail_closed() {
    let fixture = Fixture::new();
    for policy in [
        "{",
        "{}",
        "[]",
        "[[]]",
        r#"{"deny":false}"#,
        r#"{"deny":[true]}"#,
        r#"{"deny":["log.misspelled"]}"#,
        r#"{"deny":[],"allow":["*"]}"#,
        r#"{"deny":["*"],"deny":[]}"#,
    ] {
        fixture.policy(policy);
        check_error(&fixture.run(LOG, &["--cap", "log"]), "E_CAP_DENIED");
    }
    fixture.clear_policy();
    let pure = fixture.run("action main():\n    return 2 + 3\n", &["--plain"]);
    assert!(pure.status.success());
    assert!(String::from_utf8_lossy(&pure.stdout).contains("main -> 5"));
}

#[test]
fn unavailable_nonrobotics_backends_refuse_instead_of_simulating_success() {
    let fixture = Fixture::new();
    fixture.policy(r#"{"deny":[]}"#);
    for (family, source) in [
        ("memory", "needs: memory\naction main():\n    remember \"data\" as \"label\"\n"),
        ("sensor", "needs: sensor\naction main():\n    return sensor.temperature()\n"),
        ("device", "needs: device\naction main():\n    turn on fan\n"),
        ("alert", "needs: alert\naction main():\n    send alert \"message\"\n"),
        ("ai", "needs: ai\naction main():\n    return ai.prompt(\"request\")\n"),
        ("tool", "needs: tool\naction main():\n    try:\n        return tool.call(\"task\")\n    otherwise:\n        return \"caught\"\n"),
        ("policy", "needs: policy\naction main():\n    return policy.detects(\"unknown\", \"text\")\n"),
    ] {
        check_error(&fixture.run(source, &["--cap", family]), "E_CAP_MISSING");
    }
    let sensor = "needs: sensor\naction main():\n    return sensor.temperature()\n";
    check_error(
        &fixture.run(sensor, &["--cap", "sensor=sim"]),
        "E_CAP_MISSING",
    );
}

#[test]
fn invalid_physical_arguments_fail_before_a_backend_is_used() {
    let fixture = Fixture::new();
    fixture.policy(r#"{"deny":[]}"#);
    for source in [
        "needs: drone\naction main():\n    drone.goto(91, 0, 10)\n",
        "needs: drone\naction main():\n    drone.goto(0, 181, 10)\n",
        "needs: drone\naction main():\n    drone.goto(vector(91, 0, 10))\n",
    ] {
        check_error(&fixture.run(source, &["--cap", "drone"]), "E_KAZE_RANGE");
    }
    let relay = "needs: device\naction main():\n    device.set_relay(\"fan\", \"invalid\")\n";
    check_error(&fixture.run(relay, &["--cap", "device"]), "E_KAZE_TYPE");
    let record = "needs: drone\naction main(target):\n    fly to target\n";
    check_error(
        &fixture.run(record, &["--cap", "drone", "--arg", "target={\"lat\":1}"]),
        "E_KAZE_TYPE",
    );
    let duty = "needs: device\naction main():\n    device.set_duty(\"fan\", 25)\n";
    check_error(&fixture.run(duty, &["--cap", "device"]), "E_CAP_MISSING");
}

#[test]
fn bytecode_with_a_nonfinite_relay_state_is_refused_before_hardware_access() {
    let source = "needs: device\naction main():\n    device.set_relay(\"fan\", 100)\n";
    let mut module = kaze::compile_source(source, "relay.kaze").expect("compile relay program");
    let value = module.consts.iter_mut().find(|constant| matches!(constant, kaze::isa::Const::Num(100.0))).expect("find relay state constant");
    *value = kaze::isa::Const::Num(f64::NAN);
    let options = kaze::RunOptions {
        grants: vec!["device".into()],
        policy: Some(serde_json::json!({ "deny": [] })),
        ..Default::default()
    };
    let error = kaze::run_module(module, "relay.kazebc", &options).expect_err("nonfinite relay state must be rejected");
    assert_eq!(error.code, "E_KAZE_RANGE");
}

#[test]
fn drone_simulation_requires_explicit_selection_and_is_labelled() {
    let fixture = Fixture::new();
    fixture.policy(r#"{"deny":[]}"#);
    check_error(&fixture.run(DRONE, &["--cap", "drone"]), "E_CAP_MISSING");
    let result = fixture.run(DRONE, &["--cap", "drone=sim", "--json"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("JSON run report");
    assert_eq!(report["simulated_families"], serde_json::json!(["drone"]));
    let plain = fixture.run(DRONE, &["--cap", "drone=sim", "--plain"]);
    assert!(String::from_utf8_lossy(&plain.stdout)
        .contains("simulation selected (testing only): drone"));
    fixture.policy(r#"{"deny":["drone.arm"]}"#);
    let denied = fixture.run(DRONE, &["--cap", "drone=sim"]);
    check_error(&denied, "E_CAP_DENIED");
    assert!(String::from_utf8_lossy(&denied.stderr)
        .contains("simulation selected (testing only): drone"));
}

#[test]
fn rover_simulation_requires_explicit_selection_and_is_labelled() {
    let fixture = Fixture::new();
    fixture.policy(r#"{"deny":[]}"#);
    let source = "needs: rover\naction main():\n    drive forward at 1 m/s\n";
    check_error(&fixture.run(source, &["--cap", "rover"]), "E_CAP_MISSING");
    let result = fixture.run(source, &["--cap", "rover=sim", "--json"]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).expect("JSON run report");
    assert_eq!(report["simulated_families"], serde_json::json!(["rover"]));
    fixture.policy(r#"{"deny":["rover.forward"]}"#);
    check_error(&fixture.run(source, &["--cap", "rover=sim"]), "E_CAP_DENIED");
}

#[test]
fn link_loss_requires_grant_policy_and_an_explicit_backend() {
    let fixture = Fixture::new();
    fixture.policy(r#"{"deny":[]}"#);
    check_error(&fixture.run(LINK_HOOK, &["--ticks", "1"]), "E_CAP_MISSING");
    fixture.clear_policy();
    check_error(
        &fixture.run(LINK_HOOK, &["--cap", "drone", "--ticks", "1"]),
        "E_CAP_DENIED",
    );
    fixture.policy(r#"{"deny":[]}"#);
    check_error(
        &fixture.run(LINK_HOOK, &["--cap", "drone", "--ticks", "1"]),
        "E_CAP_MISSING",
    );
    fixture.policy(r#"{"deny":["drone.link_lost"]}"#);
    check_error(
        &fixture.run(LINK_HOOK, &["--cap", "drone=sim", "--ticks", "1"]),
        "E_CAP_DENIED",
    );
    fixture.policy(r#"{"deny":[]}"#);
    let simulated = fixture.run(
        LINK_HOOK,
        &[
            "--cap",
            "drone=sim",
            "--ticks",
            "11",
            "--tick-ms",
            "100",
            "--sim",
            "link_lost_after=0",
            "--plain",
        ],
    );
    assert!(
        simulated.status.success(),
        "{}",
        String::from_utf8_lossy(&simulated.stderr)
    );
    let out = String::from_utf8_lossy(&simulated.stdout);
    assert!(out.contains("simulation selected (testing only): drone"));
    assert!(out.contains("drone.rtl"));
}

#[test]
fn live_link_is_not_opened_before_the_policy_gate() {
    let fixture = Fixture::new();
    let socket = UdpSocket::bind("127.0.0.1:0").expect("reserve a real local UDP port");
    let backend = format!("drone=udp:{}", socket.local_addr().expect("local address"));
    let result = fixture.run(DRONE, &["--cap", &backend]);
    check_error(&result, "E_CAP_DENIED");
}

#[test]
fn live_drone_actuation_is_disabled_before_opening_the_link() {
    let fixture = Fixture::new();
    fixture.policy(r#"{"deny":[]}"#);
    let socket = UdpSocket::bind("127.0.0.1:0").expect("reserve a real local UDP port");
    let backend = format!("drone=udp:{}", socket.local_addr().expect("local address"));
    for command in [
        "arm drone",
        "takeoff to 10 meters",
        "fly to vector(1, 2, 3)",
        "land drone",
        "return to launch",
        "hold position",
        "stop drone",
    ] {
        let source = format!("needs: drone\naction main():\n    {command}\n");
        let result = fixture.run(&source, &["--cap", &backend]);
        check_error(&result, "E_CAP_DENIED");
        assert!(String::from_utf8_lossy(&result.stderr).contains("live drone actuation is disabled"));
    }
}

#[test]
fn repl_requires_policy_and_an_explicit_grant() {
    let fixture = Fixture::new();
    let input = ".needs log\n.cap log\naction main():\n    log \"actual log\"\n.run\n.exit\n";
    let run = || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kaze"))
            .current_dir(&fixture.0)
            .arg("repl")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start REPL");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write REPL input");
        child.wait_with_output().expect("wait for REPL")
    };
    let denied = run();
    assert!(String::from_utf8_lossy(&denied.stderr).contains("E_CAP_DENIED"));
    fixture.policy(r#"{"deny":[]}"#);
    let allowed = run();
    assert!(allowed.status.success());
    assert!(String::from_utf8_lossy(&allowed.stdout).contains("[info] actual log"));
}

#[test]
fn benchmark_reports_explicit_robotics_simulation() {
    let fixture = Fixture::new();
    fixture.policy(r#"{"deny":[]}"#);
    let source = fixture.0.join("drone.kaze");
    fs::write(&source, DRONE).expect("write test source");
    let output = Command::new(env!("CARGO_BIN_EXE_kaze"))
        .current_dir(&fixture.0)
        .args([
            "bench",
            "drone.kaze",
            "--cap",
            "drone=sim",
            "--iterations",
            "1",
            "--json",
        ])
        .output()
        .expect("run explicitly selected robotics simulation benchmark");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON benchmark report");
    assert_eq!(report["simulated_families"], serde_json::json!(["drone"]));
}
