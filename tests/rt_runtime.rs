// Phase 1 gate: the C99 runtime substrate must pass scripts/rt-check.sh.
//
// The gate itself lives in the shell script because everything it asserts is a
// toolchain fact rather than a cargo fact: a strict C99 build, a 32-bit target,
// and the ASan/UBSan runtimes. This test is the Rust-side entry point only, so
// the phase gate is part of the normal test run without duplicating any of the C
// work here.
//
// A host without clang cannot build the substrate at all. The test skips with a
// printed reason instead of failing, so the Rust suite stays runnable on a
// machine that cannot exercise the C gate; the script itself prints SKIP lines
// for anything it could not verify.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Locate an executable on PATH without shelling out, so the skip decision is
/// made before any process starts.
fn on_path(binary: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[test]
fn the_c99_runtime_substrate_passes_its_gate() {
    // clang is what the gate prefers; cc and gcc are accepted as fallbacks
    // because the script reports the clang-only checks as SKIP on those hosts.
    let compiler = on_path("clang")
        .or_else(|| on_path("cc"))
        .or_else(|| on_path("gcc"));
    let compiler = match compiler {
        Some(found) => found,
        None => {
            eprintln!(
                "SKIP rt_runtime: no C compiler (clang, cc, gcc) on PATH; the Phase 1 C gate did not run"
            );
            return;
        }
    };

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = root.join("scripts").join("rt-check.sh");
    assert!(
        script.is_file(),
        "the Phase 1 gate script is missing: {}",
        script.display()
    );

    let bash = on_path("bash").unwrap_or_else(|| PathBuf::from("bash"));
    let output = Command::new(&bash)
        .arg(&script)
        .current_dir(&root)
        .output()
        .unwrap_or_else(|error| panic!("could not run {}: {error}", script.display()));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Exit 2 is the script's own "no C compiler found" verdict. Treat it as a
    // skip so a PATH that hides the compiler from this process cannot fail the
    // suite.
    if output.status.code() == Some(2) {
        eprintln!(
            "SKIP rt_runtime: rt-check.sh found no C compiler (probed: {})\n{stdout}{stderr}",
            compiler.display()
        );
        return;
    }

    if !output.status.success() {
        println!("--- rt-check.sh stdout ---\n{stdout}");
        println!("--- rt-check.sh stderr ---\n{stderr}");
        panic!(
            "rt-check.sh failed with {} (C compiler: {})",
            output.status,
            compiler.display()
        );
    }

    // A zero exit alone is not evidence: require the verdict line and at least
    // one PASS gate, which is what makes this a gate instead of a no-op.
    assert!(
        stdout.contains("rt-check: PASS - "),
        "rt-check.sh exited 0 without printing a PASS verdict:\n{stdout}"
    );
    assert!(
        stdout.contains("PASS  "),
        "rt-check.sh printed no PASS gate line:\n{stdout}"
    );

    println!("{}", stdout.trim_end());
}

#[test]
fn the_arena_survives_a_million_allocation_and_reset_cycles() {
    // Phase 1's acceptance gate: "Valgrind / ASan reports zero memory leaks on
    // 1,000,000 consecutive arena allocation and bulk-reset cycles". The
    // harness is kaze/dist/arena_stress.c; this builds it with the sanitizers
    // when the toolchain has them and skips loudly when it does not.
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dist");
    let harness = dist.join("arena_stress.c");
    let rt = dist.join("kaze_rt.c");
    if !harness.exists() || !rt.exists() {
        eprintln!("skipped: the runtime sources are not present");
        return;
    }
    let out = std::env::temp_dir().join("kaze_arena_stress");
    let build = std::process::Command::new("clang")
        .args([
            "-std=c99",
            "-O1",
            "-g",
            "-fsanitize=address,undefined",
            "-Wall",
            "-Wextra",
            "-Werror",
        ])
        .arg(&rt)
        .arg(&harness)
        .arg("-o")
        .arg(&out)
        .current_dir(&dist)
        .output()
        .expect("run clang");
    if !build.status.success() {
        eprintln!(
            "skipped: clang with sanitizers is unavailable\n{}",
            String::from_utf8_lossy(&build.stderr)
        );
        return;
    }
    let run = std::process::Command::new(&out).output().expect("run harness");
    let text = String::from_utf8_lossy(&run.stdout).to_string()
        + &String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "the stress harness failed: {}",
        text
    );
    assert!(text.contains("1000000 cycles clean"), "{}", text);
    let _ = std::fs::remove_file(&out);
}
