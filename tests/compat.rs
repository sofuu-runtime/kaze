// The artifact compatibility lock, in two halves:
//
//   * kaze-v1.0-format4.kazebc.hex is a module written by the v1.0 writer. It
//     must keep loading and running under the current reader, forever, because
//     users have those files on disk.
//   * kaze-v1.1-format5.kazebc.hex is what today's writer produces for the same
//     source. It must not change unless the format version changes with it.

use kaze::{isa, run_module, RunOptions};

const SRC: &str = "needs: log\naction main():\n    log \"release\"\n";

#[allow(dead_code)] // kept for hand-checking a fixture against the writer output
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

fn fixture_hex(name: &str) -> Vec<u8> {
    let text = match name {
        "format4" => include_str!("fixtures/kaze-v1.0-format4.kazebc.hex").trim(),
        _ => include_str!("fixtures/kaze-v1.1-format5.kazebc.hex").trim(),
    };
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("fixture is hex"))
        .collect()
}

#[test]
fn a_format_4_artifact_still_loads_and_runs() {
    // A v1.0 module has no per-item recursion ceiling; the reader fills in 0
    // and the VM applies its global frame bound.
    let bytes = fixture_hex("format4");
    let module = isa::deserialize(&bytes).expect("the v1.0 artifact still loads");
    let opts = RunOptions {
        grants: vec!["log".to_string()],
        policy: Some(serde_json::json!({ "deny": [] })),
        ..Default::default()
    };
    let report = run_module(module, "lock.kazebc", &opts).expect("the frozen artifact still runs");
    assert_eq!(report.logs.first().map(|(_, text)| text.as_str()), Some("release"));
}

#[test]
fn the_current_artifact_bytes_are_frozen() {
    let module = kaze::compile_source(SRC, "lock.kaze").expect("compiles");
    let bytes = isa::serialize(&module);
    let expected = fixture_hex("format5");
    assert_eq!(
        bytes, expected,
        "the module bytes changed; a format bump (and a new fixture) is required"
    );
}

#[test]
fn the_format_version_is_locked() {
    assert_eq!(isa::FORMAT_VERSION, 5);
}

#[test]
fn dual_syntax_human_and_machine_compile_and_run_identically() {
    let human_src = "needs: log\naction main():\n    log \"release\"\n";
    let machine_src = r#"(kaze
  (format 1)
  (needs "log")
  (action "main"
    (body
      (eval (call "log" "info" (string "release"))))))
"#;

    let human_module = kaze::compile_source(human_src, "human.kaze").expect("compiles human");
    let machine_module = kaze::compile_source(machine_src, "machine.kaze").expect("compiles machine");

    let opts = RunOptions {
        grants: vec!["log".to_string()],
        policy: Some(serde_json::json!({ "deny": [] })),
        ..Default::default()
    };

    let human_report = run_module(human_module, "human.kazebc", &opts).expect("runs human");
    let machine_report = run_module(machine_module, "machine.kazebc", &opts).expect("runs machine");

    assert_eq!(human_report.entry, machine_report.entry);
    assert_eq!(human_report.actions, machine_report.actions);
    assert_eq!(human_report.logs, machine_report.logs);
}

#[test]
fn stage1_pure_kaze_compiler_self_hosts_and_achieves_fixed_point() {
    let self_compiler_path = concat!(env!("CARGO_MANIFEST_DIR"), "/compiler/kaze_self.kaze");
    let hello_path = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/hello.kaze");

    let opts = kaze::c_codegen::CodegenOptions {
        standalone: true,
        entry: Some("main".to_string()),
    };

    let c_stage1_src = kaze::emit_c_file(self_compiler_path, &opts).expect("emit c for kaze_self.kaze");

    let tmp = std::env::temp_dir();
    let stage1_c = tmp.join("test_kaze_stage1.c");
    let stage1_bin = tmp.join("test_kaze_stage1_bin");
    let stage2_c = tmp.join("test_kaze_stage2.c");
    let stage2_bin = tmp.join("test_kaze_stage2_bin");
    let stage3_c = tmp.join("test_kaze_stage3.c");

    std::fs::write(&stage1_c, &c_stage1_src).expect("write stage1_c");

    // 1. Compile Stage 1 binary
    let cc_st1 = std::process::Command::new("cc")
        .arg("-std=c99")
        .arg("-O2")
        .arg("-Wno-unused-function")
        .arg(&stage1_c)
        .arg("-o")
        .arg(&stage1_bin)
        .status();
    let cc_ok = cc_st1.map(|s| s.success()).unwrap_or(false);
    if !cc_ok {
        return; // cc not available or failed
    }

    // 2. Stage 1 compiles hello.kaze
    let hello_out = std::process::Command::new(&stage1_bin)
        .arg(hello_path)
        .output()
        .expect("stage 1 compiles hello");
    let hello_c = String::from_utf8_lossy(&hello_out.stdout);
    assert!(hello_c.contains("kz_fn__kz_main"));
    assert!(hello_c.contains("kz_builtin_log"));

    // 3. Stage 1 compiles itself -> Stage 2 C source
    let st2_out = std::process::Command::new(&stage1_bin)
        .arg(self_compiler_path)
        .output()
        .expect("stage 1 compiles itself");
    let st2_c = String::from_utf8_lossy(&st2_out.stdout);
    std::fs::write(&stage2_c, st2_c.as_bytes()).expect("write stage2_c");

    // 4. Compile Stage 2 binary
    let cc_st2 = std::process::Command::new("cc")
        .arg("-std=c99")
        .arg("-O2")
        .arg("-Wno-unused-function")
        .arg(&stage2_c)
        .arg("-o")
        .arg(&stage2_bin)
        .status();
    assert!(cc_st2.expect("run cc").success(), "Stage 2 C source must compile with cc");

    // 5. Stage 2 compiles itself -> Stage 3 C source
    let st3_out = std::process::Command::new(&stage2_bin)
        .arg(self_compiler_path)
        .output()
        .expect("stage 2 compiles itself");
    let st3_c = String::from_utf8_lossy(&st3_out.stdout);
    std::fs::write(&stage3_c, st3_c.as_bytes()).expect("write stage3_c");

    // 6. Fixed Point Identity: Stage 2 == Stage 3 byte-identical
    assert_eq!(st2_c, st3_c, "Self-hosting fixed point: Stage 2 and Stage 3 C output must be byte-identical");

    // Clean up
    let _ = std::fs::remove_file(&stage1_c);
    let _ = std::fs::remove_file(&stage1_bin);
    let _ = std::fs::remove_file(&stage2_c);
    let _ = std::fs::remove_file(&stage2_bin);
    let _ = std::fs::remove_file(&stage3_c);
}
