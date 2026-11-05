// v0.8: mission benchmarks. The 8-waypoint mission is deterministic in the
// simulator, and `kaze bench` reports stable timings without changing the trace.

use std::process::Command;

fn mission() -> String {
    format!("{}/examples/missions/eight_waypoints.kaze", env!("CARGO_MANIFEST_DIR"))
}

fn json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "bench did not emit JSON: {} (stderr: {})",
            e,
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn the_eight_waypoint_mission_is_benchmarked_and_deterministic() {
    let out = Command::new(env!("CARGO_BIN_EXE_kaze"))
        .arg("bench")
        .arg(mission())
        .args([
            "--iterations", "3", "--cap", "drone", "--cap", "log", "--json",
        ])
        .output()
        .expect("kaze bench runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let report = json(&out);
    assert_eq!(report["iterations"], 3);
    assert_eq!(report["actions"], 12, "one log, arm, takeoff, eight gotos, RTL");
    assert_eq!(report["gotos"], 8, "eight waypoint commands");
    assert_eq!(report["deterministic"], true);
    assert!(report["trace_fnv"].as_u64().is_some(), "trace fingerprint is reported");
    assert!(report["instructions"].as_u64().unwrap() > 0);
    assert!(report["median_ms"].as_f64().unwrap() >= 0.0);
}

#[test]
fn a_compiled_mission_benchmarks_the_same_trace() {
    let dir = std::env::temp_dir().join(format!("kaze-bench-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let artifact = dir.join("eight.kazebc");
    let bin = env!("CARGO_BIN_EXE_kaze");

    let compiled = Command::new(bin)
        .arg("compile")
        .arg(mission())
        .arg("-o")
        .arg(&artifact)
        .output()
        .expect("kaze compile runs");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let out = Command::new(bin)
        .arg("bench")
        .arg(&artifact)
        .args([
            "--iterations", "2", "--cap", "drone", "--cap", "log", "--json",
        ])
        .output()
        .expect("kaze bench runs on the artifact");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let report = json(&out);
    assert_eq!(report["iterations"], 2);
    assert_eq!(report["actions"], 12);
    assert_eq!(report["gotos"], 8);
    assert_eq!(report["deterministic"], true);
}
