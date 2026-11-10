use kaze::caps::SimState;
use kaze::isa;
use kaze::{run_file, run_source, RunOptions, RunReport};

fn example(name: &str) -> String {
    format!("{}/examples/{}", env!("CARGO_MANIFEST_DIR"), name)
}

fn grants(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn approved() -> RunOptions {
    RunOptions { policy: Some(serde_json::json!({ "deny": [] })), ..Default::default() }
}

fn drone_sim() -> Vec<(String, String)> {
    vec![("drone".into(), "sim".into())]
}

fn cli_policy_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("kaze-examples-{}-{}", label, std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test directory");
    std::fs::write(dir.join("policy.json"), r#"{"deny":[]}"#).expect("write policy");
    dir
}

fn run(name: &str, opts: &RunOptions) -> RunReport {
    let path = example(name);
    run_file(&path, opts).unwrap_or_else(|e| panic!("{} failed: {}", name, e.render()))
}

#[test]
fn supported_examples_match_their_golden_traces() {
    fn golden(name: &str) -> &'static str {
        match name {
            "hello" => include_str!("../examples/golden/hello.txt"),
            "patrol" => include_str!("../examples/golden/patrol.txt"),
            "guardrail-reject" => include_str!("../examples/golden/guardrail-reject.txt"),
            "kinematics" => include_str!("../examples/golden/kinematics.txt"),
            "fault_tolerance" => include_str!("../examples/golden/fault_tolerance.txt"),
            "eight_waypoints" => include_str!("../examples/golden/eight_waypoints.txt"),
            other => panic!("no golden for {}", other),
        }
    }

    let mut args = serde_json::Map::new();
    args.insert("points".into(), serde_json::json!([1, 2]));
    let patrol = RunOptions { args, grants: grants(&["drone", "log"]), backend: drone_sim(), ..approved() };

    let mut args = serde_json::Map::new();
    args.insert("prompt".into(), serde_json::json!("my api_key is sk-live-9"));
    let reject = RunOptions { args, grants: grants(&["policy"]), ..approved() };

    let fault = RunOptions {
        grants: grants(&["drone", "log"]),
        backend: drone_sim(),
        ticks: 35,
        tick_ms: 100,
        sim: SimState { link_lost_after_ms: Some(0), ..Default::default() },
        ..approved()
    };

    let mission = RunOptions {
        grants: grants(&["drone", "log"]),
        backend: drone_sim(),
        ..approved()
    };
    let local = RunOptions { grants: grants(&["log"]), ..approved() };

    let cases: [(&str, &str, &RunOptions); 6] = [
        ("hello", "hello.kaze", &local),
        ("patrol", "patrol.kaze", &patrol),
        ("guardrail-reject", "guardrail.kaze", &reject),
        ("kinematics", "kinematics.kaze", &local),
        ("fault_tolerance", "fault_tolerance.kaze", &fault),
        ("eight_waypoints", "missions/eight_waypoints.kaze", &mission),
    ];

    for (name, file, opts) in cases {
        let report = run(file, opts);
        if !opts.backend.is_empty() {
            assert_eq!(report.simulated_families, vec!["drone"]);
        }
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
fn examples_requiring_unavailable_nonrobotics_backends_refuse() {
    let mut greenhouse_args = serde_json::Map::new();
    greenhouse_args.insert("target_temp".into(), serde_json::json!(28));
    greenhouse_args.insert("target_humidity".into(), serde_json::json!(60));
    let greenhouse = RunOptions {
        args: greenhouse_args,
        grants: grants(&["sensor", "device", "alert"]),
        ..approved()
    };
    let mut guardrail_args = serde_json::Map::new();
    guardrail_args.insert("prompt".into(), serde_json::json!("what is the weather"));
    let guardrail = RunOptions {
        args: guardrail_args,
        grants: grants(&["policy", "memory"]),
        ..approved()
    };
    let rover = RunOptions { grants: grants(&["rover", "sensor", "log"]), ticks: 1, ..approved() };
    for (file, opts) in [("greenhouse.kaze", greenhouse), ("guardrail.kaze", guardrail), ("rover.kaze", rover)] {
        let err = run_file(&example(file), &opts).expect_err("unavailable real backend must refuse");
        assert_eq!(err.code, "E_CAP_MISSING", "{file}: {}", err.message);
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
    let opts = RunOptions { args, grants: grants(&["drone", "log"]), backend: drone_sim(), ..approved() };
    let rep_source = run("patrol.kaze", &opts);
    let rep_bc = kaze::run_module(reloaded, "patrol.kazebc", &opts).unwrap();
    assert_eq!(rep_source.actions, rep_bc.actions);
    assert_eq!(rep_source.fuel_used, rep_bc.fuel_used);
}

#[test]
fn fuel_stops_runaway_loop() {
    let src = "action loop():\n    while 1 == 1:\n        let x = 1\n";
    let opts = RunOptions { fuel: 50, ..approved() };
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
    let opts = RunOptions { grants: grants(&["drone"]), backend: drone_sim(), ..approved() };
    let err = run_source(src, "<test>", &opts).unwrap_err();
    assert_eq!(err.code, "E_KAZE_RANGE");
    assert!(err.message.contains("rejected, not clamped"));
}


// v0.3: kinematics and spatial maths

/// The argument of every traced `log` line, in order. A trace line reads
/// `1. log.info(X) -> nothing`, so the value is what sits inside the parens.
fn logged(src: &str) -> Vec<String> {
    let opts = RunOptions { grants: grants(&["log"]), ..approved() };
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
fn a_non_finite_point_sum_stops_the_run() {
    // 308 nines is finite; the sum of two such points is not.
    let big = "9".repeat(308);
    let src = format!(
        "needs: log\naction main():\n    let a = vector({big}, 0, 0)\n    let b = vector({big}, 0, 0)\n    log length of (a + b)\n"
    );
    let opts = RunOptions { grants: grants(&["log"]), ..approved() };
    let err = run_source(&src, "<math>", &opts).expect_err("an infinite point is refused");
    assert_eq!(err.code, "E_KAZE_OVERFLOW");
}

#[test]
fn a_non_finite_vector_math_result_stops_the_run() {
    // Each component is finite; the magnitudes built from them are not.
    let big = "9".repeat(308);
    let opts = RunOptions { grants: grants(&["log"]), ..approved() };
    let cases = [
        format!("needs: log\naction main():\n    log length of vector({big}, {big}, 0)\n"),
        format!("needs: log\naction main():\n    log {big} in degrees\n"),
        format!(
            "needs: log\naction main():\n    log slant distance between vector(0, 0, {big}) and vector(0, 0, 0)\n"
        ),
    ];
    for src in cases {
        let err = run_source(&src, "<math>", &opts).expect_err("an infinite result is refused");
        assert_eq!(err.code, "E_KAZE_OVERFLOW", "{}", err.message);
    }
}

#[test]
fn a_number_literal_too_large_for_f64_is_a_lex_error() {
    let big = "9".repeat(400);
    let src = format!("needs: log\naction main():\n    log {big}\n");
    let opts = RunOptions { grants: grants(&["log"]), ..approved() };
    let err = run_source(&src, "<math>", &opts).expect_err("an infinite literal is refused");
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(err.message.contains("too large"), "{}", err.message);
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
    let opts = RunOptions { grants: grants(&["log"]), ..approved() };
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
    let sim = SimState { lat: 47.6, lon: 8.5, ..Default::default() };
    let opts = RunOptions { grants: grants(&["drone", "log"]), sim, backend: drone_sim(), ..approved() };
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
    let opts = RunOptions { grants: grants(&["log"]), ..approved() };
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
    let opts = RunOptions { grants: grants(&["log"]), ..approved() };
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
    let opts = RunOptions { grants: grants(&["log"]), ..approved() };
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
    let sim = SimState { lat: 47.6, lon: 8.5, ..Default::default() };
    let opts = RunOptions { grants: grants(&["drone", "log"]), sim, backend: drone_sim(), ..approved() };
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
    let opts = RunOptions { grants: grants(&["log"]), ..approved() };
    let err = run_source(src, "<math>", &opts).unwrap_err();
    assert_eq!(err.code, "E_KAZE_RANGE", "{}", err.message);
    assert!(err.message.contains("not a latitude and longitude"), "{}", err.message);
}

#[test]
fn fly_to_a_vector_updates_simulator_position() {
    let src = "needs: drone, log\naction main():\n    arm drone\n    takeoff to 10 meters\n    fly to vector(47.6004, 8.5004, 25)\n    log latitude\n    log longitude\n    log altitude\n";
    let opts = RunOptions { grants: grants(&["drone", "log"]), backend: drone_sim(), ..approved() };
    let report = run_source(src, "<nav>", &opts).expect("it runs");
    let logs: Vec<String> = report
        .actions
        .iter()
        .filter(|a| a.contains("log.info"))
        .cloned()
        .collect();
    assert!(logs[0].contains("47.6004"), "{}", logs[0]);
    assert!(logs[1].contains("8.5004"), "{}", logs[1]);
    assert!(logs[2].contains("25"), "{}", logs[2]);
}

#[test]
fn a_three_argument_goto_bounds_the_altitude_not_the_longitude() {
    let opts = RunOptions { grants: grants(&["drone"]), backend: drone_sim(), ..approved() };
    let too_high = "needs: drone\naction main():\n    arm drone\n    takeoff to 10 meters\n    drone.goto(-33.9, 151.2, 900)\n";
    let err = run_source(too_high, "<nav>", &opts).expect_err("900 m is outside the ceiling");
    assert_eq!(err.code, "E_KAZE_RANGE");
    assert!(err.message.contains("900"), "{}", err.message);

    let west = "needs: drone\naction main():\n    arm drone\n    takeoff to 10 meters\n    drone.goto(-33.9, 151.2, 30)\n";
    run_source(west, "<nav>", &opts).expect("a western longitude is a valid coordinate");
}

#[test]
fn a_vector_gotos_own_altitude_is_bounded() {
    let src = "needs: drone\naction main():\n    arm drone\n    takeoff to 10 meters\n    fly to vector(47.6, 8.5, 900)\n";
    let opts = RunOptions { grants: grants(&["drone"]), backend: drone_sim(), ..approved() };
    let err = run_source(src, "<nav>", &opts).expect_err("the point's z is an altitude");
    assert_eq!(err.code, "E_KAZE_RANGE");
    assert!(err.message.contains("900"), "{}", err.message);
}

#[test]
fn fly_to_a_vector_at_altitude_overrides_vector_z() {
    let src = "needs: drone, log\naction main():\n    arm drone\n    takeoff to 10 meters\n    fly to vector(47.6004, 8.5004, 25) at 40 meters\n    log altitude\n";
    let opts = RunOptions { grants: grants(&["drone", "log"]), backend: drone_sim(), ..approved() };
    let report = run_source(src, "<nav>", &opts).expect("it runs");
    let line = report.actions.iter().find(|a| a.contains("log.info")).expect("log");
    assert!(line.contains("40"), "{}", line);
}

#[test]
fn point_fields_read_by_name() {
    let src = "needs: log\naction main():\n    let p = vector(47.6, 8.5, 500)\n    log p.x\n    log p.y\n    log p.z\n    log p.lat\n    log p.lon\n    log p.alt\n";
    let out = logged(src);
    assert_eq!(number(&out[0]), 47.6);
    assert_eq!(number(&out[1]), 8.5);
    assert_eq!(number(&out[2]), 500.0);
    assert_eq!(number(&out[3]), 47.6);
    assert_eq!(number(&out[4]), 8.5);
    assert_eq!(number(&out[5]), 500.0);
}

#[test]
fn invalid_point_field_is_refused_at_check_time() {
    let src = "needs: log\naction main():\n    let p = vector(47.6, 8.5, 500)\n    log p.mystery\n";
    let err = kaze::check_source(src, "<math>").unwrap_err();
    assert_eq!(err.code, "E_KAZE_CHECK");
    assert!(err.message.contains("write x/lat, y/lon, or z/alt"), "{}", err.message);
}

// v0.4: fault tolerance — try/otherwise and link-loss failsafes

#[test]
fn try_recovers_from_a_host_failure_and_binds_the_error() {
    // Not arming first makes the explicitly selected drone simulator refuse takeoff; that is
    // exactly the class `try` exists to recover from.
    let src = "needs: drone, log\naction main():\n    try:\n        takeoff to 10 meters\n    otherwise with err:\n        log warning err.code\n        log warning err.message\n        log warning err.line\n";
    let opts = RunOptions { grants: grants(&["drone", "log"]), backend: drone_sim(), ..approved() };
    let report = run_source(src, "<fault>", &opts).expect("the host failure is recoverable");

    assert!(
        report.actions.iter().any(|a| a.contains("drone.takeoff") && a.contains("error:")),
        "the failed call stays in the trace: {:?}",
        report.actions
    );
    assert_eq!(report.logs.len(), 3, "{:?}", report.logs);
    assert_eq!(report.logs[0].1, "E_ARMING_DENIED");
    assert!(report.logs[1].1.contains("not armed"), "{:?}", report.logs);
    assert_eq!(report.logs[2].1, "2", "the error record names the action's line");
}

#[test]
fn a_range_violation_is_never_catchable() {
    let src = "needs: drone, log\naction main():\n    try:\n        takeoff to 900 meters\n    otherwise:\n        log warning \"caught\"\n";
    let opts = RunOptions { grants: grants(&["drone", "log"]), backend: drone_sim(), ..approved() };
    let err = run_source(src, "<fault>", &opts).expect_err("safety still stops the run");
    assert_eq!(err.code, "E_KAZE_RANGE");
    assert!(err.message.contains("rejected, not clamped"), "{}", err.message);
}

#[test]
fn a_missing_capability_grant_is_never_catchable() {
    let src = "needs: drone, log\naction main():\n    try:\n        arm drone\n    otherwise:\n        log warning \"caught\"\n";
    let opts = RunOptions { grants: grants(&["log"]), ..approved() };
    let err = run_source(src, "<fault>", &opts).expect_err("an ungranted backend still stops");
    assert_eq!(err.code, "E_CAP_MISSING");
}

#[test]
fn the_runtime_h1_gate_refuses_an_undeclared_family() {
    // A hand-built module bypasses the checker, so this reaches the R7 H-1
    // gate itself rather than the checker's static "declare it first" error.
    let module = isa::Module {
        needs: Vec::new(),
        root: "h1.kazebc".into(),
        consts: Vec::new(),
        caps: vec![isa::CapRef {
            family: "drone".into(),
            op: "arm".into(),
            argc: 0,
        }],
        items: vec![isa::ItemCode {
            name: "main".into(),
            tag: isa::ItemTag::Action,
            params: Vec::new(),
            code: vec![
                isa::word(isa::Op::CallCap, 0, 0, 0),
                0, // capability table index
                isa::word(isa::Op::Pop, 0, 0, 0),
                isa::word(isa::Op::RetUnit, 0, 0, 0),
            ],
            line: 1,
            origin: "h1.kazebc".into(),
            period_ms: 0.0,
            slots: 0,
        }],
    };
    let opts = RunOptions { grants: grants(&["drone"]), backend: drone_sim(), ..approved() };
    let err = kaze::run_module(module, "h1.kazebc", &opts).expect_err("H-1 refuses");
    assert_eq!(err.code, "E_CAP_UNDECLARED");
    assert!(err.message.contains("not declared"), "{}", err.message);
}

#[test]
fn a_policy_denial_is_never_catchable() {
    let src = "needs: drone, log\naction main():\n    try:\n        arm drone\n    otherwise:\n        log warning \"caught\"\n";
    let opts = RunOptions {
        grants: grants(&["drone", "log"]),
        policy: Some(serde_json::json!({ "deny": ["drone.*"] })),
        ..approved()
    };
    let err = run_source(src, "<fault>", &opts).expect_err("a policy denial still stops");
    assert_eq!(err.code, "E_CAP_DENIED");
}

#[test]
fn fuel_exhaustion_is_never_catchable() {
    let src = "needs: log\naction main():\n    try:\n        while 1 == 1:\n            log \"x\"\n    otherwise:\n        log \"caught\"\n";
    let opts = RunOptions { fuel: 60, grants: grants(&["log"]), ..approved() };
    let err = run_source(src, "<fault>", &opts).expect_err("the budget still stops the run");
    assert_eq!(err.code, "E_TIMEOUT");
}

#[test]
fn a_try_inside_a_when_hook_recovers_on_every_tick() {
    let src = "needs: drone, log\nwhen battery < 101:\n    try:\n        takeoff to 10 meters\n    otherwise with err:\n        log warning err.code\n";
    let opts = RunOptions {
        grants: grants(&["drone", "log"]),
        backend: drone_sim(),
        ticks: 2,
        tick_ms: 100,
        ..approved()
    };
    let report = run_source(src, "<fault>", &opts).expect("each tick recovers");
    let caught = report.logs.iter().filter(|(_, t)| t == "E_ARMING_DENIED").count();
    assert_eq!(caught, 2, "{:?}", report.logs);
}

#[test]
fn a_try_inside_a_loop_rearms_its_handler() {
    let src = "needs: drone, log\naction main():\n    for each i in [1, 2, 3]:\n        try:\n            takeoff to 10 meters\n        otherwise with err:\n            log warning err.code\n";
    let opts = RunOptions { grants: grants(&["drone", "log"]), backend: drone_sim(), ..approved() };
    let report = run_source(src, "<fault>", &opts).expect("every iteration recovers");
    let caught = report.logs.iter().filter(|(_, t)| t == "E_ARMING_DENIED").count();
    assert_eq!(caught, 3, "{:?}", report.logs);
}

#[test]
fn a_nested_try_uses_the_innermost_handler() {
    let src = "needs: drone, log\naction main():\n    try:\n        try:\n            takeoff to 10 meters\n        otherwise with inner:\n            log warning \"inner\"\n            takeoff to 20 meters\n    otherwise with outer:\n        log warning \"outer\"\n        log warning outer.code\n";
    let opts = RunOptions { grants: grants(&["drone", "log"]), backend: drone_sim(), ..approved() };
    let report = run_source(src, "<fault>", &opts).expect("both failures are recoverable");
    let texts: Vec<&str> = report.logs.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(texts, vec!["inner", "outer", "E_ARMING_DENIED"], "{:?}", report.logs);
}

#[test]
fn try_requires_an_otherwise_block() {
    let src = "needs: log\naction main():\n    try:\n        log \"x\"\n";
    let err = kaze::check_source(src, "<fault>").unwrap_err();
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(err.message.contains("otherwise"), "{}", err.message);
}

#[test]
fn an_error_record_only_has_code_message_and_line() {
    let src = "needs: drone, log\naction main():\n    try:\n        takeoff to 10 meters\n    otherwise with err:\n        log err.mystery\n";
    let err = kaze::check_source(src, "<fault>").unwrap_err();
    assert_eq!(err.code, "E_KAZE_CHECK");
    assert!(err.message.contains(".code, .message or .line"), "{}", err.message);
}

#[test]
fn link_loss_fires_only_after_the_threshold() {
    let src = "needs: drone, log\nwhen link lost for 3 seconds:\n    log warning \"link lost\"\n";
    let fired = |ticks: u32, lost_after: u64| -> usize {
        let opts = RunOptions {
            grants: grants(&["drone", "log"]),
            backend: drone_sim(),
            ticks,
            tick_ms: 100,
            sim: SimState { link_lost_after_ms: Some(lost_after), ..Default::default() },
            ..approved()
        };
        let report = run_source(src, "<fault>", &opts).expect("the hook runs");
        report.logs.iter().filter(|(_, t)| t == "link lost").count()
    };

    // 30 ticks reach t=2900 ms: still inside the 3 s window.
    assert_eq!(fired(30, 0), 0);
    // 35 ticks reach t=3400 ms, so 3000..=3400 fire: five ticks.
    assert_eq!(fired(35, 0), 5);
    // A link that only drops at 500 ms needs until 3500 ms; 40 ticks reach it.
    assert_eq!(fired(40, 500), 5);
}

#[test]
fn a_filter_can_terminate_through_try() {
    let src = "needs: policy\nfilter f(p):\n    try:\n        reject with \"no\"\n    otherwise:\n        allow p\n";
    kaze::check_source(src, "<fault>").expect("both paths decide");
}

#[test]
fn a_filter_that_can_fall_through_a_try_is_refused() {
    let src = "needs: policy, log\nfilter f(p):\n    try:\n        log \"checking\"\n    otherwise:\n        allow p\n";
    let err = kaze::check_source(src, "<fault>").unwrap_err();
    assert_eq!(err.code, "E_KAZE_CHECK");
    assert!(err.message.contains("allow"), "{}", err.message);
}

#[test]
fn link_loss_needs_a_drone_declaration() {
    let src = "needs: log\nwhen link lost for 3 seconds:\n    log warning \"x\"\n";
    let err = kaze::check_source(src, "<fault>").unwrap_err();
    assert_eq!(err.code, "E_KAZE_CHECK");
    assert!(err.message.contains("needs: drone"), "{}", err.message);
}

#[test]
fn a_sub_millisecond_link_window_is_refused() {
    let src = "needs: drone, log\nwhen link lost for 0.5 ms:\n    log warning \"x\"\n";
    let err = kaze::check_source(src, "<fault>").unwrap_err();
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(err.message.contains("at least 1 millisecond"), "{}", err.message);
}

#[test]
fn a_caught_failure_does_not_keep_a_stale_filter_verdict() {
    // policy.allow runs before the failure; the abandoned body must not leave
    // its Allow behind as the filter's decision.
    let src = "needs: policy, drone, log\nfilter f(p):\n    try:\n        let ok = policy.allow(p)\n        takeoff to 10 meters\n        reject with \"should not fly\"\n    otherwise:\n        return\n";
    let mut args = serde_json::Map::new();
    args.insert("p".into(), serde_json::json!("hello"));
    let opts = RunOptions { args, grants: grants(&["drone", "policy"]), backend: drone_sim(), ..approved() };
    let report = run_source(src, "<filter>", &opts).expect("the failure is recovered");
    assert!(
        report.verdict.is_none(),
        "a stale allow must not survive the catch: {:?}",
        report.verdict
    );
}

#[test]
fn paced_ticks_spend_real_time() {
    let src = "needs: log\nevery 50ms:\n    log \"tick\"\n";
    let opts = RunOptions {
        grants: grants(&["log"]),
        ticks: 4,
        tick_ms: 50,
        pace_ticks: true,
        ..approved()
    };
    let start = std::time::Instant::now();
    let report = run_source(src, "<pace>", &opts).expect("paced ticks run");
    assert_eq!(report.ticks, 4);
    assert!(
        start.elapsed() >= std::time::Duration::from_millis(120),
        "paced ticks did not wait: {:?}",
        start.elapsed()
    );
}

#[test]
fn the_documented_cli_invocation_matches_the_golden() {
    let bin = env!("CARGO_BIN_EXE_kaze");
    let dir = cli_policy_dir("golden");
    let out = std::process::Command::new(bin)
        .current_dir(&dir)
        .arg("run")
        .arg(example("fault_tolerance.kaze"))
        .args([
            "--cap",
            "drone=sim",
            "--cap",
            "log",
            "--ticks",
            "35",
            "--tick-ms",
            "100",
            "--sim",
            "link_lost_after=0",
            "--plain",
        ])
        .output()
        .expect("the kaze binary runs");
    std::fs::remove_dir_all(&dir).expect("remove test directory");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        include_str!("../examples/golden/fault_tolerance.txt")
    );
}

#[test]
fn the_modular_example_refuses_an_unavailable_sensor_backend() {
    let bin = env!("CARGO_BIN_EXE_kaze");
    let dir = cli_policy_dir("modular");
    let out = std::process::Command::new(bin)
        .current_dir(&dir)
        .arg("run")
        .arg(example("modular/mission.kaze"))
        .args([
            "--cap",
            "drone=sim",
            "--cap",
            "sensor",
            "--cap",
            "log",
            "--ticks",
            "2",
            "--tick-ms",
            "100",
            "--plain",
        ])
        .output()
        .expect("the kaze binary runs");
    std::fs::remove_dir_all(&dir).expect("remove test directory");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E_CAP_MISSING"), "{stderr}");
    assert!(stderr.contains("simulation selected (testing only): drone"), "{stderr}");
}

#[test]
fn the_cli_pace_flag_spends_real_time() {
    let bin = env!("CARGO_BIN_EXE_kaze");
    let dir = cli_policy_dir("pace");
    let start = std::time::Instant::now();
    let out = std::process::Command::new(bin)
        .current_dir(&dir)
        .arg("run")
        .arg(example("fault_tolerance.kaze"))
        .args([
            "--cap",
            "drone=sim",
            "--cap",
            "log",
            "--ticks",
            "3",
            "--tick-ms",
            "40",
            "--pace",
            "--sim",
            "link_lost_after=0",
            "--plain",
        ])
        .output()
        .expect("the kaze binary runs");
    std::fs::remove_dir_all(&dir).expect("remove test directory");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("simulation selected (testing only): drone"));
    assert!(
        start.elapsed() >= std::time::Duration::from_millis(50),
        "--pace did not sleep: {:?}",
        start.elapsed()
    );
}

#[test]
fn a_long_supervisor_reclaims_the_run_arena_between_ticks() {
    // The message constant is re-allocated on every tick. Without the
    // per-tick reclaim, 3000 ticks of a 200-byte string exceed the 256 KiB
    // arena and the supervisor dies with E_KAZE_MEMORY.
    let msg = "x".repeat(200);
    let src = format!("needs: log\nevery 100ms:\n    log \"{}\"\n", msg);
    let opts = RunOptions {
        grants: grants(&["log"]),
        ticks: 3000,
        tick_ms: 100,
        ..approved()
    };
    let report = run_source(&src, "<supervisor>", &opts)
        .expect("a long supervisor must stay inside its arena");
    assert_eq!(report.ticks, 3000);
}

#[test]
fn a_try_module_round_trips_through_bytecode() {
    let src = "needs: drone, log\naction main():\n    try:\n        takeoff to 10 meters\n    otherwise with err:\n        log warning err.code\n";
    let module = kaze::compile_source(src, "<fault>").expect("it compiles");
    let bytes = isa::serialize(&module);
    let reloaded = isa::deserialize(&bytes).expect("v0.4 modules deserialize");
    let opts = RunOptions { grants: grants(&["drone", "log"]), backend: drone_sim(), ..approved() };
    let report = kaze::run_module(reloaded, "<fault>.kazebc", &opts).expect("it runs from bytecode");
    assert_eq!(report.logs[0].1, "E_ARMING_DENIED");
}
