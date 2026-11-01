// main.rs — Kaze CLI: run, check, compile, and repl for Kaze scripts.

use std::io::{BufRead, Write};
use kaze::caps::SimState;
use kaze::{self, isa, RunOptions};

pub fn print_help() {
    println!(
        "Usage: kaze <command> [options]\n\n\
         Kaze ({}) — the capability-gated scripting language for robots, drones\n\
         and edge devices. A Kaze script declares what it needs (needs: drone, log)\n\
         and the host grants a backend for each device family.\n\n\
         Commands:\n\
         \x20 run <file.kaze>      Check, compile and run a script\n\
         \x20 check <file.kaze>    Front end only: parse + check, no execution\n\
         \x20 compile <file.kaze>  Emit the .kazebc module (deterministic bytes)\n\
         \x20 repl                 Interactive session: define routines, then .run\n\
         \x20 version              Print the Kaze language version\n\n\
         run options:\n\
         \x20 --cap <family>[=<device>]  Grant a backend. `sim` (default) is the\n\
         \x20                        simulator; `drone=udp:127.0.0.1:14550` or\n\
         \x20                        `drone=uart:/dev/ttyAMA0@57600` is a real\n\
         \x20                        autopilot (v0.2, serial needs --features uart)\n\
         \x20 --pin <name>=<line>   Name a GPIO line the device family may drive\n\
         \x20                        (v0.2, e.g. --pin fan=17)\n\
         \x20 --entry <name>         Routine to run (default: main, or the only one)\n\
         \x20 --arg <name>=<json>    Bind a parameter (repeatable)\n\
         \x20 --fuel <n>             Instruction budget (default 200000)\n\
         \x20 --ticks <n>            Supervisor ticks after the entry (default 0)\n\
         \x20 --tick-ms <n>          Virtual tick period (default 100)\n\
         \x20 --sim <k=v,k=v>        Initial machine state (battery=15,armed=true)\n\
         \x20 --no-policy            Skip local policy layer\n\
         \x20 --json                 Machine-readable report\n",
        kaze::VERSION
    );
}

struct Flags {
    positional: Vec<String>,
    caps: Vec<String>,
    /// `family -> device` from `--cap drone=udp:127.0.0.1:14550` (v0.2).
    backend: Vec<(String, String)>,
    /// `name -> GPIO line` from `--pin fan=17` (v0.2).
    pins: Vec<(String, u16)>,
    entry: Option<String>,
    args: serde_json::Map<String, serde_json::Value>,
    fuel: Option<u64>,
    ticks: Option<u32>,
    tick_ms: Option<u64>,
    sim: String,
    out: Option<String>,
    disasm: bool,
    json: bool,
    plain: bool,
    no_policy: bool,
    want_help: bool,
}

fn parse_flags(argv: &[String]) -> Result<Flags, String> {
    let mut f = Flags {
        positional: Vec::new(),
        caps: Vec::new(),
        backend: Vec::new(),
        pins: Vec::new(),
        entry: None,
        args: serde_json::Map::new(),
        fuel: None,
        ticks: None,
        tick_ms: None,
        sim: String::new(),
        out: None,
        disasm: false,
        json: false,
        plain: false,
        no_policy: false,
        want_help: false,
    };
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        let mut next = |what: &str| -> Result<String, String> {
            i += 1;
            argv.get(i)
                .cloned()
                .ok_or_else(|| format!("{} needs a value", what))
        };
        match a {
            "--cap" => {
                let v = next("--cap")?;
                match v.split_once('=') {
                    Some((family, device)) => {
                        f.caps.push(family.to_string());
                        f.backend.push((family.to_string(), device.to_string()));
                    }
                    None => f.caps.push(v),
                }
            }
            "--pin" => {
                let v = next("--pin")?;
                let (name, line) = v
                    .split_once('=')
                    .ok_or_else(|| "--pin expects name=number, e.g. --pin fan=17".to_string())?;
                let line: u16 = line
                    .parse()
                    .map_err(|_| format!("--pin {} is not a GPIO line number", line))?;
                f.pins.push((name.to_string(), line));
            }
            "--entry" => f.entry = Some(next("--entry")?),
            "--arg" => {
                let v = next("--arg")?;
                let (k, raw) = v
                    .split_once('=')
                    .ok_or_else(|| "--arg expects name=value".to_string())?;
                let parsed = serde_json::from_str::<serde_json::Value>(raw)
                    .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()));
                f.args.insert(k.to_string(), parsed);
            }
            "--fuel" => {
                let v = next("--fuel")?;
                f.fuel = Some(v.parse().map_err(|_| format!("--fuel '{}' is not a number", v))?);
            }
            "--ticks" => {
                let v = next("--ticks")?;
                f.ticks = Some(v.parse().map_err(|_| format!("--ticks '{}' is not a number", v))?);
            }
            "--tick-ms" => {
                let v = next("--tick-ms")?;
                f.tick_ms = Some(v.parse().map_err(|_| format!("--tick-ms '{}' is not a number", v))?);
            }
            "--sim" => f.sim = next("--sim")?,
            "--out" | "-o" => f.out = Some(next("--out")?),
            "--disasm" => f.disasm = true,
            "--json" => f.json = true,
            "--plain" => f.plain = true,
            "--no-policy" => f.no_policy = true,
            "-h" | "--help" | "help" => f.want_help = true,
            other => f.positional.push(other.to_string()),
        }
        i += 1;
    }
    Ok(f)
}

fn policy_layer(no_policy: bool) -> Option<serde_json::Value> {
    if no_policy {
        return None;
    }
    std::fs::read_to_string("policy.json")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

fn options_from(f: &Flags) -> Result<RunOptions, String> {
    let mut sim = SimState::default();
    if !f.sim.is_empty() {
        sim.apply_overrides(&f.sim)?;
    }
    Ok(RunOptions {
        entry: f.entry.clone(),
        args: f.args.clone(),
        grants: f.caps.clone(),
        fuel: f.fuel.unwrap_or(200_000),
        ticks: f.ticks.unwrap_or(0),
        tick_ms: f.tick_ms.unwrap_or(100),
        sim,
        policy: policy_layer(f.no_policy),
        backend: f.backend.clone(),
        pins: f.pins.clone(),
    })
}

fn is_compiled_module(path: &str) -> bool {
    if path.ends_with(".kazebc") {
        return true;
    }
    let mut head = [0u8; 8];
    let opened = std::fs::File::open(path)
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head));
    opened.is_ok() && &head == b"KAZEBC1\0"
}

fn cmd_run(argv: &[String]) -> i32 {
    let flags = match parse_flags(argv) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("\x1b[31mError:\x1b[0m {}", e);
            return 2;
        }
    };
    if flags.want_help {
        print_help();
        return 0;
    }
    let Some(path) = flags.positional.first() else {
        eprintln!("\x1b[31mError:\x1b[0m 'kaze run' needs a .kaze file");
        return 2;
    };
    let opts = match options_from(&flags) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("\x1b[31mError:\x1b[0m {}", e);
            return 2;
        }
    };
    let outcome = if is_compiled_module(path) {
        std::fs::read(path)
            .map_err(|e| kaze::KazeError::new("E_KAZE_IO", format!("cannot read '{}': {}", path, e)))
            .and_then(|bytes| {
                isa::deserialize(&bytes).map_err(|e| kaze::KazeError::new("E_KAZE_BADCODE", e))
            })
            .and_then(|module| kaze::run_module(module, path, &opts))
    } else {
        kaze::run_file(path, &opts)
    };
    match outcome {
        Ok(report) => {
            if flags.json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
            } else if flags.plain {
                print!("{}", report.render_plain());
            } else {
                println!("{} \x1b[32m->\x1b[0m {}", report.entry, report.value);
                if !report.actions.is_empty() {
                    println!("\x1b[1mactions:\x1b[0m");
                    for (i, a) in report.actions.iter().enumerate() {
                        println!("  {:2}. {}", i + 1, a);
                    }
                }
                println!(
                    "\x1b[90m{} instructions · {} fuel · {} ticks\x1b[0m",
                    report.instructions, report.fuel_used, report.ticks
                );
            }
            0
        }
        Err(err) => {
            eprintln!("\x1b[31mError:\x1b[0m {}", err.render());
            1
        }
    }
}

fn cmd_check(argv: &[String]) -> i32 {
    let flags = match parse_flags(argv) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("\x1b[31mError:\x1b[0m {}", e);
            return 2;
        }
    };
    let Some(path) = flags.positional.first() else {
        eprintln!("\x1b[31mError:\x1b[0m 'kaze check' needs a .kaze file");
        return 2;
    };
    match kaze::check_file(path) {
        Ok(()) => {
            println!("\x1b[32m✓\x1b[0m {} is valid", path);
            0
        }
        Err(e) => {
            eprintln!("\x1b[31mError:\x1b[0m {}", e.render());
            1
        }
    }
}

fn cmd_compile(argv: &[String]) -> i32 {
    let flags = match parse_flags(argv) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("\x1b[31mError:\x1b[0m {}", e);
            return 2;
        }
    };
    let Some(path) = flags.positional.first() else {
        eprintln!("\x1b[31mError:\x1b[0m 'kaze compile' needs a .kaze file");
        return 2;
    };
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("\x1b[31mError:\x1b[0m cannot read '{}': {}", path, e);
            return 1;
        }
    };
    let prepared = match kaze::prepare(&src, path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("\x1b[31mError:\x1b[0m {}", e.render());
            return 1;
        }
    };
    if flags.disasm {
        println!("{}", isa::disassemble(&prepared.module));
        return 0;
    }
    let bytes = isa::serialize(&prepared.module);
    let out = flags.out.unwrap_or_else(|| {
        if path.ends_with(".kaze") {
            format!("{}bc", path)
        } else {
            format!("{}.kazebc", path)
        }
    });
    if let Err(e) = std::fs::write(&out, &bytes) {
        eprintln!("\x1b[31mError:\x1b[0m cannot write '{}': {}", out, e);
        return 1;
    }
    println!("\x1b[32m✓\x1b[0m {} → {} ({} bytes)", path, out, bytes.len());
    0
}

fn cmd_repl(_argv: &[String]) -> i32 {
    println!("kaze {} REPL — type .help for commands, Ctrl+D to exit.", kaze::VERSION);
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut buffer = String::new();
    let mut needs = Vec::new();
    let mut grants = Vec::new();

    loop {
        print!("kaze> ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed == ".exit" || trimmed == ".quit" {
            break;
        }
        if trimmed == ".help" {
            println!(".needs <family, ...>  Declare capabilities");
            println!(".cap <family>        Grant simulated backend");
            println!(".clear               Clear REPL session buffer");
            println!(".list                Print current buffer");
            println!(".exit                Exit REPL");
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(".needs ") {
            for fam in rest.split(',') {
                let f = fam.trim().to_string();
                if !f.is_empty() && !needs.contains(&f) {
                    needs.push(f);
                }
            }
            println!("needs: {}", needs.join(", "));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(".cap ") {
            for fam in rest.split(',') {
                let f = fam.trim().to_string();
                if !f.is_empty() && !grants.contains(&f) {
                    grants.push(f);
                }
            }
            println!("granted: {}", grants.join(", "));
            continue;
        }
        if trimmed == ".clear" {
            buffer.clear();
            needs.clear();
            grants.clear();
            println!("session cleared");
            continue;
        }
        if trimmed == ".list" {
            println!("needs: {}\n{}", needs.join(", "), buffer);
            continue;
        }
        buffer.push_str(&line);
    }
    0
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = argv.first().map(String::as_str) else {
        print_help();
        std::process::exit(2);
    };
    let rest = &argv[1..];
    let code = match command {
        "-h" | "--help" | "help" => {
            print_help();
            0
        }
        "version" | "--version" => {
            println!("kaze {}", kaze::VERSION);
            0
        }
        "run" => cmd_run(rest),
        "check" => cmd_check(rest),
        "compile" => cmd_compile(rest),
        "repl" => cmd_repl(rest),
        other => {
            eprintln!("\x1b[31mError:\x1b[0m unknown command '{}'", other);
            print_help();
            2
        }
    };
    std::process::exit(code);
}
