// sfs/vm.rs — the bytecode interpreter (M-K3).
//
// PLAN-KAZE-LANGUAGE §1.4/§1.5/§4. Two guarantees are enforced here and
// nowhere else:
//
//   * Fuel. Every instruction costs one unit. At zero the run stops with
//     E_TIMEOUT (fuel) — not a panic, not a hang, not a thread to kill. `FUEL_TICK` at
//     each loop head also polls the host deadline (rt/deadline.rs), so a run
//     started from JS obeys the same interrupt as any other host call.
//   * One side-effect path. CALL_CAP is the only instruction that can reach
//     the outside world, and it always asks rt/hardware.rs first. A Kaze
//     program cannot open a socket because there is no instruction for it.
//
// Arithmetic traps instead of wrapping: a non-finite result is E_KAZE_OVERFLOW,
// a division by zero is E_KAZE_DIV0, and a bad index is E_KAZE_BOUNDS. In a
// controller, a silently-wrong number is worse than a stopped run.

use crate::arena::{fmt_num, Arena};
use crate::caps::CapHost;
use crate::isa::{field_a, opcode, Const, ItemCode, Module, Op};
use crate::value::{Heap, Value};

/// How much work one run may do before it is stopped.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub fuel: u64,
    pub arena_bytes: usize,
    /// Values the collection heap may hold at once. Fuel alone does not bound
    /// a collection loop; this does.
    pub heap_values: usize,
}

impl Default for Limits {
    fn default() -> Self {
        // 200k instructions is a few milliseconds of work and far more than any
        // hardware routine needs; the point is that the bound exists. 65,536
        // heap values is roughly 1 MiB of values plus vector overhead.
        Limits { fuel: 200_000, arena_bytes: 256 * 1024, heap_values: 65_536 }
    }
}

#[derive(Debug, Clone)]
pub struct VmError {
    pub code: &'static str,
    pub msg: String,
    pub line: u32,
}

impl VmError {
    fn new(code: &'static str, msg: impl Into<String>) -> Self {
        VmError { code, msg: msg.into(), line: 0 }
    }
}

/// What a `filter` decided.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum Verdict {
    Allow(String),
    Reject(String),
}

pub struct Vm<'a> {
    pub module: &'a Module,
    pub arena: Arena,
    pub heap: Heap,
    pub host: &'a mut dyn CapHost,
    declared: &'a [String],
    grants: &'a [String],
    policy: Option<&'a serde_json::Value>,
    fuel_left: u64,
    pub fuel_used: u64,
    pub instructions: u64,
    stack: Vec<Value>,
    locals: Vec<Value>,
    handlers: Vec<Handler>,
    verdict: Option<Verdict>,
}

/// The stack is a fixed-size vector; a script cannot grow it without bound.
const MAX_STACK: usize = 256;

/// Live `try` handlers. A catchable host failure unwinds to the innermost one.
const MAX_HANDLERS: usize = 32;
/// `TRY_BEGIN`'s "do not bind the error record" slot sentinel.
const NO_ERROR_BINDING: u8 = 255;

struct Handler {
    /// Where the recovery block starts.
    target: usize,
    /// Operand-stack depth at `TRY_BEGIN`; a catch unwinds back to it.
    stack_len: usize,
    /// Local slot for the error record, or `NO_ERROR_BINDING`.
    slot: u8,
    /// The item verdict in force at `TRY_BEGIN`. A catch restores it, so an
    /// `allow`/`reject` produced by a body path that was abandoned cannot
    /// stand as the filter's decision.
    verdict: Option<Verdict>,
}

impl<'a> Vm<'a> {
    pub fn new(
        module: &'a Module,
        host: &'a mut dyn CapHost,
        declared: &'a [String],
        grants: &'a [String],
        policy: Option<&'a serde_json::Value>,
        limits: Limits,
    ) -> Self {
        Vm {
            module,
            arena: Arena::new(limits.arena_bytes),
            heap: Heap::with_limit(limits.heap_values),
            host,
            declared,
            grants,
            policy,
            fuel_left: limits.fuel,
            fuel_used: 0,
            instructions: 0,
            stack: Vec::with_capacity(32),
            locals: Vec::new(),
            handlers: Vec::new(),
            verdict: None,
        }
    }

    /// Run one item to completion. `args` bind to `item.params` by position.
    pub fn run_item(&mut self, item: &ItemCode, args: &[Value]) -> Result<Value, VmError> {
        self.stack.clear();
        self.handlers.clear();
        self.locals = vec![Value::Unit; item.slots.max(item.params.len() as u8) as usize];
        for (i, a) in args.iter().enumerate().take(self.locals.len()) {
            self.locals[i] = *a;
        }
        self.verdict = None;

        let code = &item.code;
        let mut pc = 0usize;
        while pc < code.len() {
            let w = code[pc];
            let Some(op) = Op::from_u8(opcode(w)) else {
                return Err(VmError::new("E_KAZE_BADCODE", "this module has an unknown instruction"));
            };
            pc += 1;
            // A loop head carries its own source line in the immediate word, so
            // an exhausted budget blames the loop rather than the routine.
            let step_line = if matches!(op, Op::FuelTick) {
                code.get(pc).copied().unwrap_or(item.line)
            } else {
                item.line
            };
            self.charge(step_line)?;
            match op {
                Op::Const => {
                    let imm = self.imm(code, pc, item.line)?;
                    pc += 1;
                    let v = self.constant(imm as usize, item.line)?;
                    self.push(v, item.line)?;
                }
                Op::LoadLocal => {
                    let slot = field_a(w) as usize;
                    let v = *self
                        .locals
                        .get(slot)
                        .ok_or_else(|| VmError::new("E_KAZE_BADCODE", "LOAD from a missing slot"))?;
                    self.push(v, item.line)?;
                }
                Op::StoreLocal => {
                    let slot = field_a(w) as usize;
                    let v = self.pop(item.line)?;
                    if let Some(dst) = self.locals.get_mut(slot) {
                        *dst = v;
                    } else {
                        return Err(VmError::new("E_KAZE_BADCODE", "STORE to a missing slot"));
                    }
                }
                Op::NewArr => {
                    let n = field_a(w) as usize;
                    let mut items = Vec::with_capacity(n);
                    for _ in 0..n {
                        items.push(self.pop(item.line)?);
                    }
                    items.reverse();
                    let v = self
                        .heap
                        .push_array(items)
                        .map_err(|msg| VmError { code: "E_KAZE_MEMORY", msg, line: item.line })?;
                    self.push(v, item.line)?;
                }
                Op::GetIndex => {
                    let idx = self.pop(item.line)?;
                    let coll = self.pop(item.line)?;
                    let v = self.index(coll, idx, item.line)?;
                    self.push(v, item.line)?;
                }
                Op::Len => {
                    let v = self.pop(item.line)?;
                    let n = self.length(v, item.line)?;
                    if !n.is_finite() {
                        return Err(overflow(item.line));
                    }
                    self.push(Value::Num(n), item.line)?;
                }
                Op::VecNew => {
                    // Two components is a 2-D point: x and y, with z = 0.
                    let n = field_a(w) as usize;
                    if n != 2 && n != 3 {
                        return Err(VmError::new("E_KAZE_BADCODE", "VECNEW with a bad width"));
                    }
                    let mut parts = [0.0f64; 3];
                    for slot in parts.iter_mut().take(n).rev() {
                        let v = self.pop(item.line)?;
                        *slot = self.num_of(v, item.line)?;
                    }
                    if parts.iter().any(|component| !component.is_finite()) {
                        return Err(overflow(item.line));
                    }
                    let v = self
                        .heap
                        .push_vec(parts)
                        .map_err(|msg| VmError { code: "E_KAZE_MEMORY", msg, line: item.line })?;
                    self.push(v, item.line)?;
                }
                Op::VecDist | Op::VecSlantDist | Op::VecBearingBetween => {
                    let b = self.pop(item.line)?;
                    let a = self.pop(item.line)?;
                    let (p, q) = (self.place(a, item.line)?, self.place(b, item.line)?);
                    let n = match op {
                        Op::VecDist => crate::geo::haversine_m(p, q),
                        Op::VecSlantDist => crate::geo::slant_m(p, q),
                        _ => crate::geo::bearing_deg(p, q),
                    };
                    if !n.is_finite() {
                        return Err(overflow(item.line));
                    }
                    self.push(Value::Num(n), item.line)?;
                }
                Op::VecBearing => {
                    let v = self.pop(item.line)?;
                    let p = self.point(v, item.line)?;
                    let n = crate::geo::bearing_of(p);
                    if !n.is_finite() {
                        return Err(overflow(item.line));
                    }
                    self.push(Value::Num(n), item.line)?;
                }
                Op::Sin | Op::Cos | Op::Sqrt | Op::DegToRad | Op::RadToDeg => {
                    let v = self.pop(item.line)?;
                    let x = self.num_of(v, item.line)?;
                    let r = match op {
                        Op::Sin => x.sin(),
                        Op::Cos => x.cos(),
                        Op::Sqrt => {
                            if x < 0.0 {
                                return Err(VmError {
                                    code: "E_KAZE_MATH",
                                    msg: format!("sqrt of {} has no real answer", crate::arena::fmt_num(x)),
                                    line: item.line,
                                });
                            }
                            x.sqrt()
                        }
                        Op::DegToRad => crate::geo::to_radians(x),
                        _ => crate::geo::to_degrees(x),
                    };
                    if !r.is_finite() {
                        return Err(overflow(item.line));
                    }
                    self.push(Value::Num(r), item.line)?;
                }
                Op::Atan2 => {
                    let x = self.pop(item.line)?;
                    let y = self.pop(item.line)?;
                    let (x, y) = (self.num_of(x, item.line)?, self.num_of(y, item.line)?);
                    let r = y.atan2(x);
                    if !r.is_finite() {
                        return Err(overflow(item.line));
                    }
                    self.push(Value::Num(r), item.line)?;
                }
                Op::GetField => {
                    let field = self.pop(item.line)?;
                    let base = self.pop(item.line)?;
                    let v = self.field(base, field, item.line)?;
                    self.push(v, item.line)?;
                }
                Op::Jeq | Op::Jne => {
                    let imm = self.imm(code, pc, item.line)? as i32;
                    pc += 1;
                    let b = self.pop(item.line)?;
                    let a = self.pop(item.line)?;
                    let eq = self.values_equal(a, b);
                    let taken = if op == Op::Jeq { eq } else { !eq };
                    if taken {
                        pc = jump(pc, imm, code.len(), item.line)?;
                    }
                }
                Op::Pop => {
                    self.pop(item.line)?;
                }
                Op::Add => {
                    let b = self.pop(item.line)?;
                    let a = self.pop(item.line)?;
                    let v = self.add(a, b, item.line)?;
                    self.push(v, item.line)?;
                }
                Op::Sub => self.arith(Op::Sub, item.line)?,
                Op::Mul => self.arith(Op::Mul, item.line)?,
                Op::Div => self.arith(Op::Div, item.line)?,
                Op::Mod => self.arith(Op::Mod, item.line)?,
                Op::Neg => {
                    let v = self.pop(item.line)?;
                    match v {
                        Value::Num(n) => self.push(Value::Num(-n), item.line)?,
                        other => {
                            return Err(VmError::new(
                                "E_KAZE_TYPE",
                                format!("cannot negate {}", other.type_name()),
                            ))
                        }
                    }
                }
                Op::CmpLt | Op::CmpLe | Op::CmpGt | Op::CmpGe => {
                    let b = self.pop(item.line)?;
                    let a = self.pop(item.line)?;
                    let v = self.compare(op, a, b, item.line)?;
                    self.push(Value::Bool(v), item.line)?;
                }
                Op::CmpEq | Op::CmpNe => {
                    let b = self.pop(item.line)?;
                    let a = self.pop(item.line)?;
                    let eq = self.values_equal(a, b);
                    self.push(Value::Bool(if op == Op::CmpEq { eq } else { !eq }), item.line)?;
                }
                Op::Not => {
                    let v = self.pop(item.line)?;
                    self.push(Value::Bool(!v.is_truthy()), item.line)?;
                }
                Op::Jmp => {
                    let imm = self.imm(code, pc, item.line)? as i32;
                    pc = jump(pc + 1, imm, code.len(), item.line)?;
                }
                Op::JmpIfFalse | Op::JmpIfTrue => {
                    let imm = self.imm(code, pc, item.line)? as i32;
                    pc += 1;
                    let v = self.pop(item.line)?;
                    let taken = if op == Op::JmpIfTrue { v.is_truthy() } else { !v.is_truthy() };
                    if taken {
                        pc = jump(pc, imm, code.len(), item.line)?;
                    }
                }
                Op::CallCap => {
                    let imm = self.imm(code, pc, item.line)? as usize;
                    pc += 1;
                    if let Err(err) = self.call_cap(imm, field_a(w) as usize, item.line) {
                        // v0.4: an operational host failure may recover at the
                        // innermost `try`. Safety, range, fuel and memory
                        // errors never reach catch_error (see is_catchable).
                        if !self.catch_error(&err, &mut pc)? {
                            return Err(err);
                        }
                    }
                }
                Op::PolicyStop => {
                    let v = self.pop(item.line)?;
                    return Ok(v);
                }
                Op::Ret => {
                    let v = self.pop(item.line)?;
                    return Ok(v);
                }
                Op::RetUnit => return Ok(Value::Unit),
                Op::FuelTick => {
                    // The pre-charge above already paid for this tick; the
                    // immediate only names the loop for error reporting.
                    let line = self.imm(code, pc, item.line)?;
                    pc += 1;
                    // A JS host may have armed a deadline for this whole call.
                    if crate::deadline::poll_expired() != 0 {
                        // rt/deadline.rs already recorded the firing; ask it for
                        // the report so a host sees one story, not two.
                        let detail = crate::deadline::classify_interrupted("interrupted")
                            .unwrap_or_else(|| {
                                "E_TIMEOUT: the call was interrupted after its deadline elapsed"
                                    .to_string()
                            });
                        let msg = detail
                            .strip_prefix("E_TIMEOUT: ")
                            .unwrap_or(detail.as_str())
                            .to_string();
                        return Err(VmError { code: "E_TIMEOUT", msg, line });
                    }
                }
                Op::Trap => {
                    let imm = self.imm(code, pc, item.line)? as usize;
                    let msg = match self.module.consts.get(imm) {
                        Some(Const::Str(s)) => s.clone(),
                        _ => "the script stopped".to_string(),
                    };
                    return Err(VmError { code: "E_KAZE_TRAP", msg, line: item.line });
                }
                Op::Halt => return Ok(Value::Unit),
                Op::TryBegin => {
                    let imm = self.imm(code, pc, item.line)? as i32;
                    let target = jump(pc + 1, imm, code.len(), item.line)?;
                    pc += 1;
                    if self.handlers.len() >= MAX_HANDLERS {
                        return Err(VmError {
                            code: "E_KAZE_STACK",
                            msg: format!("more than {} nested 'try' blocks", MAX_HANDLERS),
                            line: item.line,
                        });
                    }
                    self.handlers.push(Handler {
                        target,
                        stack_len: self.stack.len(),
                        slot: field_a(w),
                        verdict: self.verdict.clone(),
                    });
                }
                Op::TryEnd => {
                    if self.handlers.pop().is_none() {
                        return Err(VmError::new(
                            "E_KAZE_BADCODE",
                            "TRY_END ran without an active try block",
                        ));
                    }
                }
                Op::LinkLost => {
                    // Link monitoring is a host read and must cross the same
                    // declaration, grant, and policy gate as other reads.
                    crate::hardware::authorise(
                        self.declared,
                        self.grants,
                        self.policy,
                        "drone",
                        "link_lost",
                    )
                    .map_err(|d| VmError {
                        code: d.code,
                        msg: format!("{} ({})", d.reason, d.rule),
                        line: item.line,
                    })?;
                    let after_ms = self.imm(code, pc, item.line)? as u64;
                    pc += 1;
                    if after_ms == 0 {
                        return Err(VmError::new(
                            "E_KAZE_BADCODE",
                            "LINK_LOST with a zero window would always be true",
                        ));
                    }
                    let lost = self
                        .host
                        .read_link_silence_ms()
                        .map_err(|msg| {
                            let (code, msg) = split_host_code(msg);
                            VmError { code, msg, line: item.line }
                        })?
                        .is_some_and(|silent| silent >= after_ms);
                    self.push(Value::Bool(lost), item.line)?;
                }
            }
        }
        Ok(Value::Unit)
    }

    /// Take the verdict the last `run_item` produced, if it was a `filter`.
    pub fn take_verdict(&mut self) -> Option<Verdict> {
        self.verdict.take()
    }

    pub fn fuel_left(&self) -> u64 {
        self.fuel_left
    }

    // ── fault tolerance (v0.4) ───────────────────────────────────────
    /// Unwind a catchable host failure to the innermost active `try`.
    ///
    /// Returns `false` when nothing caught it, so the caller can propagate the
    /// original error. The operand stack returns to the depth it had at
    /// `TRY_BEGIN`; locals are deliberately left as they are, the way a
    /// recovery block expects to see the state that failed.
    fn catch_error(&mut self, err: &VmError, pc: &mut usize) -> Result<bool, VmError> {
        if !is_catchable(err.code) {
            return Ok(false);
        }
        let Some(handler) = self.handlers.pop() else {
            return Ok(false);
        };
        self.stack.truncate(handler.stack_len);
        // The body may have set a filter verdict before the failure. That path
        // was abandoned; the recovery block starts from the decision that was
        // in force when the try began, not from a stale approval.
        self.verdict = handler.verdict;
        if handler.slot != NO_ERROR_BINDING {
            let record = self.error_record(err)?;
            match self.locals.get_mut(handler.slot as usize) {
                Some(dst) => *dst = record,
                None => {
                    return Err(VmError {
                        code: "E_KAZE_BADCODE",
                        msg: "TRY_BEGIN names a local slot this routine does not have".to_string(),
                        line: err.line,
                    })
                }
            }
        }
        *pc = handler.target;
        Ok(true)
    }

    /// The record bound by `otherwise with err:`: `code`, `message`, `line`.
    fn error_record(&mut self, err: &VmError) -> Result<Value, VmError> {
        let code = self.alloc_text(err.code, err.line)?;
        let message = self.alloc_text(&err.msg, err.line)?;
        let key_code = self.alloc_text("code", err.line)?;
        let key_message = self.alloc_text("message", err.line)?;
        let key_line = self.alloc_text("line", err.line)?;
        self.heap
            .push_map(vec![
                (key_code, code),
                (key_message, message),
                (key_line, Value::Num(err.line as f64)),
            ])
            .map_err(|msg| VmError { code: "E_KAZE_MEMORY", msg, line: err.line })
    }

    fn alloc_text(&mut self, text: &str, line: u32) -> Result<Value, VmError> {
        self.arena.alloc(text).map(Value::Str).map_err(|e| VmError {
            code: "E_KAZE_MEMORY",
            // Arena::alloc names its own code; do not print it twice.
            msg: e.strip_prefix("E_KAZE_MEMORY: ").unwrap_or(&e).to_string(),
            line,
        })
    }

    // ── helpers ──────────────────────────────────────────────────────
    fn charge(&mut self, line: u32) -> Result<(), VmError> {
        self.instructions += 1;
        self.fuel_used += 1;
        if self.fuel_left == 0 {
            // PLAN-KAZE §1.5/§4: a runaway script halts uncatchably and comes
            // back as a typed E_TIMEOUT. The message names the cause, because
            // "out of fuel" and "the host's deadline fired" need different
            // fixes even though both are timeouts to the caller.
            return Err(VmError {
                code: "E_TIMEOUT",
                msg: "this run used all of its fuel and was stopped (fuel, not wall time); \
                      a loop that never ends or too much work in one tick is the usual cause"
                    .to_string(),
                line,
            });
        }
        self.fuel_left -= 1;
        Ok(())
    }

    fn imm(&self, code: &[u32], pc: usize, line: u32) -> Result<u32, VmError> {
        code.get(pc)
            .copied()
            .ok_or_else(|| VmError { code: "E_KAZE_BADCODE", msg: "truncated instruction".into(), line })
    }

    fn push(&mut self, v: Value, line: u32) -> Result<(), VmError> {
        if self.stack.len() >= MAX_STACK {
            return Err(VmError {
                code: "E_KAZE_STACK",
                msg: "this expression nests too deeply; split it into smaller steps".to_string(),
                line,
            });
        }
        self.stack.push(v);
        Ok(())
    }

    fn pop(&mut self, line: u32) -> Result<Value, VmError> {
        self.stack.pop().ok_or_else(|| VmError {
            code: "E_KAZE_BADCODE",
            msg: "the value stack underflowed".to_string(),
            line,
        })
    }

    fn constant(&mut self, idx: usize, line: u32) -> Result<Value, VmError> {
        match self.module.consts.get(idx) {
            Some(Const::Num(n)) => Ok(Value::Num(*n)),
            Some(Const::Bool(b)) => Ok(Value::Bool(*b)),
            Some(Const::Str(s)) => {
                let text = s.clone();
                let r = self.arena.alloc(&text).map_err(|e| VmError {
                    code: "E_KAZE_MEMORY",
                    msg: e,
                    line,
                })?;
                Ok(Value::Str(r))
            }
            None => Err(VmError { code: "E_KAZE_BADCODE", msg: "missing constant".into(), line }),
        }
    }

    fn add(&mut self, a: Value, b: Value, line: u32) -> Result<Value, VmError> {
        match (a, b) {
            (Value::Num(x), Value::Num(y)) => {
                let sum = x + y;
                if !sum.is_finite() {
                    return Err(overflow(line));
                }
                Ok(Value::Num(sum))
            }
            (Value::Str(x), Value::Str(y)) => {
                let text = format!("{}{}", self.arena.get(x), self.arena.get(y));
                let r = self.arena.alloc(&text).map_err(|e| VmError {
                    code: "E_KAZE_MEMORY",
                    msg: e,
                    line,
                })?;
                Ok(Value::Str(r))
            }
            (Value::Vec(x), Value::Vec(y)) => {
                let (p, q) = (self.point(Value::Vec(x), line)?, self.point(Value::Vec(y), line)?);
                let sum = [p[0] + q[0], p[1] + q[1], p[2] + q[2]];
                if sum.iter().any(|component| !component.is_finite()) {
                    return Err(overflow(line));
                }
                self.heap
                    .push_vec(sum)
                    .map_err(|msg| VmError { code: "E_KAZE_MEMORY", msg, line })
            }
            (a, b) => Err(VmError {
                code: "E_KAZE_TYPE",
                msg: format!("cannot add {} to {}", a.type_name(), b.type_name()),
                line,
            }),
        }
    }

    fn arith(&mut self, op: Op, line: u32) -> Result<(), VmError> {
        let b = self.pop(line)?;
        let a = self.pop(line)?;
        // Points: `a - b` is an offset, a number scales one, and `0 - p` (how
        // the parser spells unary minus) negates it.
        if matches!(a, Value::Vec(_)) || matches!(b, Value::Vec(_)) {
            let v = self.vector_arith(op, a, b, line)?;
            return self.push(v, line);
        }
        let (x, y) = match (a, b) {
            (Value::Num(x), Value::Num(y)) => (x, y),
            (a, b) => {
                return Err(VmError {
                    code: "E_KAZE_TYPE",
                    msg: format!("cannot do arithmetic on {} and {}", a.type_name(), b.type_name()),
                    line,
                })
            }
        };
        let v = match op {
            Op::Sub => x - y,
            Op::Mul => x * y,
            Op::Div => {
                if y == 0.0 {
                    return Err(divide_by_zero(line));
                }
                x / y
            }
            Op::Mod => {
                if y == 0.0 {
                    return Err(divide_by_zero(line));
                }
                x % y
            }
            _ => {
                return Err(VmError::new(
                    "E_KAZE_BADCODE",
                    "arithmetic ran with a non-arithmetic opcode",
                ))
            }
        };
        if !v.is_finite() {
            return Err(overflow(line));
        }
        self.push(Value::Num(v), line)
    }

    /// Equality that knows points are values, not heap slots: two separately
    /// built points with the same components are equal.
    fn values_equal(&self, a: Value, b: Value) -> bool {
        match (a, b) {
            (Value::Vec(x), Value::Vec(y)) => {
                match (self.heap.vec3(x), self.heap.vec3(y)) {
                    (Some(p), Some(q)) => p == q,
                    _ => false,
                }
            }
            (a, b) => crate::arena::values_equal(a, b, &self.arena),
        }
    }

    /// A point's three components.
    fn point(&self, v: Value, line: u32) -> Result<[f64; 3], VmError> {
        match v {
            Value::Vec(i) => self.heap.vec3(i).ok_or_else(|| VmError {
                code: "E_KAZE_BADCODE",
                msg: "a point is missing from the heap".to_string(),
                line,
            }),
            other => Err(VmError {
                code: "E_KAZE_TYPE",
                msg: format!("this needs a point, but it is {}", other.type_name()),
                line,
            }),
        }
    }

    /// A point used as a *place*. Ground distances and bearings only mean
    /// something for a latitude and longitude, so a value that cannot be one is
    /// refused here instead of quietly producing a plausible number.
    fn place(&self, v: Value, line: u32) -> Result<[f64; 3], VmError> {
        let p = self.point(v, line)?;
        let (lat, lon) = (p[0], p[1]);
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return Err(VmError {
                code: "E_KAZE_RANGE",
                msg: format!(
                    "({}, {}, {}) is not a latitude and longitude; for a local offset use 'length of' or 'bearing of'",
                    crate::arena::fmt_num(lat),
                    crate::arena::fmt_num(lon),
                    crate::arena::fmt_num(p[2])
                ),
                line,
            });
        }
        Ok(p)
    }

    fn num_of(&self, v: Value, line: u32) -> Result<f64, VmError> {
        match v {
            Value::Num(n) => Ok(n),
            other => Err(VmError {
                code: "E_KAZE_TYPE",
                msg: format!("this needs a number, but it is {}", other.type_name()),
                line,
            }),
        }
    }

    /// `op` where at least one side is a point.
    fn vector_arith(&mut self, op: Op, a: Value, b: Value, line: u32) -> Result<Value, VmError> {
        let out = match (op, a, b) {
            (Op::Sub, Value::Vec(x), Value::Vec(y)) => {
                let p = self.point(Value::Vec(x), line)?;
                let q = self.point(Value::Vec(y), line)?;
                [p[0] - q[0], p[1] - q[1], p[2] - q[2]]
            }
            (Op::Mul, Value::Vec(x), Value::Num(s))
            | (Op::Mul, Value::Num(s), Value::Vec(x)) => {
                let p = self.point(Value::Vec(x), line)?;
                [p[0] * s, p[1] * s, p[2] * s]
            }
            (Op::Div, Value::Vec(x), Value::Num(s)) => {
                if s == 0.0 {
                    return Err(divide_by_zero(line));
                }
                let p = self.point(Value::Vec(x), line)?;
                [p[0] / s, p[1] / s, p[2] / s]
            }
            (Op::Sub, Value::Num(0.0), Value::Vec(x)) => {
                let p = self.point(Value::Vec(x), line)?;
                [-p[0], -p[1], -p[2]]
            }
            (op, a, b) => {
                return Err(VmError {
                    code: "E_KAZE_TYPE",
                    msg: format!(
                        "'{}' does not apply to {} and {}",
                        op_symbol(op),
                        a.type_name(),
                        b.type_name()
                    ),
                    line,
                })
            }
        };
        for component in out {
            if !component.is_finite() {
                return Err(overflow(line));
            }
        }
        self.heap
            .push_vec(out)
            .map_err(|msg| VmError { code: "E_KAZE_MEMORY", msg, line })
    }

    fn compare(&self, op: Op, a: Value, b: Value, line: u32) -> Result<bool, VmError> {
        let ord = match (a, b) {
            (Value::Num(x), Value::Num(y)) => x.partial_cmp(&y),
            (Value::Str(x), Value::Str(y)) => self.arena.get(x).partial_cmp(self.arena.get(y)),
            (a, b) => {
                return Err(VmError {
                    code: "E_KAZE_TYPE",
                    msg: format!("cannot compare {} with {}", a.type_name(), b.type_name()),
                    line,
                })
            }
        };
        let ord = ord.ok_or_else(|| VmError {
            code: "E_KAZE_TYPE",
            msg: "this comparison has no answer (a value is not a number)".to_string(),
            line,
        })?;
        Ok(match op {
            Op::CmpLt => ord.is_lt(),
            Op::CmpLe => ord.is_le(),
            Op::CmpGt => ord.is_gt(),
            Op::CmpGe => ord.is_ge(),
            _ => {
                return Err(VmError::new(
                    "E_KAZE_BADCODE",
                    "comparison ran with a non-comparison opcode",
                ))
            }
        })
    }

    /// `record.field`. A missing field reads as `nothing` — an optional
    /// record field is normal data, not an error — but a field read on a
    /// non-record is a type error, because that is a bug in the script.
    fn field(&mut self, base: Value, field: Value, line: u32) -> Result<Value, VmError> {
        let name = match field {
            Value::Str(s) => self.arena.get(s).to_string(),
            other => {
                return Err(VmError {
                    code: "E_KAZE_TYPE",
                    msg: format!("a field name must be text, not {}", other.type_name()),
                    line,
                })
            }
        };
        match base {
            Value::Map(m) => {
                let key = self
                    .arena
                    .alloc(&name)
                    .map_err(|e| VmError { code: "E_KAZE_MEMORY", msg: e, line })?;
                Ok(self.heap.map_get(m, Value::Str(key), &self.arena).unwrap_or(Value::Unit))
            }
            Value::Vec(v) => {
                let p = self.point(Value::Vec(v), line)?;
                match name.as_str() {
                    "x" | "lat" | "latitude" => Ok(Value::Num(p[0])),
                    "y" | "lon" | "longitude" => Ok(Value::Num(p[1])),
                    "z" | "alt" | "altitude" => Ok(Value::Num(p[2])),
                    _ => Err(VmError {
                        code: "E_KAZE_TYPE",
                        msg: format!("'.{}' is not a point component; write x/lat, y/lon, or z/alt", name),
                        line,
                    }),
                }
            }
            other => Err(VmError {
                code: "E_KAZE_TYPE",
                msg: format!("'.{}' reads a field, but the value is {}", name, other.type_name()),
                line,
            }),
        }
    }

    fn length(&self, v: Value, line: u32) -> Result<f64, VmError> {
        match v {
            Value::Str(s) => Ok(self.arena.get(s).chars().count() as f64),
            Value::Arr(i) => Ok(self.heap.array(i).map(|a| a.len()).unwrap_or(0) as f64),
            Value::Map(i) => Ok(self.heap.map(i).map(|m| m.len()).unwrap_or(0) as f64),
            Value::Vec(i) => Ok(crate::geo::magnitude(self.heap.vec3(i).unwrap_or([0.0; 3]))),
            other => Err(VmError {
                code: "E_KAZE_TYPE",
                msg: format!("'length of' does not apply to {}", other.type_name()),
                line,
            }),
        }
    }

    fn index(&mut self, coll: Value, idx: Value, line: u32) -> Result<Value, VmError> {
        let i = match idx {
            Value::Num(n) => n,
            other => {
                return Err(VmError {
                    code: "E_KAZE_TYPE",
                    msg: format!("a position must be a number, not {}", other.type_name()),
                    line,
                })
            }
        };
        if i < 0.0 || i.fract() != 0.0 {
            return Err(VmError {
                code: "E_KAZE_BOUNDS",
                msg: format!("{} is not a valid position", fmt_num(i)),
                line,
            });
        }
        let i = i as usize;
        match coll {
            Value::Vec(v) => {
                let p = self.heap.vec3(v).ok_or_else(|| VmError {
                    code: "E_KAZE_BADCODE",
                    msg: "a point is missing from the heap".to_string(),
                    line,
                })?;
                if i > 2 {
                    return Err(VmError {
                        code: "E_KAZE_BOUNDS",
                        msg: format!("a point has components 0, 1 and 2, not {}", i),
                        line,
                    });
                }
                Ok(Value::Num(p[i]))
            }
            Value::Arr(a) => self
                .heap
                .array(a)
                .and_then(|items| items.get(i).copied())
                .ok_or_else(|| VmError {
                    code: "E_KAZE_BOUNDS",
                    msg: format!("position {} is past the end of the list", i),
                    line,
                }),
            Value::Str(s) => {
                let text: Vec<char> = self.arena.get(s).chars().collect();
                let Some(ch) = text.get(i).copied() else {
                    return Err(VmError {
                        code: "E_KAZE_BOUNDS",
                        msg: format!("position {} is past the end of the text", i),
                        line,
                    });
                };
                let one = ch.to_string();
                let r = self
                    .arena
                    .alloc(&one)
                    .map_err(|e| VmError { code: "E_KAZE_MEMORY", msg: e, line })?;
                Ok(Value::Str(r))
            }
            other => Err(VmError {
                code: "E_KAZE_TYPE",
                msg: format!("[ ] does not apply to {}", other.type_name()),
                line,
            }),
        }
    }

    fn call_cap(&mut self, cap_idx: usize, argc: usize, line: u32) -> Result<(), VmError> {
        let cap = self
            .module
            .caps
            .get(cap_idx)
            .cloned()
            .ok_or_else(|| VmError { code: "E_KAZE_BADCODE", msg: "missing capability".into(), line })?;
        let mut args = vec![Value::Unit; argc];
        for i in (0..argc).rev() {
            args[i] = self.pop(line)?;
        }

        // The R7 gate runs for EVERY call, including the in-process families.
        crate::hardware::authorise(
            self.declared,
            self.grants,
            self.policy,
            &cap.family,
            &cap.op,
        )
        .map_err(|d| VmError { code: d.code, msg: format!("{} ({})", d.reason, d.rule), line })?;
        crate::hardware::validate_arguments(&cap.family, &cap.op, &args, &self.heap)
            .map_err(|d| VmError { code: d.code, msg: format!("{} ({})", d.reason, d.rule), line })?;

        // Verdicts are control flow, not effects: the VM owns them, so a host
        // backend cannot fake an approval.
        if cap.family == "policy" && cap.op == "allow" {
            let v = args.first().copied().unwrap_or(Value::Unit);
            self.verdict = Some(Verdict::Allow(render(&v, &self.arena, &self.heap)));
            self.push(v, line)?;
            return Ok(());
        }
        if cap.family == "policy" && cap.op == "reject" {
            let reason = match args.first() {
                Some(Value::Str(s)) => self.arena.get(*s).to_string(),
                other => other.map(|v| render(v, &self.arena, &self.heap)).unwrap_or_default(),
            };
            self.verdict = Some(Verdict::Reject(reason));
            self.push(Value::Unit, line)?;
            return Ok(());
        }

        let result = self
            .host
            .call(&cap.family, &cap.op, &args, &mut self.arena, &mut self.heap)
            .map_err(|msg| {
                // v0.2: a live backend names its own failure — a rejected
                // command, a silent link, a pin the operator never configured —
                // and that code should reach the script, not a generic one.
                let (code, rest) = split_host_code(msg);
                VmError { code, msg: rest, line }
            })?;
        self.push(result, line)
    }

}

/// The failures a script may recover from with `try ... otherwise`.
///
/// This list is the safety boundary of the feature. It deliberately excludes
/// every capability/safety denial (`E_CAP_*`, `E_KAZE_RANGE`,
/// `E_SAFETY_VIOLATION`), every budget stop (`E_TIMEOUT`), and every memory,
/// type, maths and bytecode error: a machine acting on a value it was told to
/// refuse is a machine acting on a lie, so those still stop the run.
fn is_catchable(code: &str) -> bool {
    matches!(
        code,
        "E_KAZE_LINK" | "E_KAZE_LINK_TIMEOUT" | "E_COMMAND_REJECTED" | "E_ARMING_DENIED"
            | "E_KAZE_HOST"
    )
}

/// The error codes a live backend may report through `CapHost::call`.
const HOST_CODES: &[&str] = &[
    "E_KAZE_LINK",
    "E_KAZE_LINK_TIMEOUT",
    "E_COMMAND_REJECTED",
    "E_ARMING_DENIED",
    "E_KAZE_HOST",
    // Safety/budget codes a backend might name. They keep their identity so
    // is_catchable can refuse them; an unknown code must never become
    // E_KAZE_HOST, which is recoverable.
    "E_SAFETY_VIOLATION",
    "E_CAP_UNDECLARED",
    "E_CAP_MISSING",
    "E_CAP_DENIED",
    "E_KAZE_BADCODE",
    "E_KAZE_BOUNDS",
    "E_KAZE_DIV0",
    "E_KAZE_OVERFLOW",
    "E_KAZE_STACK",
    "E_KAZE_TRAP",
    "E_KAZE_RANGE",
    "E_KAZE_MEMORY",
    "E_KAZE_TYPE",
    "E_KAZE_MATH",
    "E_TIMEOUT",
];

/// Unknown or unclassified host failures are fatal rather than catchable.
fn split_host_code(msg: String) -> (&'static str, String) {
    if let Some((head, rest)) = msg.split_once(':') {
        for code in HOST_CODES {
            if head == *code {
                return (code, rest.trim_start().to_string());
            }
        }
    }
    ("E_KAZE_HOST_UNCLASSIFIED", msg)
}

fn jump(pc: usize, imm: i32, len: usize, line: u32) -> Result<usize, VmError> {
    let target = pc as i64 + imm as i64;
    if target < 0 || target as usize > len {
        return Err(VmError { code: "E_KAZE_BADCODE", msg: "a jump left the code".into(), line });
    }
    Ok(target as usize)
}

fn overflow(line: u32) -> VmError {
    VmError {
        code: "E_KAZE_OVERFLOW",
        msg: "this calculation overflowed; the run stopped instead of continuing with a wrong \
              number"
            .to_string(),
        line,
    }
}

fn divide_by_zero(line: u32) -> VmError {
    VmError { code: "E_KAZE_DIV0", msg: "dividing by zero".to_string(), line }
}

fn op_symbol(op: Op) -> &'static str {
    match op {
        Op::Add => "+",
        Op::Sub => "-",
        Op::Mul => "*",
        Op::Div => "/",
        Op::Mod => "%",
        _ => "?",
    }
}

fn render(v: &Value, arena: &Arena, heap: &Heap) -> String {
    crate::caps::render_value(*v, arena, heap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{Action, CapHost};

    #[test]
    fn unclassified_host_errors_are_fatal() {
        for message in ["E_FUTURE_SAFETY: refused", "untyped failure", "E_CAP_FUTURE: denied"] {
            let (code, detail) = split_host_code(message.to_string());
            assert_eq!(code, "E_KAZE_HOST_UNCLASSIFIED");
            assert_eq!(detail, message);
            assert!(!is_catchable(code));
        }
        let (code, _) = split_host_code("E_CAP_DENIED: denied".to_string());
        assert_eq!(code, "E_CAP_DENIED");
        assert!(!is_catchable(code));
        let (code, _) = split_host_code("E_KAZE_HOST: operational failure".to_string());
        assert_eq!(code, "E_KAZE_HOST");
        assert!(is_catchable(code));
    }

    /// A host that fails every non-log call with a chosen code.
    struct FakeHost {
        reply: String,
        logs: Vec<(String, String)>,
    }

    impl CapHost for FakeHost {
        fn call(
            &mut self,
            family: &str,
            op: &str,
            args: &[Value],
            arena: &mut Arena,
            heap: &mut Heap,
        ) -> Result<Value, String> {
            if family == "log" {
                let msg = match args.first() {
                    Some(Value::Str(s)) => arena.get(*s).to_string(),
                    Some(other) => crate::caps::render_value(*other, arena, heap),
                    None => String::new(),
                };
                self.logs.push((op.to_string(), msg));
                Ok(Value::Unit)
            } else {
                Err(self.reply.clone())
            }
        }

        fn actions(&self) -> &[Action] {
            &[]
        }

        fn logs(&self) -> &[(String, String)] {
            &self.logs
        }
    }

    fn run_with(reply: &str) -> (Result<(), VmError>, Vec<(String, String)>) {
        let src = "needs: drone, log\naction main():\n    try:\n        arm drone\n    otherwise with err:\n        log err.code\n";
        let program = crate::parser::parse(src).expect("parse");
        crate::checker::check(&program).expect("check");
        let module = crate::compiler::compile(&program).expect("compile");
        let declared = vec!["drone".to_string(), "log".to_string()];
        let grants = vec!["drone".to_string()];
        let mut host = FakeHost { reply: reply.to_string(), logs: Vec::new() };
        let default_policy = serde_json::json!({ "deny": [] });
        let result = {
            let mut vm = Vm::new(&module, &mut host, &declared, &grants, Some(&default_policy), Limits::default());
            let item = module.item("main").expect("main");
            vm.run_item(item, &[]).map(|_| ())
        };
        (result, host.logs)
    }

    #[test]
    fn every_operational_host_code_is_recoverable() {
        for code in [
            "E_KAZE_LINK",
            "E_KAZE_LINK_TIMEOUT",
            "E_COMMAND_REJECTED",
            "E_ARMING_DENIED",
            "E_KAZE_HOST",
        ] {
            let (result, logs) = run_with(&format!("{}: refused by the fake", code));
            assert!(result.is_ok(), "{} should be recoverable: {:?}", code, result.err());
            assert_eq!(logs.first().map(|(_, m)| m.as_str()), Some(code), "{:?}", logs);
        }
    }

    #[test]
    fn a_safety_or_budget_code_from_a_host_stays_fatal() {
        for code in [
            "E_KAZE_RANGE",
            "E_SAFETY_VIOLATION",
            "E_KAZE_MEMORY",
            "E_KAZE_TYPE",
            "E_KAZE_MATH",
            "E_TIMEOUT",
        ] {
            let (result, _) = run_with(&format!("{}: refused by the fake", code));
            let err = result.expect_err("a safety or budget code must stay fatal");
            assert_eq!(err.code, code, "{}", err.msg);
        }
    }
}
