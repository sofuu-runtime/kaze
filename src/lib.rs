// lib.rs — the Kaze PROTOTYPE engine: source → checker → bytecode → VM.
//
// THIS CRATE IS NOT THE PRODUCT. The Kaze that ships lives in
// crates/sofuu-core/src/domains/kaze/ and is specified by docs/KAZE-SPEC.md
// (language v0.1, frozen). This tree is an earlier standalone prototype: it is
// not a workspace member, make never builds it, CI never runs it, and its
// surface has drifted ahead of the product (eleven vector/trig opcodes, three
// fault-tolerance opcodes, compile-time modules, v1.0 tooling, module format 4).
// scripts/kaze-drift-check.sh is the gate that keeps that drift visible.
//
// Kaze (風) is an ultra-friendly, Rust-secured scripting language designed for
// robotics, drones, and ambient edge devices.

pub mod arena;
pub mod ast;
pub mod caps;
pub mod checker;
pub mod compiler;
pub mod deadline;
pub mod fmt;
pub mod gpio;
#[cfg(feature = "chardev")]
pub mod gpio_chardev;
pub mod hardware;
pub mod host;
pub mod isa;
pub mod geo;
pub mod link;
pub mod mavlink;
pub mod pwm;
pub mod lexer;
pub mod lsp;
pub mod parser;
pub mod value;
pub mod vm;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use caps::{Action, CapHost, SimHost, SimState};
pub use isa::Module;
pub use vm::{Limits, Verdict, Vm};

/// Language version reported by `kaze --version` / `kaze version`.
///
/// 1.0 is the production baseline. It accumulates:
/// 0.6 developer tooling (`kaze fmt`, `kaze-lsp`, the editor extension),
/// 0.7 the bytecode verifier and fuzz suites, 0.8 mission benchmarks and
/// packet-loss resilience, 0.9 the frozen spec, standard library and ISA lock,
/// and 1.0 the certification scorecard (`kaze selftest`).
///
/// 0.5 added compile-time modules, 0.4 fault tolerance, 0.3 spatial maths, and
/// 0.2 live hardware behind `--cap`. Module format 4 is frozen.
pub const VERSION: &str = "1.0";

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
#[derive(Debug, Clone)]
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

/// Expands `use "…"` directives into one flat program.
///
/// Dependencies are inlined depth-first, in use order, before the importing
/// file's own items. A file is included once (a diamond imports it once); a
/// cycle, a missing file, a non-`.kaze` path and a duplicate item name are
/// errors. The compiled module has no imports left.
#[derive(Default)]
struct Loader {
    seen: HashSet<PathBuf>,
    /// (canonical path, origin) for the active import chain.
    stack: Vec<(PathBuf, String)>,
    /// Hooks get a module-wide number so two files cannot both define `when#1`.
    hook_seq: u32,
    /// The root module's directory. Item origins are reported relative to it,
    /// so the compiled bytes do not depend on where the checkout lives.
    root_base: PathBuf,
    /// Canonical form of `root_base` when it exists. Origins are computed from
    /// the canonical dependency path, which normalizes `..`, `.` and absolute
    /// import spellings under the root.
    root_canonical: Option<PathBuf>,
}

impl Loader {
    fn new(root_base: PathBuf) -> Self {
        let canonical_base = if root_base.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            root_base.clone()
        };
        Loader {
            root_base,
            root_canonical: std::fs::canonicalize(canonical_base).ok(),
            ..Default::default()
        }
    }

    /// A file's diagnostic origin: its path relative to the root module.
    ///
    /// The canonical path is preferred so `sub/../lib.kaze` and an absolute
    /// import under the root both become `lib.kaze`. A module reached through
    /// a symlink to somewhere outside the root keeps the path it was imported
    /// by, which is the spelling the author can act on.
    fn origin_of(&self, display: &Path, canonical: &Path) -> String {
        if let Some(root) = &self.root_canonical {
            if let Ok(rel) = canonical.strip_prefix(root) {
                return rel.to_string_lossy().to_string();
            }
        }
        display
            .strip_prefix(&self.root_base)
            .unwrap_or(display)
            .to_string_lossy()
            .to_string()
    }

    fn expand(
        &mut self,
        program: ast::Program,
        origin: &str,
        base: &Path,
    ) -> Result<ast::Program, KazeError> {
        let mut out = ast::Program::default();
        let mut names: HashMap<String, String> = HashMap::new();
        for u in &program.uses {
            let (display, canonical) = resolve_import(base, &u.path, origin, u.line)?;
            let dep_origin = self.origin_of(Path::new(&display), &canonical);
            if self.stack.iter().any(|(c, _)| *c == canonical) {
                let mut chain: Vec<String> = self.stack.iter().map(|(_, d)| d.clone()).collect();
                chain.push(dep_origin.clone());
                return Err(KazeError::new(
                    "E_KAZE_IMPORT",
                    format!("import cycle: {}", chain.join(" -> ")),
                ));
            }
            if self.seen.contains(&canonical) {
                continue;
            }
            self.stack.push((canonical.clone(), dep_origin.clone()));
            let src = std::fs::read_to_string(&display).map_err(|e| {
                KazeError::new(
                    "E_KAZE_IO",
                    format!("{}:{}: cannot read '{}': {}", origin, u.line, u.path, e),
                )
            })?;
            let dep = parser::parse(&src)
                .map_err(|e| KazeError::new("E_KAZE_PARSE", e.render(&dep_origin)))?;
            let dep_base = Path::new(&display)
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf();
            let dep = self.expand(dep, &dep_origin, &dep_base)?;
            self.stack.pop();
            self.seen.insert(canonical);
            self.merge_into(&mut out, dep, &dep_origin, &mut names)?;
        }
        let own = ast::Program {
            needs: program.needs,
            uses: Vec::new(),
            root: String::new(),
            items: program.items,
        };
        self.merge_into(&mut out, own, origin, &mut names)?;
        // The top-level call records the root file; a dependency's value is
        // ignored when its parent merges the items.
        out.root = origin.to_string();
        Ok(out)
    }

    fn merge_into(
        &mut self,
        out: &mut ast::Program,
        mut dep: ast::Program,
        origin: &str,
        names: &mut HashMap<String, String>,
    ) -> Result<(), KazeError> {
        for need in dep.needs.drain(..) {
            if !out.needs.contains(&need) {
                out.needs.push(need);
            }
        }
        for mut item in dep.items.drain(..) {
            // Hooks are synthesized per file; number them across the whole
            // module so two libraries cannot both define `when#1`. The `-`
            // form cannot be produced by the parser, so a parent merge leaves
            // an already-renamed hook alone.
            if item.name.starts_with("when#") || item.name.starts_with("every#") {
                let prefix = if item.name.starts_with("when#") { "when-" } else { "every-" };
                self.hook_seq += 1;
                item.name = format!("{}{}", prefix, self.hook_seq);
            }
            // A dependency returned by `expand` already carries the true origin
            // of its own items and of everything it inlined. Only an item with
            // no origin yet belongs to the file being merged at this level.
            let item_origin = if item.origin.is_empty() {
                origin.to_string()
            } else {
                item.origin.clone()
            };
            if let Some(previous) = names.get(&item.name) {
                return Err(KazeError::new(
                    "E_KAZE_IMPORT",
                    format!(
                        "'{}' is defined in both {} and {}",
                        item.name, previous, item_origin
                    ),
                ));
            }
            names.insert(item.name.clone(), item_origin.clone());
            item.origin = item_origin;
            out.items.push(item);
        }
        Ok(())
    }
}

fn resolve_import(
    base: &Path,
    path: &str,
    origin: &str,
    line: u32,
) -> Result<(String, PathBuf), KazeError> {
    let candidate = base.join(path);
    if candidate.extension().and_then(|e| e.to_str()) != Some("kaze") {
        return Err(KazeError::new(
            "E_KAZE_IMPORT",
            format!("{}:{}: '{}' is not a .kaze module", origin, line, path),
        ));
    }
    let canonical = std::fs::canonicalize(&candidate).map_err(|e| {
        KazeError::new(
            "E_KAZE_IO",
            format!("{}:{}: cannot resolve '{}': {}", origin, line, path, e),
        )
    })?;
    Ok((candidate.to_string_lossy().to_string(), canonical))
}

/// Parse, check and compile. This is the whole front end in one call.
pub fn prepare(src: &str, name: &str) -> Result<Prepared, KazeError> {
    prepare_in(src, name, Path::new("."))
}

/// Prepare source text as if it lived at `name` inside `base_dir`. Imports are
/// resolved relative to `base_dir`; `name` is the display name, and its file
/// name is the serialized root origin. Used by the language server, where the
/// buffer lives in memory but its imports live on disk.
pub fn prepare_in(src: &str, name: &str, base_dir: &Path) -> Result<Prepared, KazeError> {
    let mut loader = Loader::new(base_dir.to_path_buf());
    // The serialized root origin is the file name, matching prepare_file, so
    // compile_source and compile_file agree on the bytes for the same source.
    let root_origin = Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    prepare_with(src, name, &root_origin, base_dir, &mut loader)
}

fn prepare_with(
    src: &str,
    display_name: &str,
    root_origin: &str,
    base: &Path,
    loader: &mut Loader,
) -> Result<Prepared, KazeError> {
    let program = parser::parse(src).map_err(|e| {
        KazeError::new("E_KAZE_PARSE", e.render(display_name))
    })?;
    let program = loader.expand(program, root_origin, base)?;
    checker::check(&program).map_err(|errors| {
        let mut message = String::new();
        for (i, err) in errors.iter().enumerate() {
            if i > 0 {
                message.push('\n');
            }
            message.push_str(&err.render(display_name));
        }
        KazeError::new("E_KAZE_CHECK", message)
    })?;
    let module = compiler::compile(&program)
        .map_err(|e| KazeError::new("E_KAZE_COMPILE", e.render(display_name)))?;
    Ok(Prepared {
        module,
        name: display_name.to_string(),
    })
}

pub fn prepare_file(path: &str) -> Result<Prepared, KazeError> {
    let src = read_source(path)?;
    let (base, root_origin) = if path == "-" {
        (PathBuf::from("."), "-".to_string())
    } else {
        let base = Path::new(path)
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        // The item origin is the file name, not the checkout path, so the
        // artifact's bytes stay identical wherever the tree lives.
        let origin = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        (base, origin)
    };
    let mut loader = Loader::new(base.clone());
    if path != "-" {
        if let Ok(canonical) = std::fs::canonicalize(path) {
            // Seed the active chain so a dependency importing this file is a
            // cycle, not an infinite recursion.
            loader.stack.push((canonical, root_origin.clone()));
        }
    }
    prepare_with(&src, path, &root_origin, &base, &mut loader)
}

/// Front end only — what `kaze check` runs.
pub fn check_source(src: &str, name: &str) -> Result<(), KazeError> {
    prepare(src, name).map(|_| ())
}

pub fn check_file(path: &str) -> Result<(), KazeError> {
    prepare_file(path).map(|_| ())
}

pub fn compile_source(src: &str, name: &str) -> Result<Module, KazeError> {
    prepare(src, name).map(|p| p.module)
}

pub fn compile_file(path: &str) -> Result<Module, KazeError> {
    prepare_file(path).map(|p| p.module)
}

/// Everything a run needs besides the script itself.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Item to run first. Default: the root file's `main`, else the only
    /// routine in the root file, else none (hooks only). Imported routines are
    /// selected only when named explicitly.
    pub entry: Option<String>,
    pub args: serde_json::Map<String, serde_json::Value>,
    /// Capability families the host grants a backend for (`drone=sim` → "drone").
    pub grants: Vec<String>,
    pub fuel: u64,
    /// Supervisor ticks after the entry finishes; each tick polls `when`/`every`.
    pub ticks: u32,
    pub tick_ms: u64,
    /// Sleep for `tick_ms` between supervisor ticks. Off by default because
    /// simulated ticks are virtual time; a live link-loss failsafe needs real
    /// time to pass, so an operator running against hardware passes `--pace`.
    pub pace_ticks: bool,
    pub sim: SimState,
    /// R7 policy layered over the capability gate. `None` denies capability
    /// calls; pure computation can run without a policy.
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
            pace_ticks: false,
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
    /// Explicit robotics simulators selected for this run (testing only).
    pub simulated_families: Vec<String>,
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
        if !self.simulated_families.is_empty() {
            out.push_str(&format!(
                "simulation selected (testing only): {}\n",
                self.simulated_families.join(", ")
            ));
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

/// Attribute a run-time failure to the item's own file when modules were
/// inlined, and to the root script otherwise.
fn item_error(
    prepared_name: &str,
    root_origin: &str,
    item: &isa::ItemCode,
    e: vm::VmError,
) -> KazeError {
    // A root item reports the path the operator used; an imported item reports
    // its relative origin. Both are the same string for a hand-built module.
    let source = if item.origin == root_origin { prepared_name } else { &item.origin };
    KazeError::new(e.code, format!("{}: {} (line {})", source, e.msg, e.line))
}

/// Run an already-prepared script. `kaze bench` prepares once and reuses it so
/// the measurement is verification plus execution, not the parser or a clone
/// (every call still verifies the module by design).
pub fn run_prepared(prepared: Prepared, opts: &RunOptions) -> Result<RunReport, KazeError> {
    let module = prepared.module;
    // Every execution path, including a hand-built Module, passes the verifier.
    if let Err(why) = isa::verify(&module) {
        return Err(KazeError::new(
            "E_KAZE_BADCODE",
            format!("{}: {}", prepared.name, why),
        ));
    }
    let declared = module.needs.clone();
    // Routing is explicit even when no live device is configured. In
    // particular, an absent backend never selects the simulator implicitly.
    let mut host: Box<dyn CapHost> = match crate::host::LiveHost::new(
        opts.sim.clone(), &opts.backend, &opts.pins, &opts.pwm,
    ) {
        Ok(host) => Box::new(host),
        Err(e) => {
            let code = if e.starts_with("E_CAP_MISSING:") {
                "E_CAP_MISSING"
            } else if e.starts_with("E_CAP_DENIED:") {
                "E_CAP_DENIED"
            } else {
                "E_KAZE_LINK"
            };
            return Err(KazeError::new(code, format!("{}: {}", prepared.name, e)));
        }
    };
    let mut simulated_families = Vec::new();
    for (family, backend) in &opts.backend {
        if backend.trim() == "sim" {
            let family = caps::family_of(family).to_string();
            if !simulated_families.contains(&family) {
                simulated_families.push(family);
            }
        }
    }
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
        simulated_families,
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
                .map_err(|e| item_error(&prepared.name, &module.root, item, e))?;
            report.value = caps::render_value(value, &vm.arena, &vm.heap);
            report.verdict = vm.take_verdict();
        }

        // Supervisory ticks: `when` predicates and due `every` blocks.
        for tick in 0..opts.ticks {
            // A real supervisor has to spend real time between polls, or a
            // live link never accumulates the silence a failsafe waits for.
            if tick > 0 && opts.pace_ticks && opts.tick_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(opts.tick_ms));
            }
            let t_ms = tick as f64 * opts.tick_ms as f64;
            // v0.4: let the host see the virtual clock before any predicate
            // reads it, so `when link lost for 3 seconds` is deterministic in
            // the simulator and unaffected in a live run.
            vm.host.advance(t_ms as u64);
            for item in module.items.iter() {
                let due = match item.tag {
                    isa::ItemTag::When => true,
                    isa::ItemTag::Every => item.period_ms > 0.0 && (t_ms % item.period_ms) == 0.0,
                    _ => false,
                };
                if !due {
                    continue;
                }
                vm.run_item(item, &[])
                    .map_err(|e| item_error(&prepared.name, &module.root, item, e))?;
            }
            report.ticks += 1;
            // A supervisor may run for hours. The arena and heap belong to the
            // run, not to the tick: reclaim them between ticks so a `when` or
            // `every` hook cannot grow memory without bound. No Value survives
            // a tick boundary — `run_item` rebuilds the locals and stack.
            vm.arena.free_wholesale();
            vm.heap.free_wholesale();
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
    // Only the root file's items decide the implicit default. An imported
    // library's routines are reachable with --entry, never by importing it.
    let is_root = |item: &isa::ItemCode| module.root.is_empty() || item.origin == module.root;
    if let Some(main) = module.item("main") {
        if is_root(main) {
            return Ok(Some("main".to_string()));
        }
    }
    let routines: Vec<&isa::ItemCode> = module
        .items
        .iter()
        .filter(|i| {
            is_root(i)
                && matches!(i.tag, isa::ItemTag::Action | isa::ItemTag::Check | isa::ItemTag::Filter)
        })
        .collect();
    if routines.len() == 1 {
        return Ok(Some(routines[0].name.clone()));
    }
    if routines.is_empty() && module.items.iter().any(is_root) {
        // The root is hooks-only: the ticks are the program.
        return Ok(None);
    }
    Err(KazeError::new(
        "E_KAZE_ENTRY",
        "this script's root file has more than one routine; name the one to run with --entry \
         (or add an action main())",
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
            heap.push_array(vals).unwrap_or(value::Value::Unit)
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
            heap.push_map(entries).unwrap_or(value::Value::Unit)
        }
    }
}
