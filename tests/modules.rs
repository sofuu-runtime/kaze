// v0.5: multi-file compilation (`use`). Resolution, inlining, diamond dedup,
// cycles, duplicate names, and per-file diagnostics.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use kaze::{check_file, isa, prepare_file, run_file, run_module, RunOptions, RunReport};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Write a throwaway module tree and return its directory.
fn fixture(files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kaze-modules-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, body) in files {
        let p = dir.join(path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("fixture dir");
        }
        std::fs::write(&p, body).expect("fixture file");
    }
    dir
}

fn path(dir: &Path, name: &str) -> String {
    dir.join(name).to_string_lossy().to_string()
}

fn opts(grants: &[&str]) -> RunOptions {
    RunOptions {
        grants: grants.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn logs(report: &RunReport) -> Vec<&str> {
    report.logs.iter().map(|(_, text)| text.as_str()).collect()
}

#[test]
fn a_library_item_and_hook_run_from_the_root() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: log\naction main():\n    log \"root\"\n",
        ),
        (
            "lib.kaze",
            "needs: log\naction lib_action():\n    log \"lib\"\nwhen 1 == 1:\n    log \"hook\"\n",
        ),
    ]);

    let mut o = opts(&["log"]);
    o.ticks = 1;
    let report = run_file(&path(&dir, "root.kaze"), &o).expect("the module runs");
    assert_eq!(logs(&report), vec!["root", "hook"]);

    let mut o = opts(&["log"]);
    o.entry = Some("lib_action".into());
    let report = run_file(&path(&dir, "root.kaze"), &o).expect("the library action runs");
    assert_eq!(logs(&report), vec!["lib"]);
}

#[test]
fn needs_are_merged_and_deduplicated() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: drone\naction main():\n    log \"root\"\n",
        ),
        (
            "lib.kaze",
            "needs: sensor, log\naction read():\n    log sensor.temperature()\n",
        ),
    ]);

    let prepared = prepare_file(&path(&dir, "root.kaze")).expect("it prepares");
    assert_eq!(prepared.module.needs, vec!["sensor", "log", "drone"]);

    // The root's `log` is legal because the library contributed the need.
    let report = run_file(&path(&dir, "root.kaze"), &opts(&["log"]))
        .expect("the root runs with the merged needs");
    assert_eq!(logs(&report), vec!["root"]);
}

#[test]
fn nested_and_diamond_imports_include_a_file_once() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"a.kaze\"\nuse \"b.kaze\"\nneeds: log\naction main():\n    log \"root\"\n",
        ),
        (
            "a.kaze",
            "use \"shared.kaze\"\nneeds: log\naction a():\n    log \"a\"\n",
        ),
        (
            "b.kaze",
            "use \"shared.kaze\"\nneeds: log\naction b():\n    log \"b\"\n",
        ),
        (
            "shared.kaze",
            "needs: log\nwhen 1 == 1:\n    log \"shared\"\n",
        ),
    ]);

    let mut o = opts(&["log"]);
    o.ticks = 1;
    let report = run_file(&path(&dir, "root.kaze"), &o).expect("a diamond import runs");
    assert_eq!(
        report.logs.iter().filter(|(_, t)| t == "shared").count(),
        1,
        "the shared file is inlined once: {:?}",
        report.logs
    );
    assert_eq!(report.ticks, 1);
}

#[test]
fn an_import_cycle_is_refused() {
    let dir = fixture(&[
        (
            "a.kaze",
            "use \"b.kaze\"\nneeds: log\naction a():\n    log \"a\"\n",
        ),
        (
            "b.kaze",
            "use \"a.kaze\"\nneeds: log\naction b():\n    log \"b\"\n",
        ),
    ]);

    let err = prepare_file(&path(&dir, "a.kaze")).expect_err("a cycle is refused");
    assert_eq!(err.code, "E_KAZE_IMPORT");
    assert!(err.message.contains("import cycle"), "{}", err.message);
    assert!(
        err.message.contains("a.kaze") && err.message.contains("b.kaze"),
        "the chain names both files: {}",
        err.message
    );
}

#[test]
fn a_root_that_imports_itself_is_a_cycle() {
    let dir = fixture(&[(
        "self.kaze",
        "use \"self.kaze\"\nneeds: log\naction main():\n    log \"x\"\n",
    )]);

    let err = prepare_file(&path(&dir, "self.kaze")).expect_err("self-import is a cycle");
    assert_eq!(err.code, "E_KAZE_IMPORT");
    assert!(err.message.contains("import cycle"), "{}", err.message);
}

#[test]
fn a_missing_import_names_the_use_site() {
    let dir = fixture(&[(
        "root.kaze",
        "needs: log\nuse \"nope.kaze\"\naction main():\n    log \"x\"\n",
    )]);

    let err = prepare_file(&path(&dir, "root.kaze")).expect_err("a missing module is an error");
    assert_eq!(err.code, "E_KAZE_IO");
    assert!(err.message.contains("root.kaze:2"), "{}", err.message);
    assert!(err.message.contains("nope.kaze"), "{}", err.message);
}

#[test]
fn a_non_kaze_import_is_refused() {
    let src = "needs: log\nuse \"x.txt\"\naction main():\n    log \"x\"\n";
    let err = kaze::check_source(src, "<test>").expect_err("only .kaze modules are importable");
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(err.message.contains("not a .kaze module"), "{}", err.message);
}

#[test]
fn duplicate_item_names_across_files_are_refused() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: log\naction main():\n    log \"root\"\n",
        ),
        (
            "lib.kaze",
            "needs: log\naction main():\n    log \"lib\"\n",
        ),
    ]);

    let err = prepare_file(&path(&dir, "root.kaze")).expect_err("duplicate items are refused");
    assert_eq!(err.code, "E_KAZE_IMPORT");
    assert!(err.message.contains("'main' is defined in both"), "{}", err.message);
    assert!(
        err.message.contains("lib.kaze") && err.message.contains("root.kaze"),
        "both origins are named: {}",
        err.message
    );
}

#[test]
fn a_use_after_an_item_is_refused() {
    let src = "needs: log\naction main():\n    log \"x\"\nuse \"lib.kaze\"\n";
    let err = kaze::check_source(src, "<test>").expect_err("use is a header line");
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(
        err.message.contains("use' lines go before the first item"),
        "{}",
        err.message
    );
}

#[test]
fn a_compiled_module_is_self_contained() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: log\naction main():\n    log \"root\"\n",
        ),
        (
            "lib.kaze",
            "needs: log\naction lib_action():\n    log \"lib\"\n",
        ),
    ]);

    let prepared = prepare_file(&path(&dir, "root.kaze")).expect("it prepares");
    let bytes = isa::serialize(&prepared.module);
    std::fs::remove_dir_all(&dir).expect("remove the source tree");

    let module = isa::deserialize(&bytes).expect("the artifact reloads");
    let mut o = opts(&["log"]);
    o.entry = Some("lib_action".into());
    let report = run_module(module, "root.kazebc", &o)
        .expect("the artifact runs with no source files present");
    assert_eq!(logs(&report), vec!["lib"]);
}

#[test]
fn a_use_inside_a_body_is_refused() {
    let err = kaze::check_source(
        "needs: log\naction main():\n    use \"lib.kaze\"\n",
        "<test>",
    )
    .expect_err("use is a header, not a statement");
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(err.message.contains("top-level header"), "{}", err.message);
}

#[test]
fn an_unexpanded_program_is_refused() {
    let program = kaze::parser::parse(
        "needs: log\nuse \"lib.kaze\"\naction main():\n    log \"x\"\n",
    )
    .expect("it parses");
    let errors = kaze::checker::check(&program).expect_err("imports must be expanded first");
    assert!(
        errors[0].msg.contains("unresolved 'use'"),
        "{}",
        errors[0].msg
    );
}

#[test]
fn a_runtime_error_in_an_imported_item_names_that_file() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: log\naction main():\n    log \"x\"\n",
        ),
        (
            "lib.kaze",
            "needs: drone\naction bad():\n    takeoff to 900 meters\n",
        ),
    ]);

    let mut o = opts(&["drone"]);
    o.entry = Some("bad".into());
    let err = run_file(&path(&dir, "root.kaze"), &o).expect_err("the safety gate still stops");
    assert_eq!(err.code, "E_KAZE_RANGE");
    assert!(err.message.contains("lib.kaze"), "{}", err.message);
    assert!(err.message.contains("900"), "{}", err.message);
}

#[test]
fn a_check_error_in_an_imported_item_names_that_file() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: log\naction main():\n    log \"x\"\n",
        ),
        (
            "lib.kaze",
            "needs: log\naction bad():\n    log missing_name\n",
        ),
    ]);

    let err = check_file(&path(&dir, "root.kaze")).expect_err("the undefined name is refused");
    assert_eq!(err.code, "E_KAZE_CHECK");
    assert!(err.message.contains("lib.kaze:"), "{}", err.message);
    assert!(err.message.contains("missing_name"), "{}", err.message);
}

#[test]
fn a_compile_error_in_an_imported_item_names_that_file() {
    let list = (0..256).map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
    let lib = format!("needs: log\naction bad():\n    let x = [{}]\n    log \"x\"\n", list);
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: log\naction main():\n    log \"x\"\n",
        ),
        ("lib.kaze", lib.as_str()),
    ]);

    let err = prepare_file(&path(&dir, "root.kaze"))
        .expect_err("the compiler refuses a 256-item list");
    assert_eq!(err.code, "E_KAZE_COMPILE");
    assert!(err.message.contains("lib.kaze:"), "{}", err.message);
}

#[test]
fn the_disassembly_names_each_items_file() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: log\naction main():\n    log \"root\"\n",
        ),
        (
            "lib.kaze",
            "needs: log\naction lib_action():\n    log \"lib\"\n",
        ),
    ]);

    let prepared = prepare_file(&path(&dir, "root.kaze")).expect("it prepares");
    let text = isa::disassemble(&prepared.module);
    assert!(text.contains("lib.kaze"), "{}", text);
    assert!(text.contains("root.kaze"), "{}", text);
}

#[test]
fn the_cli_compiles_a_module_to_one_self_contained_artifact() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: log\naction main():\n    log \"root\"\n",
        ),
        (
            "lib.kaze",
            "needs: log\naction lib_action():\n    log \"lib\"\n",
        ),
    ]);
    let artifact = dir.with_extension("kazebc");
    let bin = env!("CARGO_BIN_EXE_kaze");

    let compiled = std::process::Command::new(bin)
        .arg("compile")
        .arg(dir.join("root.kaze"))
        .arg("-o")
        .arg(&artifact)
        .output()
        .expect("kaze compile runs");
    assert!(
        compiled.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    // The artifact must not need any of the sources beside it.
    std::fs::remove_dir_all(&dir).expect("remove the source tree");
    let run = std::process::Command::new(bin)
        .arg("run")
        .arg(&artifact)
        .args(["--cap", "log", "--entry", "lib_action", "--plain"])
        .output()
        .expect("kaze run runs");
    let _ = std::fs::remove_file(&artifact);
    assert!(run.status.success(), "stderr: {}", String::from_utf8_lossy(&run.stderr));
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("log.info(\"lib\")"), "{}", stdout);
}

#[test]
fn module_bytes_do_not_depend_on_the_checkout_path() {
    let root = "use \"lib.kaze\"\nneeds: log\naction main():\n    log \"root\"\n";
    let lib = "needs: log\naction lib_action():\n    log \"lib\"\n";
    let a = fixture(&[("root.kaze", root), ("lib.kaze", lib)]);
    let b = fixture(&[("root.kaze", root), ("lib.kaze", lib)]);

    let first = prepare_file(&path(&a, "root.kaze")).expect("the first copy prepares");
    let second = prepare_file(&path(&b, "root.kaze")).expect("the second copy prepares");
    assert_eq!(
        isa::serialize(&first.module),
        isa::serialize(&second.module),
        "origins must be relative, so the checkout path cannot change the bytes"
    );
}

#[test]
fn needs_may_come_before_use() {
    let dir = fixture(&[
        (
            "root.kaze",
            "needs: log\nuse \"lib.kaze\"\naction main():\n    log \"root\"\n",
        ),
        (
            "lib.kaze",
            "needs: sensor\naction read():\n    log sensor.temperature()\n",
        ),
    ]);
    let prepared =
        prepare_file(&path(&dir, "root.kaze")).expect("the header may be needs then use");
    assert!(
        prepared.module.needs.contains(&"sensor".to_string()),
        "{:?}",
        prepared.module.needs
    );
}

#[test]
fn a_transitive_import_keeps_its_own_origin() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib/engine.kaze\"\nneeds: log\naction main():\n    log \"root\"\n",
        ),
        (
            "lib/engine.kaze",
            "use \"shared.kaze\"\nneeds: log\naction engine():\n    log \"engine\"\n",
        ),
        (
            "lib/shared.kaze",
            "needs: drone\naction deep():\n    takeoff to 900 meters\n",
        ),
    ]);

    let prepared = prepare_file(&path(&dir, "root.kaze")).expect("it prepares");
    let deep = prepared
        .module
        .items
        .iter()
        .find(|i| i.name == "deep")
        .expect("the grandchild is inlined");
    assert_eq!(
        deep.origin, "lib/shared.kaze",
        "a depth-2 item keeps its own file, not the middle importer"
    );

    let mut o = opts(&["drone"]);
    o.entry = Some("deep".into());
    let err = run_file(&path(&dir, "root.kaze"), &o).expect_err("900 m is refused");
    assert!(
        err.message.contains("lib/shared.kaze"),
        "the run-time error names the grandchild: {}",
        err.message
    );
}

#[test]
fn an_absolute_import_is_recorded_relative_to_the_root() {
    let dir = fixture(&[
        (
            "root.kaze",
            "needs: log\naction main():\n    log \"root\"\n",
        ),
        (
            "lib/x.kaze",
            "needs: log\naction lib_action():\n    log \"lib\"\n",
        ),
    ]);
    let absolute = dir.join("lib").join("x.kaze").to_string_lossy().to_string();
    let root = format!(
        "use \"{}\"\nneeds: log\naction main():\n    log \"root\"\n",
        absolute
    );
    std::fs::write(dir.join("root.kaze"), root).expect("rewrite the root");

    let prepared = prepare_file(&path(&dir, "root.kaze")).expect("it prepares");
    let item = prepared
        .module
        .items
        .iter()
        .find(|i| i.name == "lib_action")
        .expect("the imported item is present");
    assert_eq!(
        item.origin, "lib/x.kaze",
        "an absolute import under the root is recorded relative"
    );
}

#[test]
fn bare_telemetry_is_resolved_per_file() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: log\nwhen battery < 20%:\n    log \"low\"\n",
        ),
        (
            "lib.kaze",
            "needs: drone\naction arm_it():\n    arm drone\n",
        ),
    ]);

    let err = prepare_file(&path(&dir, "root.kaze"))
        .expect_err("bare telemetry needs a hardware family in its own file");
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(err.message.contains("battery"), "{}", err.message);

    // The explicit form uses the merged needs and is unambiguous.
    let explicit = "use \"lib.kaze\"\nneeds: log\nwhen drone.telemetry(\"battery\") < 20:\n    log \"low\"\n";
    std::fs::write(dir.join("root.kaze"), explicit).expect("rewrite the root");
    prepare_file(&path(&dir, "root.kaze"))
        .expect("an explicit telemetry call uses the merged needs");
}

#[test]
fn hooks_from_two_files_run_in_dependency_first_order() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"a.kaze\"\nuse \"b.kaze\"\nneeds: log\naction main():\n    log \"root\"\n",
        ),
        ("a.kaze", "needs: log\nwhen 1 == 1:\n    log \"a\"\n"),
        ("b.kaze", "needs: log\nwhen 1 == 1:\n    log \"b\"\n"),
    ]);

    let mut o = opts(&["log"]);
    o.ticks = 1;
    let report = run_file(&path(&dir, "root.kaze"), &o).expect("both hooks run");
    assert_eq!(logs(&report), vec!["root", "a", "b"]);
}

#[test]
fn a_header_only_dependency_is_refused() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"empty.kaze\"\nneeds: log\naction main():\n    log \"x\"\n",
        ),
        ("empty.kaze", "needs: log\n"),
    ]);

    let err = prepare_file(&path(&dir, "root.kaze"))
        .expect_err("a module must contain at least one item");
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(err.message.contains("empty.kaze"), "{}", err.message);
}

#[test]
fn a_parent_directory_import_resolves_from_the_importing_file() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"sub/a.kaze\"\nneeds: log\naction main():\n    log \"root\"\n",
        ),
        (
            "sub/a.kaze",
            "use \"../common.kaze\"\nneeds: log\naction a():\n    log \"a\"\n",
        ),
        (
            "common.kaze",
            "needs: log\nwhen 1 == 1:\n    log \"common\"\n",
        ),
    ]);

    let mut o = opts(&["log"]);
    o.ticks = 1;
    let report = run_file(&path(&dir, "root.kaze"), &o).expect("the parent import runs");
    assert_eq!(
        report.logs.iter().filter(|(_, t)| t == "common").count(),
        1
    );
}

#[test]
fn an_artifact_produces_the_same_trace_as_the_source() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: log\naction main():\n    log \"root\"\n",
        ),
        ("lib.kaze", "needs: log\nevery 100ms:\n    log \"tick\"\n"),
    ]);

    let mut o = opts(&["log"]);
    o.ticks = 2;
    o.tick_ms = 100;
    let source = run_file(&path(&dir, "root.kaze"), &o).expect("the source runs");
    let prepared = prepare_file(&path(&dir, "root.kaze")).expect("it prepares");
    let bytes = isa::serialize(&prepared.module);
    std::fs::remove_dir_all(&dir).expect("remove the source tree");

    let artifact = run_module(isa::deserialize(&bytes).expect("reload"), "root.kazebc", &o)
        .expect("the artifact runs");
    assert_eq!(source.render_plain(), artifact.render_plain());
}

#[test]
fn a_single_file_disassembly_names_its_file() {
    let path = format!("{}/examples/hello.kaze", env!("CARGO_MANIFEST_DIR"));
    let prepared = prepare_file(&path).expect("hello prepares");
    let text = isa::disassemble(&prepared.module);
    assert!(text.contains("[hello.kaze]"), "{}", text);
}

#[test]
fn imported_routines_are_never_the_implicit_entry() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: log\nwhen 1 == 1:\n    log \"hook\"\n",
        ),
        (
            "lib.kaze",
            "needs: log\naction rtl():\n    log \"library action\"\naction other():\n    log \"other\"\n",
        ),
    ]);

    let mut o = opts(&["log"]);
    o.ticks = 1;
    let report = run_file(&path(&dir, "root.kaze"), &o)
        .expect("imported routines must not force an entry choice");
    assert!(report.entry.is_empty(), "no implicit entry: {:?}", report.entry);
    assert_eq!(logs(&report), vec!["hook"]);

    // Explicit selection still reaches the library.
    let mut explicit = opts(&["log"]);
    explicit.entry = Some("rtl".into());
    let report = run_file(&path(&dir, "root.kaze"), &explicit)
        .expect("--entry selects an imported routine");
    assert_eq!(logs(&report), vec!["library action"]);
}

#[test]
fn compile_source_and_compile_file_agree_on_bytes() {
    let path = format!("{}/examples/hello.kaze", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).expect("read hello.kaze");
    let from_file = kaze::compile_file(&path).expect("compile_file");
    let from_source = kaze::compile_source(&src, &path).expect("compile_source");
    assert_eq!(
        isa::serialize(&from_file),
        isa::serialize(&from_source),
        "the same source must produce the same artifact through either API"
    );
}

#[test]
fn a_root_runtime_error_names_the_path_the_operator_used() {
    let dir = fixture(&[(
        "root.kaze",
        "needs: drone\naction main():\n    takeoff to 900 meters\n",
    )]);
    let full = path(&dir, "root.kaze");
    let err = run_file(&full, &opts(&["drone"])).expect_err("900 m is refused");
    assert_eq!(err.code, "E_KAZE_RANGE");
    assert!(
        err.message.starts_with(&full),
        "a root error keeps the operator's path: {}",
        err.message
    );
}

#[test]
fn origins_are_recorded_per_item() {
    let dir = fixture(&[
        (
            "root.kaze",
            "use \"lib.kaze\"\nneeds: log\naction main():\n    log \"root\"\n",
        ),
        (
            "lib.kaze",
            "needs: log\naction lib_action():\n    log \"lib\"\n",
        ),
    ]);

    let prepared = prepare_file(&path(&dir, "root.kaze")).expect("it prepares");
    let lib = prepared
        .module
        .items
        .iter()
        .find(|i| i.name == "lib_action")
        .expect("the library item is inlined");
    assert_eq!(lib.origin, "lib.kaze", "{}", lib.origin);

    let main = prepared
        .module
        .items
        .iter()
        .find(|i| i.name == "main")
        .expect("the root item is present");
    assert_eq!(main.origin, "root.kaze", "{}", main.origin);
}
