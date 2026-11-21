// v0.9/v1.0: the artifact compatibility lock. A source compiled by v1.0 must
// produce exactly the bytes in the fixture, and those bytes must still load and
// run through the current reader.

use kaze::{isa, run_module, RunOptions};

const SRC: &str = "needs: log\naction main():\n    log \"release\"\n";

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

#[test]
fn the_v1_artifact_bytes_are_frozen() {
    let module = kaze::compile_source(SRC, "lock.kaze").expect("compiles");
    let bytes = isa::serialize(&module);
    let expected = include_str!("fixtures/kaze-v1.0.kazebc.hex").trim();
    assert_eq!(
        hex(&bytes),
        expected,
        "the v1.0 module bytes changed; a format bump is required"
    );

    let module = isa::deserialize(&bytes).expect("the frozen artifact still loads");
    let opts = RunOptions {
        grants: vec!["log".to_string()],
        policy: Some(serde_json::json!({ "deny": [] })),
        ..Default::default()
    };
    let report = run_module(module, "lock.kazebc", &opts).expect("the frozen artifact still runs");
    assert_eq!(report.logs.first().map(|(_, text)| text.as_str()), Some("release"));
}

#[test]
fn the_format_version_is_locked() {
    assert_eq!(isa::FORMAT_VERSION, 4);
}

#[test]
fn dual_syntax_human_and_machine_compile_and_run_identically() {
    let human_src = "needs: log\naction main():\n    log \"release\"\n";
    let machine_src = r#"(kaze
  (format 1)
  (needs "log")
  (action "main"
    (body
      (eval (call "log" "info" (string "release"))))))
"#;

    let human_module = kaze::compile_source(human_src, "human.kaze").expect("compiles human");
    let machine_module = kaze::compile_source(machine_src, "machine.kaze").expect("compiles machine");

    let opts = RunOptions {
        grants: vec!["log".to_string()],
        policy: Some(serde_json::json!({ "deny": [] })),
        ..Default::default()
    };

    let human_report = run_module(human_module, "human.kazebc", &opts).expect("runs human");
    let machine_report = run_module(machine_module, "machine.kazebc", &opts).expect("runs machine");

    assert_eq!(human_report.entry, machine_report.entry);
    assert_eq!(human_report.actions, machine_report.actions);
    assert_eq!(human_report.logs, machine_report.logs);
}
