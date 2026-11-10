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
         \x20 fmt <file.kaze>      Format source; prints to stdout by default\n\
         \x20 bench <file.kaze>    Run a mission repeatedly and report timings\n\
         \x20 selftest             Run the software checks\n\
         \x20 repl                 Interactive session: define routines, then .run\n\
         \x20 version              Print the Kaze language version\n\n\
         fmt options:\n\
         \x20 --check              Exit 1 when the file is not formatted\n\
         \x20 --write, -w          Rewrite the file in place\n\n\
         bench options:\n\
         \x20 --iterations <n>     Number of runs (default 20)\n\
         \x20 --json               Machine-readable timings\n\n\
         run options:\n\
         \x20 --cap <family>[=<device>]  Grant a capability. Select `drone=sim` or\n\
         \x20                        `rover=sim` explicitly for simulation tests;\n\
         \x20                        `drone=udp:127.0.0.1:14550` or\n\
         \x20                        `drone=uart:/dev/ttyAMA0@57600` is a real\n\
         \x20                        autopilot (serial needs --features uart). Live\n\
         \x20                        drone actuation is currently disabled.\n\
         \x20 --pin <name>=<line>   Name a GPIO line the device family may drive\n\
         \x20                        (v0.2, e.g. --pin fan=17)\n\
         \x20 --pwm <name>=<chip>:<channel>  Name a PWM channel for set_duty\n\
         \x20                        (v0.2, e.g. --pwm fan=0:0)\n\
         \x20 --entry <name>         Routine to run (default: main, or the only one)\n\
         \x20 --arg <name>=<json>    Bind a parameter (repeatable)\n\
         \x20 --fuel <n>             Instruction budget (default 200000)\n\
         \x20 --ticks <n>            Supervisor ticks after the entry (default 0);\n\
         \x20                        required for `when`/`every` hooks\n\
         \x20 --tick-ms <n>          Tick period (default 100)\n\
         \x20 --pace                 Sleep tick-ms between ticks. Use it for a\n\
         \x20                        live link-loss failsafe; simulated ticks\n\
         \x20                        are virtual time and run instantly\n\
         \x20 --sim <k=v,k=v>        Initial machine state (battery=15,armed=true,\n\
         \x20                        link_lost_after=1500 for a simulated dropout)\n\
         \x20 --no-policy            Disable policy loading; capability calls are denied\n\
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
    /// `name -> (chip, channel)` from `--pwm fan=0:0` (v0.2).
    pwm: Vec<(String, u16, u16)>,
    entry: Option<String>,
    args: serde_json::Map<String, serde_json::Value>,
    fuel: Option<u64>,
    ticks: Option<u32>,
    tick_ms: Option<u64>,
    pace: bool,
    sim: String,
    out: Option<String>,
    disasm: bool,
    json: bool,
    plain: bool,
    no_policy: bool,
    fmt_check: bool,
    fmt_write: bool,
    iterations: Option<u32>,
    want_help: bool,
}

fn parse_flags(argv: &[String]) -> Result<Flags, String> {
    let mut f = Flags {
        positional: Vec::new(),
        caps: Vec::new(),
        backend: Vec::new(),
        pins: Vec::new(),
        pwm: Vec::new(),
        entry: None,
        args: serde_json::Map::new(),
        fuel: None,
        ticks: None,
        tick_ms: None,
        pace: false,
        sim: String::new(),
        out: None,
        disasm: false,
        json: false,
        plain: false,
        no_policy: false,
        fmt_check: false,
        fmt_write: false,
        iterations: None,
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
            "--pwm" => {
                let v = next("--pwm")?;
                let (name, address) = v.split_once('=').ok_or_else(|| {
                    "--pwm expects name=chip:channel, e.g. --pwm fan=0:0".to_string()
                })?;
                let (chip, channel) = address.split_once(':').ok_or_else(|| {
                    format!("--pwm {} is not chip:channel", address)
                })?;
                let chip: u16 = chip
                    .parse()
                    .map_err(|_| format!("--pwm chip {} is not a number", chip))?;
                let channel: u16 = channel
                    .parse()
                    .map_err(|_| format!("--pwm channel {} is not a number", channel))?;
                f.pwm.push((name.to_string(), chip, channel));
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
            "--pace" => f.pace = true,
            "--sim" => {
                // Repeatable: `--sim lat=47.6 --sim lon=8.5` is the same as
                // `--sim lat=47.6,lon=8.5` rather than the last one winning.
                let v = next("--sim")?;
                if !f.sim.is_empty() {
                    f.sim.push(',');
                }
                f.sim.push_str(&v);
            }
            "--out" | "-o" => f.out = Some(next("--out")?),
            "--disasm" => f.disasm = true,
            "--json" => f.json = true,
            "--plain" => f.plain = true,
            "--no-policy" => f.no_policy = true,
            "--check" => f.fmt_check = true,
            "--write" | "-w" => f.fmt_write = true,
            "--iterations" => {
                let v = next("--iterations")?;
                f.iterations = Some(
                    v.parse()
                        .map_err(|_| format!("--iterations '{}' is not a number", v))?,
                );
            }
            "-h" | "--help" | "help" => f.want_help = true,
            other => f.positional.push(other.to_string()),
        }
        i += 1;
    }
    Ok(f)
}

// Deserialize the policy as a struct rather than a JSON map. A map silently
// accepts repeated keys, so {"deny":["*"],"deny":[]} could otherwise erase a
// denial. Serde rejects duplicate fields, missing fields, and unknown fields.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadedPolicy {
    deny: Vec<String>,
}

fn policy_layer(no_policy: bool) -> Option<serde_json::Value> {
    if no_policy {
        return None;
    }
    let path = std::path::Path::new("policy.json");
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(path) {
        // Serde also permits a struct to be represented as a JSON array;
        // policies have exactly one supported top-level shape: an object.
        Ok(s) if !s.trim_start().starts_with('{') => Some(serde_json::json!({ "__malformed": true })),
        Ok(s) => match serde_json::from_str::<LoadedPolicy>(&s) {
            Ok(v) => Some(serde_json::json!({ "deny": v.deny })),
            Err(_) => Some(serde_json::json!({ "__malformed": true })),
        },
        Err(_) => Some(serde_json::json!({ "__malformed": true })),
    }
}

fn options_from(f: &Flags) -> Result<RunOptions, String> {
    let mut sim = SimState::default();
    if !f.sim.is_empty() {
        if !f.backend.iter().any(|(family, backend)| {
            backend == "sim" && matches!(kaze::caps::family_of(family), "drone" | "rover")
        }) {
            return Err("--sim requires an explicit --cap drone=sim or --cap rover=sim testing backend".to_string());
        }
        sim.apply_overrides(&f.sim)?;
    }
    Ok(RunOptions {
        entry: f.entry.clone(),
        args: f.args.clone(),
        grants: f.caps.clone(),
        fuel: f.fuel.unwrap_or(200_000),
        ticks: f.ticks.unwrap_or(0),
        tick_ms: f.tick_ms.unwrap_or(100),
        pace_ticks: f.pace,
        sim,
        policy: policy_layer(f.no_policy),
        backend: f.backend.clone(),
        pins: f.pins.clone(),
        pwm: f.pwm.clone(),
    })
}

fn report_simulation_on_failure(opts: &RunOptions) {
    let families: Vec<&str> = opts.backend.iter()
        .filter(|(_, backend)| backend.trim() == "sim")
        .map(|(family, _)| family.as_str())
        .collect();
    if !families.is_empty() {
        eprintln!("simulation selected (testing only): {}", families.join(", "));
    }
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
                if !report.simulated_families.is_empty() {
                    println!("simulation selected (testing only): {}", report.simulated_families.join(", "));
                }
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
            report_simulation_on_failure(&opts);
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
    // `prepare_file` resolves `use` relative to this file, not the CWD, and
    // handles stdin ("-") the same way `run` does.
    let prepared = match kaze::prepare_file(path) {
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

fn cmd_fmt(argv: &[String]) -> i32 {
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
    if flags.fmt_check && flags.fmt_write {
        eprintln!("\x1b[31mError:\x1b[0m choose --check or --write, not both");
        return 2;
    }
    if flags.positional.is_empty() {
        eprintln!("\x1b[31mError:\x1b[0m 'kaze fmt' needs a .kaze file");
        return 2;
    }
    if flags.positional.len() > 1 && !flags.fmt_check && !flags.fmt_write {
        eprintln!(
            "\x1b[31mError:\x1b[0m printing several files would be ambiguous; use --check or --write"
        );
        return 2;
    }

    let mut status = 0;
    for path in &flags.positional {
        let src = match kaze::read_source(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("\x1b[31mError:\x1b[0m {}", e.render());
                status = 1;
                continue;
            }
        };
        let formatted = match kaze::fmt::format_source(&src, path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("\x1b[31mError:\x1b[0m {}", e.render());
                status = 1;
                continue;
            }
        };
        if flags.fmt_check {
            if formatted != src {
                eprintln!("{} is not formatted", path);
                status = 1;
            }
        } else if flags.fmt_write {
            if path == "-" {
                eprintln!("\x1b[31mError:\x1b[0m cannot --write stdin; omit --write to print it");
                status = 2;
                continue;
            }
            if formatted != src {
                if let Err(e) = std::fs::write(path, &formatted) {
                    eprintln!("\x1b[31mError:\x1b[0m cannot write '{}': {}", path, e);
                    status = 1;
                }
            }
        } else {
            print!("{}", formatted);
        }
    }
    status
}

fn cmd_bench(argv: &[String]) -> i32 {
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
        eprintln!("\x1b[31mError:\x1b[0m 'kaze bench' needs a .kaze or .kazebc file");
        return 2;
    };
    let opts = match options_from(&flags) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("\x1b[31mError:\x1b[0m {}", e);
            return 2;
        }
    };
    let iterations = flags.iterations.unwrap_or(20).max(1);

    enum Source {
        Prepared(kaze::Prepared),
        Module(isa::Module),
    }
    let source = if is_compiled_module(path) {
        match std::fs::read(path)
            .map_err(|e| format!("cannot read '{}': {}", path, e))
            .and_then(|bytes| isa::deserialize(&bytes))
        {
            Ok(module) => Source::Module(module),
            Err(e) => {
                eprintln!("\x1b[31mError:\x1b[0m E_KAZE_BADCODE: {}: {}", path, e);
                return 1;
            }
        }
    } else {
        match kaze::prepare_file(path) {
            Ok(prepared) => Source::Prepared(prepared),
            Err(e) => {
                eprintln!("\x1b[31mError:\x1b[0m {}", e.render());
                return 1;
            }
        }
    };

    let mut durations = Vec::with_capacity(iterations as usize);
    let mut reference: Option<String> = None;
    let mut deterministic = true;
    let mut last: Option<kaze::RunReport> = None;
    for _ in 0..iterations {
        // Clone outside the timer: the measurement is verification + execution.
        let (prepared, module) = match &source {
            Source::Prepared(prepared) => (Some(prepared.clone()), None),
            Source::Module(module) => (None, Some(module.clone())),
        };
        let start = std::time::Instant::now();
        let outcome = match (prepared, module) {
            (Some(prepared), _) => kaze::run_prepared(prepared, &opts),
            (_, Some(module)) => kaze::run_module(module, path, &opts),
            (None, None) => {
                eprintln!("\x1b[31mError:\x1b[0m bench lost its prepared source");
                return 1;
            }
        };
        let report = match outcome {
            Ok(report) => report,
            Err(e) => {
                report_simulation_on_failure(&opts);
                eprintln!("\x1b[31mError:\x1b[0m {}", e.render());
                return 1;
            }
        };
        durations.push(start.elapsed());
        let trace = report.render_plain();
        match &reference {
            Some(expected) if *expected != trace => deterministic = false,
            None => reference = Some(trace),
            _ => {}
        }
        last = Some(report);
    }
    let Some(report) = last else {
        eprintln!("\x1b[31mError:\x1b[0m no benchmark iterations ran");
        return 1;
    };

    durations.sort();
    let millis = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    let min = millis(durations[0]);
    let n = durations.len();
    let median = if n % 2 == 0 {
        (millis(durations[n / 2 - 1]) + millis(durations[n / 2])) / 2.0
    } else {
        millis(durations[n / 2])
    };
    // Nearest-rank percentile: the smallest sample that covers p% of the runs.
    let p95_index = (((0.95 * n as f64).ceil() as usize).saturating_sub(1)).min(n - 1);
    let p95 = millis(durations[p95_index]);
    let mean = durations.iter().map(|d| millis(*d)).sum::<f64>() / durations.len() as f64;

    if flags.json {
        println!(
            "{}",
            serde_json::json!({
                "path": path,
                "iterations": iterations,
                "entry": report.entry,
                "actions": report.actions.len(),
                "gotos": report.actions.iter().filter(|a| a.contains("drone.goto(")).count(),
                "instructions": report.instructions,
                "fuel": report.fuel_used,
                "ticks": report.ticks,
                "simulated_families": report.simulated_families,
                "min_ms": min,
                "median_ms": median,
                "p95_ms": p95,
                "mean_ms": mean,
                "deterministic": deterministic,
                // Stable across runs; lets a soak compare traces without
                // depending on timing fields or a JSON parser.
                "trace_fnv": trace_fingerprint(&report.render_plain())
            })
        );
    } else {
        println!("{}: {} iterations", path, iterations);
        if !report.simulated_families.is_empty() {
            println!("simulation selected (testing only): {}", report.simulated_families.join(", "));
        }
        println!(
            "actions {} · instructions {} · fuel {} · ticks {}",
            report.actions.len(),
            report.instructions,
            report.fuel_used,
            report.ticks
        );
        println!(
            "min {:.3} ms · median {:.3} ms · p95 {:.3} ms · mean {:.3} ms",
            min, median, p95, mean
        );
        println!(
            "deterministic: {}",
            if deterministic { "yes" } else { "NO (live hardware?)" }
        );
    }
    // A benchmark of a non-deterministic mission is not evidence, so it fails.
    if deterministic { 0 } else { 1 }
}

/// FNV-1a over the canonical trace. Deterministic across processes and hosts,
/// so a soak can compare iteration traces with a plain string compare.
fn trace_fingerprint(trace: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in trace.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn cmd_selftest(argv: &[String]) -> i32 {
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

    const MISSION: &str = include_str!("../examples/missions/eight_waypoints.kaze");
    let mut checks: Vec<(String, bool, String)> = Vec::new();

    // The v1.x LTS line: a patch release must not fail its own scorecard.
    let version_ok = kaze::VERSION.starts_with("1.") && isa::FORMAT_VERSION == 4;
    checks.push((
        "version and module format".into(),
        version_ok,
        format!("kaze {} / format {}", kaze::VERSION, isa::FORMAT_VERSION),
    ));

    let prepared = kaze::prepare(MISSION, "selftest.kaze");
    let prepared = match prepared {
        Ok(prepared) => {
            let verified = isa::verify(&prepared.module);
            checks.push((
                "mission compiles and verifies".into(),
                verified.is_ok(),
                verified.err().unwrap_or_default(),
            ));
            Some(prepared)
        }
        Err(e) => {
            checks.push(("mission compiles and verifies".into(), false, e.render()));
            None
        }
    };

    // The default scorecard executes only pure computation. Robotics
    // simulation requires an explicit testing selection from the operator.
    const PURE: &str = "action main():\n    return 2 + 3\n";
    let opts = RunOptions::default();
    let (run_ok, run_detail) = match (
        kaze::run_source(PURE, "selftest-pure.kaze", &opts),
        kaze::run_source(PURE, "selftest-pure.kaze", &opts),
    ) {
        (Ok(first), Ok(second)) => {
            let same = first.render_plain() == second.render_plain();
            (first.value == "5" && same, format!("value {}, deterministic {}", first.value, if same { "yes" } else { "no" }))
        }
        (Err(e), _) | (_, Err(e)) => (false, e.render()),
    };
    checks.push(("pure computation runs deterministically".into(), run_ok, run_detail));

    let fmt_ok = match kaze::fmt::format_source(MISSION, "selftest.kaze") {
        Ok(once) => kaze::fmt::format_source(&once, "selftest.kaze")
            .map(|twice| twice == once)
            .unwrap_or(false),
        Err(_) => false,
    };
    checks.push(("formatter is idempotent".into(), fmt_ok, String::new()));

    let round_trip_ok = match &prepared {
        Some(prepared) => {
            let bytes = isa::serialize(&prepared.module);
            isa::deserialize(&bytes).and_then(|module| isa::verify(&module)).is_ok()
        }
        None => false,
    };
    checks.push((
        "artifact round-trips".into(),
        round_trip_ok,
        String::new(),
    ));

    let bad = isa::Module {
        items: vec![isa::ItemCode {
            name: "bad".into(),
            tag: isa::ItemTag::Action,
            params: Vec::new(),
            code: vec![
                isa::word(isa::Op::Const, 0, 0, 0),
                99,
                isa::word(isa::Op::Halt, 0, 0, 0),
            ],
            line: 1,
            origin: String::new(),
            period_ms: 0.0,
            slots: 0,
        }],
        ..Default::default()
    };
    checks.push((
        "verifier rejects a corrupt module".into(),
        isa::verify(&bad).is_err(),
        String::new(),
    ));

    let h4 = kaze::run_source(
        "needs: drone\naction main():\n    takeoff to 900 meters\n",
        "selftest.kaze",
        &RunOptions {
            grants: vec!["drone".into()],
            policy: Some(serde_json::json!({ "deny": [] })),
            ..Default::default()
        },
    );
    checks.push((
        "H-4 rejects 900 m".into(),
        matches!(&h4, Err(e) if e.code == "E_KAZE_RANGE"),
        String::new(),
    ));

    // A hand-built module bypasses the checker so the runtime H-1 gate itself
    // is exercised (source that calls an undeclared family fails earlier).
    let h1_module = isa::Module {
        needs: Vec::new(),
        root: "selftest.kazebc".into(),
        consts: Vec::new(),
        caps: vec![isa::CapRef {
            family: "drone".into(),
            op: "arm".into(),
            argc: 0,
        }],
        items: vec![isa::ItemCode {
            name: "main".into(),
            tag: isa::ItemTag::Action,
            params: Vec::new(),
            code: vec![
                isa::word(isa::Op::CallCap, 0, 0, 0),
                0, // capability table index
                isa::word(isa::Op::Pop, 0, 0, 0),
                isa::word(isa::Op::RetUnit, 0, 0, 0),
            ],
            line: 1,
            origin: "selftest.kazebc".into(),
            period_ms: 0.0,
            slots: 0,
        }],
    };
    let h1 = kaze::run_module(h1_module, "selftest.kazebc", &RunOptions::default());
    let h1_detail = match &h1 {
        Err(e) => e.render(),
        Ok(report) => format!("ran: {}", report.render_plain().trim()),
    };
    checks.push((
        "H-1 rejects an undeclared capability".into(),
        matches!(&h1, Err(e) if e.code == "E_CAP_UNDECLARED"),
        h1_detail,
    ));

    let gate = kaze::run_source(
        "needs: drone\naction main():\n    arm drone\n",
        "selftest.kaze",
        &RunOptions::default(),
    );
    checks.push((
        "H-2 refuses an ungranted backend".into(),
        matches!(&gate, Err(e) if e.code == "E_CAP_MISSING"),
        String::new(),
    ));

    let h3 = kaze::run_source(
        "needs: drone\naction main():\n    arm drone\n",
        "selftest.kaze",
        &RunOptions {
            grants: vec!["drone".into()],
            policy: Some(serde_json::json!({ "deny": ["drone.*"] })),
            ..Default::default()
        },
    );
    checks.push((
        "H-3 refuses a policy denial".into(),
        matches!(&h3, Err(e) if e.code == "E_CAP_DENIED"),
        String::new(),
    ));

    let fuel = kaze::run_source(
        "action loop():\n    while 1 == 1:\n        let x = 1\n",
        "selftest.kaze",
        &RunOptions { fuel: 50, ..Default::default() },
    );
    checks.push((
        "fuel stops a runaway loop".into(),
        matches!(&fuel, Err(e) if e.code == "E_TIMEOUT"),
        String::new(),
    ));

    let pass = checks.iter().all(|(_, ok, _)| *ok);
    if flags.json {
        let items: Vec<serde_json::Value> = checks
            .iter()
            .map(|(name, ok, detail)| {
                serde_json::json!({"name": name, "pass": ok, "detail": detail})
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "version": kaze::VERSION,
                "format": isa::FORMAT_VERSION,
                "checks": items,
                "pass": pass
            })
        );
    } else {
        println!(
            "\x1b[1mKaze selftest\x1b[0m — kaze {} (module format {})",
            kaze::VERSION,
            isa::FORMAT_VERSION
        );
        for (name, ok, detail) in &checks {
            let mark = if *ok {
                "\x1b[32mPASS\x1b[0m"
            } else {
                "\x1b[31mFAIL\x1b[0m"
            };
            if detail.is_empty() {
                println!("  {}  {}", mark, name);
            } else {
                println!("  {}  {} — {}", mark, name, detail);
            }
        }
        let passed = checks.iter().filter(|(_, ok, _)| *ok).count();
        println!(
            "selftest: {} ({}/{})",
            if pass { "PASS" } else { "FAIL" },
            passed,
            checks.len()
        );
    }
    if pass { 0 } else { 1 }
}

fn cmd_repl(_argv: &[String]) -> i32 {
    println!("kaze {} REPL — type .help for commands, Ctrl+D to exit.", kaze::VERSION);
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut buffer = String::new();
    let mut needs = Vec::new();
    let mut grants = Vec::new();
    let mut backends = Vec::new();

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
            println!(".cap <family>[=backend]  Grant a family; select drone=sim or rover=sim explicitly for tests");
            println!(".run [routine]       Compile and run the buffered program");
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
                let (f, backend) = match fam.trim().split_once('=') {
                    Some((family, backend)) => (family.trim().to_string(), Some(backend.trim().to_string())),
                    None => (fam.trim().to_string(), None),
                };
                if !f.is_empty() && !grants.contains(&f) {
                    grants.push(f.clone());
                }
                if let Some(backend) = backend {
                    backends.push((f, backend));
                }
            }
            println!("granted: {}", grants.join(", "));
            continue;
        }
        if trimmed == ".run" || trimmed.starts_with(".run ") {
            let entry = trimmed.strip_prefix(".run").unwrap_or("").trim();
            if buffer.trim().is_empty() {
                println!("nothing to run — type a program first");
                continue;
            }
            // `.needs` supplies the header when the buffered source has none.
            let declares_needs = buffer
                .lines()
                .any(|line| line.trim_start().starts_with("needs:"));
            let mut source = String::new();
            if !declares_needs && !needs.is_empty() {
                source.push_str("needs: ");
                source.push_str(&needs.join(", "));
                source.push('\n');
            }
            source.push_str(&buffer);
            if !source.ends_with('\n') {
                source.push('\n');
            }
            let opts = RunOptions {
                entry: if entry.is_empty() { None } else { Some(entry.to_string()) },
                grants: grants.clone(),
                backend: backends.clone(),
                policy: policy_layer(false),
                ..Default::default()
            };
            match kaze::run_source(&source, "<repl>", &opts) {
                Ok(report) => {
                    if !report.simulated_families.is_empty() {
                        println!("simulation selected (testing only): {}", report.simulated_families.join(", "));
                    }
                    for (level, msg) in &report.logs {
                        println!("[{}] {}", level, msg);
                    }
                    if !report.entry.is_empty() {
                        println!("{} \x1b[32m->\x1b[0m {}", report.entry, report.value);
                    }
                    for (i, action) in report.actions.iter().enumerate() {
                        println!("  {:2}. {}", i + 1, action);
                    }
                    println!(
                        "\x1b[90m{} instructions · {} fuel · {} ticks\x1b[0m",
                        report.instructions, report.fuel_used, report.ticks
                    );
                }
                Err(err) => {
                    report_simulation_on_failure(&opts);
                    eprintln!("\x1b[31m{}\x1b[0m", err);
                }
            }
            continue;
        }
        if trimmed == ".clear" {
            buffer.clear();
            needs.clear();
            grants.clear();
            backends.clear();
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
        "fmt" => cmd_fmt(rest),
        "bench" => cmd_bench(rest),
        "selftest" => cmd_selftest(rest),
        "repl" => cmd_repl(rest),
        other => {
            eprintln!("\x1b[31mError:\x1b[0m unknown command '{}'", other);
            print_help();
            2
        }
    };
    std::process::exit(code);
}
