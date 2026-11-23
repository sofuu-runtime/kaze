// The native backend: emitted C must build, run, and stay inside a memory
// budget instead of taking the host into swap.

use std::path::PathBuf;
use std::process::Command;

fn work(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kaze-emitted-{}-{}", name, std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn have_cc() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Emit C for `src`, compile it with cc, and return the binary path.
fn build(name: &str, src: &str) -> Option<PathBuf> {
    if !have_cc() {
        eprintln!("SKIP {}: no C compiler", name);
        return None;
    }
    let dir = work(name);
    let c_path = dir.join("program.c");
    let bin = dir.join("program");
    let code = kaze::emit_c_source(
        src,
        "case.kaze",
        &kaze::c_codegen::CodegenOptions { standalone: true, entry: Some("main".into()) },
    )
    .expect("emit C");
    std::fs::write(&c_path, code).expect("write C");
    let status = Command::new("cc")
        .arg("-std=c99")
        .arg("-O2")
        .arg("-Wno-unused-function")
        .arg(&c_path)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("run cc");
    assert!(status.success(), "the emitted C must compile");
    Some(bin)
}

#[test]
fn a_compiled_program_runs_and_prints_its_value() {
    let Some(bin) = build("hello", "action main():\n    return 2 + 3\n") else { return };
    let out = Command::new(&bin).output().expect("run");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.trim().contains('5'), "the program returns 5: {:?}", text);
}

#[test]
fn a_compiled_program_stops_with_a_diagnostic_past_its_memory_budget() {
    // A loop that keeps building lists under a tiny budget: the runtime must
    // stop with E_KAZE_MEMORY instead of being killed by the OS.
    let src = "action main():\n    let xs = []\n    let i = 0\n    while i < 4000 bounded 5000:\n        set xs to xs + [i, i, i, i]\n        set i to i + 1\n    return length of xs\n";
    let Some(bin) = build("budget", src) else { return };
    let out = Command::new(&bin)
        .env("KAZE_MAX_MEMORY", "1048576")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("E_KAZE_MEMORY"),
        "a bounded program reports E_KAZE_MEMORY: {:?}",
        err
    );
    assert_eq!(out.status.code(), Some(70), "the runtime exits 70: {:?}", out.status);
}

#[test]
fn the_same_program_runs_when_the_budget_is_raised() {
    let src = "action main():\n    let xs = []\n    let i = 0\n    while i < 200 bounded 5000:\n        set xs to xs + [i]\n        set i to i + 1\n    return length of xs\n";
    let Some(bin) = build("budget-ok", src) else { return };
    let out = Command::new(&bin)
        .env("KAZE_MAX_MEMORY", "1073741824")
        .output()
        .expect("run");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {:?}", String::from_utf8_lossy(&out.stderr));
    assert!(text.trim().contains("200"), "the loop ran: {:?}", text);
}

#[test]
fn the_declared_recursion_ceiling_is_enforced_in_native_code() {
    // The interpreter refuses the fifth frame; the emitted C must agree.
    let src = "action bang(n: int) -> int:\n    recursion bounded 4\n    if n <= 0:\n        return 0\n    return bang(n - 1)\n\naction main():\n    return bang(10)\n";
    let Some(bin) = build("recursion", src) else { return };
    let out = Command::new(&bin).output().expect("run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_KAZE_RECURSION_DEPTH"), "{:?}", err);
    assert!(err.contains("ceiling 4"), "the message names the ceiling: {:?}", err);
    assert_eq!(out.status.code(), Some(70));
}

#[test]
fn arithmetic_overflow_stops_the_native_program() {
    let src = "action main():\n    let x = 1.0\n    let i = 0\n    while i < 400 bounded 500:\n        set x to x * 1000000\n        set i to i + 1\n    return x\n";
    let Some(bin) = build("overflow", src) else { return };
    let out = Command::new(&bin).output().expect("run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_KAZE_OVERFLOW"), "{:?}", err);
    assert_eq!(out.status.code(), Some(70));
}

#[test]
fn a_contract_is_asserted_at_run_time_in_native_code() {
    // The checker cannot see through a CLI argument, so the callee's entry
    // assertion is what refuses 500.
    let src = "action check_it(duty: percent) -> bool:\n    requires duty <= 100\n    return duty > 10\n\naction main(duty: percent):\n    let v = check_it(duty)\n    return v\n";
    let Some(bin) = build("contract", src) else { return };
    let out = Command::new(&bin).arg("duty=500").output().expect("run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_KAZE_CONTRACT"), "{:?}", err);
    assert_eq!(out.status.code(), Some(70));
}

#[test]
fn a_proved_append_compiles_to_an_in_place_push() {
    let src = "action main():\n    let xs = []\n    let i = 0\n    while i < 5 bounded 10:\n        set xs to xs + [i]\n        set i to i + 1\n    return length of xs\n";
    let c = kaze::emit_c_source(
        src,
        "case.kaze",
        &kaze::c_codegen::CodegenOptions { standalone: true, entry: Some("main".into()) },
    )
    .expect("emit C");
    assert!(
        c.contains("kz_list_push_own(&xs"),
        "a proved unique owner appends in place"
    );
}

#[test]
fn a_second_binding_of_the_same_list_disables_the_in_place_push() {
    let src = "action main():\n    let xs = []\n    let i = 0\n    while i < 5 bounded 10:\n        set xs to xs + [i]\n        set i to i + 1\n    let copy = xs\n    return length of copy\n";
    let c = kaze::emit_c_source(
        src,
        "case.kaze",
        &kaze::c_codegen::CodegenOptions { standalone: true, entry: Some("main".into()) },
    )
    .expect("emit C");
    assert!(
        !c.contains("kz_list_push_own(&xs"),
        "an aliasable list must keep the copying append"
    );
}

#[test]
fn an_aliased_list_does_not_see_later_appends() {
    // The optimisation must not change what a script observes.
    let src = "action main():\n    let a = [1]\n    let b = a\n    set a to a + [2]\n    return length of b\n";
    let report = kaze::run_source(src, "case.kaze", &kaze::RunOptions::default()).expect("runs");
    assert_eq!(report.value, "1", "b keeps the list it was given");
}

#[test]
fn a_big_append_loop_stays_inside_a_small_budget() {
    // Twenty thousand appends. With a copying append this needs gigabytes of
    // intermediate lists; the in-place push needs a few hundred kilobytes.
    let src = "action main():\n    let xs = []\n    let i = 0\n    while i < 20000 bounded 20001:\n        set xs to xs + [i]\n        set i to i + 1\n    return length of xs\n";
    let Some(bin) = build("append-loop", src) else { return };
    let out = Command::new(&bin)
        .env("KAZE_MAX_MEMORY", "67108864")
        .output()
        .expect("run");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.trim().contains("20000"), "the loop ran: {:?}", text);
}

#[test]
fn appending_through_a_parameter_does_not_leak_into_an_alias() {
    // The callee aliases its parameter to a local and appends. Because the
    // caller holds a second name for that list, the callee must copy.
    let src = "action acc(list, n):\n    let out = list\n    set out to out + [1]\n    return out\n\naction main():\n    let xs = []\n    let keep = xs\n    set xs to acc(xs, 3)\n    return length of keep\n";
    let c = kaze::emit_c_source(
        src,
        "case.kaze",
        &kaze::c_codegen::CodegenOptions { standalone: true, entry: Some("main".into()) },
    )
    .expect("emit C");
    assert!(
        !c.contains("kz_list_push_own(&out"),
        "a parameter's alias must not be appended to in place"
    );
    let report = kaze::run_source(src, "case.kaze", &kaze::RunOptions::default()).expect("runs");
    assert_eq!(report.value, "0", "the alias keeps the list it was given");
}

#[test]
fn a_fallback_runs_only_when_the_operation_fails() {
    // The interpreter evaluates a fallback lazily; the emitted C used to
    // evaluate it eagerly, so a fallback with a visible effect ran even when
    // the value was fine. Both directions are pinned here.
    let src = "needs: log\naction side(n):\n    log \"fallback ran\"\n    return n\n\naction main():\n    let xs = [10, 20]\n    let i = 1\n    return xs[i] otherwise side(99)\n";
    let Some(bin) = build("lazy-ok", src) else { return };
    let out = Command::new(&bin).output().expect("run");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("20"), "the value is used: {:?}", text);
    assert!(
        !text.contains("fallback ran"),
        "an unused fallback must not run: {:?}",
        text
    );

    let miss = "needs: log\naction side(n):\n    log \"fallback ran\"\n    return n\n\naction main():\n    let xs = [10, 20]\n    let i = 9\n    return xs[i] otherwise side(99)\n";
    let Some(bin) = build("lazy-miss", miss) else { return };
    let out = Command::new(&bin).output().expect("run");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("99"), "the fallback supplies the value: {:?}", text);
    assert!(text.contains("fallback ran"), "and it ran: {:?}", text);
}

#[test]
fn a_division_fallback_only_runs_on_a_zero_divisor() {
    let src = "action main():\n    let a = 10\n    let b = 2\n    return a / b otherwise 99\n";
    let Some(bin) = build("lazy-div", src) else { return };
    let out = Command::new(&bin).output().expect("run");
    assert!(String::from_utf8_lossy(&out.stdout).contains("5"));

    let zero = "action main():\n    let a = 10\n    let b = 0\n    let c = a / b otherwise 3\n    return c\n";
    let Some(bin) = build("lazy-div-zero", zero) else { return };
    let out = Command::new(&bin).output().expect("run");
    assert!(String::from_utf8_lossy(&out.stdout).contains("3"));
}

#[test]
fn a_nested_fallback_is_lazy_too() {
    // A fallback inside a larger expression used to be eager even after the
    // statement forms became lazy. With pure operands the compiler emits a
    // ternary, so the fallback is evaluated only on failure.
    let src = "needs: log\naction side(n):\n    log \"fallback ran\"\n    return n\n\naction main():\n    let xs = [10, 20]\n    let i = 1\n    return (xs[i] otherwise side(99)) + 1\n";
    let Some(bin) = build("nested-lazy", src) else { return };
    let out = Command::new(&bin).output().expect("run");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("21"), "the value is used: {:?}", text);
    assert!(
        !text.contains("fallback ran"),
        "a nested unused fallback must not run: {:?}",
        text
    );
}

#[test]
fn a_nested_fallback_with_an_effectful_operand_is_lazy_too() {
    // The operands of the read have effects, so they cannot be evaluated twice.
    // The emitter hoists them into temporaries declared before the statement and
    // keeps the fallback in a ternary, so it still runs only on failure.
    let src = "needs: log\naction step(n):\n    log \"step ran\"\n    return n\n\naction side(n):\n    log \"FALLBACK ran\"\n    return n\n\naction main():\n    let xs = [10, 20, 30]\n    let i = 1\n    return (xs[i + step(0)] otherwise side(99)) + 1\n";
    let Some(bin) = build("hoist-lazy", src) else { return };
    let out = Command::new(&bin).output().expect("run");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("21"), "the value is used: {:?}", text);
    assert!(text.contains("step ran"), "the operand ran once: {:?}", text);
    assert_eq!(
        text.matches("step ran").count(),
        1,
        "the operand is evaluated once, not twice: {:?}",
        text
    );
    assert!(
        !text.contains("FALLBACK ran"),
        "the unused fallback must not run: {:?}",
        text
    );
}

#[test]
fn indexing_text_does_not_rescan_the_whole_string() {
    // Every `s[i]` used to call strlen on the whole string, so a character loop
    // was quadratic: a 300 KB source took seconds to lex. Strings now carry
    // their length, so this finishes immediately.
    let src = "action main():\n    let s = \"abcdefghij\"\n    let i = 0\n    let n = 0\n    while i < (length of s) bounded 100:\n        if s[i] == \"e\":\n            set n to n + 1\n        set i to i + 1\n    return n\n";
    let Some(bin) = build("str-index", src) else { return };
    let out = Command::new(&bin).output().expect("run");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("1"),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn emitted_c_compiles_with_zero_warnings_under_strict_flags() {
    // The roadmap's gate: emitted C must build with
    // `-Wall -Wextra -Werror -pedantic -std=c99`. This caught a real bug: a
    // routine with an `ensures` clause and its own local named `result` used to
    // initialise the return value from itself.
    let src = "record Point:\n    x: num\n    y: num\n\naction labelled(p: Point, tag: str) -> str:\n    ensures length of result >= 0\n    let result = tag\n    return result + \"!\"\n\naction main():\n    let xs = [1, 2, 3]\n    let i = 1\n    let v = xs[i] otherwise 0\n    let p = Point(x: 1, y: 2)\n    let s = labelled(p, \"go\")\n    return v + (length of s)\n";
    let c = kaze::emit_c_source(
        src,
        "strict.kaze",
        &kaze::c_codegen::CodegenOptions { standalone: true, entry: Some("main".into()) },
    )
    .expect("emit C");
    let dir = std::env::temp_dir();
    let path = dir.join("kaze_strict_emitted.c");
    std::fs::write(&path, &c).expect("write C");
    let obj = dir.join("kaze_strict_emitted.o");
    let out = Command::new("clang")
        .args(["-std=c99", "-O2", "-Wall", "-Wextra", "-pedantic", "-Werror", "-c"])
        .arg(&path)
        .arg("-o")
        .arg(&obj)
        .output()
        .expect("run clang");
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "strict build failed:\n{}", text);
    assert!(text.trim().is_empty(), "warnings under -Werror:\n{}", text);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&obj);
}

#[test]
fn point_arithmetic_and_spatial_math_match_the_interpreter() {
    // These used to lower to 0 in the native backend: a script computing a
    // distance got zero. The formulas mirror the interpreter's geo module.
    let src = "needs: log\naction main():\n    let a = vector(47.6, 8.5, 500)\n    let b = vector(47.6004, 8.5004, 520)\n    let ground = distance between a and b\n    let slant = slant distance between a and b\n    let bearing = bearing from a to b\n    let offset = b - a\n    let doubled = offset * 2\n    let length = length of doubled\n    return ground\n";
    let Some(bin) = build("geo", src) else { return };
    let out = Command::new(&bin).output().expect("run");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    // The interpreter returns 53.64501025917031 for the same points.
    let value: f64 = text
        .trim()
        .lines()
        .last()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(f64::NAN);
    assert!((value - 53.645_010_259_17).abs() < 1e-6, "ground distance: {:?}", text);

    // scaling a point keeps its direction; the length doubles
    let src2 = "action main():\n    let v = vector(3, 4, 0)\n    let w = v * 2\n    return length of w\n";
    let Some(bin2) = build("geo-scale", src2) else { return };
    let out2 = Command::new(&bin2).output().expect("run");
    assert!(
        String::from_utf8_lossy(&out2.stdout).trim().contains("10"),
        "the doubled vector is 10 long: {:?}",
        String::from_utf8_lossy(&out2.stdout)
    );
}

#[test]
fn the_two_engines_agree_on_a_program_that_uses_points() {
    // Differential gate: the interpreter and the emitted C must print the same
    // value for the same program.
    let src = "action main():\n    let a = vector(1, 2, 2)\n    let b = vector(4, 6, 2)\n    let offset = b - a\n    return length of offset\n";
    let dir = std::env::temp_dir().join(format!("kaze-diff-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("program.kaze");
    std::fs::write(&path, src).expect("write");
    let vm = kaze::run_source(src, "program.kaze", &kaze::RunOptions::default());
    let Ok(vm) = vm else { return };
    let Some(bin) = build("diff-points", src) else { return };
    let out = Command::new(&bin).output().expect("run");
    let native = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let vm_value: f64 = vm.value.parse().unwrap_or(f64::NAN);
    let nat_value: f64 = native.parse().unwrap_or(f64::NAN);
    assert!(
        (vm_value - nat_value).abs() < 1e-9,
        "vm {} vs native {}",
        vm.value,
        native
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_two_engines_agree_on_every_example() {
    // The strongest gate for "the native backend implements the language": every
    // example that both engines can run must produce the same value and the same
    // sequence of logged output. It found the missing point arithmetic and the
    // spatial maths, which used to lower to zero.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = root.join("scripts/diff-check.sh");
    if !script.exists() {
        eprintln!("skipped: diff-check.sh is not present");
        return;
    }
    let mut examples: Vec<std::path::PathBuf> = Vec::new();
    for dir in ["examples", "examples/missions", "examples/modular"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "kaze").unwrap_or(false) {
                examples.push(path);
            }
        }
    }
    examples.sort();
    if examples.is_empty() {
        eprintln!("skipped: no examples");
        return;
    }
    let out = Command::new("bash")
        .arg(&script)
        .args(&examples)
        .current_dir(root)
        .output()
        .expect("run diff-check");
    let text = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "the engines disagree:\n{}", text);
    assert!(text.contains(" agree"), "the check ran: {}", text);
}

#[test]
fn hooks_run_on_ticks_in_the_compiled_binary() {
    // The native backend used to drop `when`/`every` bodies entirely and print
    // "no main entry". They are generated now and driven per host tick, the same
    // way the interpreter's --ticks works.
    let src = "needs: log\nwhen 1 > 0:\n    log \"hook fired\"\n\nevery 100 ms:\n    log \"tick\"\n";
    let Some(bin) = build("hooks", src) else { return };
    let two = Command::new(&bin).arg("--ticks").arg("2").output().expect("run");
    let text = String::from_utf8_lossy(&two.stdout).to_string();
    assert_eq!(text.matches("hook fired").count(), 2, "one per tick: {:?}", text);
    assert_eq!(text.matches("tick").count(), 2, "one per tick: {:?}", text);

    let none = Command::new(&bin).output().expect("run");
    let text = String::from_utf8_lossy(&none.stdout).to_string();
    assert!(
        !text.contains("hook fired") && !text.contains("tick"),
        "no ticks means no hooks: {:?}",
        text
    );
}

#[test]
fn a_parameter_resolves_through_all_four_layers_in_the_compiled_binary() {
    // Phase 4.4: CLI > environment > config file > signature default. The
    // compiled binary had three layers; it also used to invent 0 when nothing
    // was supplied, instead of refusing with E_KAZE_PARAM.
    let src = "action main(duty: percent) -> bool:\n    return duty > 10\n";
    let Some(bin) = build("layers", src) else { return };
    let dir = std::env::temp_dir().join(format!("kaze-layers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let cfg = dir.join("agent.config.json");
    std::fs::write(&cfg, "{\"duty\": 55}").expect("write config");

    // 1. config file alone
    let out = Command::new(&bin)
        .current_dir(&dir)
        .arg("--config")
        .arg(&cfg)
        .output()
        .expect("run");
    assert!(String::from_utf8_lossy(&out.stdout).contains("true"), "config layer");

    // 2. environment beats the config file
    let out = Command::new(&bin)
        .current_dir(&dir)
        .arg("--config")
        .arg(&cfg)
        .env("AGENT_DUTY", "4")
        .output()
        .expect("run");
    assert!(String::from_utf8_lossy(&out.stdout).contains("false"), "env beats config");

    // 3. the command line beats both
    let out = Command::new(&bin)
        .current_dir(&dir)
        .arg("--config")
        .arg(&cfg)
        .arg("--arg")
        .arg("duty=5")
        .env("AGENT_DUTY", "4")
        .output()
        .expect("run");
    assert!(String::from_utf8_lossy(&out.stdout).contains("false"), "cli beats env");

    // 4. nothing at all is a refusal with guidance, not a silent zero
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).expect("empty dir");
    let out = Command::new(&bin)
        .current_dir(&empty)
        .env_remove("AGENT_DUTY")
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_KAZE_PARAM"), "refuses: {:?}", err);
    assert!(err.contains("--arg duty=<value>"), "names the fix: {:?}", err);
    assert_eq!(out.status.code(), Some(70));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_typed_parameter_refuses_a_value_that_is_not_its_type() {
    // The interpreter refuses `--arg duty=abc` for a `percent`. The compiled
    // binary used to pass the raw text through, so a numeric parameter could
    // hold a string and arithmetic on it silently produced zero.
    let src = "action main(duty: percent, armed: bool) -> num:\n    return duty\n";
    let Some(bin) = build("coerce", src) else { return };

    // a number spelled as text is coerced, as the interpreter does
    let out = Command::new(&bin).args(["--arg", "duty=42", "--arg", "armed=true"]).output().expect("run");
    assert!(String::from_utf8_lossy(&out.stdout).contains("42"));

    // junk in a numeric parameter is refused with the interpreter's code
    let out = Command::new(&bin).args(["--arg", "duty=abc", "--arg", "armed=true"]).output().expect("run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_KAZE_PARAM"), "refuses: {:?}", err);
    assert!(err.contains("declared as a number"), "says what it wanted: {:?}", err);
    assert_eq!(out.status.code(), Some(70));

    // and junk in a boolean parameter too
    let out = Command::new(&bin).args(["--arg", "duty=1", "--arg", "armed=yes"]).output().expect("run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("true or false"), "boolean message: {:?}", err);
    assert_eq!(out.status.code(), Some(70));
}

#[test]
fn a_field_read_on_something_that_is_not_a_record_is_a_type_error() {
    // The interpreter refuses `.x` on text with E_KAZE_TYPE. The compiled binary
    // used to answer "nothing", so a wrong value flowed on as zero/empty.
    let src = "record Point:\n    x: num\n\naction main(p: Point) -> num:\n    return p.x\n";
    let Some(bin) = build("field-type", src) else { return };
    let out = Command::new(&bin).arg("--arg").arg("p=hello").output().expect("run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_KAZE_TYPE"), "refuses: {:?}", err);
    assert_eq!(out.status.code(), Some(70));

    // and a real record still reads its field
    let ok = "record Point:\n    x: num\n\naction main() -> num:\n    let p = Point(x: 7)\n    return p.x\n";
    let Some(bin) = build("field-ok", ok) else { return };
    let out = Command::new(&bin).output().expect("run");
    assert!(String::from_utf8_lossy(&out.stdout).contains("7"));
}

#[test]
fn a_pipe_fills_the_first_parameter_in_both_engines() {
    // AGENTS.md documents pipes as an input path. The compiled binary read one;
    // the interpreter did not, so the same invocation gave a value and a refusal.
    let src = "action main(src: str) -> num:\n    return length of src\n";
    let opts = kaze::RunOptions {
        stdin_value: Some("abcd\n".to_string()),
        ..Default::default()
    };
    let report = kaze::run_source(src, "pipe.kaze", &opts).expect("runs");
    assert_eq!(report.value, "4", "the trailing newline is not part of the value");

    // an explicit argument still wins over the pipe
    let mut opts = kaze::RunOptions {
        stdin_value: Some("abcd\n".to_string()),
        ..Default::default()
    };
    opts.args.insert("src".into(), serde_json::json!("xyz"));
    let report = kaze::run_source(src, "pipe.kaze", &opts).expect("runs");
    assert_eq!(report.value, "3");

    // and the compiled binary agrees
    let Some(bin) = build("pipe", src) else { return };
    use std::io::Write;
    let mut child = Command::new(&bin)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"abcd\n")
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("4"),
        "the binary trims the newline too: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn the_two_engines_agree_on_generated_programs() {
    // Differential fuzzing: the generator emits programs that only use
    // constructs both engines define, and every one must produce the same
    // output. This is the gate that found the spatial maths answering zero, and
    // then the last-digit formatting difference.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = root.join("scripts/diff-fuzz.sh");
    if !script.exists() {
        eprintln!("skipped: diff-fuzz.sh is not present");
        return;
    }
    let out = Command::new("bash")
        .arg(&script)
        .arg("12")
        .arg("1")
        .current_dir(root)
        .output()
        .expect("run diff-fuzz");
    let text = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "the engines disagree:\n{}", text);
    assert!(text.contains(" agree"), "the fuzz ran: {}", text);
    assert!(text.contains("0 differ"), "no disagreements: {}", text);
}

#[test]
fn a_capability_call_without_a_backend_fails_closed() {
    // The compiled binary used to print a stub line and return nothing, so a
    // script that needs a sensor succeeded with fabricated data — the fail-open
    // RULES.md §1.9 forbids. Both engines now refuse with E_CAP_MISSING.
    let src = "needs: lidar\naction main() -> num:\n    let d = lidar.front_distance() otherwise 5 meters\n    return 1\n";
    let Some(bin) = build("cap-closed", src) else { return };
    let out = Command::new(&bin).output().expect("run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_CAP_MISSING"), "refuses: {:?}", err);
    assert!(err.contains("lidar"), "names the family: {:?}", err);
    assert_eq!(out.status.code(), Some(70));

    // a capability the binary does implement (log) still runs
    let log = "needs: log\naction main() -> num:\n    log \"fine\"\n    return 3\n";
    let Some(bin) = build("cap-log", log) else { return };
    let out = Command::new(&bin).output().expect("run");
    assert!(String::from_utf8_lossy(&out.stdout).contains("fine"));
}

#[test]
fn floating_point_math_is_bit_identical_across_engines() {
    // A C compiler may contract a*b - c*d into a fused multiply-add, which rounds
    // differently from the interpreter and made a bearing between two identical
    // points come out 180 instead of 0. The emitted C disables contraction.
    let src = "action main() -> num:\n    let p = vector(-3, 1, 2)\n    let q = vector(-3, 1, 4)\n    let br = bearing from p to q\n    return br\n";
    let report = kaze::run_source(src, "fma.kaze", &kaze::RunOptions::default()).expect("runs");
    assert_eq!(report.value, "0", "the interpreter gets zero");
    let Some(bin) = build("fma", src) else { return };
    let out = Command::new(&bin).output().expect("run");
    let native = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(native, report.value, "the binary must agree bit for bit");
}

#[test]
fn the_two_c_emitters_build_programs_with_the_same_behaviour() {
    // The Rust emitter and the pure-Kaze emitter (the bootstrap's own) used to
    // disagree: wrong capability code, flat-Euclidean distances, and hooks that
    // never ran. This gate builds stage 1 from the current monolith, emits every
    // example with both emitters, and compares the binaries.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = root.join("scripts/emitter-triangle.sh");
    if !script.exists() {
        eprintln!("skipped: emitter-triangle.sh is not present");
        return;
    }
    let mut examples: Vec<std::path::PathBuf> = Vec::new();
    for dir in ["examples", "examples/missions", "examples/modular"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "kaze").unwrap_or(false) {
                examples.push(path);
            }
        }
    }
    examples.sort();
    let out = Command::new("bash")
        .arg(&script)
        .args(&examples)
        .current_dir(root)
        .output()
        .expect("run the triangle check");
    let text = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "the emitters disagree:\n{}", text);
    assert!(text.contains("0 differ"), "no disagreements: {}", text);
}
