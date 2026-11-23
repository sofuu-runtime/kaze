// Phase 4: routine contracts, untrusted parameters and the 12-factor
// configuration hierarchy. These tests drive the public API, which is what a
// host embedding Kaze actually calls.

use kaze::{check_source, run_source, RunOptions};

fn approved() -> RunOptions {
    RunOptions {
        policy: Some(serde_json::json!({ "deny": [] })),
        ..Default::default()
    }
}

fn run(src: &str, opts: &RunOptions) -> Result<kaze::RunReport, kaze::KazeError> {
    run_source(src, "<contract>", opts)
}

const THROTTLE: &str = "needs: log\n\naction set_throttle(channel: int, duty: percent) -> bool:\n    requires channel >= 0\n    requires channel <= 8\n    requires duty >= 0\n    requires duty <= 100\n    return duty > 50\n";

#[test]
fn a_constant_argument_that_breaks_a_precondition_is_refused_at_check_time() {
    let src = format!(
        "{} \naction main():\n    return set_throttle(9, 50)\n",
        THROTTLE
    );
    let err = check_source(&src, "<contract>").expect_err("9 is out of the declared range");
    assert_eq!(err.code, "E_KAZE_CONTRACT", "{}", err.message);
    assert!(err.message.contains("precondition"), "{}", err.message);

    let ok = format!("{} \naction main():\n    return set_throttle(4, 80)\n", THROTTLE);
    check_source(&ok, "<contract>").expect("a value inside the range is accepted");
}

#[test]
fn a_dynamic_argument_that_breaks_a_precondition_traps_at_run_time() {
    // The checker cannot see through --arg, so the routine's own entry
    // assertion is what refuses it. E_KAZE_TRAP is not catchable.
    let src = format!("{} \naction main(channel: int):\n    return set_throttle(channel, 50)\n", THROTTLE);
    let opts = RunOptions {
        args: serde_json::json!({ "channel": 99 })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        ..approved()
    };
    let err = run(&src, &opts).expect_err("99 is out of range");
    assert!(
        err.code == "E_KAZE_TRAP" || err.code == "E_KAZE_CONTRACT",
        "got {}: {}",
        err.code,
        err.message
    );
    assert!(err.message.contains("line"), "{}", err.message);
}

#[test]
fn an_ensures_clause_sees_the_result_and_is_checked() {
    let good = "action double(n: number) -> number:\n    ensures result == 2\n    return n + n\n\naction main():\n    return double(1)\n";
    let report = run(good, &approved()).expect("2 == 2 holds");
    assert_eq!(report.value, "2");

    let bad = "action double(n: number) -> number:\n    ensures result == 3\n    return n + n\n\naction main():\n    return double(1)\n";
    let err = run(bad, &approved()).expect_err("2 == 3 does not hold");
    assert!(
        err.code == "E_KAZE_TRAP" || err.code == "E_KAZE_CONTRACT",
        "got {}: {}",
        err.code,
        err.message
    );
}

#[test]
fn an_untrusted_parameter_needs_a_precondition_to_validate_it() {
    let unvalidated = "action ingest(count: untrusted<int>):\n    return count\n";
    let err = check_source(unvalidated, "<contract>").expect_err("no requires clause");
    assert_eq!(err.code, "E_KAZE_UNTRUSTED", "{}", err.message);
    assert!(err.message.contains("requires"), "{}", err.message);

    let validated = "action ingest(count: untrusted<int>):\n    requires count >= 0\n    return count\n";
    check_source(validated, "<contract>").expect("a validated untrusted value is accepted");
}

#[test]
fn a_typed_parameter_with_no_value_anywhere_names_the_three_fixes() {
    let src = "action main(duty: percent):\n    return duty\n";
    let err = run(src, &approved()).expect_err("no value for a typed parameter");
    assert_eq!(err.code, "E_KAZE_PARAM", "{}", err.message);
    assert!(err.message.contains("--arg duty="), "{}", err.message);
    assert!(err.message.contains("AGENT_DUTY"), "{}", err.message);
    assert!(err.message.contains("config"), "{}", err.message);
}

#[test]
fn the_hierarchy_is_cli_then_environment_then_config_then_default() {
    let src = "action main(duty: percent = 20):\n    return duty\n";

    // 4. signature default
    let report = run(src, &approved()).expect("default runs");
    assert_eq!(report.value, "20");

    // 3. config document
    let opts = RunOptions {
        config: Some(serde_json::json!({ "duty": 40 })),
        ..approved()
    };
    assert_eq!(run(src, &opts).expect("config runs").value, "40");

    // 2. environment beats the config document. The variable name is unique to
    // this test so a developer's shell cannot change the result.
    std::env::set_var("AGENT_DUTY", "60");
    let report = run(src, &opts).expect("environment runs");
    std::env::remove_var("AGENT_DUTY");
    assert_eq!(report.value, "60");

    // 1. a CLI argument beats everything
    let opts = RunOptions {
        args: serde_json::json!({ "duty": 80 })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        config: Some(serde_json::json!({ "duty": 40 })),
        ..approved()
    };
    assert_eq!(run(src, &opts).expect("argument runs").value, "80");
}

#[test]
fn a_value_of_the_wrong_type_is_refused_with_the_type_named() {
    let src = "action main(duty: percent):\n    return duty\n";
    let opts = RunOptions {
        args: serde_json::json!({ "duty": "not a number" })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        ..approved()
    };
    let err = run(src, &opts).expect_err("text is not a percentage");
    assert_eq!(err.code, "E_KAZE_PARAM", "{}", err.message);
    assert!(err.message.contains("number"), "{}", err.message);
}

#[test]
fn a_guard_traps_with_its_own_code() {
    let src = "action main(n: number):\n    guard n > 0\n    return n\n";
    let opts = RunOptions {
        args: serde_json::json!({ "n": -1 })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        ..approved()
    };
    let err = run(src, &opts).expect_err("the invariant is false");
    assert_eq!(err.code, "E_KAZE_TRAP", "{}", err.message);
    assert!(err.message.contains("E_KAZE_GUARD"), "{}", err.message);
}

#[test]
fn a_proved_call_site_needs_no_runtime_contract_check() {
    // Every caller passes literals that satisfy the precondition, so the
    // callee's entry assertion is dead code and the compiler leaves it out.
    let src = "action throttle(channel: int) -> bool:\n    requires channel >= 0\n    requires channel <= 8\n    return channel > 2\n\naction main():\n    return throttle(4)\n";
    let module = kaze::compile_source(src, "<contract>").expect("compiles");
    let traps = |name: &str| -> usize {
        module
            .items
            .iter()
            .find(|i| i.name == name)
            .map(|i| {
                // A TRAP is a wide instruction: opcode word plus immediate.
                i.code
                    .chunks(2)
                    .filter(|pair| (pair[0] & 0xff) as u8 == 27)
                    .count()
            })
            .unwrap_or(0)
    };
    assert_eq!(traps("throttle"), 0, "the proof removed the entry assertion");
    assert_eq!(traps("main"), 0);
}

#[test]
fn a_dynamic_call_site_keeps_its_runtime_contract_check() {
    // A value from the command line cannot be proved, so the assertion stays.
    let src = "action throttle(channel: int) -> bool:\n    requires channel >= 0\n    requires channel <= 8\n    return channel > 2\n\naction main(channel: int):\n    return throttle(channel)\n";
    let module = kaze::compile_source(src, "<contract>").expect("compiles");
    let throttle = module.items.iter().find(|i| i.name == "throttle").expect("routine");
    let traps = throttle
        .code
        .chunks(2)
        .filter(|pair| (pair[0] & 0xff) as u8 == 27)
        .count();
    assert!(traps >= 1, "a dynamic call keeps the check, found {} traps", traps);
}

#[test]
fn a_routine_the_host_can_start_always_asserts_its_preconditions() {
    // `main` has no call site: its arguments come from outside, so the
    // assertion must stay even though nothing in the file calls it.
    let src = "action main(channel: int) -> bool:\n    requires channel <= 8\n    return channel > 2\n";
    let module = kaze::compile_source(src, "<contract>").expect("compiles");
    let main = module.items.iter().find(|i| i.name == "main").expect("routine");
    let traps = main
        .code
        .chunks(2)
        .filter(|pair| (pair[0] & 0xff) as u8 == 27)
        .count();
    assert!(traps >= 1, "an entry routine keeps its check, found {} traps", traps);
}
