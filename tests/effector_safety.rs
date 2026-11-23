//! Phase 5: capability gates, alert sinks, attenuation, effector watchdogs and
//! abnormal-termination cleanup.
//!
//! Three layers, each proved without fabricating success:
//!
//! * the registry sweep — every `(family, op)` in `kaze::caps::OPS` either
//!   crosses the gate and performs a real backend call, or is refused by name;
//! * the sinks — stdout/stderr/file alert records with a timestamp, credential
//!   redaction and a 1024-byte ceiling, plus an opt-in structured audit trail;
//! * the effectors — an uncatchable 1500 ms freshness veto (also exercised over
//!   a real socket in tests/live.rs) and signal-driven GPIO/PWM de-assert.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use kaze::arena::Arena;
use kaze::caps::{self, CapHost, SimState};
use kaze::hardware;
use kaze::host::LiveHost;
use kaze::value::{Heap, Value};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

/// A throwaway directory that is the run's working directory, so a relative
/// alert/audit/policy path lands somewhere this test owns.
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "kaze-effector-{}-{}-{}",
            tag,
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create the test directory");
        Fixture(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn policy(&self, contents: &str) {
        fs::write(self.path("policy.json"), contents).expect("write policy");
    }

    fn run(&self, source: &str, args: &[&str]) -> Output {
        self.run_with_env(source, args, &[])
    }

    fn run_with_env(&self, source: &str, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kaze"));
        command
            .current_dir(&self.0)
            .env_remove("KAZE_GPIO_ROOT")
            .env_remove("KAZE_PWM_ROOT")
            .env_remove("KAZE_AUDIT")
            .arg("run")
            .arg("-")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in envs {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("start Kaze");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(source.as_bytes())
            .expect("write source");
        child.wait_with_output().expect("wait for Kaze")
    }

    fn spawn(&self, source: &str, args: &[&str], envs: &[(&str, &str)]) -> Child {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kaze"));
        command
            .current_dir(&self.0)
            .env_remove("KAZE_GPIO_ROOT")
            .env_remove("KAZE_PWM_ROOT")
            .env_remove("KAZE_AUDIT")
            .arg("run")
            .arg("-")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (key, value) in envs {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("start a long-running Kaze");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(source.as_bytes())
            .expect("write source");
        child
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn expect_failure(output: &Output, code: &str) {
    assert!(
        !output.status.success(),
        "expected {}, got success: {}",
        code,
        stdout_of(output)
    );
    let err = stderr_of(output);
    assert!(err.contains(code), "expected {}, got {}", code, err);
}

fn expect_success(output: &Output) -> String {
    assert!(
        output.status.success(),
        "expected success, got {}",
        stderr_of(output)
    );
    stdout_of(output)
}

fn code_of(message: &str) -> &str {
    message
        .split_once(':')
        .map(|(head, _)| head.trim())
        .unwrap_or("")
}

// ── Item 1: every registry entry acts or fails closed ──────────────────────

/// Every key in `caps::OPS`, in the same order. Kept as a table so the sweep
/// below cannot silently cover a shorter list than the registry: the assertion
/// at the start of the sweep compares the two.
const RECIPE_KEYS: &[(&str, &str)] = &[
    ("drone", "arm"),
    ("drone", "land"),
    ("drone", "rtl"),
    ("drone", "hold"),
    ("drone", "stop"),
    ("drone", "takeoff"),
    ("drone", "goto"),
    ("drone", "telemetry"),
    ("rover", "forward"),
    ("rover", "backward"),
    ("rover", "turn_left"),
    ("rover", "turn_right"),
    ("rover", "stop"),
    ("rover", "telemetry"),
    ("sensor", "temperature"),
    ("sensor", "humidity"),
    ("sensor", "distance"),
    ("sensor", "front_distance"),
    ("sensor", "read"),
    ("sensor", "telemetry"),
    ("device", "set_relay"),
    ("device", "pulse"),
    ("device", "read_gpio"),
    ("device", "set_duty"),
    ("memory", "save"),
    ("memory", "recall"),
    ("policy", "detects"),
    ("policy", "detect"),
    ("policy", "allow"),
    ("policy", "reject"),
    ("log", "info"),
    ("log", "warn"),
    ("log", "error"),
    ("log", "debug"),
    ("alert", "send"),
    ("ai", "prompt"),
    ("ai", "complete"),
    ("ai", "classify"),
    ("tool", "call"),
    ("tool", "run"),
];

/// Well-typed arguments for one registry operation, or `None` when the table
/// above has no recipe. Text lives in the caller's arena.
fn arguments(family: &str, op: &str, arena: &mut Arena) -> Option<Vec<Value>> {
    let text = |arena: &mut Arena, value: &str| {
        Value::Str(arena.alloc(value).expect("the test arena holds the argument"))
    };
    let args = match (family, op) {
        ("drone", "arm" | "land" | "rtl" | "hold" | "stop") => vec![],
        ("drone", "takeoff") => vec![Value::Num(10.0)],
        ("drone", "goto") => vec![Value::Num(47.6), Value::Num(8.5), Value::Num(20.0)],
        ("drone", "telemetry") => vec![text(arena, "altitude")],
        ("rover", "forward" | "backward") => vec![Value::Num(1.0)],
        ("rover", "turn_left" | "turn_right") => vec![Value::Num(10.0)],
        ("rover", "stop") => vec![],
        ("rover", "telemetry") => vec![text(arena, "speed")],
        ("sensor", "temperature" | "humidity" | "distance" | "front_distance") => vec![],
        ("sensor", "read") => vec![text(arena, "probe")],
        ("sensor", "telemetry") => vec![text(arena, "temperature")],
        ("device", "set_relay") => vec![text(arena, "fan"), Value::Bool(true)],
        ("device", "pulse") => vec![text(arena, "fan"), Value::Num(1.0)],
        ("device", "read_gpio") => vec![text(arena, "fan")],
        ("device", "set_duty") => vec![text(arena, "fan"), Value::Num(50.0)],
        ("memory", "save") => vec![text(arena, "label"), text(arena, "value")],
        ("memory", "recall") => vec![text(arena, "label")],
        ("policy", "detects" | "detect") => vec![text(arena, "secrets"), text(arena, "hello")],
        ("policy", "allow") => vec![],
        ("policy", "reject") => vec![text(arena, "reason")],
        ("log", "info" | "warn" | "error" | "debug") => vec![text(arena, "message")],
        ("alert", "send") => vec![text(arena, "alert text")],
        ("ai", "prompt" | "complete") => vec![text(arena, "question")],
        ("ai", "classify") => vec![text(arena, "text"), text(arena, "a,b")],
        ("tool", "call" | "run") => vec![text(arena, "name")],
        _ => return None,
    };
    Some(args)
}

#[test]
fn every_registered_operation_acts_or_fails_closed_with_no_backend() {
    assert_eq!(
        RECIPE_KEYS,
        caps::OPS,
        "the sweep and the registry have drifted apart; add (or remove) a recipe"
    );

    for (family, op) in caps::OPS {
        let mut arena = Arena::new(8 * 1024);
        let mut heap = Heap::new();
        let args = arguments(family, op, &mut arena)
            .unwrap_or_else(|| panic!("no argument recipe for {}.{}", family, op));
        let mut host = LiveHost::new(SimState::default(), &[], &[], &[])
            .expect("a host with no configured backends");

        let result = host.call(family, op, &args, &mut arena, &mut heap);
        match *family {
            "log" => {
                let value = result.unwrap_or_else(|e| {
                    panic!("log.{} is a real local backend and must not fail: {}", op, e)
                });
                assert_eq!(value, Value::Unit);
                assert_eq!(
                    host.logs().len(),
                    1,
                    "log.{} must record a real log line",
                    op
                );
            }
            "policy" => {
                let value = result.unwrap_or_else(|e| {
                    panic!("policy.{} is a real local backend and must not fail: {}", op, e)
                });
                match *op {
                    "detects" | "detect" => assert_eq!(
                        value,
                        Value::Bool(false),
                        "the detector answered instead of fabricating a verdict"
                    ),
                    _ => assert_eq!(value, Value::Unit),
                }
                assert_eq!(host.actions().len(), 1, "policy.{} is traced", op);
            }
            _ => {
                let err = result.err().unwrap_or_else(|| {
                    panic!(
                        "{}.{} returned a value with no backend configured: it must fail closed",
                        family, op
                    )
                });
                let code = code_of(&err);
                assert!(
                    code == "E_CAP_MISSING" || code == "E_CAP_DENIED",
                    "{}.{} must fail closed with E_CAP_MISSING/E_CAP_DENIED, got {:?}: {}",
                    family,
                    op,
                    code,
                    err
                );
                // A refusal may be traced (that is evidence), but no action
                // line may claim a value came back from a backend that does
                // not exist.
                for action in host.actions() {
                    assert!(
                        action.result.starts_with("error:"),
                        "{}.{} fabricated a result with no backend: {:?}",
                        family,
                        op,
                        action.render()
                    );
                }
            }
        }
    }
}

/// The gate itself, walked over the whole registry: an undeclared family, an
/// ungranted family, a missing policy and a denying policy are each refused,
/// and the fully authorise case passes. `drone.link_lost` is not a registry
/// verb and is covered by the capability_safety suite.
#[test]
fn every_registered_operation_crosses_every_gate_stage() {
    let allow = serde_json::json!({ "deny": [] });
    for (family, op) in caps::OPS {
        let declared = vec![family.to_string()];
        let target = format!("{}.{}", family, op);

        let denial = hardware::authorise(&[], &declared, Some(&allow), family, op)
            .expect_err("H-1 must refuse an undeclared family");
        assert_eq!(denial.code, "E_CAP_UNDECLARED", "{}", target);

        let denial = hardware::authorise(&declared, &[], Some(&allow), family, op)
            .expect_err("H-2 must refuse an ungranted family");
        assert_eq!(denial.code, "E_CAP_MISSING", "{}", target);

        let denial = hardware::authorise(&declared, &declared, None, family, op)
            .expect_err("H-3 must refuse a missing policy");
        assert_eq!(denial.code, "E_CAP_DENIED", "{}", target);

        let denying = serde_json::json!({ "deny": [target] });
        let denial = hardware::authorise(&declared, &declared, Some(&denying), family, op)
            .expect_err("H-3 must refuse a denying policy");
        assert_eq!(denial.code, "E_CAP_DENIED", "{}", target);

        hardware::authorise(&declared, &declared, Some(&allow), family, op)
            .unwrap_or_else(|d| panic!("{} should be authorised, got {:?}", target, d.code));
    }
}

#[test]
fn a_configured_gpio_and_pwm_backend_is_really_driven() {
    let root = std::env::temp_dir().join(format!("kaze-effector-gpio-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("gpio17")).expect("fake sysfs root");
    fs::write(root.join("export"), "").expect("export file");
    fs::write(root.join("gpio17").join("value"), "0").expect("value file");
    let pwm = std::env::temp_dir().join(format!("kaze-effector-pwm-{}", std::process::id()));
    let _ = fs::remove_dir_all(&pwm);
    fs::create_dir_all(pwm.join("pwmchip0").join("pwm0")).expect("fake pwm root");
    fs::write(pwm.join("pwmchip0").join("export"), "").expect("pwm export");
    for file in ["period", "duty_cycle", "enable"] {
        fs::write(pwm.join("pwmchip0").join("pwm0").join(file), "0").expect("channel file");
    }
    std::env::set_var("KAZE_GPIO_ROOT", &root);
    std::env::set_var("KAZE_PWM_ROOT", &pwm);

    let mut host = LiveHost::new(
        SimState::default(),
        &[],
        &[("fan".to_string(), 17u16)],
        &[("mist".to_string(), 0u16, 0u16)],
    )
    .expect("a device host with a named pin and channel");
    let mut arena = Arena::new(4096);
    let mut heap = Heap::new();
    let text = |arena: &mut Arena, value: &str| Value::Str(arena.alloc(value).expect("arg"));

    let name = text(&mut arena, "fan");
    host.call("device", "set_relay", &[name, Value::Bool(true)], &mut arena, &mut heap)
        .expect("the relay is driven");
    assert_eq!(
        fs::read_to_string(root.join("gpio17").join("value")).expect("pin file"),
        "1",
        "the fake sysfs line really went high"
    );

    let name = text(&mut arena, "fan");
    let read_back = host
        .call("device", "read_gpio", &[name], &mut arena, &mut heap)
        .expect("the line is read back");
    assert_eq!(read_back, Value::Bool(true), "the read is the real file's value");

    let name = text(&mut arena, "fan");
    host.call("device", "pulse", &[name, Value::Num(1.0)], &mut arena, &mut heap)
        .expect("the pulse arms the line");

    let name = text(&mut arena, "mist");
    host.call("device", "set_duty", &[name, Value::Num(50.0)], &mut arena, &mut heap)
        .expect("the pwm channel is driven");
    assert_eq!(
        fs::read_to_string(pwm.join("pwmchip0").join("pwm0").join("duty_cycle"))
            .expect("duty file"),
        "10000000",
        "50% of the 20 ms period is a real write"
    );
    assert_eq!(
        fs::read_to_string(pwm.join("pwmchip0").join("pwm0").join("enable"))
            .expect("enable file"),
        "1"
    );
    assert_eq!(host.actions().len(), 4, "{:?}", host.actions());

    drop(host);
    std::env::remove_var("KAZE_GPIO_ROOT");
    std::env::remove_var("KAZE_PWM_ROOT");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&pwm);
}

#[test]
fn the_explicitly_selected_drone_simulator_changes_real_state() {
    let mut host = LiveHost::new(
        SimState::default(),
        &[("drone".to_string(), "sim".to_string())],
        &[],
        &[],
    )
    .expect("an explicitly selected simulator");
    let mut arena = Arena::new(4096);
    let mut heap = Heap::new();

    host.call("drone", "arm", &[], &mut arena, &mut heap)
        .expect("the selected simulator arms");
    let field = Value::Str(arena.alloc("armed").expect("arg"));
    assert_eq!(
        host.call("drone", "telemetry", &[field], &mut arena, &mut heap)
            .expect("telemetry"),
        Value::Bool(true),
        "the simulator's state changed; the reading is not a canned constant"
    );
    host.call("drone", "takeoff", &[Value::Num(15.0)], &mut arena, &mut heap)
        .expect("the selected simulator takes off");
    let field = Value::Str(arena.alloc("altitude").expect("arg"));
    assert_eq!(
        host.call("drone", "telemetry", &[field], &mut arena, &mut heap)
            .expect("telemetry"),
        Value::Num(15.0)
    );
}

// ── Item 2: real alert sinks, redaction, ceiling, audit trail ──────────────

#[test]
fn an_alert_sink_writes_a_timestamped_line_to_stdout_stderr_and_a_file() {
    let fixture = Fixture::new("alert-sinks");
    fixture.policy(r#"{"deny":[]}"#);
    let source = "needs: alert\naction main():\n    send alert \"fan failure at 90C\"\n";

    let stdout_run = fixture.run(source, &["--cap", "alert=stdout", "--plain"]);
    let out = expect_success(&stdout_run);
    let line = out
        .lines()
        .find(|line| line.contains(" alert fan failure at 90C"))
        .unwrap_or_else(|| panic!("no alert record on stdout: {}", out));
    assert_eq!(line.as_bytes().get(4), Some(&b'-'), "a UTC date: {}", line);
    assert_eq!(line.as_bytes().get(10), Some(&b'T'), "date and time split: {}", line);
    assert!(!stderr_of(&stdout_run).contains(" alert fan failure"), "not on stderr");

    let stderr_run = fixture.run(source, &["--cap", "alert=stderr", "--plain"]);
    expect_success(&stderr_run);
    assert!(
        stderr_of(&stderr_run).contains(" alert fan failure at 90C"),
        "{}",
        stderr_of(&stderr_run)
    );
    assert!(!stdout_of(&stderr_run).contains(" alert fan failure"), "not on stdout");

    // The file sink appends one line per alert across runs.
    fixture.run(source, &["--cap", "alert=file:alerts.log"]);
    fixture.run(source, &["--cap", "alert=file:alerts.log"]);
    let text = fs::read_to_string(fixture.path("alerts.log")).expect("the file sink wrote");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "one line per alert, appended: {}", text);
    for line in &lines {
        assert!(line.contains(" alert fan failure at 90C"), "{}", line);
        assert_eq!(line.as_bytes().get(4), Some(&b'-'), "timestamp: {}", line);
    }
}

#[test]
fn an_alert_redacts_credentials_and_stays_under_the_byte_ceiling() {
    let fixture = Fixture::new("alert-redaction");
    fixture.policy(r#"{"deny":[]}"#);
    let source = "needs: alert\naction main():\n    send alert \"api_key=abcd1234efgh bearer eyJhbGciOi.def password=hunter2 key sk-live-0123456789\"\n";
    let run = fixture.run(source, &["--cap", "alert=file:alerts.log", "--plain"]);
    let report = expect_success(&run);
    let text = fs::read_to_string(fixture.path("alerts.log")).expect("the file sink wrote");
    for secret in ["abcd1234efgh", "eyJhbGciOi.def", "hunter2", "0123456789"] {
        assert!(!text.contains(secret), "the alert file leaked '{}': {}", secret, text);
        assert!(
            !report.contains(secret),
            "the run report leaked '{}': {}",
            secret,
            report
        );
    }
    assert!(text.contains("***"), "the redaction is visible: {}", text);

    // A message over the ceiling is truncated and marked, never written whole.
    let long = "x".repeat(4_000);
    let long_source =
        format!("needs: alert\naction main():\n    send alert \"{}\"\n", long);
    expect_success(&fixture.run(&long_source, &["--cap", "alert=file:long.log"]));
    let text = fs::read_to_string(fixture.path("long.log")).expect("the file sink wrote");
    let line = text.lines().next().expect("one record");
    let (_, body) = line.split_once(" alert ").expect("a record body");
    assert!(body.len() <= 1024, "the alert body is {} bytes", body.len());
    assert!(body.ends_with("[truncated]"), "the reader is told it was cut");
}

#[test]
fn an_alert_without_a_sink_is_refused_and_names_the_fix() {
    let fixture = Fixture::new("alert-missing");
    fixture.policy(r#"{"deny":[]}"#);
    let source = "needs: alert\naction main():\n    send alert \"nobody hears this\"\n";
    let grant_only = fixture.run(source, &["--cap", "alert"]);
    expect_failure(&grant_only, "E_CAP_MISSING");
    assert!(
        stderr_of(&grant_only).contains("--cap alert=stdout"),
        "the refusal names the fix: {}",
        stderr_of(&grant_only)
    );

    let bad_sink = fixture.run(source, &["--cap", "alert=telegram"]);
    expect_failure(&bad_sink, "E_CAP_MISSING");
    assert!(
        stderr_of(&bad_sink).contains("is not an alert sink"),
        "{}",
        stderr_of(&bad_sink)
    );
}

#[test]
fn the_audit_trail_is_off_by_default_and_records_one_line_per_decision() {
    let fixture = Fixture::new("audit");
    fixture.policy(r#"{"deny":[]}"#);
    let source = "needs: log\naction main():\n    log \"hello\"\n";

    // Off: no file is created, and the run is unaffected.
    expect_success(&fixture.run(source, &["--cap", "log", "--plain"]));
    assert!(
        !fixture.path("audit.log").exists(),
        "the audit trail must be opt-in"
    );

    // On: exactly one line, with family, op, decision and reason.
    expect_success(&fixture.run_with_env(
        source,
        &["--cap", "log", "--plain"],
        &[("KAZE_AUDIT", "file:audit.log")],
    ));
    let text = fs::read_to_string(fixture.path("audit.log")).expect("the audit sink wrote");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one line per gated call: {}", text);
    let line = lines[0];
    assert!(line.contains("capability log.info"), "{}", line);
    assert!(line.contains("decision=allow"), "{}", line);
    assert!(line.contains("reason=H-1..H-3 passed"), "{}", line);
    assert_eq!(line.as_bytes().get(4), Some(&b'-'), "UTC timestamp: {}", line);

    // A policy denial is recorded as a denial, with the reason that refused it.
    fixture.policy(r#"{"deny":["log.info"]}"#);
    let denied = fixture.run_with_env(
        source,
        &["--cap", "log", "--plain"],
        &[("KAZE_AUDIT", "file:denied.log")],
    );
    expect_failure(&denied, "E_CAP_DENIED");
    let text = fs::read_to_string(fixture.path("denied.log")).expect("the audit sink wrote");
    assert!(
        text.contains("capability log.info decision=deny"),
        "the denial is audited: {}",
        text
    );
    assert!(text.contains("E_CAP_DENIED/H-3/policy"), "{}", text);
}

#[test]
fn an_audit_line_carries_no_credential_and_a_broken_sink_fails_closed() {
    let fixture = Fixture::new("audit-secrets");
    fixture.policy(r#"{"deny":[]}"#);
    let source = "needs: alert\naction main():\n    send alert \"password=hunter2 key sk-live-0123456789\"\n";
    expect_success(&fixture.run_with_env(
        source,
        &["--cap", "alert=file:alerts.log", "--plain"],
        &[("KAZE_AUDIT", "file:audit.log")],
    ));
    let audit = fs::read_to_string(fixture.path("audit.log")).expect("the audit sink wrote");
    let alert = fs::read_to_string(fixture.path("alerts.log")).expect("the alert sink wrote");
    for secret in ["hunter2", "0123456789"] {
        assert!(!audit.contains(secret), "audit leaked '{}': {}", secret, audit);
        assert!(!alert.contains(secret), "alert leaked '{}': {}", secret, alert);
    }
    assert!(audit.contains("capability alert.send decision=allow"), "{}", audit);

    // A trail that cannot be written denies the capability: telemetry that
    // silently disappears is worse than no telemetry.
    fixture.policy(r#"{"deny":[]}"#);
    let broken = fixture.run_with_env(
        "needs: log\naction main():\n    log \"hello\"\n",
        &["--cap", "log", "--plain"],
        &[("KAZE_AUDIT", "file:no-such-directory/audit.log")],
    );
    expect_failure(&broken, "E_CAP_DENIED");
    assert!(
        stderr_of(&broken).contains("audit"),
        "the refusal names the audit sink: {}",
        stderr_of(&broken)
    );
}

// ── Item 3: monotonic attenuation (host side) ──────────────────────────────

#[test]
fn a_routine_that_needs_drone_cannot_run_with_only_a_log_grant() {
    let source = "needs: drone\naction main():\n    arm drone\n";
    let opts = kaze::RunOptions {
        grants: vec!["log".to_string()],
        policy: Some(serde_json::json!({ "deny": [] })),
        ..Default::default()
    };
    let err = kaze::run_source(source, "attenuation.kaze", &opts)
        .expect_err("a log grant does not cover a drone routine");
    assert_eq!(err.code, "E_CAP_MISSING", "{}", err.message);
    assert!(err.message.contains("did not grant 'drone'"), "{}", err.message);
}

#[test]
fn both_execution_paths_refuse_a_grant_the_script_does_not_hold() {
    let source = "needs: drone\naction main():\n    arm drone\n";
    let module = kaze::compile_source(source, "attenuation.kaze").expect("it compiles");
    let opts = kaze::RunOptions {
        grants: vec!["log".to_string()],
        policy: Some(serde_json::json!({ "deny": [] })),
        ..Default::default()
    };
    let from_source = kaze::run_source(source, "attenuation.kaze", &opts)
        .expect_err("the source path refuses the missing grant");
    let from_module = kaze::run_module(module, "attenuation.kazebc", &opts)
        .expect_err("the bytecode path refuses the missing grant too");
    assert_eq!(from_source.code, "E_CAP_MISSING");
    assert_eq!(from_module.code, "E_CAP_MISSING");
}

#[test]
fn a_module_whose_declaration_was_stripped_is_refused_at_runtime() {
    // A deployed artifact can be hand-built or edited; the runtime gate must
    // still refuse a capability the module does not declare, even when the
    // host would grant it. This is the second lock behind the checker.
    let source = "needs: drone\naction main():\n    arm drone\n";
    let mut module = kaze::compile_source(source, "stripped.kaze").expect("it compiles");
    module.needs.clear();
    let opts = kaze::RunOptions {
        grants: vec!["drone".to_string()],
        policy: Some(serde_json::json!({ "deny": [] })),
        backend: vec![("drone".to_string(), "sim".to_string())],
        ..Default::default()
    };
    let err = kaze::run_module(module, "stripped.kazebc", &opts)
        .expect_err("an undeclared capability is refused before the host");
    assert_eq!(err.code, "E_CAP_UNDECLARED", "{}", err.message);
}

#[test]
fn the_language_checker_rejects_an_undeclared_family_before_any_run() {
    // The enforcement point for attenuation is the front end: a routine that
    // uses a family its file never declared does not compile. (The host-side
    // locks above cover artifacts that bypass the checker.)
    let source = "needs: log\naction main():\n    arm drone\n";
    let err = kaze::check_source(source, "undeclared.kaze")
        .expect_err("the checker refuses an undeclared family");
    assert!(
        err.code == "E_CAP_UNDECLARED",
        "expected E_CAP_UNDECLARED, got {}: {}",
        err.code,
        err.message
    );
}

// ── Item 5: abnormal-termination cleanup ───────────────────────────────────

fn fake_gpio_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "kaze-effector-cleanup-{}-{}",
        tag,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("gpio17")).expect("fake sysfs root");
    fs::write(root.join("export"), "").expect("export file");
    fs::write(root.join("gpio17").join("value"), "0").expect("value file");
    root
}

fn fake_pwm_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "kaze-effector-cleanup-{}-{}",
        tag,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("pwmchip0").join("pwm0")).expect("fake pwm root");
    fs::write(root.join("pwmchip0").join("export"), "").expect("pwm export");
    for file in ["period", "duty_cycle", "enable"] {
        fs::write(root.join("pwmchip0").join("pwm0").join(file), "0").expect("channel file");
    }
    root
}

fn wait_for_contents(path: &Path, want: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(text) = fs::read_to_string(path) {
            if text.trim() == want {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

/// Send a POSIX signal to a running child and require that the child dies from
/// it (not from an exit code), so the handler's re-raise is part of the proof.
fn terminate_with_signal(child: &mut Child, signal: &str) -> i32 {
    let status = Command::new("kill")
        .arg(format!("-{}", signal))
        .arg(child.id().to_string())
        .status()
        .expect("send the signal");
    assert!(status.success(), "kill -{} failed", signal);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait().expect("poll the child") {
            Some(status) => {
                use std::os::unix::process::ExitStatusExt;
                let received = status.signal().unwrap_or_else(|| {
                    panic!("the child exited with {:?} instead of dying from SIG{}", status.code(), signal)
                });
                return received;
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("the child survived SIG{} past the deadline", signal);
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn assert_cleanup_signal(tag: &str, signal: &str, expected: i32) {
    let fixture = Fixture::new(tag);
    fixture.policy(r#"{"deny":[]}"#);

    if signal == "INT" || signal == "TERM" {
        // GPIO: a pin driven high by a long-running supervisor.
        let root = fake_gpio_root(&format!("{}-gpio", tag));
        let env_value = format!("{}", root.display());
        let source = "needs: fan\n\nevery 1 seconds:\n    turn on fan\n";
        let mut child = fixture.spawn(
            source,
            &[
                "--cap", "device", "--pin", "fan=17", "--ticks", "100", "--tick-ms", "100",
                "--pace",
            ],
            &[("KAZE_GPIO_ROOT", env_value.as_str())],
        );
        assert!(
            wait_for_contents(&root.join("gpio17").join("value"), "1", Duration::from_secs(5)),
            "the pin never went high; nothing to clean up"
        );
        let received = terminate_with_signal(&mut child, signal);
        assert_eq!(received, expected, "the process died from SIG{}", signal);
        assert_eq!(
            fs::read_to_string(root.join("gpio17").join("value")).expect("pin file").trim(),
            "0",
            "SIG{} must de-assert the GPIO line",
            signal
        );
        let _ = fs::remove_dir_all(&root);
    }

    if signal == "TERM" {
        // PWM: a channel switched on by a long-running supervisor.
        let root = fake_pwm_root(&format!("{}-pwm", tag));
        let env_value = root.display().to_string();
        let source = "needs: fan\n\nevery 1 seconds:\n    device.set_duty(\"fan\", 60)\n";
        let mut child = fixture.spawn(
            source,
            &[
                "--cap", "device", "--pwm", "fan=0:0", "--ticks", "100", "--tick-ms", "100",
                "--pace",
            ],
            &[("KAZE_PWM_ROOT", env_value.as_str())],
        );
        let enable = root.join("pwmchip0").join("pwm0").join("enable");
        assert!(
            wait_for_contents(&enable, "1", Duration::from_secs(5)),
            "the channel never enabled; nothing to clean up"
        );
        let received = terminate_with_signal(&mut child, signal);
        assert_eq!(received, expected, "the process died from SIGTERM");
        assert_eq!(
            fs::read_to_string(&enable).expect("enable file").trim(),
            "0",
            "SIGTERM must disable the PWM channel"
        );
        let _ = fs::remove_dir_all(&root);
    }
}

#[test]
fn sigint_de_asserts_a_gpio_line() {
    assert_cleanup_signal("sigint", "INT", 2);
}

#[test]
fn sigterm_de_asserts_gpio_and_pwm_outputs() {
    assert_cleanup_signal("sigterm", "TERM", 15);
}
