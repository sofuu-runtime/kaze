// lib.rs — the Kaze engine: source → checker → bytecode → VM.
//
// Kaze (風) is an ultra-friendly, Rust-secured scripting language designed for
// robotics, drones, and ambient edge devices.

pub mod arena;
pub mod ast;
pub mod caps;
pub mod checker;
pub mod compiler;
pub mod deadline;
pub mod gpio;
#[cfg(feature = "chardev")]
pub mod gpio_chardev;
pub mod hardware;
pub mod host;
pub mod isa;
pub mod link;
pub mod mavlink;
pub mod pwm;
pub mod lexer;
pub mod parser;
pub mod value;
pub mod vm;

pub use caps::{Action, CapHost, SimHost, SimState};
pub use isa::Module;
pub use vm::{Limits, Verdict, Vm};

/// Language version reported by `kaze --version` / `kaze version`.
///
/// 0.2 is the hardware release: the language is unchanged, and a script can now
/// reach a real autopilot and real GPIO pins through `--cap drone=<device>`.
pub const VERSION: &str = "0.2";

#[derive(Debug, Clone)]
pub struct KazeError {
    pub code: String,
    pub message: String,
}

impl KazeError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        KazeError {
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn render(&self) -> String {
        format!("{}: {}", self.code, self.message)
    }
}

impl std::fmt::Display for KazeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.render())
    }
}

impl std::error::Error for KazeError {}

/// A compiled script plus the source name it came from.
pub struct Prepared {
    pub module: Module,
    pub name: String,
}

/// Read a script from disk (or stdin when path is "-").
pub fn read_source(path: &str) -> Result<String, KazeError> {
    if path == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| KazeError::new("E_KAZE_IO", format!("cannot read stdin: {}", e)))?;
        return Ok(buf);
    }
    std::fs::read_to_string(path)
        .map_err(|e| KazeError::new("E_KAZE_IO", format!("cannot read '{}': {}", path, e)))
}

/// Parse, check and compile. This is the whole front end in one call.
pub fn prepare(src: &str, name: &str) -> Result<Prepared, KazeError> {
    let program = parser::parse(src).map_err(|e| {
        KazeError::new("E_KAZE_PARSE", e.render(name))
    })?;
    checker::check(&program).map_err(|errors| {
        let mut message = String::new();
        for (i, err) in errors.iter().enumerate() {
            if i > 0 {
                message.push('\n');
            }
            message.push_str(&err.render(name));
        }
        KazeError::new("E_KAZE_CHECK", message)
    })?;
    let module = compiler::compile(&program)
        .map_err(|e| KazeError::new("E_KAZE_COMPILE", e.render(name)))?;
    Ok(Prepared {
        module,
        name: name.to_string(),
    })
}

pub fn prepare_file(path: &str) -> Result<Prepared, KazeError> {
    let src = read_source(path)?;
    prepare(&src, path)
}

/// Front end only — what `kaze check` runs.
pub fn check_source(src: &str, name: &str) -> Result<(), KazeError> {
    prepare(src, name).map(|_| ())
}

pub fn check_file(path: &str) -> Result<(), KazeError> {
    let src = read_source(path)?;
    check_source(&src, path)
}

pub fn compile_source(src: &str, name: &str) -> Result<Module, KazeError> {
    prepare(src, name).map(|p| p.module)
}

pub fn compile_file(path: &str) -> Result<Module, KazeError> {
    let src = read_source(path)?;
    compile_source(&src, path)
}

/// Everything a run needs besides the script itself.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Item to run first. Default: `main`, else the only routine in the file.
    pub entry: Option<String>,
    pub args: serde_json::Map<String, serde_json::Value>,
    /// Capability families the host grants a backend for (`drone=sim` → "drone").
    pub grants: Vec<String>,
    pub fuel: u64,
    /// Supervisor ticks after the entry finishes; each tick polls `when`/`every`.
    pub ticks: u32,
    pub tick_ms: u64,
    pub sim: SimState,
    /// R7 policy layered over the hardware gate. `None` means "declaration and
    /// grant only", which is what the tests use.
    pub policy: Option<serde_json::Value>,
    /// `family -> device` from `--cap drone=udp:127.0.0.1:14550`. A device
    /// other than `sim` puts the live backend behind that family (v0.2).
    pub backend: Vec<(String, String)>,
    /// `device name -> GPIO line` from `--pin fan=17` (v0.2).
    pub pins: Vec<(String, u16)>,
    /// `name -> (chip, channel)` from `--pwm fan=0:0` (v0.2).
    pub pwm: Vec<(String, u16, u16)>,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            entry: None,
            args: serde_json::Map::new(),
            grants: Vec::new(),
            fuel: Limits::default().fuel,
            ticks: 0,
            tick_ms: 100,
            sim: SimState::default(),
            policy: None,
            backend: Vec::new(),
            pins: Vec::new(),
            pwm: Vec::new(),
        }
    }
}

/// What one run did. Everything here is evidence: the trace, the numbers and
/// the verdict are all derived, not narrated.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RunReport {
    pub entry: String,
    pub value: String,
    pub actions: Vec<String>,
    pub logs: Vec<(String, String)>,
    pub fuel_used: u64,
    pub instructions: u64,
    pub ticks: u32,
    pub verdict: Option<Verdict>,
}

impl RunReport {
    /// The canonical text form of a run — the line a mission review reads and
    /// the exact bytes the golden corpus stores. The CLI adds colour; nothing
    /// else about the shape changes.
    pub fn render_plain(&self) -> String {
        let mut out = String::new();
        if !self.entry.is_empty() {
            out.push_str(&format!("{} -> {}\n", self.entry, self.value));
        }
        if !self.actions.is_empty() {
            out.push_str("actions:\n");
            for (i, a) in self.actions.iter().enumerate() {
                out.push_str(&format!("  {:>2}. {}\n", i + 1, a));
            }
        }
        if let Some(v) = &self.verdict {
            match v {
                Verdict::Allow(text) => out.push_str(&format!("verdict: allow {}\n", text)),
                Verdict::Reject(reason) => out.push_str(&format!("verdict: reject - {}\n", reason)),
            }
        }
        out.push_str(&format!(
            "{} instructions, {} fuel, {} ticks\n",
            self.instructions, self.fuel_used, self.ticks
        ));
        out
    }
}

/// Run a script from source text.
pub fn run_source(src: &str, name: &str, opts: &RunOptions) -> Result<RunReport, KazeError> {
    let prepared = prepare(src, name)?;
    run_prepared(prepared, opts)
}

pub fn run_file(path: &str, opts: &RunOptions) -> Result<RunReport, KazeError> {
    let prepared = prepare_file(path)?;
    run_prepared(prepared, opts)
}

/// Run a compiled module directly: no lexer, no parser, no checker. This is the
/// path a `.kazebc` artifact takes, so a deployed script needs no front end.
pub fn run_module(module: Module, name: &str, opts: &RunOptions) -> Result<RunReport, KazeError> {
    run_prepared(Prepared { module, name: name.to_string() }, opts)
}

fn run_prepared(prepared: Prepared, opts: &RunOptions) -> Result<RunReport, KazeError> {
    let module = prepared.module;
    let declared = module.needs.clone();
    // v0.2: a family granted a real device runs live; every other family keeps
    // the deterministic simulator.
    let live =
        opts.backend.iter().any(|(_, d)| !d.is_empty() && d != "sim")
            || !opts.pins.is_empty()
            || !opts.pwm.is_empty();
    let mut host: Box<dyn CapHost> = if live {
        match crate::host::LiveHost::new(opts.sim.clone(), &opts.backend, &opts.pins, &opts.pwm) {
            Ok(host) => Box::new(host),
            Err(e) => return Err(KazeError::new("E_KAZE_LINK", format!("{}: {}", prepared.name, e))),
        }
    } else {
        Box::new(SimHost::new(opts.sim.clone()))
    };
    let limits = Limits {
        fuel: opts.fuel.max(1),
        ..Limits::default()
    };

    let entry = resolve_entry(&module, opts)?;

    let mut report = RunReport {
        entry: entry.clone().unwrap_or_default(),
        value: "nothing".to_string(),
        actions: Vec::new(),
        logs: Vec::new(),
        fuel_used: 0,
        instructions: 0,
        ticks: 0,
        verdict: None,
    };

    {
        let mut vm = Vm::new(
            &module,
            &mut *host,
            &declared,
            &opts.grants,
            opts.policy.as_ref(),
            limits,
        );

        if let Some(name) = &entry {
            let item = module
                .item(name)
                .ok_or_else(|| KazeError::new("E_KAZE_ENTRY", format!("there is no '{}' in this script", name)))?;
            let args = bind_args(item.params.as_slice(), &opts.args, &mut vm);
            let value = vm
                .run_item(item, &args)
                .map_err(|e| KazeError::new(e.code, format!("{}: {} (line {})", prepared.name, e.msg, e.line)))?;
            report.value = caps::render_value(value, &vm.arena, &vm.heap);
            report.verdict = vm.take_verdict();
        }

        // Supervisory ticks: `when` predicates and due `every` blocks.
        for tick in 0..opts.ticks {
            let t_ms = tick as f64 * opts.tick_ms as f64;
            for item in module.items.iter() {
                let due = match item.tag {
                    isa::ItemTag::When => true,
                    isa::ItemTag::Every => item.period_ms > 0.0 && (t_ms % item.period_ms) == 0.0,
                    _ => false,
                };
                if !due {
                    continue;
                }
                vm.run_item(item, &[]).map_err(|e| {
                    KazeError::new(e.code, format!("{}: {} (line {})", prepared.name, e.msg, e.line))
                })?;
            }
            report.ticks += 1;
        }

        report.fuel_used = vm.fuel_used;
        report.instructions = vm.instructions;
    }

    report.actions = host.actions().iter().map(|a: &Action| a.render()).collect();
    report.logs = host.logs().to_vec();
    Ok(report)
}

fn resolve_entry(module: &Module, opts: &RunOptions) -> Result<Option<String>, KazeError> {
    if let Some(name) = &opts.entry {
        return Ok(Some(name.clone()));
    }
    if module.item("main").is_some() {
        return Ok(Some("main".to_string()));
    }
    let routines: Vec<&isa::ItemCode> = module
        .items
        .iter()
        .filter(|i| matches!(i.tag, isa::ItemTag::Action | isa::ItemTag::Check | isa::ItemTag::Filter))
        .collect();
    if routines.len() == 1 {
        return Ok(Some(routines[0].name.clone()));
    }
    if routines.is_empty() && !module.items.is_empty() {
        // Hooks only: the ticks are the program.
        return Ok(None);
    }
    Err(KazeError::new(
        "E_KAZE_ENTRY",
        "this script has more than one routine; name the one to run with --entry (or add an action main())",
    ))
}

fn bind_args(
    params: &[String],
    args: &serde_json::Map<String, serde_json::Value>,
    vm: &mut Vm,
) -> Vec<value::Value> {
    let mut out = Vec::with_capacity(params.len());
    for (i, p) in params.iter().enumerate() {
        let json = args
            .get(p)
            .or_else(|| args.get(&i.to_string()))
            .unwrap_or(&serde_json::Value::Null);
        out.push(json_to_value(json, &mut vm.arena, &mut vm.heap));
    }
    out
}

fn json_to_value(
    j: &serde_json::Value,
    arena: &mut arena::Arena,
    heap: &mut value::Heap,
) -> value::Value {
    use serde_json::Value as J;
    match j {
        J::Null => value::Value::Unit,
        J::Bool(b) => value::Value::Bool(*b),
        J::Number(n) => value::Value::Num(n.as_f64().unwrap_or(0.0)),
        J::String(s) => match arena.alloc(s) {
            Ok(r) => value::Value::Str(r),
            Err(_) => value::Value::Unit,
        },
        J::Array(items) => {
            let vals: Vec<value::Value> = items
                .iter()
                .map(|i| json_to_value(i, arena, heap))
                .collect();
            heap.push_array(vals)
        }
        J::Object(map) => {
            let mut entries = Vec::with_capacity(map.len());
            for (k, v) in map {
                let key = match arena.alloc(k) {
                    Ok(r) => value::Value::Str(r),
                    Err(_) => value::Value::Unit,
                };
                entries.push((key, json_to_value(v, arena, heap)));
            }
            heap.push_map(entries)
        }
    }
}
