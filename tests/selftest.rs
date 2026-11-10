// The software scorecard checks pure computation and gate refusals; it does
// not certify live hardware or flight readiness.

use std::process::Command;

#[test]
fn the_selftest_scorecard_passes() {
    let out = Command::new(env!("CARGO_BIN_EXE_kaze"))
        .args(["selftest", "--json"])
        .output()
        .expect("kaze selftest runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let report: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("selftest emits JSON");
    assert_eq!(report["pass"], true, "{}", report);
    assert!(
        report["version"].as_str().unwrap_or_default().starts_with("1."),
        "{}",
        report
    );
    assert_eq!(report["format"], 4);

    let checks = report["checks"].as_array().expect("checks");
    assert_eq!(checks.len(), 11);
    for check in checks {
        assert_eq!(check["pass"], true, "{}", check);
    }
}
