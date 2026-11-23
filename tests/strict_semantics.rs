// Phase 2 acceptance corpus: ultra-strict semantics & zero-panic invariants.
//
// Roadmap (docs/kaze-plan/00-DETAILED-MASTER-ROADMAP.md §"Phase 2") asks for a
// suite of 50+ intentionally flawed programs that all fail compilation with a
// deterministic E_KAZE_* code. Every program lives in
// tests/fixtures/strict/*.kaze, one rule per file, with a header naming the
// code it must produce and an inline `# BAD` mark on the offending line.
//
// Three properties are load-bearing here, and each has its own test:
//
//   1. Exactness. A negative fixture must produce the *exact* code its rule
//      names — not E_KAZE_CHECK, not a generic refusal — and the diagnostic
//      must point at the marked line.
//   2. Coverage without rot. Every file on disk is in exactly one table, so a
//      new fixture cannot be forgotten and a deleted one cannot linger in the
//      tables. The corpus must stay at 50+ rejected programs.
//   3. No panics. Every fixture, flawed or not, runs through the whole front
//      end (and the VM) under catch_unwind. A hostile program is a diagnostic,
//      never a process abort.
//
// Run it with:
//     cargo test --manifest-path kaze/Cargo.toml --test strict_semantics

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{catch_unwind, AssertUnwindSafe};

use kaze::RunOptions;

const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/strict");

// ── the corpus shape ─────────────────────────────────────────────────

/// Which stage must refuse a fixture.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// `prepare` (parse → check → compile) refuses the program.
    Front,
    /// The front end accepts the program; the VM aborts while running it.
    Run,
}

/// Where the diagnostic must point.
#[derive(Clone, Copy)]
enum Points {
    /// Exactly this 1-based line — the one carrying the fixture's `# BAD` mark.
    Line(usize),
    /// Any line of the fixture. Whole-item analyses (recursion and fuel
    /// ceilings) may legitimately name the declaration site or the call site,
    /// so the table only requires that *a* real line of the file is named.
    Anywhere,
}

/// One intentionally flawed program and the diagnostic it must earn.
struct Rejection {
    file: &'static str,
    code: &'static str,
    stage: Stage,
    points: Points,
}

const fn fallback(file: &'static str, line: usize) -> Rejection {
    Rejection {
        file,
        code: "E_KAZE_MISSING_FALLBACK",
        stage: Stage::Front,
        points: Points::Line(line),
    }
}

const fn bounds(file: &'static str, line: usize) -> Rejection {
    Rejection {
        file,
        code: "E_KAZE_BOUNDS",
        stage: Stage::Front,
        points: Points::Line(line),
    }
}

const fn loop_unbounded(file: &'static str, line: usize) -> Rejection {
    Rejection {
        file,
        code: "E_KAZE_LOOP_UNBOUNDED",
        stage: Stage::Front,
        points: Points::Line(line),
    }
}

const fn recursion_unbounded(file: &'static str, _line: usize) -> Rejection {
    Rejection {
        file,
        code: "E_KAZE_RECURSION_UNBOUNDED",
        stage: Stage::Front,
        points: Points::Anywhere,
    }
}

const fn recursion_depth(file: &'static str, _line: usize) -> Rejection {
    Rejection {
        file,
        code: "E_KAZE_RECURSION_DEPTH",
        stage: Stage::Run,
        points: Points::Anywhere,
    }
}

const fn parse_rejection(file: &'static str, line: usize) -> Rejection {
    Rejection {
        file,
        code: "E_KAZE_PARSE",
        stage: Stage::Front,
        points: Points::Line(line),
    }
}

const fn cap_undeclared(file: &'static str, line: usize) -> Rejection {
    Rejection {
        file,
        code: "E_CAP_UNDECLARED",
        stage: Stage::Front,
        points: Points::Line(line),
    }
}

// Division and modulo: a dynamic divisor (or a literal zero) needs an inline
// fallback before the program may run.
const ARITHMETIC_WITHOUT_FALLBACK: &[Rejection] = &[
    fallback("div_no_fallback.kaze", 4),
    fallback("div_no_fallback_return.kaze", 4),
    fallback("div_no_fallback_eval.kaze", 4),
    fallback("div_no_fallback_condition.kaze", 4),
    fallback("div_no_fallback_nested.kaze", 4),
    fallback("div_no_fallback_list_entry.kaze", 4),
    fallback("div_no_fallback_set.kaze", 5),
    fallback("div_no_fallback_each_body.kaze", 5),
    fallback("div_no_fallback_while_body.kaze", 6),
    fallback("div_by_zero_literal.kaze", 4),
    fallback("div_param_over_param.kaze", 4),
    fallback("mod_no_fallback.kaze", 4),
    fallback("mod_no_fallback_return.kaze", 4),
    fallback("mod_no_fallback_set.kaze", 5),
    fallback("mod_by_zero_literal.kaze", 4),
    fallback("mod_no_fallback_condition.kaze", 4),
];

// Dynamic indexing of a list, text or point value.
const DYNAMIC_INDEX_WITHOUT_FALLBACK: &[Rejection] = &[
    fallback("index_dynamic_no_fallback.kaze", 4),
    fallback("index_dynamic_return.kaze", 4),
    fallback("index_dynamic_text_no_fallback.kaze", 4),
    fallback("index_dynamic_point_no_fallback.kaze", 5),
    fallback("index_dynamic_each_body.kaze", 5),
    fallback("index_dynamic_condition.kaze", 4),
    fallback("index_dynamic_nested.kaze", 4),
];

// A field read on a host-supplied record may be absent, so it is fallible.
const HOST_RECORD_FIELD_WITHOUT_FALLBACK: &[Rejection] = &[
    fallback("field_dynamic_no_fallback.kaze", 4),
    fallback("field_dynamic_return.kaze", 4),
    fallback("field_dynamic_nested.kaze", 4),
    fallback("field_dynamic_set.kaze", 5),
];

// A value-returning capability family read in expression position is a host
// call that can fail, so it needs the same fallback.
const CAPABILITY_VALUE_WITHOUT_FALLBACK: &[Rejection] = &[
    fallback("cap_sensor_expr_no_fallback.kaze", 6),
    fallback("cap_device_expr_no_fallback.kaze", 6),
    fallback("cap_memory_expr_no_fallback.kaze", 6),
    fallback("cap_sensor_condition_no_fallback.kaze", 6),
    fallback("cap_sensor_list_entry_no_fallback.kaze", 6),
];

// A capability family used without being declared in `needs:`.
const CAPABILITY_UNDECLARED: &[Rejection] = &[
    cap_undeclared("cap_undeclared_drone.kaze", 6),
    cap_undeclared("cap_undeclared_sensor.kaze", 6),
    cap_undeclared("cap_undeclared_device.kaze", 6),
    cap_undeclared("cap_undeclared_memory.kaze", 6),
    cap_undeclared("cap_undeclared_alert.kaze", 6),
    cap_undeclared("cap_undeclared_policy.kaze", 6),
];

// A constant index outside the known length of a literal is a check-time error.
const CONSTANT_INDEX_OUT_OF_BOUNDS: &[Rejection] = &[
    bounds("index_const_oob.kaze", 4),
    bounds("index_const_oob_tail.kaze", 4),
    bounds("index_const_oob_return.kaze", 4),
    bounds("index_const_oob_nested.kaze", 4),
    bounds("index_const_oob_empty_list.kaze", 4),
    bounds("index_const_oob_each_body.kaze", 5),
];

// Every `while` must declare its fuel ceiling.
const WHILE_WITHOUT_FUEL_CEILING: &[Rejection] = &[
    loop_unbounded("while_unbounded.kaze", 5),
    loop_unbounded("while_unbounded_true.kaze", 4),
    loop_unbounded("while_unbounded_in_every.kaze", 4),
    loop_unbounded("while_unbounded_in_when.kaze", 4),
    loop_unbounded("while_unbounded_nested.kaze", 7),
    loop_unbounded("while_unbounded_dynamic_condition.kaze", 4),
];

// Direct and mutual recursion need `recursion bounded <n>`, and the VM
// enforces the declared ceiling at run time.
const RECURSION_WITHOUT_CEILING: &[Rejection] = &[
    recursion_unbounded("recursion_unbounded.kaze", 5),
    recursion_unbounded("recursion_unbounded_return.kaze", 6),
    recursion_unbounded("recursion_unbounded_mutual.kaze", 5),
    recursion_unbounded("recursion_unbounded_in_while.kaze", 6),
    // A ceiling outside 1..=256 cannot be honoured, so the declaration itself is
    // refused while parsing — still one deterministic code for the program.
    parse_rejection("recursion_declared_too_high.kaze", 5),
    // The declared ceiling itself is enforced by the VM, so this one is
    // accepted by the front end and aborts while running.
    recursion_depth("recursion_depth_exceeded.kaze", 10),
];

// Positive controls: proper programs that must compile and run to a known
// value. They are the other half of each rule — the corpus proves the checker
// refuses the flaw *and* still accepts the fix.
struct Control {
    file: &'static str,
    value: &'static str,
    /// JSON object of `--arg` bindings, or "" for none.
    args: &'static str,
}

const POSITIVE_CONTROLS: &[Control] = &[
    Control { file: "div_const_divisor_ok.kaze", value: "25", args: "" },
    Control { file: "mod_const_divisor_ok.kaze", value: "2", args: "{\"total\":10}" },
    Control { file: "index_const_in_range_ok.kaze", value: "20", args: "" },
    Control { file: "index_const_nested_ok.kaze", value: "5", args: "" },
    Control { file: "text_index_const_ok.kaze", value: "1", args: "" },
    Control { file: "div_otherwise_ok.kaze", value: "3", args: "{\"d\":4}" },
    Control { file: "mod_otherwise_ok.kaze", value: "1", args: "{\"total\":10,\"parts\":3}" },
    Control { file: "index_otherwise_ok.kaze", value: "7", args: "{\"items\":[5,6,7],\"i\":2}" },
    Control { file: "index_loop_guarded_ok.kaze", value: "18", args: "{\"items\":[5,6,7]}" },
    Control { file: "field_otherwise_ok.kaze", value: "7", args: "{\"reading\":{\"value\":7}}" },
    Control { file: "while_bounded_ok.kaze", value: "3", args: "" },
    Control { file: "while_bounded_sum_ok.kaze", value: "15", args: "" },
    Control { file: "for_each_literal_ok.kaze", value: "6", args: "" },
    Control { file: "for_each_list_value_ok.kaze", value: "12", args: "{\"xs\":[2,4,6]}" },
    Control { file: "recursion_bounded_ok.kaze", value: "120", args: "" },
    Control { file: "recursion_bounded_mutual_ok.kaze", value: "true", args: "" },
];

// ── helpers ──────────────────────────────────────────────────────────

/// Every `.kaze` fixture, keyed by file name.
fn corpus() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let entries = std::fs::read_dir(FIXTURE_DIR)
        .unwrap_or_else(|e| panic!("cannot list {}: {}", FIXTURE_DIR, e));
    for entry in entries {
        let path = entry.expect("fixture directory entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("kaze") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("fixture file name")
            .to_string();
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {}", path.display(), e));
        out.insert(name, src);
    }
    assert!(!out.is_empty(), "no fixtures under {}", FIXTURE_DIR);
    out
}

fn all_rejections() -> Vec<&'static Rejection> {
    let mut out: Vec<&'static Rejection> = Vec::new();
    for table in [
        ARITHMETIC_WITHOUT_FALLBACK,
        DYNAMIC_INDEX_WITHOUT_FALLBACK,
        HOST_RECORD_FIELD_WITHOUT_FALLBACK,
        CAPABILITY_VALUE_WITHOUT_FALLBACK,
        CAPABILITY_UNDECLARED,
        CONSTANT_INDEX_OUT_OF_BOUNDS,
        WHILE_WITHOUT_FUEL_CEILING,
        RECURSION_WITHOUT_CEILING,
    ] {
        out.extend(table.iter());
    }
    out
}

/// The 1-based line the `# BAD` marker sits on.
fn bad_mark_line(src: &str) -> Option<usize> {
    src.lines()
        .position(|line| line.contains("# BAD"))
        .map(|i| i + 1)
}

/// The code a fixture's `# expect:` header claims, or "ok" for a control.
fn declared_expectation(src: &str) -> Option<&str> {
    src.lines().find_map(|line| {
        line.strip_prefix("# expect:")
            .map(|rest| rest.split_whitespace().next().unwrap_or(""))
    })
}

/// The line number a rendered diagnostic names: the digits right after
/// `<file>:`. A checker/parse error renders as `file:line[:col]: message`.
fn named_line(message: &str, file: &str) -> Option<u32> {
    let at = message.find(file)? + file.len();
    let rest = message[at..].strip_prefix(':')?;
    rest.chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

fn run_options(args: &str) -> RunOptions {
    let mut opts = RunOptions::default();
    if !args.trim().is_empty() {
        opts.args = serde_json::from_str(args).expect("fixture args must be a JSON object");
    }
    opts
}

/// Render a diagnostic (which may hold several joined errors) on one line.
fn one_line(message: &str) -> String {
    message.replace('\n', " | ")
}

/// Assert that one flawed program earns exactly the code and the line its
/// table entry names.
fn expect_rejected(case: &Rejection, src: &str) {
    match case.stage {
        Stage::Front => {
            let err = match kaze::prepare(src, case.file) {
                Ok(_) => panic!(
                    "{}: expected {} but the front end accepted the program",
                    case.file, case.code
                ),
                Err(err) => err,
            };
            if err.code != case.code {
                // The most common wiring gap: the checker names the right code in
                // its message, but the error object still carries E_KAZE_CHECK.
                let hint = if err.message.contains(case.code) {
                    format!(
                        " — the message names {} but KazeError.code is {}; \
                         surface the specific code as the error's code",
                        case.code, err.code
                    )
                } else {
                    String::new()
                };
                panic!(
                    "{}: expected {} but got {}{} (message: {})",
                    case.file,
                    case.code,
                    err.code,
                    hint,
                    one_line(&err.message)
                );
            }
            let line = named_line(&err.message, case.file).unwrap_or_else(|| {
                panic!(
                    "{}: the diagnostic must name the file and line, got: {}",
                    case.file,
                    one_line(&err.message)
                )
            });
            match case.points {
                Points::Line(want) => assert_eq!(
                    line as usize, want,
                    "{}: the diagnostic points at line {}, the flaw is on line {}: {}",
                    case.file,
                    line,
                    want,
                    one_line(&err.message)
                ),
                Points::Anywhere => {
                    let total = src.lines().count();
                    assert!(
                        line >= 1 && line as usize <= total,
                        "{}: the diagnostic points at line {} of a {}-line file: {}",
                        case.file,
                        line,
                        total,
                        one_line(&err.message)
                    );
                }
            }
        }
        Stage::Run => {
            let opts = run_options("");
            let err = match kaze::run_source(src, case.file, &opts) {
                Ok(_) => panic!(
                    "{}: expected {} at run time but the run finished cleanly",
                    case.file, case.code
                ),
                Err(err) => err,
            };
            assert_eq!(err.code, case.code, "{}: wrong diagnostic (got: {})", case.file, err.message);
            assert!(
                err.message.contains("(line "),
                "{}: a run-time diagnostic must name the line it failed on, got: {}",
                case.file,
                err.message
            );
        }
    }
}

// ── one table-driven test per rule family ────────────────────────────

#[test]
fn division_and_modulo_without_a_fallback_are_rejected() {
    let corpus = corpus();
    for case in ARITHMETIC_WITHOUT_FALLBACK {
        expect_rejected(case, &corpus[case.file]);
    }
}

#[test]
fn dynamic_indexing_without_a_fallback_is_rejected() {
    let corpus = corpus();
    for case in DYNAMIC_INDEX_WITHOUT_FALLBACK {
        expect_rejected(case, &corpus[case.file]);
    }
}

#[test]
fn host_record_field_reads_without_a_fallback_are_rejected() {
    let corpus = corpus();
    for case in HOST_RECORD_FIELD_WITHOUT_FALLBACK {
        expect_rejected(case, &corpus[case.file]);
    }
}

#[test]
fn capability_values_without_a_fallback_are_rejected() {
    let corpus = corpus();
    for case in CAPABILITY_VALUE_WITHOUT_FALLBACK {
        expect_rejected(case, &corpus[case.file]);
    }
}

#[test]
fn undeclared_capability_families_are_rejected() {
    let corpus = corpus();
    for case in CAPABILITY_UNDECLARED {
        expect_rejected(case, &corpus[case.file]);
    }
}

#[test]
fn constant_indices_outside_literal_bounds_are_rejected() {
    let corpus = corpus();
    for case in CONSTANT_INDEX_OUT_OF_BOUNDS {
        expect_rejected(case, &corpus[case.file]);
    }
}

#[test]
fn while_loops_without_a_fuel_ceiling_are_rejected() {
    let corpus = corpus();
    for case in WHILE_WITHOUT_FUEL_CEILING {
        expect_rejected(case, &corpus[case.file]);
    }
}

#[test]
fn recursion_without_a_declared_ceiling_is_rejected() {
    let corpus = corpus();
    for case in RECURSION_WITHOUT_CEILING.iter().filter(|r| r.stage == Stage::Front) {
        expect_rejected(case, &corpus[case.file]);
    }
}

#[test]
fn a_declared_recursion_ceiling_is_enforced_at_run_time() {
    let corpus = corpus();
    let runtime: Vec<&Rejection> = RECURSION_WITHOUT_CEILING
        .iter()
        .filter(|r| r.stage == Stage::Run)
        .collect();
    assert!(
        !runtime.is_empty(),
        "the corpus must cover the run-time enforcement of the declared ceiling"
    );
    for case in runtime {
        expect_rejected(case, &corpus[case.file]);
    }
}

#[test]
fn positive_controls_compile_and_run_clean() {
    let corpus = corpus();
    for control in POSITIVE_CONTROLS {
        let opts = run_options(control.args);
        let report = kaze::run_source(&corpus[control.file], control.file, &opts).unwrap_or_else(|e| {
            panic!(
                "{}: a valid program must be accepted, got {}: {}",
                control.file,
                e.code,
                e.message
            )
        });
        assert_eq!(
            report.value, control.value,
            "{}: wrong result after a clean run",
            control.file
        );
    }
}

// ── corpus integrity, determinism, panic freedom ─────────────────────

#[test]
fn the_corpus_is_large_enough_and_every_fixture_is_classified() {
    let corpus = corpus();
    let rejections = all_rejections();
    let negative: BTreeSet<&str> = rejections.iter().map(|r| r.file).collect();
    let positive: BTreeSet<&str> = POSITIVE_CONTROLS.iter().map(|c| c.file).collect();

    assert!(
        negative.len() >= 50,
        "Phase 2 needs 50+ rejected programs, found {}",
        negative.len()
    );
    assert_eq!(
        negative.len(),
        rejections.len(),
        "a fixture is listed twice in the negative tables"
    );
    assert_eq!(
        positive.len(),
        POSITIVE_CONTROLS.len(),
        "a fixture is listed twice in the positive controls"
    );
    assert!(
        negative.is_disjoint(&positive),
        "a fixture cannot be both rejected and a positive control: {:?}",
        negative.intersection(&positive).collect::<Vec<_>>()
    );

    let listed: BTreeSet<&str> = negative.union(&positive).copied().collect();
    let on_disk: BTreeSet<&str> = corpus.keys().map(String::as_str).collect();
    let orphans: Vec<&&str> = on_disk.difference(&listed).collect();
    assert!(
        orphans.is_empty(),
        "these fixtures are on disk but in no table (add them, or delete them): {:?}",
        orphans
    );
    let missing: Vec<&&str> = listed.difference(&on_disk).collect();
    assert!(
        missing.is_empty(),
        "these tables name fixtures that are not on disk: {:?}",
        missing
    );

    // A fixture's own header and marker must agree with the table, so editing
    // one without the other is impossible.
    for case in &rejections {
        let src = &corpus[case.file];
        assert_eq!(
            declared_expectation(src),
            Some(case.code),
            "{}: the '# expect:' header disagrees with the table ({} expected)",
            case.file,
            case.code
        );
        let mark = bad_mark_line(src)
            .unwrap_or_else(|| panic!("{}: a negative fixture needs a '# BAD' marker", case.file));
        if let Points::Line(want) = case.points {
            assert_eq!(
                mark, want,
                "{}: '# BAD' is on line {} but the table says {}",
                case.file, mark, want
            );
        }
    }
    for control in POSITIVE_CONTROLS {
        let src = &corpus[control.file];
        assert_eq!(
            declared_expectation(src),
            Some("ok"),
            "{}: a positive control must declare '# expect: ok'",
            control.file
        );
        assert!(
            bad_mark_line(src).is_none(),
            "{}: a positive control must not carry a '# BAD' marker",
            control.file
        );
    }

    println!(
        "strict corpus: {} fixtures — {} rejected, {} positive controls",
        on_disk.len(),
        negative.len(),
        positive.len()
    );
}

#[test]
fn checking_a_flawed_source_twice_gives_the_same_code_and_message() {
    let corpus = corpus();
    for case in all_rejections() {
        let src = &corpus[case.file];
        let (first_code, first_message) = diagnostic(case, src);
        let (second_code, second_message) = diagnostic(case, src);
        assert_eq!(
            (first_code.as_str(), first_message.as_str()),
            (second_code.as_str(), second_message.as_str()),
            "{}: the diagnostic is not identical on the second pass",
            case.file
        );
    }
}

/// Run the stage that must refuse `src` and return the code and message it
/// produced. Panics when the program is accepted — determinism only means
/// something for a program that is actually refused.
fn diagnostic(case: &Rejection, src: &str) -> (String, String) {
    match case.stage {
        Stage::Front => match kaze::prepare(src, case.file) {
            Ok(_) => panic!(
                "{}: expected {} but the front end accepted the program",
                case.file, case.code
            ),
            Err(err) => (err.code, err.message),
        },
        Stage::Run => match kaze::run_source(src, case.file, &run_options("")) {
            Ok(_) => panic!(
                "{}: expected {} at run time but the run finished cleanly",
                case.file, case.code
            ),
            Err(err) => (err.code, err.message),
        },
    }
}

#[test]
fn no_fixture_panics_the_engine() {
    let corpus = corpus();
    for (file, src) in &corpus {
        let front = catch_unwind(AssertUnwindSafe(|| kaze::prepare(src, file)));
        assert!(front.is_ok(), "{}: the front end panicked", file);
        let run = catch_unwind(AssertUnwindSafe(|| kaze::run_source(src, file, &RunOptions::default())));
        assert!(run.is_ok(), "{}: the VM panicked", file);
    }
}
