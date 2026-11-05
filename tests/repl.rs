// REPL: the documented `.run` compiles and executes the buffered program.

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn the_repl_runs_the_buffered_program_with_dot_run() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kaze"))
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
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("[info] hello repl"), "{stdout}");
    assert!(stdout.contains("main") && stdout.contains("nothing"), "{stdout}");
}
