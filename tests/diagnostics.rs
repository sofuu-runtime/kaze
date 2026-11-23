// Phase 2/3: a diagnostic must name the code, the path, the line, the column,
// show the offending source line and point a caret at the construct. These
// tests pin that shape so it cannot regress into a one-liner again.

use kaze::check_source;

fn block(src: &str) -> String {
    check_source(src, "case.kaze")
        .expect_err("the program is meant to be refused")
        .message
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

fn caret_column(text: &str) -> usize {
    text.lines()
        .find(|l| l.contains('^'))
        .and_then(|l| l.find('^'))
        .unwrap_or(0)
}

fn assert_block_shape(text: &str, code: &str, line: u32) {
    assert!(
        first_line(text).starts_with(&format!("error[{}]: ", code)),
        "the header names the code: {}",
        text
    );
    assert!(
        text.contains(&format!("--> case.kaze:{}:", line)),
        "the location names path:line:col: {}",
        text
    );
    assert!(
        text.contains(&format!("{} | ", line)),
        "the snippet shows the line with a gutter: {}",
        text
    );
    assert!(text.contains('^'), "there is a caret: {}", text);
    assert!(
        caret_column(text) > 4,
        "the caret points at the construct, not the margin: {}",
        text
    );
}

#[test]
fn a_unit_mismatch_points_at_the_operator_and_names_both_dimensions() {
    let text = block("action main():\n    let x = 1 meter + 2 seconds\n    return x\n");
    assert_block_shape(&text, "E_KAZE_UNIT_MISMATCH", 2);
    assert!(text.contains("length (meter)"), "{}", text);
    assert!(text.contains("time (second)"), "{}", text);
}

#[test]
fn a_division_without_a_fallback_points_at_the_operator() {
    let text = block("action main():\n    let x = 10 / 0\n    return x\n");
    assert_block_shape(&text, "E_KAZE_MISSING_FALLBACK", 2);
    assert!(text.contains("otherwise <fallback>"), "{}", text);
}

#[test]
fn an_unproven_index_points_at_the_bracket() {
    let text = block("action main():\n    let xs = [1, 2]\n    let i = 5\n    return xs[i]\n");
    assert_block_shape(&text, "E_KAZE_MISSING_FALLBACK", 4);
}

#[test]
fn an_unbounded_loop_points_at_the_keyword() {
    let text = block("action main():\n    while 1 == 1:\n        let x = 1\n");
    assert_block_shape(&text, "E_KAZE_LOOP_UNBOUNDED", 2);
    assert!(text.contains("bounded"), "{}", text);
}

#[test]
fn an_unbounded_recursion_points_at_the_routine_name() {
    let text = block(
        "action fact(n):\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n\naction main():\n    return fact(3)\n",
    );
    assert_block_shape(&text, "E_KAZE_RECURSION_UNBOUNDED", 1);
    assert!(text.contains("recursion bounded"), "{}", text);
}

#[test]
fn an_undeclared_capability_points_at_the_family() {
    let text = block("needs: log\naction a():\n    arm drone\n");
    assert_block_shape(&text, "E_CAP_UNDECLARED", 3);
    assert!(text.contains("needs: drone"), "{}", text);
}

#[test]
fn a_host_field_without_a_fallback_points_at_the_field() {
    let text = block("action read_value(reading):\n    let v = reading.value\n    return v\n");
    assert_block_shape(&text, "E_KAZE_MISSING_FALLBACK", 2);
    assert!(text.contains("record type"), "{}", text);
}

#[test]
fn a_parse_error_points_at_its_column() {
    let text = block("action main(\n");
    assert!(first_line(&text).starts_with("error[E_KAZE_PARSE]: "), "{}", text);
    assert!(text.contains("--> case.kaze:"), "{}", text);
    assert!(text.contains('^'), "{}", text);
}

#[test]
fn a_constant_index_past_the_end_points_at_the_bracket() {
    let text = block("action main():\n    let p = vector(1, 2, 3)\n    return p[3]\n");
    assert_block_shape(&text, "E_KAZE_BOUNDS", 3);
}

#[test]
fn a_unit_mismatch_points_at_both_operands() {
    // Phase 3's illustration: two carets, one per operand, each with its unit.
    let src = "action main():\n    let runway_length = 1200 meter\n    let flight_time = 40 second\n    let total = runway_length + flight_time\n    return total\n";
    let err = kaze::check_source(src, "u.kaze").expect_err("a mismatch");
    let text = err.message.clone();
    assert!(text.contains("^^^"), "two caret runs: {}", text);
    assert!(text.contains("here: length (meter)"), "{}", text);
    assert!(text.contains("here: time (second)"), "{}", text);
    assert!(text.contains("--> u.kaze:4:"), "names the line: {}", text);
}
