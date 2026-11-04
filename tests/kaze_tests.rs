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
            "kinematics" => include_str!("../examples/golden/kinematics.txt"),
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

    let cases: [(&str, &str, &RunOptions); 7] = [
        ("hello", "hello.kaze", &RunOptions::default()),
        ("patrol", "patrol.kaze", &patrol),
        ("rover", "rover.kaze", &rover),
        ("greenhouse", "greenhouse.kaze", &greenhouse),
        ("guardrail", "guardrail.kaze", &allow),
        ("guardrail-reject", "guardrail.kaze", &reject),
        ("kinematics", "kinematics.kaze", &RunOptions::default()),
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


// v0.3: kinematics and spatial maths

/// The argument of every traced `log` line, in order. A trace line reads
/// `1. log.info(X) -> nothing`, so the value is what sits inside the parens.
fn logged(src: &str) -> Vec<String> {
    let opts = RunOptions { grants: grants(&["log"]), ..Default::default() };
    let report = run_source(src, "<math>", &opts).expect("the script should run");
    report
        .actions
        .iter()
        .map(|line| {
            let start = line.find('(').map(|i| i + 1).unwrap_or(0);
            let end = line.rfind(')').unwrap_or(line.len());
            line[start..end].to_string()
        })
        .collect()
}

fn one_log(src: &str) -> String {
    let mut all = logged(src);
    assert_eq!(all.len(), 1, "expected one log line, got {:?}", all);
    all.remove(0)
}

fn number(text: &str) -> f64 {
    text.trim_matches('"').parse().expect("a number in the trace")
}

#[test]
fn a_distance_between_two_places_is_geodesic_metres() {
    let out = one_log(
        "needs: log\naction main():\n    log distance between vector(0, 0, 0) and vector(1, 0, 0)\n",
    );
    let metres = number(&out);
    assert!((metres - 111_194.9).abs() < 50.0, "{metres}");
}

#[test]
fn bearings_are_compass_degrees() {
    let out = one_log(
        "needs: log\naction main():\n    log bearing from vector(0, 0, 0) to vector(0, 1, 0)\n",
    );
    // A tolerance, not an exact string: sin/cos/atan2 come from the platform
    // libm, and the last bit of a different libm must not fail this test.
    let deg = number(&out);
    assert!((deg - 90.0).abs() < 1e-9, "east is 90 degrees: {deg}");
}

#[test]
fn points_add_to_points_and_scale_by_numbers() {
    let out = one_log(
        "needs: log\naction main():\n    let d = vector(1, 2, 3) - vector(1, 1, 1)\n    log d * 3\n",
    );
    assert_eq!(out, "(0, 3, 6)");
}

#[test]
fn length_of_a_point_is_its_magnitude() {
    let out = one_log("needs: log\naction main():\n    log length of vector(3, 4, 0)\n");
    assert_eq!(out, "5");
}

#[test]
fn a_degree_conversion_goes_both_ways() {
    let out = logged(
        "needs: log\naction main():\n    log 180 in radians\n    log 3.141592653589793 in degrees\n",
    );
    assert_eq!(out.len(), 2, "{:?}", out);
    let rad = number(&out[0]);
    let deg = number(&out[1]);
    assert!((rad - std::f64::consts::PI).abs() < 1e-12, "{rad}");
    assert!((deg - 180.0).abs() < 1e-9, "{deg}");
}

#[test]
fn two_separately_built_points_are_equal() {
    let out = one_log(
        "needs: log\naction main():\n    if vector(1, 2, 3) == vector(1, 2, 3):\n        log \"same\"\n",
    );
    assert_eq!(out, "\"same\"");
}

#[test]
fn a_point_and_a_number_are_not_additive() {
    let src = "needs: log\naction main():\n    let x = vector(1, 2, 3) + 4\n    log x\n";
    let err = kaze::check_source(src, "<math>").unwrap_err();
    assert_eq!(err.code, "E_KAZE_CHECK");
    assert!(err.message.contains("points add to points"), "{}", err.message);
}

#[test]
fn a_point_needs_two_or_three_numbers() {
    let src = "needs: log\naction main():\n    log vector(1)\n";
    let err = kaze::check_source(src, "<math>").unwrap_err();
    assert_eq!(err.code, "E_KAZE_CHECK");
    assert!(err.message.contains("2 or 3 values"), "{}", err.message);
}

#[test]
fn the_square_root_of_a_negative_number_stops_the_run() {
    let src = "needs: log\naction main():\n    log sqrt(0 - 4)\n";
    let opts = RunOptions { grants: grants(&["log"]), ..Default::default() };
    let err = run_source(src, "<math>", &opts).unwrap_err();
    assert_eq!(err.code, "E_KAZE_MATH");
    assert!(err.message.contains("no real answer"), "{}", err.message);
}

#[test]
fn distance_needs_the_word_and() {
    let src = "needs: log\naction main():\n    log distance between vector(1, 2, 3) vector(1, 2, 4)\n";
    let err = kaze::check_source(src, "<math>").unwrap_err();
    assert_eq!(err.code, "E_KAZE_PARSE");
}

#[test]
fn a_point_can_be_built_from_the_vehicles_own_position() {
    let src = "needs: drone, log\naction main():\n    let here = vector(latitude, longitude, 0)\n    let waypoint = vector(47.6004, 8.5004, 0)\n    log distance between here and waypoint\n";
    let mut sim = SimState::default();
    sim.lat = 47.6;
    sim.lon = 8.5;
    let opts = RunOptions { grants: grants(&["drone", "log"]), sim, ..Default::default() };
    let report = run_source(src, "<math>", &opts).expect("the script should run");
    // Ambient telemetry reads are traced too, so find the log line rather than
    // assuming it is the first action.
    let line = report
        .actions
        .iter()
        .find(|a| a.contains("log.info"))
        .expect("the script logs its answer");
    let start = line.find('(').expect("a call") + 1;
    let end = line.rfind(')').expect("a call");
    let metres: f64 = line[start..end].parse().expect("a number");
    assert!((metres - 53.4).abs() < 2.0, "{metres}");
}

// Regressions found by auditing v0.3 against the plan, not by writing it.

#[test]
fn a_two_component_vector_is_a_point_on_the_ground() {
    let out = one_log("needs: log\naction main():\n    log vector(3, 4)\n");
    assert_eq!(out, "(3, 4, 0)");
}

#[test]
fn a_points_components_can_be_read_back() {
    let out = logged(
        "needs: log\naction main():\n    let p = vector(7, 8, 9)\n    log p[0]\n    log p[2]\n",
    );
    assert_eq!(out, vec!["7".to_string(), "9".to_string()]);
}

#[test]
fn a_component_past_the_third_is_out_of_bounds() {
    let src = "needs: log\naction main():\n    let p = vector(1, 2, 3)\n    log p[3]\n";
    let opts = RunOptions { grants: grants(&["log"]), ..Default::default() };
    let err = run_source(src, "<math>", &opts).unwrap_err();
    assert_eq!(err.code, "E_KAZE_BOUNDS", "{}", err.message);
    assert!(err.message.contains("components 0, 1 and 2"), "{}", err.message);
}

#[test]
fn negating_a_point_flips_every_component() {
    let out = one_log("needs: log\naction main():\n    log 0 - vector(1, 2, 3)\n");
    assert_eq!(out, "(-1, -2, -3)");
}

#[test]
fn a_point_in_a_log_line_is_not_a_question_mark() {
    let src = "needs: log\naction main():\n    log vector(1, 2, 3)\n";
    let opts = RunOptions { grants: grants(&["log"]), ..Default::default() };
    let report = run_source(src, "<math>", &opts).expect("it runs");
    assert_eq!(report.logs.len(), 1);
    assert_eq!(report.logs[0].1, "(1, 2, 3)", "a point must render, not (?, ?, ?)");
}

#[test]
fn a_v03_module_survives_serialization() {
    let src = std::fs::read_to_string(example("kinematics.kaze")).unwrap();
    let prep = kaze::prepare(&src, "kinematics.kaze").unwrap();
    let bytes = isa::serialize(&prep.module);
    let reloaded = isa::deserialize(&bytes).expect("a v0.3 module must reload");
    let opts = RunOptions::default();
    let from_source = run("kinematics.kaze", &opts);
    let from_bytes = kaze::run_module(reloaded, "kinematics.kazebc", &opts).unwrap();
    assert_eq!(from_source.actions, from_bytes.actions);
    assert_eq!(from_source.fuel_used, from_bytes.fuel_used);
}

// The rest of v0.3: a bearing from the machine's own position, an offset's
// heading, slant range, and the refusals that keep them honest.

#[test]
fn bearing_to_measures_from_the_machines_own_position() {
    let src = "needs: drone, log\naction main():\n    log bearing to vector(47.6004, 8.5004, 0)\n";
    let mut sim = SimState::default();
    sim.lat = 47.6;
    sim.lon = 8.5;
    let opts = RunOptions { grants: grants(&["drone", "log"]), sim, ..Default::default() };
    let report = run_source(src, "<math>", &opts).expect("it runs");
    let line = report
        .actions
        .iter()
        .find(|a| a.contains("log.info"))
        .expect("a log line");
    let start = line.find('(').expect("a call") + 1;
    let end = line.rfind(')').expect("a call");
    let deg: f64 = line[start..end].parse().expect("a number");
    assert!((deg - 33.99).abs() < 0.05, "{deg}");
    // It read the position from the machine, visibly, rather than inventing one.
    assert!(report.actions.iter().any(|a| a.contains("drone.telemetry(\"latitude\")")));
}

#[test]
fn bearing_to_without_a_position_to_measure_from_says_what_to_write() {
    let src = "needs: log\naction main():\n    log bearing to vector(47.6, 8.5, 0)\n";
    let err = kaze::check_source(src, "<math>").unwrap_err();
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(err.message.contains("bearing from here to target"), "{}", err.message);
}

#[test]
fn bearing_of_an_offset_is_its_compass_direction() {
    let out = one_log("needs: log\naction main():\n    log bearing of vector(1, 1, 0)\n");
    let deg = number(&out);
    assert!((deg - 45.0).abs() < 1e-6, "{deg}");
}

#[test]
fn slant_distance_includes_the_climb() {
    let out = logged(
        "needs: log\naction main():\n    log distance between vector(47.6, 8.5, 0) and vector(47.6004, 8.5004, 30)\n    log slant distance between vector(47.6, 8.5, 0) and vector(47.6004, 8.5004, 30)\n",
    );
    let ground = number(&out[0]);
    let slant = number(&out[1]);
    assert!(slant > ground, "{slant} should exceed {ground}");
    assert!((slant - (ground * ground + 900.0).sqrt()).abs() < 1e-6, "{slant}");
}

#[test]
fn a_value_that_cannot_be_a_place_is_refused() {
    let src = "needs: log\naction main():\n    log distance between vector(100, 200, 0) and vector(0, 0, 0)\n";
    let opts = RunOptions { grants: grants(&["log"]), ..Default::default() };
    let err = run_source(src, "<math>", &opts).unwrap_err();
    assert_eq!(err.code, "E_KAZE_RANGE", "{}", err.message);
    assert!(err.message.contains("not a latitude and longitude"), "{}", err.message);
}
