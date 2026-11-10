// REPL: the documented `.run` compiles and executes the buffered program.

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn the_repl_runs_the_buffered_program_with_dot_run() {
    let dir = std::env::temp_dir().join(format!("kaze-repl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create isolated REPL directory");
    std::fs::write(dir.join("policy.json"), r#"{"deny":[]}"#).expect("write policy");
    let mut child = Command::new(env!("CARGO_BIN_EXE_kaze"))
        .current_dir(&dir)
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("kaze repl spawns");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        write!(
            stdin,
            ".needs log\n.cap log\naction main():\n    log \"hello repl\"\n.run\n.exit\n"
        )
        .expect("write REPL input");
    }
    let out = child.wait_with_output().expect("repl exits");
    std::fs::remove_dir_all(&dir).expect("remove REPL directory");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("[info] hello repl"), "{stdout}");
    assert!(stdout.contains("main") && stdout.contains("nothing"), "{stdout}");
}
