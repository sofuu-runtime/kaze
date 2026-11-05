// v0.6: `kaze fmt` — idempotence, comment preservation, and the CLI modes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn temp_file(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kaze-fmt-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(name);
    std::fs::write(&path, body).expect("temp file");
    path
}

fn collect_kaze(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read examples dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.is_dir() {
            collect_kaze(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("kaze") {
            out.push(path);
        }
    }
}

#[test]
fn every_example_formats_idempotently() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut files = Vec::new();
    collect_kaze(&root, &mut files);
    assert!(files.len() >= 8, "the example corpus should be present");

    for path in files {
        let name = path.display().to_string();
        let src = std::fs::read_to_string(&path).expect("read example");
        let once = kaze::fmt::format_source(&src, &name)
            .unwrap_or_else(|e| panic!("{}: {}", name, e.render()));
        let twice = kaze::fmt::format_source(&once, &name)
            .unwrap_or_else(|e| panic!("{} (second pass): {}", name, e.render()));
        assert_eq!(once, twice, "formatting is not idempotent for {}", name);
        kaze::parser::parse(&once)
            .unwrap_or_else(|e| {
                panic!("{}: the formatted source does not parse: {}", name, e.render(&name))
            });
    }
}

#[test]
fn blank_lines_and_err_line_survive_formatting() {
    let src = "needs: drone, log\n\n\naction main():\n    try:\n        takeoff to 15 meters\n    otherwise with err:\n        log warning err.line\n";
    let formatted = kaze::fmt::format_source(src, "<fmt>").expect("formats");
    assert_eq!(
        formatted.lines().nth(3),
        Some("action main():"),
        "the action stays on physical line 4:\n{}",
        formatted
    );

    let opts = kaze::RunOptions {
        grants: vec!["drone".into(), "log".into()],
        ..Default::default()
    };
    let before = kaze::run_source(src, "<before>", &opts).expect("runs");
    let after = kaze::run_source(&formatted, "<after>", &opts).expect("runs");
    assert_eq!(
        before.logs.first().map(|(_, text)| text.as_str()),
        after.logs.first().map(|(_, text)| text.as_str()),
        "err.line is observable and must not shift"
    );
}

#[test]
fn unary_minus_hugs_after_expression_keywords() {
    let src = "needs: log\naction main():\n    let x = 1\n    set x to -1\n    log -5\n";
    let out = kaze::fmt::format_source(src, "<fmt>").expect("formats");
    assert!(out.contains("set x to -1"), "{}", out);
    assert!(out.contains("log -5"), "{}", out);
}

#[test]
fn ordinary_division_keeps_its_spaces() {
    let src = "needs: log\naction main():\n    log 1 m / 2\n    log 1 m / x\n    log 1 m/s\n";
    let out = kaze::fmt::format_source(src, "<fmt>").expect("formats");
    assert!(out.contains("1 m / 2"), "{}", out);
    assert!(out.contains("1 m / x"), "{}", out);
    assert!(out.contains("1 m/s"), "{}", out);
}

#[test]
fn nested_lists_keep_their_shape() {
    let src = "action main():\n    let x = [\n        [\n            1\n        ]\n    ]\n    log x\n";
    let out = kaze::fmt::format_source(src, "<fmt>").expect("formats");
    assert_eq!(out, src, "nested list indentation is already canonical");
}

#[test]
fn comments_survive_in_place() {
    let src = "# top note\nneeds: log\naction main():  # header\n  # inside\n  log \"x\" # trail\n";
    let out = kaze::fmt::format_source(src, "<test>").expect("formats");
    for comment in ["# top note", "# header", "# inside", "# trail"] {
        assert!(out.contains(comment), "missing {} in:\n{}", comment, out);
    }
    assert!(out.contains("    # inside"), "comment aligned to the body:\n{}", out);
}

#[test]
fn indentation_is_four_spaces_and_blocks_keep_their_shape() {
    let src = "needs: log\naction main():\n  if 1 == 1:\n      log \"a\"\n  otherwise:\n   log \"b\"\n";
    let out = kaze::fmt::format_source(src, "<test>").expect("formats");
    assert!(out.contains("\n    if 1 == 1:\n"), "{}", out);
    assert!(out.contains("\n        log \"a\"\n"), "{}", out);
    assert!(out.contains("\n    otherwise:\n"), "{}", out);
    assert!(out.contains("\n        log \"b\"\n"), "{}", out);
}

#[test]
fn a_raw_cr_inside_a_string_survives() {
    let src = "needs: log\naction main():\n    log \"a\rb\"\n";
    let out = kaze::fmt::format_source(src, "<fmt>")
        .expect("a parseable CR string must not be split into lines");
    assert!(out.contains("a\rb"), "{:?}", out);
}

#[test]
fn the_cli_check_write_and_print_modes_agree() {
    let bin = env!("CARGO_BIN_EXE_kaze");
    let path = temp_file("mission.kaze", "needs:log\naction main():\n  log\"hi\"\n");
    let canonical = "needs: log\naction main():\n    log \"hi\"\n";

    let printed = std::process::Command::new(bin)
        .arg("fmt")
        .arg(&path)
        .output()
        .expect("fmt runs");
    assert!(printed.status.success());
    assert_eq!(String::from_utf8_lossy(&printed.stdout), canonical);

    let check = std::process::Command::new(bin)
        .args(["fmt", "--check"])
        .arg(&path)
        .output()
        .expect("fmt --check runs");
    assert_eq!(check.status.code(), Some(1), "unformatted input fails --check");

    let write = std::process::Command::new(bin)
        .args(["fmt", "--write"])
        .arg(&path)
        .output()
        .expect("fmt --write runs");
    assert!(write.status.success(), "{}", String::from_utf8_lossy(&write.stderr));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), canonical);

    let clean = std::process::Command::new(bin)
        .args(["fmt", "--check"])
        .arg(&path)
        .output()
        .expect("fmt --check runs");
    assert!(clean.status.success(), "{}", String::from_utf8_lossy(&clean.stderr));
}
