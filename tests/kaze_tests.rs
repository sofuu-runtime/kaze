use kaze::caps::SimState;
use kaze::isa;
use kaze::{run_file, run_source, RunOptions, RunReport};

fn example(name: &str) -> String {
    format!("{}/examples/{}", env!("CARGO_MANIFEST_DIR"), name)
}

fn grants(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn run(name: &str, opts: &RunOptions) -> RunReport {
    let path = example(name);
    run_file(&path, opts).unwrap_or_else(|e| panic!("{} failed: {}", name, e.render()))
}

#[test]
fn every_example_matches_its_golden_trace() {
    fn golden(name: &str) -> &'static str {
        match name {
            "hello" => include_str!("../examples/golden/hello.txt"),
            "patrol" => include_str!("../examples/golden/patrol.txt"),
            "rover" => include_str!("../examples/golden/rover.txt"),
            "greenhouse" => include_str!("../examples/golden/greenhouse.txt"),
            "guardrail" => include_str!("../examples/golden/guardrail.txt"),
            "guardrail-reject" => include_str!("../examples/golden/guardrail-reject.txt"),
            other => panic!("no golden for {}", other),
        }
    }

    let mut args = serde_json::Map::new();
    args.insert("points".into(), serde_json::json!([1, 2]));
    let patrol = RunOptions { args, grants: grants(&["drone"]), ..Default::default() };

    let mut sim = SimState::default();
    sim.distance = 0.3;
    let rover = RunOptions {
        grants: grants(&["rover", "lidar"]),
        ticks: 1,
        sim,
        ..Default::default()
    };

    let mut args = serde_json::Map::new();
    args.insert("target_temp".into(), serde_json::json!(28));
    args.insert("target_humidity".into(), serde_json::json!(60));
    let greenhouse = RunOptions {
        args,
        grants: grants(&["sensor", "device", "alert"]),
        ..Default::default()
    };

    let mut args = serde_json::Map::new();
    args.insert("prompt".into(), serde_json::json!("what is the weather"));
    let allow = RunOptions { args, ..Default::default() };

    let mut args = serde_json::Map::new();
    args.insert("prompt".into(), serde_json::json!("my api_key is sk-live-9"));
    let reject = RunOptions { args, ..Default::default() };

    let cases: [(&str, &str, &RunOptions); 6] = [
        ("hello", "hello.kaze", &RunOptions::default()),
        ("patrol", "patrol.kaze", &patrol),
        ("rover", "rover.kaze", &rover),
        ("greenhouse", "greenhouse.kaze", &greenhouse),
        ("guardrail", "guardrail.kaze", &allow),
        ("guardrail-reject", "guardrail.kaze", &reject),
    ];

    for (name, file, opts) in cases {
        let report = run(file, opts);
        let actual = report.render_plain();
        let expected = golden(name);
        assert_eq!(
            actual, expected,
            "trace mismatch for {}:\n--- actual ---\n{}\n--- expected ---\n{}",
            name, actual, expected
        );
    }
}

#[test]
fn compiled_bytes_are_stable_and_reload_into_the_same_program() {
    let src = std::fs::read_to_string(example("patrol.kaze")).unwrap();
    let prep = kaze::prepare(&src, "patrol.kaze").unwrap();
    let bytes_a = isa::serialize(&prep.module);
    let bytes_b = isa::serialize(&prep.module);
    assert_eq!(bytes_a, bytes_b, "module serialization is not deterministic");

    let reloaded = isa::deserialize(&bytes_a).expect("module should deserialize");
    let mut args = serde_json::Map::new();
    args.insert("points".into(), serde_json::json!([1, 2]));
    let opts = RunOptions { args, grants: grants(&["drone"]), ..Default::default() };
    let rep_source = run("patrol.kaze", &opts);
    let rep_bc = kaze::run_module(reloaded, "patrol.kazebc", &opts).unwrap();
    assert_eq!(rep_source.actions, rep_bc.actions);
    assert_eq!(rep_source.fuel_used, rep_bc.fuel_used);
}

#[test]
fn fuel_stops_runaway_loop() {
    let src = "action loop():\n    while 1 == 1:\n        let x = 1\n";
    let opts = RunOptions { fuel: 50, ..Default::default() };
    let err = run_source(src, "<test>", &opts).unwrap_err();
    assert_eq!(err.code, "E_TIMEOUT");
    assert!(err.message.contains("fuel, not wall time"));
}

#[test]
fn undeclared_capability_fails_checker() {
    let src = "action test():\n    arm drone\n";
    let err = kaze::check_source(src, "<test>").unwrap_err();
    assert_eq!(err.code, "E_KAZE_CHECK");
    assert!(err.message.contains("not declared"));
}

#[test]
fn range_violations_are_rejected_not_clamped() {
    let src = "needs: drone\naction test():\n    takeoff to 900 meters\n";
    let opts = RunOptions { grants: grants(&["drone"]), ..Default::default() };
    let err = run_source(src, "<test>", &opts).unwrap_err();
    assert_eq!(err.code, "E_KAZE_RANGE");
    assert!(err.message.contains("rejected, not clamped"));
}
