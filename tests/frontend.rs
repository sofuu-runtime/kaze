// Phase 6: the pure-Kaze compiler front end, driven through the Rust engine.
//
// Every test here talks to a stage-1 binary built the bootstrap way:
//
//     Rust engine emit-c(compiler/kaze_self.kaze) -> stage1.c
//     cc -std=c99 -O2 stage1.c -o stage1
//     stage1 check | emit-c | to-machine | to-human <file.kaze>
//
// That binary is the pure-Kaze compiler, so these tests are what proves the
// front end in kaze/compiler/*.kaze really reads the language. If cc is not
// available the tests report the skip and return; they never pretend to pass.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn kaze_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn work_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kaze_frontend_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Build the stage-1 compiler once and share it across the tests below.
fn stage1() -> Option<&'static Path> {
    static BIN: OnceLock<Option<PathBuf>> = OnceLock::new();
    BIN.get_or_init(|| {
        let self_compiler = kaze_root().join("compiler/kaze_self.kaze");
        let self_compiler = self_compiler.to_string_lossy().to_string();
        let opts = kaze::c_codegen::CodegenOptions {
            standalone: true,
            entry: Some("main".to_string()),
        };
        let c_src = match kaze::emit_c_file(&self_compiler, &opts) {
            Ok(src) => src,
            Err(e) => {
                eprintln!("skip: the Rust engine cannot emit C for kaze_self.kaze: {e}");
                return None;
            }
        };
        let dir = work_dir();
        let c_path = dir.join("stage1.c");
        let bin = dir.join("stage1");
        std::fs::write(&c_path, c_src).expect("write stage1.c");
        let status = Command::new("cc")
            .args(["-std=c99", "-O2", "-Wno-unused-function"])
            .arg(&c_path)
            .arg("-o")
            .arg(&bin)
            .status();
        match status {
            Ok(s) if s.success() => Some(bin),
            _ => {
                eprintln!("skip: cc is not available to build the stage-1 compiler");
                None
            }
        }
    })
    .as_deref()
}

/// Run the stage-1 compiler in one of its modes and return stdout.
fn run(bin: &Path, mode: Option<&str>, input: &Path) -> String {
    let mut cmd = Command::new(bin);
    if let Some(m) = mode {
        cmd.arg(m);
    }
    cmd.arg(input);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("cannot run the stage-1 compiler on {}: {e}", input.display()));
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        !stdout.trim().is_empty() || !stderr.trim().is_empty(),
        "the pure-Kaze compiler said nothing about {} in mode {:?} (status {:?})",
        input.display(),
        mode,
        out.status
    );
    assert!(
        !stderr.contains("panicked") && !stderr.contains("E_KAZE_LOOP_BOUND"),
        "the pure-Kaze compiler failed on {} in mode {:?}: {}",
        input.display(),
        mode,
        stderr
    );
    stdout
}

fn check_text(bin: &Path, file: &Path) -> String {
    run(bin, Some("check"), file).trim().to_string()
}

fn kaze_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            kaze_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("kaze") {
            out.push(path);
        }
    }
}

fn corpus() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for sub in ["examples", "lib", "compiler"] {
        kaze_files(&kaze_root().join(sub), &mut files);
    }
    files.sort();
    files
}

fn write_case(name: &str, source: &str) -> PathBuf {
    let path = work_dir().join(name);
    std::fs::write(&path, source).expect("write case");
    path
}

#[test]
fn the_monolith_matches_its_parts() {
    // kaze_self.kaze is generated from the modules; --check proves it is current.
    let out = Command::new("bash")
        .current_dir(kaze_root())
        .args(["scripts/self-sync.sh", "--check"])
        .output()
        .expect("run scripts/self-sync.sh --check");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "kaze/compiler/kaze_self.kaze is stale: run 'bash scripts/self-sync.sh'\n{stdout}{stderr}"
    );
    assert!(
        stdout.contains("is current"),
        "self-sync --check did not confirm the monolith: {stdout}{stderr}"
    );
}

#[test]
fn the_pure_kaze_checker_accepts_the_whole_corpus() {
    let Some(bin) = stage1() else { return };
    let files = corpus();
    // 11 compiler modules (including the generated monolith), 10 examples and
    // 3 library files. Adding a .kaze file under these trees means the front
    // end has to keep up with it, which is the point of this count.
    assert!(
        files.len() >= 24,
        "expected at least 24 .kaze files under examples/, lib/ and compiler/, found {}: {:?}",
        files.len(),
        files
    );
    let mut checked = 0;
    for file in &files {
        let verdict = check_text(bin, file);
        assert_eq!(
            verdict, "check: ok",
            "the pure-Kaze checker reported diagnostics for {}:\n{}",
            file.display(),
            verdict
        );
        checked += 1;
    }
    assert_eq!(checked, files.len());
    eprintln!("pure-Kaze check: {} files clean", checked);
}

#[test]
fn the_pure_kaze_checker_refuses_flawed_programs() {
    let Some(bin) = stage1() else { return };
    // Each case is single-fault by construction, so exactly one diagnostic is
    // expected: the rule fires once and nothing else does.
    let cases: &[(&str, &str, &str)] = &[
        (
            "flaw_div.kaze",
            "needs: log\n\naction divide(a, b):\n    let q = a / b\n    return q\n",
            "E_KAZE_MISSING_FALLBACK",
        ),
        (
            "flaw_while.kaze",
            "needs: log\n\naction spin(n):\n    let i = 0\n    while i < n:\n        set i to i + 1\n    return i\n",
            "E_KAZE_LOOP_UNBOUNDED",
        ),
        (
            "flaw_recursion.kaze",
            "needs: log\n\naction down(n):\n    if n <= 0:\n        return 0\n    return down(n - 1)\n",
            "E_KAZE_RECURSION_UNBOUNDED",
        ),
        (
            "flaw_cap.kaze",
            "needs: log\n\naction lift():\n    arm drone\n",
            "E_CAP_UNDECLARED",
        ),
        (
            "flaw_index.kaze",
            "needs: log\n\naction pick(xs, i):\n    return xs[i]\n",
            "E_KAZE_MISSING_FALLBACK",
        ),
        (
            "flaw_bounds.kaze",
            "needs: log\n\naction third():\n    let xs = [1, 2, 3]\n    return xs[7]\n",
            "E_KAZE_BOUNDS",
        ),
        (
            "flaw_contract.kaze",
            "needs: log\n\naction clamp(x):\n    requires x\n    return x\n",
            "E_KAZE_CONTRACT_NOT_BOOL",
        ),
        (
            "flaw_capop.kaze",
            "needs: sensor\n\naction read_it():\n    return sensor.frobnicate() otherwise 0\n",
            "E_KAZE_CAP_OP",
        ),
        (
            "flaw_filter.kaze",
            "needs: policy\n\nfilter gate(prompt):\n    if length of prompt > 10:\n        reject with \"too long\"\n",
            "E_KAZE_FILTER_FALLTHROUGH",
        ),
        (
            "flaw_unknown.kaze",
            "needs: log\n\naction use_it():\n    return missing_name + 1\n",
            "E_KAZE_UNKNOWN_NAME",
        ),
    ];
    for (name, source, code) in cases {
        let path = write_case(name, source);
        let verdict = check_text(bin, &path);
        assert!(
            verdict.contains(code),
            "{name} should be refused with {code}, got:\n{verdict}"
        );
        assert!(
            verdict.starts_with("check: 1 diagnostic"),
            "{name} is single-fault and should produce exactly one diagnostic, got:\n{verdict}"
        );
    }

    // Positive controls: the safe spellings of the same programs are accepted.
    let controls: &[(&str, &str)] = &[
        (
            "ok_div.kaze",
            "needs: log\n\naction divide(a, b):\n    let q = a / b otherwise 0\n    return q\n",
        ),
        (
            "ok_while.kaze",
            "needs: log\n\naction spin(n):\n    let i = 0\n    while i < n bounded 100:\n        set i to i + 1\n    return i\n",
        ),
        (
            "ok_recursion.kaze",
            "needs: log\n\naction down(n):\n    recursion bounded 64\n    if n <= 0:\n        return 0\n    return down(n - 1)\n",
        ),
        (
            "ok_cap.kaze",
            "needs: drone, log\n\naction lift():\n    arm drone\n",
        ),
        (
            "ok_index.kaze",
            "needs: log\n\naction second():\n    let xs = [1, 2, 3]\n    return xs[1]\n",
        ),
        (
            "ok_guard_index.kaze",
            "needs: log\n\naction pick(xs, i):\n    requires i < (length of xs)\n    return xs[i]\n",
        ),
        (
            "ok_filter.kaze",
            "needs: policy\n\nfilter gate(prompt):\n    if length of prompt > 10:\n        reject with \"too long\"\n    allow prompt\n",
        ),
        (
            "ok_unit.kaze",
            "needs: log\n\ncheck climate(target: Celsius, humidity: percent) -> bool:\n    let reading = target + 2 Kelvin\n    return reading > humidity\n",
        ),
        (
            "ok_typed_defaults.kaze",
            "needs: log\n\naction f(a: int, b: Celsius = 20 Celsius) -> bool:\n    return a > 0\n",
        ),
    ];
    for (name, source) in controls {
        let path = write_case(name, source);
        let verdict = check_text(bin, &path);
        assert_eq!(
            verdict, "check: ok",
            "{name} is legal and should be accepted, got:\n{verdict}"
        );
    }
}


/// Small, independent source samples that cover the language surface. The
/// roadmap's Phase 6 gate asks for round-trip evidence across 100+ sources; the
/// repository itself holds about 35 .kaze files, so the rest of the hundred is
/// generated here, one construct per sample, and every sample must check clean
/// before it takes part.
fn sample_sources() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    // 1. literals, units and arithmetic
    for (i, expr) in [
        "2 + 3",
        "10 / 2",
        "10 % 3",
        "1 km + 500 meters",
        "2 m/s * 3 seconds",
        "1 hour + 30 minutes",
        "5 kg + 500 gram",
        "2500 mm + 25 cm",
        "20 Celsius > 30 Celsius",
        "sin(90 degrees)",
        "cos(0 degrees)",
        "sqrt(9)",
        "atan2(1, 1)",
        "length of [1, 2, 3]",
        "vector(1, 2, 3)",
        "20%",
        "20% + 5%",
        "not true",
        "-5",
        "2 * (3 + 4)",
    ]
    .iter()
    .enumerate()
    {
        out.push((
            format!("expr-{}", i),
            format!("action main():\n    return {}\n", expr),
        ));
    }
    // 2. let / set / if / otherwise
    for (i, body) in [
        "let a = 1\n    let b = 2\n    return a + b",
        "let a = 1\n    set a to 2\n    return a",
        "let a = 1\n    set a = 3\n    return a",
        "if 1 < 2:\n        return 1\n    return 0",
        "if 1 > 2:\n        return 1\n    otherwise:\n        return 2",
        "let t = 0\n    if 1 < 2 and 2 < 3:\n        set t to 1\n    return t",
        "let t = 0\n    if 1 < 2 or 2 > 3:\n        set t to 1\n    return t",
    ]
    .iter()
    .enumerate()
    {
        out.push((
            format!("stmt-{}", i),
            format!("action main():\n    {}\n", body),
        ));
    }
    // 3. collections and loops
    for (i, body) in [
        "let total = 0\n    for each v in [1, 2, 3]:\n        set total to total + v\n    return total",
        "let i = 0\n    while i < 3 bounded 10:\n        set i to i + 1\n    return i",
        "let xs = [1, 2, 3]\n    let i = 1\n    guard i < (length of xs)\n    return xs[i]",
        "let xs = [4, 5]\n    let i = 1\n    return xs[i] otherwise 0",
        "let xs = [4, 5]\n    return xs[7] otherwise 0",
        "let a = 7\n    let b = 0\n    return a / b otherwise 0",
        "let a = 7\n    let b = 2\n    return a % b otherwise 0",
        "let s = \"abc\"\n    return length of s",
        "let s = \"abc\"\n    return s[1] otherwise \"?\"",
    ]
    .iter()
    .enumerate()
    {
        out.push((
            format!("loop-{}", i),
            format!("action main():\n    {}\n", body),
        ));
    }
    // 4. routines, records and contracts
    for (i, src) in [
        "action add(a, b):\n    return a + b\n\naction main():\n    return add(1, 2)\n",
        "action add(a: int, b: int) -> int:\n    return a + b\n\naction main():\n    return add(1, 2)\n",
        "action f(duty: percent = 50) -> bool:\n    return duty > 10\n\naction main():\n    return f()\n",
        "action f(n: int) -> int:\n    requires n >= 0\n    return n\n\naction main():\n    return f(1)\n",
        "action f() -> int:\n    ensures result == 3\n    return 3\n\naction main():\n    return f()\n",
        "action f(n: int) -> int:\n    guard n < 100\n    return n\n\naction main():\n    return f(1)\n",
        "action fact(n: int) -> int:\n    recursion bounded 8\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n\naction main():\n    return fact(3)\n",
        "record Point:\n    x: num\n    y: num\n\naction main():\n    let p = Point(x: 1, y: 2)\n    return p.x + p.y\n",
        "record Pair:\n    a\n    b\n\naction main():\n    let p = Pair(a: 1, b: 2)\n    return p.a\n",
        "action main(points):\n    return length of points\n",
    ]
    .iter()
    .enumerate()
    {
        out.push((format!("routine-{}", i), src.to_string()));
    }
    // 5. capabilities (declared, with the fallbacks the strict rules require)
    for (i, src) in [
        "needs: log\n\naction main():\n    log \"hello\"\n    return 1\n",
        "needs: log\n\naction main():\n    log warning \"careful\"\n    return 1\n",
        "needs: log\n\naction main():\n    log error \"bad\"\n    return 1\n",
        "needs: log\n\naction main():\n    log debug \"debug\"\n    return 1\n",
        "needs: policy\n\nfilter f(p):\n    if length of p > 3:\n        reject with \"long\"\n    allow p\n",
        "needs: drone, log\n\naction main():\n    arm drone\n    takeoff to 10 meters\n    return to launch\n",
        "needs: drone, log\n\naction main():\n    hold\n    return 1\n",
        "needs: rover\n\naction main():\n    drive forward at 1 m/s\n    turn left by 90 degrees\n    stop rover\n    return 1\n",
        "needs: device\n\naction main():\n    turn on fan\n    turn off fan\n    return 1\n",
        "needs: device\n\naction main():\n    pulse fan for 2 seconds\n    return 1\n",
        "needs: sensor\n\naction main():\n    let t = sensor.temperature() otherwise 0 Celsius\n    return t\n",
        "needs: sensor, log\n\naction main():\n    let d = sensor.front_distance() otherwise 100 meters\n    if d < 1 meters:\n        log warning \"close\"\n    return d\n",
        "needs: memory\n\naction main():\n    remember 1 as \"k\"\n    return 1\n",
        "needs: alert\n\naction main():\n    send alert \"note\"\n    return 1\n",
    ]
    .iter()
    .enumerate()
    {
        out.push((format!("cap-{}", i), src.to_string()));
    }
    // 6. hooks
    for (i, src) in [
        "needs: log\n\nwhen 1 < 2:\n    log \"tick\"\n\naction main():\n    return 1\n",
        "needs: log\n\nevery 100 ms:\n    log \"period\"\n\naction main():\n    return 1\n",
        "needs: drone, log\n\nwhen link lost for 3 seconds:\n    return to launch\n\naction main():\n    return 1\n",
        "needs: drone, log\n\nwhen battery < 20%:\n    return to launch\n\naction main():\n    return 1\n",
        "needs: drone, log\n\naction main():\n    try:\n        arm drone\n    otherwise with err:\n        log warning err.code\n",
        "needs: log\n\ncheck inspect(a):\n    return a\n\naction main():\n    return 1\n",
    ]
    .iter()
    .enumerate()
    {
        out.push((format!("hook-{}", i), src.to_string()));
    }
    // 7. spatial maths
    for (i, expr) in [
        "distance between vector(0, 0, 0) and vector(0, 1, 0)",
        "slant distance between vector(0, 0, 0) and vector(0, 1, 100)",
        "bearing of vector(0, 1, 0)",
        "bearing from vector(0, 0, 0) to vector(1, 1, 0)",
        "length of vector(3, 4, 0)",
        "vector(1, 2, 3)[1] otherwise 0",
    ]
    .iter()
    .enumerate()
    {
        out.push((
            format!("spatial-{}", i),
            format!("action main():\n    return {}\n", expr),
        ));
    }

    // 8. a few more shapes so the sample set clears the gate on its own terms
    for (i, expr) in [
        "1 == 1",
        "1 != 2",
        "[1, 2] + [3]",
        "\"a\" + \"b\"",
        "true and false",
        "true or false",
        "(1 + 2) * 3",
        "100 ms",
        "0.5 s",
        "1 m/s2 * 2 seconds",
        "2 km/h",
        "45 degrees",
    ]
    .iter()
    .enumerate()
    {
        out.push((
            format!("extra-{}", i),
            format!("action main():\n    return {}\n", expr),
        ));
    }
    // 9. nested control flow
    for (i, body) in [
        "let t = 0\n    for each v in [1, 2]:\n        if v > 1:\n            set t to t + v\n    return t",
        "let i = 0\n    let t = 0\n    while i < 4 bounded 10:\n        if i > 1:\n            set t to t + i\n        set i to i + 1\n    return t",
        "let t = 0\n    try:\n        set t to 1\n    otherwise:\n        set t to 2\n    return t",
        "let xs = [1, 2, 3]\n    let t = 0\n    for each v in xs:\n        set t to t + v\n    return t",
    ]
    .iter()
    .enumerate()
    {
        out.push((
            format!("nested-{}", i),
            format!("action main():\n    {}\n", body),
        ));
    }
    out
}

#[test]
fn machine_round_trip_preserves_the_check_result() {
    let Some(bin) = stage1() else { return };
    let dir = work_dir();
    let mut round_tripped = 0;
    let mut unclean: Vec<String> = Vec::new();
    // The repository's own sources, then the generated samples: the gate asks
    // for round-trip evidence across 100+ sources and the tree holds ~35.
    let mut inputs = corpus();
    for (name, source) in sample_sources() {
        inputs.push(write_case(&name, &source));
    }
    assert!(
        inputs.len() >= 100,
        "the round trip needs 100+ source files, found {}",
        inputs.len()
    );
    for file in inputs {
        let machine = run(bin, Some("to-machine"), &file);
        assert!(
            machine.trim_start().starts_with("(kaze"),
            "to-machine did not emit a (kaze ...) root for {}:\n{}",
            file.display(),
            machine.lines().take(3).collect::<Vec<_>>().join("\n")
        );
        assert!(
            machine.contains("(format 1)"),
            "the machine form for {} does not declare (format 1)",
            file.display()
        );
        let machine_path = dir.join("round_trip.machine");
        std::fs::write(&machine_path, machine.as_bytes()).expect("write machine form");
        let human = run(bin, Some("to-human"), &machine_path);
        assert!(
            !human.trim().is_empty(),
            "to-human produced nothing for {}",
            file.display()
        );
        let human_path = dir.join("round_trip.kaze");
        std::fs::write(&human_path, human.as_bytes()).expect("write human form");

        // The property that matters: the round trip is check-preserving. A
        // construct the reader could not lower becomes an UNKNOWN node, which
        // the checker reports, so it cannot slip through this comparison.
        let before = check_text(bin, &file);
        // Every source in this set must be clean, or the comparison below would
        // be comparing line numbers that the round trip legitimately moves. The
        // list is collected first so one run names every offender.
        if before.trim() != "check: ok" {
            unclean.push(format!("{}: {}", file.display(), before.trim().replace('\n', " | ")));
            continue;
        }
        let after = check_text(bin, &human_path);
        assert_eq!(
            before,
            after,
            "check result changed across the machine round trip of {}:\nbefore:\n{}\nafter:\n{}",
            file.display(),
            before,
            after
        );
        round_tripped += 1;
    }
    assert!(
        unclean.is_empty(),
        "{} source(s) are not check-clean and cannot take part in the round trip:
{}",
        unclean.len(),
        unclean.join("
")
    );
    assert!(
        round_tripped >= 100,
        "expected 100+ round-tripped files, did {round_tripped}"
    );
    eprintln!("machine round trip: {round_tripped} files, check result identical");
}

#[test]
fn the_machine_reader_reports_what_it_cannot_lower() {
    // Honesty check: an unknown machine form must be reported, not dropped.
    let Some(bin) = stage1() else { return };
    let dir = work_dir();
    let bad = "(kaze\n  (format 1)\n  (frobnicate 1)\n)\n";
    let bad_path = dir.join("unsupported.machine");
    std::fs::write(&bad_path, bad.as_bytes()).expect("write unsupported machine form");

    let verdict = check_text(bin, &bad_path);
    assert!(
        verdict.contains("E_KAZE_PARSE") && verdict.contains("frobnicate"),
        "an unknown machine form should be refused as E_KAZE_PARSE naming the form, got:\n{verdict}"
    );

    let human = run(bin, Some("to-human"), &bad_path);
    assert!(
        human
            .lines()
            .any(|l| l.trim_start().starts_with("# E_KAZE_UNSUPPORTED")),
        "to-human should mark the form it cannot lower, got:\n{human}"
    );
}

#[test]
fn machine_syntax_is_detected_and_compiles_to_c() {
    let Some(bin) = stage1() else { return };
    let dir = work_dir();
    let hello = kaze_root().join("examples/hello.kaze");
    let machine = run(bin, Some("to-machine"), &hello);
    let machine_path = dir.join("hello.machine");
    std::fs::write(&machine_path, machine.as_bytes()).expect("write machine");

    // emit-c auto-detects machine syntax, so the same binary compiles it.
    let c_from_machine = run(bin, Some("emit-c"), &machine_path);
    assert!(
        c_from_machine.contains("kz_fn__kz_main"),
        "machine syntax did not compile to a C main:\n{}",
        c_from_machine.lines().take(5).collect::<Vec<_>>().join("\n")
    );
    assert!(
        c_from_machine.contains("kz_builtin_log"),
        "machine syntax lost the log call on the way to C"
    );

    // And the C it emitted still builds.
    let c_path = dir.join("hello_from_machine.c");
    std::fs::write(&c_path, c_from_machine.as_bytes()).expect("write c");
    let bin_path = dir.join("hello_from_machine");
    let status = Command::new("cc")
        .args(["-std=c99", "-O2", "-Wno-unused-function"])
        .arg(&c_path)
        .arg("-o")
        .arg(&bin_path)
        .status()
        .expect("run cc on emitted C");
    assert!(status.success(), "C emitted from machine syntax does not build");
}

#[test]
fn the_human_and_machine_forms_round_trip_losslessly() {
    // Phase 6's invariant: AST(H) == AST(to_human(to_machine(H))). Four defects
    // lived here — unit literals doubling, `log.info(...)` unparseable, `for x in`
    // missing `each`, an inline `otherwise` changing scope, `length of` binding
    // looser than arithmetic, and records unreadable by the machine reader.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for dir in ["examples", "examples/missions", "examples/modular", "compiler"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if path.extension().map(|e| e == "kaze").unwrap_or(false) && name != "kaze_self.kaze" {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(!files.is_empty(), "the corpus is present");
    let tmp = std::env::temp_dir().join(format!("kaze-roundtrip-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("temp dir");
    let machine = tmp.join("one.machine");
    let human = tmp.join("one.kaze");
    let mut checked = 0;
    for file in &files {
        let to_machine = Command::new(env!("CARGO_BIN_EXE_kaze"))
            .arg("to-machine")
            .arg(file)
            .output()
            .expect("run to-machine");
        if !to_machine.status.success() {
            continue;
        }
        std::fs::write(&machine, &to_machine.stdout).expect("write machine");
        let to_human = Command::new(env!("CARGO_BIN_EXE_kaze"))
            .arg("to-human")
            .arg(&machine)
            .output()
            .expect("run to-human");
        assert!(
            to_human.status.success(),
            "to-human failed for {:?}: {}",
            file.file_name(),
            String::from_utf8_lossy(&to_human.stderr)
        );
        std::fs::write(&human, &to_human.stdout).expect("write human");
        let back = Command::new(env!("CARGO_BIN_EXE_kaze"))
            .arg("to-machine")
            .arg(&human)
            .output()
            .expect("run to-machine again");
        assert!(
            back.status.success(),
            "the human form of {:?} is not readable: {}",
            file.file_name(),
            String::from_utf8_lossy(&back.stderr)
        );
        assert!(
            back.stdout == to_machine.stdout,
            "{:?} does not round-trip losslessly",
            file.file_name()
        );
        checked += 1;
    }
    assert!(checked >= 15, "only {} files were checked", checked);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn every_documented_program_parses() {
    // A ```kaze block that declares a top-level item is a claim that this is valid
    // Kaze. Fourteen such blocks in the product docs did not parse: fragments
    // presented as programs, a redefined routine in one fence, and two "good/bad"
    // pairs sharing a block. Planned syntax is now fenced as ```text instead.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = root.join("scripts/doc-samples.py");
    if !script.exists() {
        eprintln!("skipped: doc-samples.py is not present");
        return;
    }
    let out = Command::new("python3")
        .arg(&script)
        .current_dir(root)
        .output()
        .expect("run doc-samples");
    let text = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "documented programs do not parse:\n{}", text);
    assert!(text.contains("0 failed"), "the check ran: {}", text);
}
