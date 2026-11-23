// sfs/strict.rs -- the Phase 2/3/4 strictness pass.
//
// The checker in checker.rs answers "is this program meaningful?". This pass
// answers the harder question the roadmap asks: "is this program *provably*
// free of the failures that are not allowed to happen?".
//
// Four obligations, all decided before a single instruction runs:
//
//   1. Fallibility. A division, a modulo, a dynamic index or a value read from
//      a device that can go silent must say what to do when it fails, either
//      with an inline "otherwise <fallback>" or by being provably safe (a
//      non-zero literal divisor, an index an enclosing guard has already
//      bounded).
//   2. Termination. Every while loop declares an iteration ceiling; every
//      routine on a call cycle declares a recursion ceiling.
//   3. Dimensions. Every arithmetic operator is checked against the dimensional
//      vector of its operands (see dim_result in checker.rs).
//   4. Contracts and attenuation. requires/ensures are boolean, untrusted
//      parameters are validated by a precondition, and a routine can never
//      declare -- or use -- a capability its caller does not hold.
//
// The pass is deliberately conservative: when it cannot prove safety it asks
// for the fallback. That is the point of a strict language; the cost is paid
// once, by the person writing the script, instead of at 3 a.m. by the pilot.

use std::collections::HashMap;

use crate::ast::*;
use crate::caps;
use crate::checker::{at, e, ec, infer, CheckError, Ty};

/// Facts the pass has proved about the expressions in scope at this point.
///
/// Keys are expression shapes, not just names, so a guard on "i + 1" proves the
/// index "src[i + 1]" and a guard on "pos" proves "tokens[pos]". That is what
/// makes ordinary scanning code check without a fallback on every line.
#[derive(Clone, Default, Debug)]
struct Facts {
    /// Keys of expressions proved to be >= 0.
    nonneg: Vec<String>,
    /// index key -> collection key, proved by a guard such as "i < length of xs".
    lt_len: Vec<(String, String)>,
    /// collection key -> k, proved by a guard such as "length of xs > k".
    len_gt: Vec<(String, f64)>,
    /// name -> (collection key, k), for "let n = length of xs - k".
    len_alias: Vec<(String, String, f64)>,
    /// name -> length, for a value whose builder promised its size with
    /// "ensures length of result == k".
    known_len: Vec<(String, usize)>,
}

impl Facts {
    fn is_nonneg(&self, key: &str) -> bool {
        self.nonneg.iter().any(|n| n == key)
    }

    fn bounded_by(&self, key: &str, coll: &str) -> bool {
        self.lt_len.iter().any(|(i, c)| i == key && c == coll)
    }

    /// Is the collection proved longer than `k`? A fact `len > n` with
    /// `n >= k` implies it, so a proof of a stronger bound counts.
    fn longer_than(&self, coll: &str, k: f64) -> bool {
        self.len_gt.iter().any(|(c, n)| c == coll && *n >= k)
    }

    fn alias_of(&self, name: &str) -> Option<(&str, f64)> {
        self.len_alias
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, c, k)| (c.as_str(), *k))
    }

    fn forget(&mut self, name: &str) {
        self.nonneg.retain(|n| n != name);
        self.lt_len.retain(|(n, _)| n != name);
        self.len_alias.retain(|(n, _, _)| n != name);
        self.known_len.retain(|(n, _)| n != name);
    }

    fn length_of(&self, name: &str) -> Option<usize> {
        self.known_len.iter().find(|(n, _)| n == name).map(|(_, k)| *k)
    }
}

/// Where the pass is: which routine, whether it is inside a try, what that
/// routine has declared it needs.
#[derive(Clone, Debug)]
struct Ctx {
    facts: Facts,
    in_try: bool,
    /// Empty means "inherits the module's needs", which is what every routine
    /// written before Phase 5 does.
    routine_needs: Vec<String>,
    item: String,
    /// The statement being walked, so an expression-level diagnostic still
    /// points at a real source line.
    line: u32,
    /// The routine's parameter names.
    params: Vec<String>,
    /// The parameters declared with a record type: a field read on one of
    /// these is checked against the record, not against a maybe-missing field.
    record_params: Vec<String>,
}

/// A short, stable key for a collection expression, so a guard written against
/// "tokens" matches the index "tokens[pos]".
fn coll_key(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(n) => Some(n.clone()),
        Expr::Field { base, field, .. } => Some(format!("{}.{}", base, field)),
        Expr::Ambient { field, .. } => Some(field.clone()),
        _ => None,
    }
}

/// The same, for an index expression: "i", "i + 1", "number 3".
fn index_key(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(n) => Some(n.clone()),
        Expr::Num(n, _) => Some(format!("num:{}", n)),
        Expr::Bin(op, l, r) => {
            let a = index_key(l)?;
            let b = index_key(r)?;
            Some(format!("{} {} {}", a, op.symbol(), b))
        }
        _ => None,
    }
}

fn literal_len(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::List(items) => Some(items.len()),
        Expr::Str(s) => Some(s.chars().count()),
        // A point always has exactly three components.
        Expr::Math { kind: MathKind::Vector, args, .. } => Some(args.len()),
        _ => None,
    }
}

/// What the pass knows about one index expression.
enum IndexProof {
    Safe,
    OutOfBounds,
    Unproven,
}

fn prove_index(facts: &Facts, coll: &Expr, idx: &Expr) -> IndexProof {
    let key = coll_key(coll);
    let literal = literal_len(coll);
    match idx {
        Expr::Num(n, _) => {
            if *n < 0.0 || n.fract() != 0.0 {
                return IndexProof::OutOfBounds;
            }
            let i = *n as usize;
            // The length is known from a literal, or from the builder's
            // postcondition ("ensures length of result == 2").
            let known = literal.or_else(|| match coll {
                Expr::Name(name) => facts.length_of(name),
                _ => None,
            });
            if known.is_none() {
                // "length of xs > 3" proves xs[3] without knowing its size.
                if let Some(name) = coll_key(coll) {
                    if facts.longer_than(&name, i as f64) {
                        return IndexProof::Safe;
                    }
                }
            }
            match known {
                Some(len) if i < len => IndexProof::Safe,
                Some(_) => IndexProof::OutOfBounds,
                None => IndexProof::Unproven,
            }
        }
        // "length of xs - k" is a valid position only while xs is longer than k.
        Expr::Bin(BinOp::Sub, left, right) => {
            if let (Expr::Len(inner), Expr::Num(k, _)) = (&**left, &**right) {
                if let Some(inner_key) = coll_key(inner) {
                    if *k >= 0.0 && facts.longer_than(&inner_key, *k) {
                        return IndexProof::Safe;
                    }
                }
            }
            dynamic_index(facts, idx, key.as_deref())
        }
        _ => dynamic_index(facts, idx, key.as_deref()),
    }
}

fn dynamic_index(facts: &Facts, idx: &Expr, coll: Option<&str>) -> IndexProof {
    let Some(idx_key) = index_key(idx) else {
        return IndexProof::Unproven;
    };
    if !nonneg_of(facts, idx) {
        return IndexProof::Unproven;
    }
    match coll {
        Some(coll) if facts.bounded_by(&idx_key, coll) => IndexProof::Safe,
        _ => IndexProof::Unproven,
    }
}

/// Is this expression proved >= 0? A proved non-negative name plus a
/// non-negative literal keeps that property through addition.
fn nonneg_of(facts: &Facts, expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => facts.is_nonneg(n),
        Expr::Num(n, _) => *n >= 0.0,
        Expr::Bin(BinOp::Add, l, r) => {
            (nonneg_of(facts, l) && matches!(&**r, Expr::Num(k, _) if *k >= 0.0))
                || (nonneg_of(facts, r) && matches!(&**l, Expr::Num(k, _) if *k >= 0.0))
        }
        Expr::Bin(BinOp::Sub, l, _) => matches!(&**l, Expr::Num(n, _) if *n >= 0.0),
        Expr::Len(_) => true,
        _ => false,
    }
}

/// Add the facts a true condition proves.
fn facts_from_cond(cond: &Expr, facts: &mut Facts) {
    match cond {
        Expr::Bin(BinOp::And, a, b) => {
            facts_from_cond(a, facts);
            facts_from_cond(b, facts);
        }
        Expr::Bin(BinOp::Lt, left, right) => {
            // i < length of xs, i < n where n is an alias of a length,
            // and k < length of xs (which proves xs holds more than k).
            if let (Some(ik), Some(ck)) = (index_key(left), coll_key_through_len(right, facts)) {
                push_lt_len(facts, &ik, &ck);
            }
            if let (Expr::Num(k, _), Some(ck)) = (&**left, coll_key_through_len(right, facts)) {
                push_len_gt(facts, &ck, *k);
            }
            nonneg_from_cond(cond, facts);
        }
        Expr::Bin(BinOp::Gt, left, right) => {
            if let (Some(ck), Some(ik)) = (coll_key_through_len(left, facts), index_key(right)) {
                push_lt_len(facts, &ik, &ck);
            }
            if let (Some(ck), Expr::Num(k, _)) = (coll_key_through_len(left, facts), &**right) {
                push_len_gt(facts, &ck, *k);
            }
            nonneg_from_cond(cond, facts);
        }
        Expr::Bin(BinOp::Ge, left, right) => {
            // length of xs >= k is the same as "longer than k - 1"
            if let (Some(ck), Expr::Num(k, _)) = (coll_key_through_len(left, facts), &**right) {
                push_len_gt(facts, &ck, *k - 1.0);
            }
            nonneg_from_cond(cond, facts);
        }
        _ => nonneg_from_cond(cond, facts),
    }
}

fn push_lt_len(facts: &mut Facts, idx: &str, coll: &str) {
    if !facts.bounded_by(idx, coll) {
        facts.lt_len.push((idx.to_string(), coll.to_string()));
    }
}

fn push_len_gt(facts: &mut Facts, coll: &str, k: f64) {
    if !facts.len_gt.iter().any(|(c, n)| c == coll && *n == k) {
        facts.len_gt.push((coll.to_string(), k));
    }
}

/// "length of xs", or a name that aliases it (with the subtracted constant kept
/// so "n = length of xs - 1" still proves "i < n" implies "i < length of xs").
fn coll_key_through_len(expr: &Expr, facts: &Facts) -> Option<String> {
    match expr {
        Expr::Len(inner) => coll_key(inner),
        Expr::Name(n) => facts.alias_of(n).and_then(|(c, k)| if k >= 0.0 { Some(c.to_string()) } else { None }),
        Expr::Bin(BinOp::Sub, left, right) => match (&**left, &**right) {
            (Expr::Len(inner), Expr::Num(k, _)) if *k >= 0.0 => coll_key(inner),
            _ => None,
        },
        _ => None,
    }
}

fn nonneg_from_cond(cond: &Expr, facts: &mut Facts) {
    if let Expr::Bin(op, left, right) = cond {
        if let (Some(key), Expr::Num(n, _)) = (index_key(left), &**right) {
            let ok = match op {
                BinOp::Ge => *n >= 0.0,
                BinOp::Gt => *n >= -1.0,
                _ => false,
            };
            if ok && !facts.is_nonneg(&key) {
                facts.nonneg.push(key);
            }
        }
    }
}

/// The size a routine promises for its result, read from
/// "ensures length of result == k".
fn promised_length(callee: &Item) -> Option<usize> {
    for clause in &callee.contracts.ensures {
        if let Expr::Bin(BinOp::Eq, left, right) = &clause.expr {
            if let (Expr::Len(inner), Expr::Num(k, _)) = (&**left, &**right) {
                if matches!(&**inner, Expr::Name(n) if n == "result") && *k >= 0.0 {
                    return Some(*k as usize);
                }
            }
        }
    }
    None
}

/// When the assigned expression is itself a proven shape ("pos + 1", a name
/// that already carries a bound, a call whose postcondition bounds it), the
/// target name inherits that proof. Sound because the value is the same value.
fn copy_facts_from_value(
    target: &str,
    value: &Expr,
    saved: &Facts,
    facts: &mut Facts,
) {
    // "set i to i + 1" has to see what was known about i before the assignment
    // cleared it, so the caller passes the pre-assignment snapshot.
    let Some(key) = index_key(value) else { return };
    if nonneg_of(saved, value) && !facts.is_nonneg(target) {
        facts.nonneg.push(target.to_string());
    }
    for (idx, coll) in saved.lt_len.iter() {
        if *idx == key && !facts.bounded_by(target, coll) {
            facts.lt_len.push((target.to_string(), coll.clone()));
        }
    }
}

/// The facts a callee's postconditions establish about its result:
/// "ensures result >= 0" and "ensures result < (length of xs)".
fn postcondition_facts(callee: &Item) -> (bool, Vec<String>) {
    let mut nonneg = false;
    let mut bounded: Vec<String> = Vec::new();
    for clause in &callee.contracts.ensures {
        if let Expr::Bin(op, left, right) = &clause.expr {
            match op {
                BinOp::Ge => {
                    if matches!(&**left, Expr::Name(n) if n == "result")
                        && matches!(&**right, Expr::Num(k, _) if *k >= 0.0)
                    {
                        nonneg = true;
                    }
                }
                BinOp::Lt => {
                    if matches!(&**left, Expr::Name(n) if n == "result") {
                        if let Some(key) = coll_key_through_len_plain(right) {
                            bounded.push(key);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    (nonneg, bounded)
}

fn coll_key_through_len_plain(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Len(inner) => coll_key(inner),
        _ => None,
    }
}

fn facts_from_let(name: &str, value: &Expr, facts: &mut Facts) {
    let saved = facts.clone();
    facts.forget(name);
    copy_facts_from_value(name, value, &saved, facts);
    match value {
        Expr::Num(n, _) if *n >= 0.0 => facts.nonneg.push(name.to_string()),
        Expr::Len(_) => {
            facts.nonneg.push(name.to_string());
            if let Some(key) = coll_key_through_len(value, facts) {
                facts.len_alias.push((name.to_string(), key, 0.0));
            }
        }
        Expr::Bin(BinOp::Sub, left, right) => {
            if let (Expr::Len(inner), Expr::Num(k, _)) = (&**left, &**right) {
                if let Some(key) = coll_key(inner) {
                    facts.len_alias.push((name.to_string(), key, *k));
                }
            }
        }
        // "let p = vector(x, y, z)" gives p a known width of three.
        Expr::Math { kind: MathKind::Vector, args, .. } => {
            if !args.is_empty() {
                facts.known_len.push((name.to_string(), args.len()));
            }
        }
        _ => {}
    }
}

/// Does the routine declare that it needs this capability family?
fn declares(needs: &[String], family: &str) -> bool {
    needs.iter().any(|n| caps::family_of(n.split('.').next().unwrap_or(n)) == family)
}

/// A value read from a device that can be silent must say what to do when it
/// is. Pure computations (policy detectors) and effector verbs are exempt.
fn needs_fallback(family: &str, op: &str) -> bool {
    match family {
        // Every sensor read returns a number the script will act on.
        "sensor" => true,
        // Device reads return a value; effector writes (relay, pulse, duty) are
        // statements whose refusal is already fatal and loud.
        "device" => op == "read_gpio" || op == "read",
        "memory" => op == "recall",
        _ => false,
    }
}

pub fn check(program: &Program, declared: &[&str], errors: &mut Vec<CheckError>) {
    check_recursion(program, errors);
    for item in &program.items {
        let before = errors.len();
        let ctx = Ctx {
            facts: Facts::default(),
            in_try: false,
            routine_needs: item.contracts.routine_needs.clone(),
            item: item.name.clone(),
            line: item.line,
            params: item.params.clone(),
            record_params: item
                .params
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    // The record must actually be declared in this module.
                    matches!(
                        item.contracts.param_types.get(*i).and_then(|t| t.as_ref()),
                        Some(TypeRef::Record(n))
                            if program
                                .items
                                .iter()
                                .any(|it| it.name == *n && matches!(it.kind, ItemKind::Record(_)))
                    )
                })
                .map(|(_, p)| p.clone())
                .collect(),
        };
        // A routine may not hold a capability the module never declared.
        for need in &item.contracts.routine_needs {
            let family = caps::family_of(need.split('.').next().unwrap_or(need));
            if !declared.iter().any(|d| caps::family_of(d) == family) {
                errors.push(ec(
                    "E_CAP_UNDECLARED",
                    item.line,
                    format!(
                        "routine '{}' declares needs '{}' but the script does not; a routine can \
                         only narrow capabilities, never add one: E_CAP_UNDECLARED",
                        item.name, need
                    ),
                ));
            }
        }
        check_contracts(item, declared, program, errors);
        let mut ctx = ctx;
        // A routine's own preconditions are facts inside its body: that is how
        // "requires pos < length of tokens" makes "tokens[pos]" provable
        // without a fallback on every line.
        for clause in &item.contracts.requires {
            facts_from_cond(&clause.expr, &mut ctx.facts);
        }
        strict_block(&item.body, &mut ctx, declared, program, errors);
        for err in &mut errors[before..] {
            if err.origin.is_empty() {
                err.origin = item.origin.clone();
            }
        }
    }
}

fn check_recursion(program: &Program, errors: &mut Vec<CheckError>) {
    let routines: Vec<&Item> = program
        .items
        .iter()
        .filter(|i| matches!(i.kind, ItemKind::Action | ItemKind::Check | ItemKind::Filter))
        .collect();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for item in &routines {
        let mut calls: Vec<&str> = Vec::new();
        collect_calls(&item.body, &mut calls);
        edges.insert(item.name.as_str(), calls);
    }
    for item in &routines {
        if on_a_cycle(item.name.as_str(), &edges) && item.contracts.recursion_bound.is_none() {
            let before = errors.len();
            errors.push(at(
                "E_KAZE_RECURSION_UNBOUNDED",
                item.line,
                item.name.as_str(),
                "add 'recursion bounded <n>' as its first line",
                format!(
                    "'{}' can call itself (directly or through another routine) but declares no \
                     depth ceiling; add 'recursion bounded <n>' (1 to 256) as its first line: \
                     E_KAZE_RECURSION_UNBOUNDED",
                    item.name
                ),
            ));
            if let Some(err) = errors.get_mut(before) {
                if err.origin.is_empty() {
                    err.origin = item.origin.clone();
                }
            }
        }
    }
}

fn collect_calls<'a>(stmts: &'a [Stmt], out: &mut Vec<&'a str>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let(_, v, _) | Stmt::Set(_, v, _) | Stmt::Eval(v, _) | Stmt::Guard(v, _) => {
                collect_calls_expr(v, out)
            }
            Stmt::Return(Some(v), _) => collect_calls_expr(v, out),
            Stmt::Return(None, _) => {}
            Stmt::If { cond, then, otherwise, .. } => {
                collect_calls_expr(cond, out);
                collect_calls(then, out);
                collect_calls(otherwise, out);
            }
            Stmt::ForEach { list, body, .. } => {
                collect_calls_expr(list, out);
                collect_calls(body, out);
            }
            Stmt::While { cond, body, .. } => {
                collect_calls_expr(cond, out);
                collect_calls(body, out);
            }
            Stmt::Try { body, otherwise, .. } => {
                collect_calls(body, out);
                collect_calls(otherwise, out);
            }
        }
    }
}

fn collect_calls_expr<'a>(expr: &'a Expr, out: &mut Vec<&'a str>) {
    match expr {
        Expr::CallRoutine { name, args, .. } => {
            out.push(name);
            for a in args {
                collect_calls_expr(a, out);
            }
        }
        Expr::Call { args, .. } | Expr::Math { args, .. } | Expr::List(args) => {
            for a in args {
                collect_calls_expr(a, out);
            }
        }
        Expr::RecordInit { fields, .. } => {
            for (_, v) in fields {
                collect_calls_expr(v, out);
            }
        }
        Expr::Bin(_, l, r) => {
            collect_calls_expr(l, out);
            collect_calls_expr(r, out);
        }
        Expr::Not(inner) | Expr::Len(inner) => collect_calls_expr(inner, out),
        Expr::Index(c, i) => {
            collect_calls_expr(c, out);
            collect_calls_expr(i, out);
        }
        Expr::Fallback { inner, fallback, .. } => {
            collect_calls_expr(inner, out);
            collect_calls_expr(fallback, out);
        }
        Expr::Num(..) | Expr::Str(_) | Expr::Bool(_) | Expr::Name(_) | Expr::Ambient { .. }
        | Expr::Field { .. } | Expr::LinkLost { .. } => {}
    }
}

fn on_a_cycle(start: &str, edges: &HashMap<&str, Vec<&str>>) -> bool {
    // Depth-first from each direct callee; reaching start again is a cycle.
    let mut stack: Vec<&str> = edges.get(start).cloned().unwrap_or_default();
    let mut seen: Vec<&str> = Vec::new();
    while let Some(next) = stack.pop() {
        if next == start {
            return true;
        }
        if seen.contains(&next) {
            continue;
        }
        seen.push(next);
        if let Some(more) = edges.get(next) {
            stack.extend(more.iter().copied());
        }
    }
    false
}

fn ty_of(t: &TypeRef) -> Ty {
    match t {
        TypeRef::Int | TypeRef::Number | TypeRef::Percent => Ty::Num(None),
        TypeRef::Text => Ty::Str,
        TypeRef::Bool => Ty::Bool,
        TypeRef::Length => Ty::Num(Some(Unit::Meter)),
        TypeRef::Time => Ty::Num(Some(Unit::Sec)),
        TypeRef::Angle => Ty::Num(Some(Unit::Deg)),
        TypeRef::Temperature => Ty::Num(Some(Unit::Celsius)),
        TypeRef::Record(_) => Ty::Any,
        TypeRef::Untrusted(inner) => ty_of(inner),
    }
}

fn mentions(expr: &Expr, name: &str) -> bool {
    let mut found = false;
    let mut check = |e: &Expr| {
        if let Expr::Name(n) = e {
            if n == name {
                found = true;
            }
        }
    };
    walk_expr(expr, &mut check);
    found
}

fn walk_expr(expr: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(expr);
    match expr {
        Expr::Call { args, .. } | Expr::Math { args, .. } | Expr::List(args) => {
            for a in args {
                walk_expr(a, f);
            }
        }
        Expr::CallRoutine { args, .. } => {
            for a in args {
                walk_expr(a, f);
            }
        }
        Expr::RecordInit { fields, .. } => {
            for (_, v) in fields {
                walk_expr(v, f);
            }
        }
        Expr::Bin(_, l, r) => {
            walk_expr(l, f);
            walk_expr(r, f);
        }
        Expr::Not(inner) | Expr::Len(inner) => walk_expr(inner, f),
        Expr::Index(c, i) => {
            walk_expr(c, f);
            walk_expr(i, f);
        }
        Expr::Fallback { inner, fallback, .. } => {
            walk_expr(inner, f);
            walk_expr(fallback, f);
        }
        _ => {}
    }
}

/// A call with constant arguments is checked against the callee's
/// preconditions at compile time, so "set_throttle(9, 50)" is refused by the
/// compiler instead of asserted at run time. Dynamic arguments are left to the
/// callee's own entry assertion.
fn check_call_site(callee: &Item, args: &[Expr], line: u32, errors: &mut Vec<CheckError>) {
    if callee.contracts.requires.is_empty() {
        return;
    }
    let mut env: Vec<(String, f64)> = Vec::new();
    for (param, arg) in callee.params.iter().zip(args) {
        if let Expr::Num(n, _) = arg {
            env.push((param.clone(), *n));
        }
    }
    if env.is_empty() {
        return;
    }
    for clause in &callee.contracts.requires {
        if let Some(ok) = eval_const(&clause.expr, &env) {
            if !ok {
                errors.push(at(
                    "E_KAZE_CONTRACT",
                    line,
                    callee.name.as_str(),
                    "this argument breaks the routine's precondition",
                    format!(
                        "this call to '{}' breaks its own precondition; the clause at line {} is                          false for the arguments given: E_KAZE_CONTRACT",
                        callee.name, clause.line
                    ),
                ));
            }
        }
    }
}

/// Evaluate a condition over constant arguments. None means "not decidable
/// here", which is not an error.
fn eval_const(expr: &Expr, env: &[(String, f64)]) -> Option<bool> {
    let num = |e: &Expr| -> Option<f64> { eval_const_num(e, env) };
    match expr {
        Expr::Bool(b) => Some(*b),
        Expr::Not(inner) => eval_const(inner, env).map(|v| !v),
        Expr::Bin(op @ (BinOp::And | BinOp::Or), l, r) => {
            let a = eval_const(l, env)?;
            let b = eval_const(r, env)?;
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

fn eval_const_num(e: &Expr, env: &[(String, f64)]) -> Option<f64> {
    match e {
        Expr::Num(n, _) => Some(*n),
        Expr::Name(n) => env.iter().find(|(k, _)| k == n).map(|(_, v)| *v),
        Expr::Bin(op, l, r) => {
            let a = eval_const_num(l, env)?;
            let b = eval_const_num(r, env)?;
            match op {
                BinOp::Add => Some(a + b),
                BinOp::Sub => Some(a - b),
                BinOp::Mul => Some(a * b),
                BinOp::Div => if b == 0.0 { None } else { Some(a / b) },
                BinOp::Mod => if b == 0.0 { None } else { Some(a % b) },
                _ => None,
            }
        }
        Expr::Len(_) => None,
        _ => None,
    }
}

fn check_contracts(item: &Item, declared: &[&str], program: &Program, errors: &mut Vec<CheckError>) {
    let mut scope: Vec<(String, Ty)> = Vec::new();
    for (i, p) in item.params.iter().enumerate() {
        let ty = match item.contracts.param_types.get(i).and_then(|t| t.as_ref()) {
            Some(t) => ty_of(t),
            None => Ty::Any,
        };
        scope.push((p.clone(), ty));
    }
    for clause in &item.contracts.requires {
        let t = infer(&clause.expr, &mut scope, declared, &program.items, errors);
        if !matches!(t, Ty::Bool | Ty::Any) {
            errors.push(e(
                clause.line,
                format!(
                    "a 'requires' clause must be true or false, but this is {}",
                    t.label()
                ),
            ));
        }
    }
    let mut ensured = scope.clone();
    let result_ty = match item.contracts.result.as_ref() {
        Some(t) => ty_of(t),
        None => Ty::Any,
    };
    ensured.push(("result".to_string(), result_ty));
    for clause in &item.contracts.ensures {
        let t = infer(&clause.expr, &mut ensured, declared, &program.items, errors);
        if !matches!(t, Ty::Bool | Ty::Any) {
            errors.push(e(
                clause.line,
                format!(
                    "an 'ensures' clause must be true or false, but this is {}",
                    t.label()
                ),
            ));
        }
    }
    // An untrusted parameter has to be validated by a precondition before the
    // body may use it: that is the whole point of the untrusted type.
    for (i, p) in item.params.iter().enumerate() {
        let untrusted = matches!(
            item.contracts.param_types.get(i).and_then(|t| t.as_ref()),
            Some(TypeRef::Untrusted(_))
        );
        if !untrusted {
            continue;
        }
        let validated = item
            .contracts
            .requires
            .iter()
            .any(|c| mentions(&c.expr, p));
        if !validated {
            errors.push(at(
                "E_KAZE_UNTRUSTED",
                item.line,
                p.as_str(),
                "add a requires clause that validates it",
                format!(
                    "parameter '{}' is untrusted, but no 'requires' clause checks it; add \
                     'requires {} >= ...' (or drop the untrusted type): E_KAZE_UNTRUSTED",
                    p, p
                ),
            ));
        }
    }
}

fn strict_block(
    stmts: &[Stmt],
    ctx: &mut Ctx,
    declared: &[&str],
    program: &Program,
    errors: &mut Vec<CheckError>,
) {
    for stmt in stmts {
        ctx.line = stmt.line();
        match stmt {
            Stmt::Let(name, value, _) => {
                strict_expr(value, ctx, declared, program, errors);
                facts_from_let(name, value, &mut ctx.facts);
                if let Expr::CallRoutine { name: callee, .. } = value {
                    if let Some(target) = program.items.iter().find(|i| i.name == *callee) {
                        if let Some(len) = promised_length(target) {
                            ctx.facts.known_len.push((name.clone(), len));
                        }
                        let (nonneg, bounded) = postcondition_facts(target);
                        if nonneg && !ctx.facts.is_nonneg(name) {
                            ctx.facts.nonneg.push(name.clone());
                        }
                        for coll in bounded {
                            if !ctx.facts.bounded_by(name, &coll) {
                                ctx.facts.lt_len.push((name.clone(), coll));
                            }
                        }
                    }
                }
            }
            Stmt::Set(name, value, line) => {
                strict_expr(value, ctx, declared, program, errors);
                // "set i = i + 1" keeps a proved non-negative index non-negative;
                // anything else drops what we knew about the name.
                let saved = ctx.facts.clone();
                ctx.facts.forget(name);
                copy_facts_from_value(name, value, &saved, &mut ctx.facts);
                if let Expr::Num(k, _) = value {
                    if *k >= 0.0 {
                        ctx.facts.nonneg.push(name.clone());
                    }
                }
                if let Expr::CallRoutine { name: callee, .. } = value {
                    if let Some(target) = program.items.iter().find(|i| i.name == *callee) {
                        let (nonneg, bounded) = postcondition_facts(target);
                        if nonneg {
                            ctx.facts.nonneg.push(name.clone());
                        }
                        for coll in bounded {
                            ctx.facts.lt_len.push((name.clone(), coll));
                        }
                    }
                }
                let _ = line;
            }
            Stmt::Eval(value, _) => strict_expr(value, ctx, declared, program, errors),
            Stmt::Return(Some(value), _) => strict_expr(value, ctx, declared, program, errors),
            Stmt::Return(None, _) => {}
            Stmt::If { cond, then, otherwise, line: _ } => {
                strict_cond(cond, ctx, declared, program, errors);
                let mut narrowed = ctx.clone();
                facts_from_cond(cond, &mut narrowed.facts);
                strict_block(then, &mut narrowed, declared, program, errors);
                let mut other = ctx.clone();
                strict_block(otherwise, &mut other, declared, program, errors);
            }
            Stmt::ForEach { var, list, body, line: _ } => {
                strict_expr(list, ctx, declared, program, errors);
                let mut inner = ctx.clone();
                inner.facts.nonneg.push(var.clone());
                strict_block(body, &mut inner, declared, program, errors);
            }
            Stmt::While { cond, body, line, bound } => {
                if bound.is_none() {
                    errors.push(at(
                        "E_KAZE_LOOP_UNBOUNDED",
                        *line,
                        "while",
                        "write 'while <cond> bounded <n>:'",
                        "this loop has no iteration ceiling; write 'while <cond> bounded <n>:' so a \
                         stuck sensor or a bad counter cannot hang the run: E_KAZE_LOOP_UNBOUNDED"
                            .to_string(),
                    ));
                }
                strict_cond(cond, ctx, declared, program, errors);
                let mut inner = ctx.clone();
                facts_from_cond(cond, &mut inner.facts);
                strict_block(body, &mut inner, declared, program, errors);
            }
            Stmt::Guard(cond, _) => {
                strict_expr(cond, ctx, declared, program, errors);
                // Everything after the guard may assume it held.
                facts_from_cond(cond, &mut ctx.facts);
            }
            Stmt::Try { body, otherwise, .. } => {
                let mut guarded = ctx.clone();
                guarded.in_try = true;
                strict_block(body, &mut guarded, declared, program, errors);
                let mut recovery = ctx.clone();
                strict_block(otherwise, &mut recovery, declared, program, errors);
            }
        }
    }
}

fn strict_expr(
    expr: &Expr,
    ctx: &mut Ctx,
    declared: &[&str],
    program: &Program,
    errors: &mut Vec<CheckError>,
) {
    strict_expr_guarded(expr, false, ctx, declared, program, errors)
}

/// A condition is checked left to right, because "i < length of xs and xs[i]"
/// only makes the second half legal when the first half has already been
/// proved. Short-circuit evaluation means that is exactly the run-time order.
fn strict_cond(
    cond: &Expr,
    ctx: &mut Ctx,
    declared: &[&str],
    program: &Program,
    errors: &mut Vec<CheckError>,
) {
    if let Expr::Bin(BinOp::And, left, right) = cond {
        strict_cond(left, ctx, declared, program, errors);
        facts_from_cond(left, &mut ctx.facts);
        strict_cond(right, ctx, declared, program, errors);
        return;
    }
    strict_expr(cond, ctx, declared, program, errors);
}

/// "guarded" means this exact node already carries an inline otherwise, so the
/// fallibility rules are satisfied one level up. Children are never guarded:
/// each one has to prove itself.
#[allow(clippy::only_used_in_recursion)] // one uniform walker signature
fn strict_expr_guarded(
    expr: &Expr,
    guarded: bool,
    ctx: &mut Ctx,
    declared: &[&str],
    program: &Program,
    errors: &mut Vec<CheckError>,
) {
    match expr {
        Expr::Fallback { inner, fallback, .. } => {
            strict_expr_guarded(inner, true, ctx, declared, program, errors);
            strict_expr_guarded(fallback, false, ctx, declared, program, errors);
        }
        Expr::Bin(op, left, right) => {
            strict_expr_guarded(left, false, ctx, declared, program, errors);
            strict_expr_guarded(right, false, ctx, declared, program, errors);
            let fallible = matches!(op, BinOp::Div | BinOp::Mod);
            let proved = matches!(&**right, Expr::Num(n, _) if *n != 0.0);
            if fallible && !proved && !guarded {
                errors.push(at(
                    "E_KAZE_MISSING_FALLBACK",
                    ctx.line,
                    op.symbol(),
                    "this divisor can be zero at run time",
                    format!(
                        "this '{}' can divide by zero and has no fallback; write '<value> {} <divisor> otherwise <fallback>': E_KAZE_MISSING_FALLBACK",
                        op.symbol(),
                        op.symbol()
                    ),
                ));
            }
        }
        Expr::Index(coll, idx) => {
            strict_expr_guarded(coll, false, ctx, declared, program, errors);
            strict_expr_guarded(idx, false, ctx, declared, program, errors);
            if guarded {
                return;
            }
            match prove_index(&ctx.facts, coll, idx) {
                IndexProof::Safe => {}
                IndexProof::OutOfBounds => errors.push(at(
                    "E_KAZE_BOUNDS",
                    ctx.line,
                    "[",
                    "a constant position past the end is refused",
                    "this index is outside the value it reads; a constant position past the end is refused: E_KAZE_BOUNDS".to_string(),
                )),
                IndexProof::Unproven => errors.push(at(
                    "E_KAZE_MISSING_FALLBACK",
                    ctx.line,
                    "[",
                    "prove it is in range or give a fallback",
                    "this index is not proved to be inside the value; guard it (if i < length of xs:) or write '<value>[<i>] otherwise <fallback>': E_KAZE_MISSING_FALLBACK".to_string(),
                )),
            }
        }
        Expr::Call { recv, op, args, line } => {
            for a in args {
                strict_expr_guarded(a, false, ctx, declared, program, errors);
            }
            let family = caps::family_of(recv);
            if !ctx.routine_needs.is_empty() && !declares(&ctx.routine_needs, family) {
                errors.push(at(
                    "E_CAP_UNDECLARED",
                    *line,
                    family,
                    "declare it in the routine's own needs: line",
                    format!(
                        "'{}' uses '{}', which this routine does not declare; add it to the routine's own 'needs:' line: E_CAP_UNDECLARED",
                        ctx.item, family
                    ),
                ));
            }
            if needs_fallback(family, op) && !ctx.in_try && !guarded {
                errors.push(at(
                    "E_KAZE_MISSING_FALLBACK",
                    *line,
                    op.as_str(),
                    "this read can fail; say what to use instead",
                    format!(
                        "{}.{} reads a device that can be silent or unavailable and has no fallback; write 'let v = {}.{}(..) otherwise <fallback>' or wrap the statement in 'try:': E_KAZE_MISSING_FALLBACK",
                        family, op, family, op
                    ),
                ));
            }
        }
        Expr::CallRoutine { name, args, line } => {
            for a in args {
                strict_expr_guarded(a, false, ctx, declared, program, errors);
            }
            if let Some(callee) = program.items.iter().find(|i| i.name == *name) {
                check_call_site(callee, args, *line, errors);
                for need in &callee.contracts.routine_needs {
                    let family = caps::family_of(need.split('.').next().unwrap_or(need));
                    if !ctx.routine_needs.is_empty() && !declares(&ctx.routine_needs, family) {
                        errors.push(ec(
                            "E_CAP_UNDECLARED",
                            *line,
                            format!(
                                "'{}' calls '{}', which needs '{}', but '{}' does not declare it: capabilities can only narrow: E_CAP_UNDECLARED",
                                ctx.item, name, need, ctx.item
                            ),
                        ));
                    }
                }
            }
        }
        Expr::RecordInit { fields, .. } => {
            for (_, v) in fields {
                strict_expr_guarded(v, false, ctx, declared, program, errors);
            }
        }
        Expr::Math { args, .. } | Expr::List(args) => {
            for a in args {
                strict_expr_guarded(a, false, ctx, declared, program, errors);
            }
        }
        Expr::Not(inner) | Expr::Len(inner) => {
            strict_expr_guarded(inner, false, ctx, declared, program, errors)
        }
        Expr::Field { base, field, line } => {
            // A field read on a value this routine received from outside (an
            // untyped parameter) may name a field the record does not have,
            // and the language answers that with nothing. Say what to use
            // instead. A record-typed parameter is checked against its record,
            // so its fields need no fallback.
            let from_outside = ctx.params.iter().any(|p| p == base)
                && !ctx.record_params.iter().any(|p| p == base);
            if from_outside && !guarded {
                errors.push(at(
                    "E_KAZE_MISSING_FALLBACK",
                    *line,
                    field.as_str(),
                    "a field the record may not have needs a fallback",
                    format!(
                        "'{}.{}' reads a field of '{}', which came from outside this routine and may not have it; write '{}.{} otherwise <fallback>' or declare '{}' with a record type: E_KAZE_MISSING_FALLBACK",
                        base, field, base, base, field, base
                    ),
                ));
            }
        }
        Expr::Num(..) | Expr::Str(_) | Expr::Bool(_) | Expr::Name(_) | Expr::Ambient { .. }
        | Expr::LinkLost { .. } => {}
    }
}
