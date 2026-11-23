// Phase 3: compile-time dimensional physics. A dimension is checked by the
// checker and never exists at run time, so these tests assert both the numbers
// and the absence of any runtime unit machinery in the artifact.

use kaze::{check_source, run_source, RunOptions};

fn approved() -> RunOptions {
    RunOptions {
        policy: Some(serde_json::json!({ "deny": [] })),
        ..Default::default()
    }
}

fn value(expr: &str) -> String {
    let src = format!("action main():\n    return {}\n", expr);
    run_source(&src, "<units>", &approved())
        .unwrap_or_else(|e| panic!("{}: {} ({})", expr, e.code, e.message))
        .value
}

fn mismatch(expr: &str) -> String {
    let src = format!("action main():\n    return {}\n", expr);
    let err = check_source(&src, "<units>").expect_err("the checker refuses it");
    assert_eq!(err.code, "E_KAZE_UNIT_MISMATCH", "{}", err.message);
    err.message
}

#[test]
fn same_dimension_literals_add_in_their_base_unit() {
    // The conversion is folded into the literal: 1 km is stored as 1000 m.
    assert_eq!(value("1 km + 500 meters"), "1500");
    assert_eq!(value("1 hour + 30 minutes"), "5400");
    assert_eq!(value("5 kg + 500 gram"), "5.5");
    assert_eq!(value("1 hPa + 1 mbar"), "200");
    assert_eq!(value("2500 mm + 25 cm"), "2.75");
}

#[test]
fn multiplication_and_division_combine_dimensions() {
    // velocity * time = length
    assert_eq!(value("2 m/s * 3 seconds"), "6");
    // length / time = velocity
    assert_eq!(value("10 meters / 2 seconds"), "5");
    // acceleration * time * time = length
    assert_eq!(value("2 m/s2 * 3 seconds * 2 seconds"), "12");
}

#[test]
fn temperature_literals_normalise_to_kelvin_and_still_compare() {
    assert_eq!(value("20 Celsius"), "293.15");
    assert_eq!(value("20 Celsius > 30 Celsius"), "false");
    assert_eq!(value("30 Celsius > 20 Celsius"), "true");
    assert_eq!(value("0 Kelvin"), "0");
}

#[test]
fn an_angle_literal_is_degrees_and_trig_converts_it() {
    // The angle base unit is degrees, so a bearing and a `degrees` literal are
    // comparable, and sin() converts the literal to radians at compile time.
    assert_eq!(value("sin(90 degrees)"), "1");
    let expected = format!("{}", f64::sin(0.5));
    assert_eq!(value("sin(0.5 radians)"), expected);
}

#[test]
fn incompatible_dimensions_are_refused_with_the_quantity_named() {
    let msg = mismatch("1 meter + 1 second");
    assert!(msg.contains("length"), "{}", msg);
    assert!(msg.contains("time"), "{}", msg);

    let msg = mismatch("1 m/s + 1 meter");
    assert!(msg.contains("velocity") || msg.contains("m/s"), "{}", msg);

    let msg = mismatch("20 Celsius > 5 seconds");
    assert!(msg.contains("temperature"), "{}", msg);
}

#[test]
fn a_dimension_is_free_at_run_time() {
    // The same program with plain numbers and with units compiles to the same
    // number of instructions: units exist only in the checker.
    let plain = kaze::compile_source(
        "action main():\n    let a = 1000.0\n    let b = 500.0\n    return a + b\n",
        "<plain>",
    )
    .expect("plain compiles");
    let united = kaze::compile_source(
        "action main():\n    let a = 1 km\n    let b = 500 meters\n    return a + b\n",
        "<united>",
    )
    .expect("units compile");
    let count = |m: &kaze::isa::Module| m.items.iter().map(|i| i.code.len()).sum::<usize>();
    assert_eq!(
        count(&plain),
        count(&united),
        "a unit must not add a single instruction"
    );
}

#[test]
fn percent_stays_a_whole_number() {
    assert_eq!(value("20%"), "20");
    assert_eq!(value("20% + 5%"), "25");
}
