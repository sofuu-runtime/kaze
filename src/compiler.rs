// sfs/compiler.rs — AST → Kaze bytecode (M-K3).
//
// PLAN-KAZE-LANGUAGE §4/§5. One pass, one stack, no optimisation: the goal is
// an artefact that is small, auditable and byte-identical for the same source.
// Two properties are load-bearing:
//
//   1. Determinism. Constants and capabilities are deduplicated by linear
//      scan, never by hashing, so the same script compiles to the same bytes
//      on every machine and every run (§2 "bit-exact execution trace").
//   2. Explicit side effects. The only instruction that can leave the VM is
//      CALL_CAP, and every CALL_CAP names an entry in the module's capability
//      table, which the R7 gate re-checks at run time.

use crate::ast::*;
use crate::caps;
use crate::isa::{word, CapRef, Const, ItemCode, ItemTag, Module, Op};

#[derive(Debug, Clone)]
pub struct CompileError {
    pub line: u32,
    pub msg: String,
    /// The file this error belongs to when modules were inlined. Empty means
    /// the root source; `render` then uses the caller's name.
    pub origin: String,
}

impl CompileError {
    pub fn render(&self, source_name: &str) -> String {
        let source = if self.origin.is_empty() { source_name } else { &self.origin };
        format!("{}:{}: {}", source, self.line, self.msg)
    }
}

fn e<T>(line: u32, msg: impl Into<String>) -> Result<T, CompileError> {
    Err(CompileError { line, msg: msg.into(), origin: String::new() })
}

const MAX_SLOTS: u8 = 255;
/// `TRY_BEGIN`'s "do not bind the error record" slot sentinel.
const NO_ERROR_BINDING: u8 = 255;

pub fn compile(program: &Program) -> Result<Module, CompileError> {
    if let Some(u) = program.uses.first() {
        return e(
            u.line,
            "unresolved 'use' directive: call prepare/prepare_file so imports are expanded",
        );
    }
    let mut module = Module {
        needs: program.needs.clone(),
        root: program.root.clone(),
        ..Default::default()
    };
    for item in &program.items {
        let built = (|| -> Result<ItemCode, CompileError> {
            let mut cx = Cx {
                consts: &mut module.consts,
                caps: &mut module.caps,
                code: Vec::new(),
                scopes: vec![Vec::new()],
                next_slot: 0,
                item_name: item.name.clone(),
                items: &program.items,
                requires: item.contracts.requires.clone(),
                requires_proved: requires_proved_everywhere(&program.items, item),
                ensures: item.contracts.ensures.clone(),
            };
            for p in &item.params {
                cx.bind(p)?;
            }
            // A contract is only a promise once the run checks it: every
            // `requires` becomes an assertion at entry, and a false one stops
            // the run with E_KAZE_CONTRACT (uncatchable).
            cx.entry_assertions()?;
            match &item.kind {
                ItemKind::When(cond) => {
                    // `when` compiles the predicate into the item itself: the host
                    // polls it, and the body runs only when it is true.
                    cx.expr(cond)?;
                    let skip = cx.emit_wide(Op::JmpIfFalse, 0, 0);
                    cx.stmts(&item.body)?;
                    cx.emit(Op::RetUnit, 0, 0, 0);
                    cx.patch(skip);
                    cx.emit(Op::RetUnit, 0, 0, 0);
                }
                ItemKind::Record(fields) => {
                    for (slot, f) in fields.iter().enumerate() {
                        let k_idx = cx.push_const(Const::Str(f.name.clone()))?;
                        cx.emit_wide(Op::Const, 0, k_idx);
                        cx.emit(Op::LoadLocal, slot as u8, 0, 0);
                    }
                    cx.emit(Op::NewMap, fields.len() as u8, 0, 0);
                    cx.emit(Op::Ret, 0, 0, 0);
                }
                _ => {
                    cx.stmts(&item.body)?;
                    cx.emit(Op::RetUnit, 0, 0, 0);
                }
            }
            let (tag, period) = match &item.kind {
                ItemKind::Action => (ItemTag::Action, 0.0),
                ItemKind::Check => (ItemTag::Check, 0.0),
                ItemKind::Filter => (ItemTag::Filter, 0.0),
                ItemKind::When(_) => (ItemTag::When, 0.0),
                ItemKind::Every(ms) => (ItemTag::Every, *ms),
                ItemKind::Record(_) => (ItemTag::Action, 0.0),
            };
            let slots = cx.next_slot.max(item.params.len() as u8);
            Ok(ItemCode {
                name: item.name.clone(),
                tag,
                params: item.params.clone(),
                code: std::mem::take(&mut cx.code),
                line: item.line,
                origin: item.origin.clone(),
                period_ms: period,
                slots,
                recursion_bound: item.contracts.recursion_bound.unwrap_or(0) as u16,
            })
        })();
        match built {
            Ok(code) => module.items.push(code),
            Err(mut err) => {
                if err.origin.is_empty() {
                    err.origin = item.origin.clone();
                }
                return Err(err);
            }
        }
    }
    Ok(module)
}

struct Cx<'a> {
    consts: &'a mut Vec<Const>,
    caps: &'a mut Vec<CapRef>,
    code: Vec<u32>,
    scopes: Vec<Vec<(String, u8)>>,
    next_slot: u8,
    item_name: String,
    items: &'a [Item],
    /// The routine's preconditions, checked at entry.
    requires: Vec<Clause>,
    /// True when every call site passes constants that satisfy those
    /// preconditions, so the entry assertion can be left out.
    requires_proved: bool,
    /// The routine's postconditions, checked at every return.
    ensures: Vec<Clause>,
}

impl<'a> Cx<'a> {
    fn stmts(&mut self, stmts: &[Stmt]) -> Result<(), CompileError> {
        self.scopes.push(Vec::new());
        for stmt in stmts {
            self.stmt(stmt)?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn stmt(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        match stmt {
            Stmt::Let(name, value, _line) => {
                self.expr_root(value)?;
                let slot = self.bind(name)?;
                self.emit(Op::StoreLocal, slot, 0, 0);
            }
            Stmt::Set(name, value, line) => {
                self.expr_root(value)?;
                let Some(slot) = self.resolve(name) else {
                    return e(*line, format!("'{}' is not defined", name));
                };
                self.emit(Op::StoreLocal, slot, 0, 0);
            }
            Stmt::Eval(expr, _line) => {
                self.expr_root(expr)?;
                match expr {
                    Expr::Call { recv, op, .. }
                        if caps::family_of(recv) == "policy"
                            && (op == "allow" || op == "reject") =>
                    {
                        self.emit(Op::PolicyStop, 0, 0, 0);
                    }
                    _ => {
                        self.emit(Op::Pop, 0, 0, 0);
                    }
                }
            }
            Stmt::If { cond, then, otherwise, .. } => {
                let to_else = self.jump_if_false(cond)?;
                self.stmts(then)?;
                if otherwise.is_empty() {
                    self.patch(to_else);
                } else {
                    let to_end = self.emit_wide(Op::Jmp, 0, 0);
                    self.patch(to_else);
                    self.stmts(otherwise)?;
                    self.patch(to_end);
                }
            }
            Stmt::ForEach { var, list, body, line } => {
                // Three hidden slots: the list, the index, the loop variable.
                self.expr(list)?;
                let list_slot = self.temp()?;
                self.emit(Op::StoreLocal, list_slot, 0, 0);
                let zero = self.push_const(Const::Num(0.0))?;
                self.emit_wide(Op::Const, 0, zero);
                let idx_slot = self.temp()?;
                self.emit(Op::StoreLocal, idx_slot, 0, 0);

                // The loop head carries its own source line, so an exhausted
                // budget points at the loop instead of at the routine.
                let head = self.code.len();
                self.emit_wide(Op::FuelTick, 0, *line);
                self.emit(Op::LoadLocal, list_slot, 0, 0);
                self.emit(Op::Len, 0, 0, 0);
                self.emit(Op::LoadLocal, idx_slot, 0, 0);
                self.emit(Op::CmpLe, 0, 0, 0);
                let to_end = self.emit_wide(Op::JmpIfTrue, 0, 0);

                self.scopes.push(Vec::new());
                let var_slot = self.bind(var)?;
                self.emit(Op::LoadLocal, list_slot, 0, 0);
                self.emit(Op::LoadLocal, idx_slot, 0, 0);
                self.emit(Op::GetIndex, 0, 0, 0);
                self.emit(Op::StoreLocal, var_slot, 0, 0);
                for s in body {
                    self.stmt(s)?;
                }
                self.scopes.pop();

                self.emit(Op::LoadLocal, idx_slot, 0, 0);
                let one = self.push_const(Const::Num(1.0))?;
                self.emit_wide(Op::Const, 0, one);
                self.emit(Op::Add, 0, 0, 0);
                self.emit(Op::StoreLocal, idx_slot, 0, 0);
                self.jump_back_to(head);
                self.patch(to_end);
            }
            Stmt::While { cond, body, line, bound } => {
                // A declared iteration ceiling is emitted, not trusted: the
                // counter lives in a hidden local and a trip past the bound
                // stops the run with E_KAZE_LOOP_BOUND instead of hanging.
                let counter = match bound {
                    Some(_) => Some(self.temp()?),
                    None => None,
                };
                if let Some(c) = counter {
                    let zero = self.push_const(Const::Num(0.0))?;
                    self.emit_wide(Op::Const, 0, zero);
                    self.emit(Op::StoreLocal, c, 0, 0);
                }
                let head = self.code.len();
                self.emit_wide(Op::FuelTick, 0, *line);
                let to_trap = match (counter, bound) {
                    (Some(c), Some(n)) => {
                        self.emit(Op::LoadLocal, c, 0, 0);
                        let n_idx = self.push_const(Const::Num(f64::from(*n)))?;
                        self.emit_wide(Op::Const, 0, n_idx);
                        self.emit(Op::CmpGe, 0, 0, 0);
                        Some(self.emit_wide(Op::JmpIfTrue, 0, 0))
                    }
                    _ => None,
                };
                let to_end = self.jump_if_false(cond)?;
                for s in body {
                    self.stmt(s)?;
                }
                if let Some(c) = counter {
                    self.emit(Op::LoadLocal, c, 0, 0);
                    let one = self.push_const(Const::Num(1.0))?;
                    self.emit_wide(Op::Const, 0, one);
                    self.emit(Op::Add, 0, 0, 0);
                    self.emit(Op::StoreLocal, c, 0, 0);
                }
                self.jump_back_to(head);
                match to_trap {
                    Some(trap) => {
                        // The normal exit has to step over the trap block.
                        let over = self.emit_wide(Op::Jmp, 0, 0);
                        self.patch(trap);
                        let n = bound.unwrap_or(0);
                        let msg = self.push_const(Const::Str(format!(
                            "E_KAZE_LOOP_BOUND: the loop at line {} ran past its declared bound of \
                             {} iterations",
                            line, n
                        )))?;
                        self.emit_wide(Op::Trap, 0, msg);
                        self.emit(Op::RetUnit, 0, 0, 0);
                        self.patch(over);
                        self.patch(to_end);
                    }
                    None => self.patch(to_end),
                }
            }
            Stmt::Try { body, otherwise, error_name, .. } => {
                // Reserve the error slot before the body is compiled, so the
                // handler target can name it; it is NOT visible in the body.
                let slot = if error_name.is_some() {
                    self.temp()?
                } else {
                    NO_ERROR_BINDING
                };
                let handler = self.emit_wide(Op::TryBegin, slot, 0);
                self.stmts(body)?;
                self.emit(Op::TryEnd, 0, 0, 0);
                let to_end = self.emit_wide(Op::Jmp, 0, 0);
                // The VM jumps here after unwinding a catchable failure.
                self.patch(handler);
                self.scopes.push(Vec::new());
                if let Some(name) = error_name {
                    if let Some(top) = self.scopes.last_mut() {
                        top.push((name.clone(), slot));
                    }
                }
                self.stmts(otherwise)?;
                self.scopes.pop();
                self.patch(to_end);
            }
            Stmt::Guard(cond, line) => {
                self.expr_root(cond)?;
                let held = self.emit_wide(Op::JmpIfTrue, 0, 0);
                let msg = self.push_const(Const::Str(format!(
                    "E_KAZE_GUARD: the invariant at line {} is false",
                    line
                )))?;
                self.emit_wide(Op::Trap, 0, msg);
                self.patch(held);
            }
            Stmt::Return(value, _line) => match value {
                Some(v) => {
                    // Bind `result` before the postconditions so an ensures
                    // clause can talk about the value being returned.
                    let slot = self.temp()?;
                    self.expr_root(v)?;
                    self.emit(Op::StoreLocal, slot, 0, 0);
                    self.exit_assertions(slot)?;
                    self.emit(Op::LoadLocal, slot, 0, 0);
                    self.emit(Op::Ret, 0, 0, 0);
                }
                None => {
                    self.emit(Op::RetUnit, 0, 0, 0);
                }
            },
        }
        Ok(())
    }

    /// Branch when `cond` is false. `a == b` and `a != b` compile to the
    /// fused compare-and-branch opcodes: one instruction fewer per decision and
    /// no boolean materialised on the stack.
    fn jump_if_false(&mut self, cond: &Expr) -> Result<usize, CompileError> {
        if let Expr::Bin(op @ (BinOp::Eq | BinOp::Ne), left, right) = cond {
            self.expr(left)?;
            self.expr(right)?;
            // Jump to the else-branch when the comparison does NOT hold.
            let fused = if *op == BinOp::Eq { Op::Jne } else { Op::Jeq };
            return Ok(self.emit_wide(fused, 0, 0));
        }
        self.expr_root(cond)?;
        Ok(self.emit_wide(Op::JmpIfFalse, 0, 0))
    }

    /// Emit the routine's preconditions as entry assertions.
    fn entry_assertions(&mut self) -> Result<(), CompileError> {
        // A routine whose every call site passes provable literals needs no
        // runtime check: the proof is done, so the assertion is dead code.
        if self.requires_proved {
            return Ok(());
        }
        let clauses = self.requires.clone();
        for clause in clauses {
            self.expr_root(&clause.expr)?;
            let held = self.emit_wide(Op::JmpIfTrue, 0, 0);
            let msg = self.push_const(Const::Str(format!(
                "E_KAZE_CONTRACT: the requires clause at line {} is false for these arguments",
                clause.line
            )))?;
            self.emit_wide(Op::Trap, 0, msg);
            self.patch(held);
        }
        Ok(())
    }

    /// Emit the routine's postconditions over a value already in `slot`.
    fn exit_assertions(&mut self, slot: u8) -> Result<(), CompileError> {
        let clauses = self.ensures.clone();
        for clause in clauses {
            self.scopes.push(vec![("result".to_string(), slot)]);
            self.expr_root(&clause.expr)?;
            self.scopes.pop();
            let held = self.emit_wide(Op::JmpIfTrue, 0, 0);
            let msg = self.push_const(Const::Str(format!(
                "E_KAZE_CONTRACT: the ensures clause at line {} is false for this result",
                clause.line
            )))?;
            self.emit_wide(Op::Trap, 0, msg);
            self.patch(held);
        }
        Ok(())
    }

    /// Compile one value-producing expression as a nested operand.
    fn expr(&mut self, expr: &Expr) -> Result<(), CompileError> {
        self.expr_inner(expr, false)
    }

    /// Compile the expression that produces a whole statement's value. Only
    /// here may a capability call carry an inline `otherwise` fallback: the VM's
    /// recovery handler needs an empty operand stack, and a nested operand has
    /// one below it by construction.
    fn expr_root(&mut self, expr: &Expr) -> Result<(), CompileError> {
        self.expr_inner(expr, true)
    }

    fn expr_inner(&mut self, expr: &Expr, root: bool) -> Result<(), CompileError> {
        match expr {
            Expr::Num(n, _) => {
                let idx = self.push_const(Const::Num(*n))?;
                self.emit_wide(Op::Const, 0, idx);
            }
            Expr::Str(s) => {
                let idx = self.push_const(Const::Str(s.clone()))?;
                self.emit_wide(Op::Const, 0, idx);
            }
            Expr::Bool(b) => {
                let idx = self.push_const(Const::Bool(*b))?;
                self.emit_wide(Op::Const, 0, idx);
            }
            Expr::List(items) => {
                if items.len() > 255 {
                    return e(0, "this list has more than 255 items");
                }
                for i in items {
                    self.expr(i)?;
                }
                self.emit(Op::NewArr, items.len() as u8, 0, 0);
            }
            Expr::Math { kind, args, line: _ } => {
                for (i, a) in args.iter().enumerate() {
                    self.expr(a)?;
                    // sin/cos read radians; an angle literal is written in
                    // degrees, so the conversion is folded into the literal (a
                    // constant multiply, with no runtime unit check).
                    let trig = matches!(kind, MathKind::Sin | MathKind::Cos) && i == 0;
                    if trig {
                        if let Expr::Num(_, unit) = a {
                            if unit.dim() == Dim::ANGLE {
                                self.emit(Op::DegToRad, 0, 0, 0);
                            }
                        }
                    }
                }
                let op = match kind {
                    MathKind::Vector => Op::VecNew,
                    MathKind::Distance => Op::VecDist,
                    MathKind::SlantDistance => Op::VecSlantDist,
                    MathKind::BearingOf => Op::VecBearing,
                    MathKind::BearingFrom => Op::VecBearingBetween,
                    MathKind::Sin => Op::Sin,
                    MathKind::Cos => Op::Cos,
                    MathKind::Atan2 => Op::Atan2,
                    MathKind::Sqrt => Op::Sqrt,
                    MathKind::ToRadians => Op::DegToRad,
                    MathKind::ToDegrees => Op::RadToDeg,
                };
                let a = if *kind == MathKind::Vector { args.len() as u8 } else { 0 };
                self.emit(op, a, 0, 0);
            }
            Expr::LinkLost { after_ms, line } => {
                if !after_ms.is_finite() || *after_ms < 1.0 || *after_ms > u32::MAX as f64 {
                    return e(
                        *line,
                        "link lost needs at least 1 millisecond (and less than about 49 days); \
                         a shorter window cannot be told apart from a healthy link",
                    );
                }
                self.emit_wide(Op::LinkLost, 0, *after_ms as u32);
            }
            Expr::Name(name) => {
                match self.resolve(name) {
                    Some(slot) => {
                        self.emit(Op::LoadLocal, slot, 0, 0);
                    }
                    None => {
                        return e(0, format!("'{}' is not defined in '{}'", name, self.item_name))
                    }
                }
            }
            Expr::Ambient { field, family } => {
                let idx = self.push_const(Const::Str(field.clone()))?;
                self.emit_wide(Op::Const, 0, idx);
                let cap = self.push_cap(family, "telemetry", 1)?;
                self.emit_wide(Op::CallCap, 1, cap);
            }
            Expr::Call { recv, op, args, line } => {
                if args.len() > 255 {
                    return e(*line, format!("{}.{} takes too many values", recv, op));
                }
                for a in args {
                    self.expr(a)?;
                }
                let family = caps::family_of(recv);
                let cap = self.push_cap(family, op, args.len() as u8)?;
                self.emit_wide(Op::CallCap, args.len() as u8, cap);
            }
            Expr::CallRoutine { name, args, line } => {
                if args.len() > 255 {
                    return e(*line, format!("'{}' takes too many values", name));
                }
                let target_idx = match self.items.iter().position(|it| it.name == *name) {
                    Some(i) => i,
                    None => return e(*line, format!("'{}' is not defined in this module", name)),
                };
                for a in args {
                    self.expr(a)?;
                }
                self.emit_wide(Op::CallRoutine, args.len() as u8, target_idx as u32);
            }
            Expr::RecordInit { name, fields, line } => {
                let target_item = match self.items.iter().find(|it| it.name == *name) {
                    Some(it) => it,
                    None => return e(*line, format!("record '{}' is not defined in this module", name)),
                };
                let rec_fields = match &target_item.kind {
                    ItemKind::Record(ref f) => f,
                    _ => return e(*line, format!("'{}' is not a record", name)),
                };
                for rf in rec_fields {
                    let (_, expr) = match fields.iter().find(|(fname, _)| fname == &rf.name) {
                        Some(p) => p,
                        None => return e(*line, format!("missing field '{}' in instantiation of record '{}'", rf.name, name)),
                    };
                    let k_idx = self.push_const(Const::Str(rf.name.clone()))?;
                    self.emit_wide(Op::Const, 0, k_idx);
                    self.expr(expr)?;
                }
                self.emit(Op::NewMap, rec_fields.len() as u8, 0, 0);
            }
            Expr::Not(inner) => {
                self.expr(inner)?;
                self.emit(Op::Not, 0, 0, 0);
            }
            Expr::Len(inner) => {
                self.expr(inner)?;
                self.emit(Op::Len, 0, 0, 0);
            }
            Expr::Index(list, idx) => {
                self.expr(list)?;
                self.expr(idx)?;
                self.emit(Op::GetIndex, 0, 0, 0);
            }
            Expr::Field { base, field, line } => {
                match self.resolve(base) {
                    Some(slot) => {
                        self.emit(Op::LoadLocal, slot, 0, 0);
                    }
                    None => return e(*line, format!("'{}' is not a value here", base)),
                }
                let idx = self.push_const(Const::Str(field.clone()))?;
                self.emit_wide(Op::Const, 0, idx);
                self.emit(Op::GetField, 0, 0, 0);
            }
            Expr::Fallback { inner, fallback, line } => {
                self.fallback(inner, fallback, *line, root)?;
            }
            Expr::Bin(op, left, right) => match op {
                BinOp::And => {
                    self.expr(left)?;
                    let to_false = self.emit_wide(Op::JmpIfFalse, 0, 0);
                    self.expr(right)?;
                    let to_end = self.emit_wide(Op::Jmp, 0, 0);
                    self.patch(to_false);
                    let f = self.push_const(Const::Bool(false))?;
                    self.emit_wide(Op::Const, 0, f);
                    self.patch(to_end);
                }
                BinOp::Or => {
                    self.expr(left)?;
                    let to_rhs = self.emit_wide(Op::JmpIfFalse, 0, 0);
                    let t = self.push_const(Const::Bool(true))?;
                    self.emit_wide(Op::Const, 0, t);
                    let to_end = self.emit_wide(Op::Jmp, 0, 0);
                    self.patch(to_rhs);
                    self.expr(right)?;
                    self.patch(to_end);
                }
                _ => {
                    self.expr(left)?;
                    self.expr(right)?;
                    let opcode = match op {
                        BinOp::Add => Op::Add,
                        BinOp::Sub => Op::Sub,
                        BinOp::Mul => Op::Mul,
                        BinOp::Div => Op::Div,
                        BinOp::Mod => Op::Mod,
                        BinOp::Lt => Op::CmpLt,
                        BinOp::Le => Op::CmpLe,
                        BinOp::Gt => Op::CmpGt,
                        BinOp::Ge => Op::CmpGe,
                        BinOp::Eq => Op::CmpEq,
                        BinOp::Ne => Op::CmpNe,
                        BinOp::And | BinOp::Or => {
                            return e(0, "internal: And/Or must be compiled as branches")
                        }
                    };
                    self.emit(opcode, 0, 0, 0);
                }
            },
        }
        Ok(())
    }

    /// Lower `<inner> otherwise <fallback>`.
    ///
    /// Three real lowerings, all with the opcodes that already exist:
    ///   * `a / b` and `a % b`  — test the divisor, then divide
    ///   * `xs[i]`              — test `0 <= i < length of xs`, then index
    ///   * `cap.op(..)`         — a `TRY` frame around the call
    ///
    /// Everything else is proved safe by the checker (a non-zero literal
    /// divisor, a constant index inside a literal), so the fallback there is
    /// unreachable and simply not emitted.
    fn fallback(
        &mut self,
        inner: &Expr,
        fallback: &Expr,
        line: u32,
        root: bool,
    ) -> Result<(), CompileError> {
        match inner {
            Expr::Bin(op @ (BinOp::Div | BinOp::Mod), left, right) => {
                // A literal divisor needs no test: the parser already knows it.
                if matches!(right.as_ref(), Expr::Num(n, _) if *n != 0.0) {
                    self.expr(inner)?;
                    return Ok(());
                }
                let divisor = self.temp()?;
                self.expr(right)?;
                self.emit(Op::StoreLocal, divisor, 0, 0);
                self.emit(Op::LoadLocal, divisor, 0, 0);
                let zero = self.push_const(Const::Num(0.0))?;
                self.emit_wide(Op::Const, 0, zero);
                let to_fallback = self.emit_wide(Op::Jeq, 0, 0);
                self.expr(left)?;
                self.emit(Op::LoadLocal, divisor, 0, 0);
                self.emit(if *op == BinOp::Div { Op::Div } else { Op::Mod }, 0, 0, 0);
                let to_end = self.emit_wide(Op::Jmp, 0, 0);
                self.patch(to_fallback);
                self.expr(fallback)?;
                self.patch(to_end);
            }
            Expr::Index(collection, index) => {
                let coll_slot = self.temp()?;
                let idx_slot = self.temp()?;
                self.expr(collection)?;
                self.emit(Op::StoreLocal, coll_slot, 0, 0);
                self.expr(index)?;
                self.emit(Op::StoreLocal, idx_slot, 0, 0);

                self.emit(Op::LoadLocal, idx_slot, 0, 0);
                let zero = self.push_const(Const::Num(0.0))?;
                self.emit_wide(Op::Const, 0, zero);
                self.emit(Op::CmpLt, 0, 0, 0);
                let too_low = self.emit_wide(Op::JmpIfTrue, 0, 0);

                self.emit(Op::LoadLocal, idx_slot, 0, 0);
                self.emit(Op::LoadLocal, coll_slot, 0, 0);
                self.emit(Op::Len, 0, 0, 0);
                self.emit(Op::CmpGe, 0, 0, 0);
                let too_high = self.emit_wide(Op::JmpIfTrue, 0, 0);

                self.emit(Op::LoadLocal, coll_slot, 0, 0);
                self.emit(Op::LoadLocal, idx_slot, 0, 0);
                self.emit(Op::GetIndex, 0, 0, 0);
                let to_end = self.emit_wide(Op::Jmp, 0, 0);

                self.patch(too_low);
                self.patch(too_high);
                self.expr(fallback)?;
                self.patch(to_end);
            }
            Expr::Call { .. } | Expr::Ambient { .. } => {
                if !root {
                    return e(
                        line,
                        "a capability fallback must be the whole value of its statement; give it \
                         a 'let' of its own (or wrap the statement in 'try:') so the recovery \
                         handler starts with an empty stack",
                    );
                }
                let handler = self.emit_wide(Op::TryBegin, NO_ERROR_BINDING, 0);
                self.expr(inner)?;
                self.emit(Op::TryEnd, 0, 0, 0);
                let to_end = self.emit_wide(Op::Jmp, 0, 0);
                self.patch(handler);
                self.expr(fallback)?;
                self.patch(to_end);
            }
            other => {
                // The checker proved this safe; the fallback cannot run.
                self.expr(other)?;
            }
        }
        Ok(())
    }

    // ── emission ─────────────────────────────────────────────────────
    fn emit(&mut self, op: Op, a: u8, b: u8, c: u8) -> usize {
        let at = self.code.len();
        self.code.push(word(op, a, b, c));
        at
    }

    fn emit_wide(&mut self, op: Op, a: u8, imm: u32) -> usize {
        let at = self.code.len();
        self.code.push(word(op, a, 0, 0));
        self.code.push(imm);
        at
    }

    /// Point a forward jump emitted at `at` at the current end of code.
    fn patch(&mut self, at: usize) {
        let rel = self.code.len() as i64 - (at as i64 + 2);
        if let Some(slot) = self.code.get_mut(at + 1) {
            *slot = rel as u32;
        }
    }

    /// Emit a backward jump to `head`.
    fn jump_back_to(&mut self, head: usize) {
        let at = self.code.len();
        self.code.push(word(Op::Jmp, 0, 0, 0));
        let rel = head as i64 - (at as i64 + 2);
        self.code.push(rel as u32);
    }

    fn push_const(&mut self, c: Const) -> Result<u32, CompileError> {
        for (i, existing) in self.consts.iter().enumerate() {
            let same = match (existing, &c) {
                (Const::Num(a), Const::Num(b)) => a.to_bits() == b.to_bits(),
                (Const::Str(a), Const::Str(b)) => a == b,
                (Const::Bool(a), Const::Bool(b)) => a == b,
                _ => false,
            };
            if same {
                return Ok(i as u32);
            }
        }
        self.consts.push(c);
        Ok(self.consts.len() as u32 - 1)
    }

    fn push_cap(&mut self, family: &str, op: &str, argc: u8) -> Result<u32, CompileError> {
        for (i, existing) in self.caps.iter().enumerate() {
            if existing.family == family && existing.op == op && existing.argc == argc {
                return Ok(i as u32);
            }
        }
        self.caps.push(CapRef { family: family.to_string(), op: op.to_string(), argc });
        Ok(self.caps.len() as u32 - 1)
    }

    // ── slots ────────────────────────────────────────────────────────
    fn bind(&mut self, name: &str) -> Result<u8, CompileError> {
        let slot = self.temp()?;
        if let Some(top) = self.scopes.last_mut() {
            top.push((name.to_string(), slot));
        }
        Ok(slot)
    }

    fn temp(&mut self) -> Result<u8, CompileError> {
        if self.next_slot == MAX_SLOTS {
            return e(0, "this block uses more than 255 values; split it into actions");
        }
        let slot = self.next_slot;
        self.next_slot += 1;
        Ok(slot)
    }

    fn resolve(&self, name: &str) -> Option<u8> {
        for scope in self.scopes.iter().rev() {
            for (n, slot) in scope.iter().rev() {
                if n == name {
                    return Some(*slot);
                }
            }
        }
        None
    }
}

/// Does every call to `item` in this program pass arguments that provably
/// satisfy its `requires` clauses? If so the entry assertions are dead code and
/// the compiler drops them, which is what the roadmap asks for: a provable call
/// compiles with zero runtime contract checks. A routine the host can start
/// (entry or hook) has no call site, so its assertions stay.
fn requires_proved_everywhere(items: &[Item], item: &Item) -> bool {
    if item.contracts.requires.is_empty() {
        return true;
    }
    let mut sites = 0usize;
    let mut proved = true;
    for caller in items {
        walk_calls(&caller.body, &item.name, &mut |args| {
            sites += 1;
            if !call_proves_requires(item, args) {
                proved = false;
            }
        });
    }
    sites > 0 && proved
}

/// Visit every `CallRoutine` in a statement tree.
fn walk_calls(stmts: &[Stmt], callee: &str, f: &mut impl FnMut(&[Expr])) {
    for stmt in stmts {
        match stmt {
            Stmt::Let(_, v, _) | Stmt::Set(_, v, _) | Stmt::Eval(v, _) | Stmt::Guard(v, _) => {
                walk_calls_expr(v, callee, f)
            }
            Stmt::Return(Some(v), _) => walk_calls_expr(v, callee, f),
            Stmt::Return(None, _) => {}
            Stmt::If { cond, then, otherwise, .. } => {
                walk_calls_expr(cond, callee, f);
                walk_calls(then, callee, f);
                walk_calls(otherwise, callee, f);
            }
            Stmt::ForEach { list, body, .. } => {
                walk_calls_expr(list, callee, f);
                walk_calls(body, callee, f);
            }
            Stmt::While { cond, body, .. } => {
                walk_calls_expr(cond, callee, f);
                walk_calls(body, callee, f);
            }
            Stmt::Try { body, otherwise, .. } => {
                walk_calls(try_body(body), callee, f);
                walk_calls(otherwise, callee, f);
            }
        }
    }
}

fn try_body(body: &[Stmt]) -> &[Stmt] {
    body
}

fn walk_calls_expr(expr: &Expr, callee: &str, f: &mut impl FnMut(&[Expr])) {
    match expr {
        Expr::CallRoutine { name, args, .. } => {
            if name == callee {
                f(args);
            }
            for a in args {
                walk_calls_expr(a, callee, f);
            }
        }
        Expr::Call { args, .. } | Expr::Math { args, .. } | Expr::List(args) => {
            for a in args {
                walk_calls_expr(a, callee, f);
            }
        }
        Expr::RecordInit { fields, .. } => {
            for (_, v) in fields {
                walk_calls_expr(v, callee, f);
            }
        }
        Expr::Bin(_, l, r) => {
            walk_calls_expr(l, callee, f);
            walk_calls_expr(r, callee, f);
        }
        Expr::Not(inner) | Expr::Len(inner) => walk_calls_expr(inner, callee, f),
        Expr::Index(c, i) => {
            walk_calls_expr(c, callee, f);
            walk_calls_expr(i, callee, f);
        }
        Expr::Fallback { inner, fallback, .. } => {
            walk_calls_expr(inner, callee, f);
            walk_calls_expr(fallback, callee, f);
        }
        _ => {}
    }
}

/// Every argument is a numeric literal and every clause holds for them.
fn call_proves_requires(callee: &Item, args: &[Expr]) -> bool {
    let mut env: Vec<(String, f64)> = Vec::new();
    for (param, arg) in callee.params.iter().zip(args) {
        match arg {
            Expr::Num(n, _) => env.push((param.clone(), *n)),
            _ => return false,
        }
    }
    if env.len() != callee.params.len() {
        return false;
    }
    for clause in &callee.contracts.requires {
        match eval_const_clause(&clause.expr, &env) {
            Some(true) => {}
            _ => return false,
        }
    }
    true
}

fn eval_const_clause(expr: &Expr, env: &[(String, f64)]) -> Option<bool> {
    let num = |e: &Expr| eval_const_number(e, env);
    match expr {
        Expr::Bool(b) => Some(*b),
        Expr::Not(inner) => eval_const_clause(inner, env).map(|v| !v),
        Expr::Bin(op @ (BinOp::And | BinOp::Or), l, r) => {
            let a = eval_const_clause(l, env)?;
            let b = eval_const_clause(r, env)?;
            Some(if *op == BinOp::And { a && b } else { a || b })
        }
        Expr::Bin(op, l, r) => {
            let a = num(l)?;
            let b = num(r)?;
            Some(match op {
                BinOp::Lt => a < b,
                BinOp::Le => a <= b,
                BinOp::Gt => a > b,
                BinOp::Ge => a >= b,
                BinOp::Eq => a == b,
                BinOp::Ne => a != b,
                _ => return None,
            })
        }
        _ => None,
    }
}

fn eval_const_number(expr: &Expr, env: &[(String, f64)]) -> Option<f64> {
    match expr {
        Expr::Num(n, _) => Some(*n),
        Expr::Name(n) => env.iter().find(|(k, _)| k == n).map(|(_, v)| *v),
        Expr::Bin(op, l, r) => {
            let a = eval_const_number(l, env)?;
            let b = eval_const_number(r, env)?;
            match op {
                BinOp::Add => Some(a + b),
                BinOp::Sub => Some(a - b),
                BinOp::Mul => Some(a * b),
                BinOp::Div => if b == 0.0 { None } else { Some(a / b) },
                BinOp::Mod => if b == 0.0 { None } else { Some(a % b) },
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isa::opcode;
    use crate::parser::parse;

    fn compiles(src: &str) -> Module {
        compile(&parse(src).expect("parse")).expect("compile")
    }

    #[test]
    fn the_patrol_example_compiles_to_a_call_per_verb() {
        let src = "needs: drone, log\n\naction patrol(points):\n    log \"go\"\n    arm drone\n    takeoff to 15 meters\n    return to launch\n";
        let m = compiles(src);
        let item = m.item("patrol").expect("patrol");
        let caps: Vec<String> = m.caps.iter().map(|c| format!("{}.{}", c.family, c.op)).collect();
        assert!(caps.contains(&"drone.arm".to_string()), "{:?}", caps);
        assert!(caps.contains(&"drone.takeoff".to_string()), "{:?}", caps);
        assert!(caps.contains(&"drone.rtl".to_string()), "{:?}", caps);
        assert!(caps.contains(&"log.info".to_string()), "{:?}", caps);
        assert_eq!(item.params, vec!["points".to_string()]);
    }

    #[test]
    fn compilation_is_byte_identical() {
        let src = "needs: rover, lidar, log\n\nevery 100ms:\n    let front = lidar.front_distance()\n    if front < 0.5 meters:\n        stop rover\n    otherwise:\n        drive forward at 1.2 m/s\n";
        let a = crate::isa::serialize(&compiles(src));
        let b = crate::isa::serialize(&compiles(src));
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn equality_branches_fuse_into_one_instruction() {
        let m = compiles(
            "needs: log\naction a(x, y):\n    if x == y:\n        log \"same\"\n    if x != y:\n        log \"different\"\n",
        );
        let item = m.item("a").unwrap();
        let ops: Vec<u8> = item.code.iter().map(|w| opcode(*w)).collect();
        assert!(ops.contains(&(Op::Jne as u8)), "== should branch on JNE: {:?}", ops);
        assert!(ops.contains(&(Op::Jeq as u8)), "!= should branch on JEQ: {:?}", ops);
        assert!(
            !ops.contains(&(Op::CmpEq as u8)),
            "a fused branch must not also materialise a boolean: {:?}",
            ops
        );
    }

    #[test]
    fn constants_are_deduplicated() {
        let m = compiles("needs: log\naction a():\n    log \"same\"\n    log \"same\"\n");
        assert_eq!(m.consts.len(), 1);
    }

    #[test]
    fn a_loop_emits_a_fuel_checkpoint_and_a_back_edge() {
        let m = compiles("needs: log\naction a(items):\n    for each i in items:\n        log \"x\"\n");
        let item = m.item("a").unwrap();
        let ops: Vec<u8> = item.code.iter().map(|w| opcode(*w)).collect();
        assert!(ops.contains(&(Op::FuelTick as u8)));
        assert!(ops.contains(&(Op::Jmp as u8)));
    }

    #[test]
    fn a_while_loop_is_bounded_by_fuel_not_by_the_compiler() {
        let m = compiles("needs: log\naction a():\n    while true:\n        log \"spin\"\n");
        assert!(m.item("a").unwrap().code.iter().any(|w| opcode(*w) == Op::FuelTick as u8));
    }
}
