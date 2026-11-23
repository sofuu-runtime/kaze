// sfs/checker.rs — the lightweight checker (M-K2).
//
// PLAN-KAZE-LANGUAGE §5 (M-K2): "Capability verification, type inference,
// exhaustiveness checks." The checker is what makes the safety story true
// before a single instruction runs:
//
//   * every capability family used is declared in `needs:` (H-1, early);
//   * every operation is a real v0.1 operation with the right argument count;
//   * every name resolves to a binding (no accidental ambient state);
//   * a comparison between `meters` and `m/s` is refused;
//   * a `filter` ends in `allow` or `reject` on every path — a guardrail that
//     can fall off the end of the function is not a guardrail.
//
// It is deliberately not a full type system: it knows numbers/units, text,
// true-or-false, lists and "unknown" (what a capability returns). Unknown is
// permissive, so a host that returns a new shape does not need a language
// change. Every error carries a line and a suggestion.

use crate::ast::*;
use crate::caps;
use crate::strict;

#[derive(Debug, Clone)]
pub struct CheckError {
    /// Labelled spans on the offending line (the two operands of a unit
    /// mismatch). Empty means \`needle\` alone.
    pub spans: Vec<(String, String)>,
    /// The stable diagnostic code. Most checker errors are "E_KAZE_CHECK"; the
    /// Phase 2/3/4 rules name their own code so a caller (and a test) can act on
    /// the specific refusal instead of parsing prose.
    pub code: String,
    pub line: u32,
    /// The text the caret should point at (an operator, a name, a bracket).
    /// The renderer searches the source line for it, so the checker does not
    /// have to carry a column on every AST node.
    pub needle: Option<String>,
    /// A concrete suggestion printed under the caret.
    pub hint: Option<String>,
    pub msg: String,
    /// The file this error belongs to when modules were inlined. Empty means
    /// the root source; `render` then uses the caller's name.
    pub origin: String,
}

impl CheckError {
    /// One line: file:line: message. Used where a block would be noise (JSON
    /// reports, imported files whose source is not at hand).
    pub fn render(&self, source_name: &str) -> String {
        let source = if self.origin.is_empty() { source_name } else { &self.origin };
        format!("{}:{}: {}", source, self.line, self.msg)
    }

    /// The block a person reads: code, path:line:col, the source line, a caret
    /// under the construct and the suggestion.
    pub fn render_detailed(&self, source_name: &str, map: &crate::diag::SourceMap<'_>) -> String {
        let source = if self.origin.is_empty() { source_name } else { &self.origin };
        // Two labelled spans draw the picture a reader expects of a mismatch:
        // one caret per operand, with the unit under each.
        if !self.spans.is_empty() {
            let spans: Vec<crate::diag::Span> = self
                .spans
                .iter()
                .map(|(needle, label)| crate::diag::Span {
                    needle: needle.clone(),
                    label: label.clone(),
                })
                .collect();
            if let Some(block) =
                crate::diag::render_spans(&self.code, &self.msg, source, self.line, &spans, Some(map))
            {
                return block;
            }
        }
        let needles: Vec<&str> = self.needle.as_deref().map(|n| vec![n]).unwrap_or_default();
        let caret = map.locate(self.line, &needles);
        crate::diag::render(
            &self.code,
            &self.msg,
            source,
            self.line,
            caret,
            self.hint.as_deref(),
            Some(map),
        )
    }
}

pub(crate) fn e(line: u32, msg: impl Into<String>) -> CheckError {
    CheckError {
        spans: Vec::new(),
        code: "E_KAZE_CHECK".to_string(),
        line,
        needle: None,
        hint: None,
        msg: msg.into(),
        origin: String::new(),
    }
}

/// A checker error with its own diagnostic code.
pub(crate) fn ec(code: &str, line: u32, msg: impl Into<String>) -> CheckError {
    CheckError {
        spans: Vec::new(),
        code: code.to_string(),
        line,
        needle: None,
        hint: None,
        msg: msg.into(),
        origin: String::new(),
    }
}

/// A diagnostic with two labelled spans, for a mismatch between two operands.
pub(crate) fn at_spans(
    code: &str,
    line: u32,
    spans: Vec<(String, String)>,
    needle: impl Into<String>,
    msg: impl Into<String>,
) -> CheckError {
    CheckError {
        spans,
        code: code.to_string(),
        line,
        needle: Some(needle.into()),
        hint: None,
        msg: msg.into(),
        origin: String::new(),
    }
}

/// A diagnostic that knows where to point and what to suggest.
pub(crate) fn at(
    code: &str,
    line: u32,
    needle: impl Into<String>,
    hint: impl Into<String>,
    msg: impl Into<String>,
) -> CheckError {
    CheckError {
        spans: Vec::new(),
        code: code.to_string(),
        line,
        needle: Some(needle.into()),
        hint: Some(hint.into()),
        msg: msg.into(),
        origin: String::new(),
    }
}

thread_local! {
    /// The statement being checked. An expression node has no line of its own,
    /// so the checker records the enclosing statement here; a diagnostic built
    /// from a bare expression still names a real line instead of 0.
    static CURRENT_LINE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

pub(crate) fn current_line() -> u32 {
    CURRENT_LINE.with(|c| c.get())
}

/// What the checker can prove about a value. `Any` is "the host decides".
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Ty {
    Num(Option<Unit>),
    Str,
    Bool,
    List,
    /// A point or an offset: `vector(x, y, z)` and what arithmetic on
    /// points produces. Added in v0.3.
    Point,
    /// The record bound by `otherwise with err:` (v0.4). It has exactly three
    /// readable fields: `code`, `message` and `line`.
    Error,
    Unit,
    Any,
}

impl Ty {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Ty::Num(Some(u)) => u.label(),
            Ty::Num(None) => "a number",
            Ty::Str => "text",
            Ty::Bool => "true or false",
            Ty::List => "a list",
            Ty::Point => "a point",
            Ty::Error => "an error",
            Ty::Unit => "nothing",
            Ty::Any => "a host value",
        }
    }
}

/// The declared rows, so `filter` bodies can be checked for a verdict.
pub fn check(p: &Program) -> Result<(), Vec<CheckError>> {
    if let Some(u) = p.uses.first() {
        // The loader consumes `use` lines. Reaching here means somebody called
        // the checker on a parsed-but-unexpanded Program; refuse rather than
        // silently ignore the imports.
        return Err(vec![e(
            u.line,
            "unresolved 'use' directive: call prepare/prepare_file so imports are expanded",
        )]);
    }
    let mut errors: Vec<CheckError> = Vec::new();
    let declared: Vec<&str> = p.needs.iter().map(|s| s.as_str()).collect();

    for need in &p.needs {
        let family = caps::family_of(need);
        if need != family && !caps::FAMILIES.contains(&family) {
            errors.push(e(
                1,
                format!("'{}' does not name a capability family or an instance", need),
            ));
        }
    }

    for item in &p.items {
        let before = errors.len();
        let mut scope: Vec<(String, Ty)> = Vec::new();
        for p in &item.params {
            scope.push((p.clone(), Ty::Any));
        }
        if let ItemKind::When(cond) = &item.kind {
            let t = infer(cond, &mut scope, &declared, &p.items, &mut errors);
            if !matches!(t, Ty::Bool | Ty::Any) {
                errors.push(e(
                    item.line,
                    format!("a 'when' condition must be true or false, but this is {}", t.label()),
                ));
            }
        }
        check_block(&item.body, &mut scope, &declared, &p.items, &mut errors);
        if matches!(item.kind, ItemKind::Filter) && !terminates(&item.body) {
            errors.push(e(
                item.line,
                format!(
                    "filter '{}' can reach its end without allowing or rejecting; end every \
                     path with 'allow …' or 'reject with …'",
                    item.name
                ),
            ));
        }
        // Every error produced while checking this item belongs to its file.
        for err in &mut errors[before..] {
            if err.origin.is_empty() {
                err.origin = item.origin.clone();
            }
        }
    }

    // Phase 2/3/4: fallibility, termination, dimensions, contracts.
    strict::check(p, &declared, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn check_block(
    stmts: &[Stmt],
    scope: &mut Vec<(String, Ty)>,
    declared: &[&str],
    items: &[Item],
    errors: &mut Vec<CheckError>,
) {
    for stmt in stmts {
        CURRENT_LINE.with(|c| c.set(stmt.line()));
        match stmt {
            Stmt::Let(name, value, line) => {
                let t = infer(value, scope, declared, items, errors);
                if let Some(slot) = scope.iter_mut().find(|(n, _)| n == name) {
                    slot.1 = t;
                } else {
                    scope.push((name.clone(), t));
                }
                let _ = line;
            }
            Stmt::Set(name, value, line) => {
                infer(value, scope, declared, items, errors);
                if !scope.iter().any(|(n, _)| n == name) {
                    errors.push(e(*line, format!("'{}' is not defined", name)));
                }
            }
            Stmt::Eval(expr, _) => {
                infer(expr, scope, declared, items, errors);
            }
            Stmt::If { cond, then, otherwise, line } => {
                let t = infer(cond, scope, declared, items, errors);
                if !matches!(t, Ty::Bool | Ty::Any) {
                    errors.push(e(
                        *line,
                        format!("an 'if' condition must be true or false, but this is {}", t.label()),
                    ));
                }
                let mut inner = scope.clone();
                check_block(then, &mut inner, declared, items, errors);
                let mut inner2 = scope.clone();
                check_block(otherwise, &mut inner2, declared, items, errors);
            }
            Stmt::ForEach { var, list, body, line } => {
                let t = infer(list, scope, declared, items, errors);
                if !matches!(t, Ty::List | Ty::Any) {
                    errors.push(e(
                        *line,
                        format!("'for each' walks a list, but '{}' is {}", expr_label(list), t.label()),
                    ));
                }
                let mut inner = scope.clone();
                inner.push((var.clone(), Ty::Any));
                check_block(body, &mut inner, declared, items, errors);
            }
            Stmt::While { cond, body, line, .. } => {
                let t = infer(cond, scope, declared, items, errors);
                if !matches!(t, Ty::Bool | Ty::Any) {
                    errors.push(e(
                        *line,
                        format!("a 'while' condition must be true or false, but this is {}", t.label()),
                    ));
                }
                let mut inner = scope.clone();
                check_block(body, &mut inner, declared, items, errors);
            }
            Stmt::Try { body, otherwise, error_name, line } => {
                let mut inner = scope.clone();
                check_block(body, &mut inner, declared, items, errors);
                let mut recovery = scope.clone();
                if let Some(name) = error_name {
                    recovery.push((name.clone(), Ty::Error));
                }
                let _ = line;
                check_block(otherwise, &mut recovery, declared, items, errors);
            }
            Stmt::Return(value, _) => {
                if let Some(v) = value {
                    infer(v, scope, declared, items, errors);
                }
            }
            Stmt::Guard(cond, line) => {
                let t = infer(cond, scope, declared, items, errors);
                if !matches!(t, Ty::Bool | Ty::Any) {
                    errors.push(e(
                        *line,
                        format!("a 'guard' must be true or false, but this is {}", t.label()),
                    ));
                }
            }
        }
    }
}

fn expr_label(e: &Expr) -> String {
    match e {
        Expr::Name(n) => n.clone(),
        Expr::Ambient { field, .. } => field.clone(),
        Expr::Call { recv, op, .. } => format!("{}.{}", recv, op),
        Expr::CallRoutine { name, .. } => format!("{}()", name),
        Expr::RecordInit { name, .. } => format!("{}(...)", name),
        Expr::Field { base, field, .. } => format!("{}.{}", base, field),
        _ => "an expression".to_string(),
    }
}

pub(crate) fn infer(
    expr: &Expr,
    scope: &mut Vec<(String, Ty)>,
    declared: &[&str],
    items: &[Item],
    errors: &mut Vec<CheckError>,
) -> Ty {
    match expr {
        Expr::Num(_, unit) => Ty::Num(if *unit == Unit::None { None } else { Some(*unit) }),
        Expr::Str(_) => Ty::Str,
        Expr::Bool(_) => Ty::Bool,
        Expr::List(list_items) => {
            for i in list_items {
                infer(i, scope, declared, items, errors);
            }
            Ty::List
        }
        Expr::CallRoutine { name, args, line } => {
            let target = items.iter().find(|it| it.name == *name);
            match target {
                Some(it) => {
                    if it.params.len() != args.len() {
                        errors.push(e(
                            *line,
                            format!("'{}' takes {} value(s), but {} were given", name, it.params.len(), args.len()),
                        ));
                    }
                }
                None => {
                    errors.push(e(
                        *line,
                        format!("'{}' is not defined; add an action or record '{}'", name, name),
                    ));
                }
            }
            for a in args {
                infer(a, scope, declared, items, errors);
            }
            Ty::Any
        }
        Expr::RecordInit { name, fields, line } => {
            let target = items.iter().find(|it| it.name == *name);
            match target {
                Some(it) => {
                    if let ItemKind::Record(ref rec_fields) = it.kind {
                        for (fname, val) in fields {
                            if !rec_fields.iter().any(|rf| rf.name == *fname) {
                                errors.push(e(
                                    *line,
                                    format!("record '{}' has no field '{}'", name, fname),
                                ));
                            }
                            infer(val, scope, declared, items, errors);
                        }
                    } else {
                        errors.push(e(
                            *line,
                            format!("'{}' is not a record", name),
                        ));
                    }
                }
                None => {
                    errors.push(e(
                        *line,
                        format!("record '{}' is not defined", name),
                    ));
                }
            }
            Ty::Any
        }
        Expr::Name(name) => match scope.iter().rev().find(|(n, _)| n == name) {
            Some((_, t)) => *t,
            None => {
                errors.push(e(
                    0,
                    format!(
                        "'{}' is not defined here; add it as a parameter, a 'let', or a capability value",
                        name
                    ),
                ));
                Ty::Any
            }
        },
        Expr::Ambient { field, family } => {
            if !declared.iter().any(|d| caps::family_of(d) == *family) {
                errors.push(ec(
                    "E_CAP_UNDECLARED",
                    0,
                    format!("'{}' is read from '{}', which this script does not declare", field, family),
                ));
            }
            Ty::Any
        }
        Expr::Field { base, field, line } => {
            if let Some((_, ty)) = scope.iter().rev().find(|(n, _)| n == base) {
                if let Ty::Point = ty {
                    match field.as_str() {
                        "x" | "y" | "z" | "lat" | "lon" | "alt" | "latitude" | "longitude" | "altitude" => {
                            return Ty::Num(None);
                        }
                        _ => {
                            errors.push(e(
                                *line,
                                format!(
                                    "'.{}' is not a component of point '{}'; write x/lat, y/lon, or z/alt",
                                    field, base
                                ),
                            ));
                            return Ty::Any;
                        }
                    }
                } else if let Ty::Error = ty {
                    return match field.as_str() {
                        "code" | "message" => Ty::Str,
                        "line" => Ty::Num(None),
                        _ => {
                            errors.push(e(
                                *line,
                                format!(
                                    "'.{}' is not part of an error; write .code, .message or .line",
                                    field
                                ),
                            ));
                            Ty::Any
                        }
                    };
                }
            } else {
                errors.push(e(
                    *line,
                    format!("'.{}' reads a field of '{}', which is not a value here", field, base),
                ));
            }
            Ty::Any
        }
        Expr::Not(inner) => {
            let t = infer(inner, scope, declared, items, errors);
            if !matches!(t, Ty::Bool | Ty::Any) {
                errors.push(e(0, format!("'not' needs true or false, but this is {}", t.label())));
            }
            Ty::Bool
        }
        Expr::Len(inner) => {
            let t = infer(inner, scope, declared, items, errors);
            if !matches!(t, Ty::Str | Ty::List | Ty::Point | Ty::Any) {
                errors.push(e(
                    0,
                    format!("'length of' needs text, a list or a point, but this is {}", t.label()),
                ));
            }
            Ty::Num(None)
        }
        Expr::Index(list, idx) => {
            let lt = infer(list, scope, declared, items, errors);
            let it = infer(idx, scope, declared, items, errors);
            // `p[0]`, `p[1]`, `p[2]` read a point's components.
            if !matches!(lt, Ty::List | Ty::Str | Ty::Point | Ty::Any) {
                errors.push(e(
                    0,
                    format!("[ ] needs a list, text or a point, but this is {}", lt.label()),
                ));
            }
            if !matches!(it, Ty::Num(_) | Ty::Any) {
                errors.push(e(0, format!("a position must be a number, but this is {}", it.label())));
            }
            Ty::Any
        }
        Expr::Call { recv, op, args, line } => {
            let family = caps::family_of(recv);
            if !declared.iter().any(|d| *d == recv || caps::family_of(d) == family) {
                errors.push(at(
                    "E_CAP_UNDECLARED",
                    *line,
                    family,
                    format!("add '{}' to needs: ...", family),
                    format!(
                        "'{}' is not declared. Add '{}' to the script's first line: needs: {}",
                        family, family, family
                    ),
                ));
            }
            let arg_tys: Vec<Ty> = args.iter().map(|a| infer(a, scope, declared, items, errors)).collect();
            match caps::op_arity(family, op) {
                None => errors.push(e(
                    *line,
                    format!(
                        "{}.{} is not something a script can ask for. Known {} operations: {}",
                        family,
                        op,
                        family,
                        known_ops(family)
                    ),
                )),
                Some((min, max)) => {
                    if arg_tys.len() < min || arg_tys.len() > max {
                        let want = if min == max {
                            format!("{}", min)
                        } else {
                            format!("{} to {}", min, max)
                        };
                        errors.push(e(
                            *line,
                            format!(
                                "{}.{} takes {} value(s), but {} were given",
                                family,
                                op,
                                want,
                                arg_tys.len()
                            ),
                        ));
                    }
                }
            }
            capability_result(family, op)
        }
        Expr::Bin(op, left, right) => {
            let lt = infer(left, scope, declared, items, errors);
            let rt = infer(right, scope, declared, items, errors);
            // `-p` is written `0 - p` by the parser, and negating a point is
            // meaningful (the opposite offset), so it is not a type error.
            let negation = *op == BinOp::Sub
                && rt == Ty::Point
                && matches!(**left, Expr::Num(n, _) if n == 0.0);
            if negation {
                return Ty::Point;
            }
            point_aware_bin(*op, lt, rt, left, right, 0, errors)
        }
        Expr::Fallback { inner, fallback, line } => {
            let t = infer(inner, scope, declared, items, errors);
            let f = infer(fallback, scope, declared, items, errors);
            // The fallback has to be usable where the value is: same shape.
            if !compatible(t, f) {
                errors.push(e(
                    *line,
                    format!(
                        "the fallback is {}, but the value it replaces is {}; make them the same kind",
                        f.label(),
                        t.label()
                    ),
                ));
            }
            t
        }
        Expr::Math { kind, args, line } => infer_math(*kind, args, *line, scope, declared, items, errors),
        Expr::LinkLost { after_ms, line } => {
            let _ = after_ms;
            if !declared.iter().any(|d| caps::family_of(d) == "drone") {
                errors.push(ec(
                    "E_CAP_UNDECLARED",
                    *line,
                    "'link lost' reads the drone link; declare it first with 'needs: drone'",
                ));
            }
            Ty::Bool
        }
    }
}

/// Arithmetic that understands points: two points add or subtract to a point
/// (an offset), a number scales a point, and every other mixture is refused
/// here rather than at run time.
fn point_aware_bin(
    op: BinOp,
    lt: Ty,
    rt: Ty,
    left: &Expr,
    right: &Expr,
    line: u32,
    errors: &mut Vec<CheckError>,
) -> Ty {
    use BinOp::*;
    if lt != Ty::Point && rt != Ty::Point {
        return dim_result(op, lt, rt, left, right, line, errors);
    }
    match (op, lt, rt) {
        (Add | Sub, Ty::Point, Ty::Point) => Ty::Point,
        (Add | Sub, Ty::Point, Ty::Any) | (Add | Sub, Ty::Any, Ty::Point) => Ty::Point,
        (Mul, Ty::Point, Ty::Num(_)) | (Mul, Ty::Num(_), Ty::Point) => Ty::Point,
        (Div, Ty::Point, Ty::Num(_)) => Ty::Point,
        (Eq | Ne, Ty::Point, Ty::Point) => Ty::Bool,
        _ => {
            errors.push(e(
                0,
                format!(
                    "'{}' does not apply to {} and {}: points add to points, and a number scales one",
                    op.symbol(),
                    lt.label(),
                    rt.label()
                ),
            ));
            Ty::Any
        }
    }
}

fn infer_math(
    kind: MathKind,
    args: &[Expr],
    line: u32,
    scope: &mut Vec<(String, Ty)>,
    declared: &[&str],
    items: &[Item],
    errors: &mut Vec<CheckError>,
) -> Ty {
    let tys: Vec<Ty> = args.iter().map(|a| infer(a, scope, declared, items, errors)).collect();
    let want = match kind {
        // `vector(x, y)` is a 2-D point: z defaults to 0.
        MathKind::Vector => 2,
        MathKind::Distance
        | MathKind::SlantDistance
        | MathKind::BearingFrom
        | MathKind::Atan2 => 2,
        _ => 1,
    };
    let wrong_count = match kind {
        MathKind::Vector => tys.len() != 2 && tys.len() != 3,
        _ => tys.len() != want,
    };
    if wrong_count {
        let expected = match kind {
            MathKind::Vector => "2 or 3 values".to_string(),
            _ => format!("{} value(s)", want),
        };
        errors.push(e(
            line,
            format!("{:?} takes {}, but {} were given", kind, expected, tys.len()),
        ));
        return Ty::Any;
    }
    let numeric = |t: Ty| matches!(t, Ty::Num(_) | Ty::Any);
    let point = |t: Ty| matches!(t, Ty::Point | Ty::Any);
    match kind {
        MathKind::Vector => {
            for t in &tys {
                if !numeric(*t) {
                    errors.push(e(line, format!("a point is three numbers, but this is {}", t.label())));
                }
            }
            Ty::Point
        }
        MathKind::Distance | MathKind::SlantDistance => {
            for t in &tys {
                if !point(*t) {
                    errors.push(e(
                        line,
                        format!("{:?} needs two points, but this is {}", kind, t.label()),
                    ));
                }
            }
            Ty::Num(Some(Unit::Meter))
        }
        MathKind::BearingOf => {
            if !point(tys[0]) {
                errors.push(e(
                    line,
                    format!("'bearing of' needs a point, but this is {}", tys[0].label()),
                ));
            }
            Ty::Num(Some(Unit::Deg))
        }
        MathKind::BearingFrom => {
            for t in &tys {
                if !point(*t) {
                    errors.push(e(
                        line,
                        format!("'bearing from ... to' needs two points, but this is {}", t.label()),
                    ));
                }
            }
            Ty::Num(Some(Unit::Deg))
        }
        MathKind::Sin | MathKind::Cos | MathKind::Sqrt => {
            if !numeric(tys[0]) {
                errors.push(e(line, format!("{:?} needs a number, but this is {}", kind, tys[0].label())));
            }
            Ty::Num(None)
        }
        MathKind::Atan2 => {
            for t in &tys {
                if !numeric(*t) {
                    errors.push(e(line, format!("atan2 needs two numbers, but this is {}", t.label())));
                }
            }
            Ty::Num(None)
        }
        MathKind::ToRadians | MathKind::ToDegrees => {
            if !numeric(tys[0]) {
                errors.push(e(line, format!("a conversion needs a number, but this is {}", tys[0].label())));
            }
            Ty::Num(None)
        }
    }
}

fn known_ops(family: &str) -> String {
    let mut names: Vec<&str> = Vec::new();
    for op in [
        "arm", "takeoff", "land", "rtl", "hold", "goto", "stop", "telemetry", "forward", "backward",
        "turn_left", "turn_right", "temperature", "humidity", "distance", "front_distance", "read",
        "set_relay", "pulse", "read_gpio", "set_duty", "save", "recall", "detects", "allow",
        "reject", "info", "warn", "error", "debug", "send",
    ] {
        if caps::known_op(family, op) {
            names.push(op);
        }
    }
    names.join(", ")
}

fn capability_result(family: &str, op: &str) -> Ty {
    match (family, op) {
        ("policy", "detects") | ("policy", "detect") => Ty::Bool,
        ("memory", "recall") => Ty::List,
        // A sensor reads a physical quantity, so it carries a dimension: a
        // distance compares against meters, a temperature against Celsius.
        ("sensor", "distance") | ("sensor", "front_distance") => Ty::Num(Some(Unit::Meter)),
        ("sensor", "temperature") => Ty::Num(Some(Unit::Celsius)),
        ("sensor", "humidity") => Ty::Num(Some(Unit::Pct)),
        ("sensor", "read") | ("device", "read_gpio") => Ty::Num(None),
        ("log", _) | ("alert", _) | ("memory", "save") | ("policy", "reject") | ("drone", "arm")
        | ("drone", "takeoff") | ("drone", "land") | ("drone", "rtl") | ("drone", "hold")
        | ("drone", "goto") | ("drone", "stop") | ("rover", "forward") | ("rover", "backward")
        | ("rover", "turn_left") | ("rover", "turn_right") | ("rover", "stop")
        | ("device", "set_relay") | ("device", "pulse") | ("device", "set_duty") => Ty::Unit,
        _ => Ty::Any,
    }
}

/// Are two inferred types interchangeable for a fallback?
fn compatible(a: Ty, b: Ty) -> bool {
    matches!(a, Ty::Any) || matches!(b, Ty::Any) || a == b
        || matches!((a, b), (Ty::Num(_), Ty::Num(_)))
}

/// The dimensional arithmetic the checker enforces (Phase 3).
///
/// Addition and subtraction need the same dimension; multiplication and
/// division combine them (length divided by time is a velocity). Every literal
/// was normalised into its base unit at parse time, so this is a pure check:
/// no conversion instruction is ever emitted.
/// A mismatch diagnostic that points at both operands when they are named.
fn unit_spans(
    left: &Expr,
    right: &Expr,
    a: crate::ast::Dim,
    b: crate::ast::Dim,
    line: u32,
    op: BinOp,
    msg: String,
) -> CheckError {
    let mut spans: Vec<(String, String)> = Vec::new();
    if let Expr::Name(n) = left {
        spans.push((n.clone(), a.describe()));
    }
    if let Expr::Name(n) = right {
        spans.push((n.clone(), b.describe()));
    }
    if spans.len() == 2 {
        at_spans("E_KAZE_UNIT_MISMATCH", line, spans, op.symbol(), msg)
    } else {
        at(
            "E_KAZE_UNIT_MISMATCH",
            line,
            op.symbol(),
            format!("here: {} and {}", a.describe(), b.describe()),
            msg,
        )
    }
}

fn dim_result(
    op: BinOp,
    lt: Ty,
    rt: Ty,
    left: &Expr,
    right: &Expr,
    line: u32,
    errors: &mut Vec<CheckError>,
) -> Ty {
    use BinOp::*;
    let dim_of = |t: Ty| match t {
        Ty::Num(Some(u)) => Some(u.dim()),
        Ty::Num(None) => Some(Dim::NONE),
        _ => None,
    };
    let (da, db) = (dim_of(lt), dim_of(rt));
    let (a, b) = match (da, db) {
        (Some(a), Some(b)) => (a, b),
        _ => return bin_result(op, lt, rt),
    };
    match op {
        Add | Sub => {
            if a != b {
                // Point at both operands when they are named, which is how the
                // roadmap draws this diagnostic.
                let mut spans: Vec<(String, String)> = Vec::new();
                if let Expr::Name(n) = left {
                    spans.push((n.clone(), a.describe()));
                }
                if let Expr::Name(n) = right {
                    spans.push((n.clone(), b.describe()));
                }
                let line_now = if current_line() == 0 { line } else { current_line() };
                let msg = format!(
                    "cannot {} {} and {}: E_KAZE_UNIT_MISMATCH",
                    if op == Add { "add" } else { "subtract" },
                    a.describe(),
                    b.describe()
                );
                errors.push(if spans.len() == 2 {
                    at_spans("E_KAZE_UNIT_MISMATCH", line_now, spans, op.symbol(), msg)
                } else {
                    at(
                        "E_KAZE_UNIT_MISMATCH",
                        line_now,
                        op.symbol(),
                        format!("here: {} and {}", a.describe(), b.describe()),
                        msg,
                    )
                });
                return Ty::Num(unit_for(a));
            }
            Ty::Num(unit_for(a))
        }
        Mul => Ty::Num(unit_for(a.plus(b))),
        Div => Ty::Num(unit_for(a.minus(b))),
        Mod => {
            if a != b {
                let msg = format!(
                    "cannot take the remainder of {} and {}: E_KAZE_UNIT_MISMATCH",
                    a.describe(),
                    b.describe()
                );
                errors.push(unit_spans(left, right, a, b, if current_line() == 0 { line } else { current_line() }, op, msg));
            }
            Ty::Num(unit_for(a))
        }
        Lt | Le | Gt | Ge | Eq | Ne => {
            if a != b && !matches!((lt, rt), (Ty::Str, Ty::Str)) {
                let msg = format!(
                    "cannot compare {} with {}: E_KAZE_UNIT_MISMATCH",
                    a.describe(),
                    b.describe()
                );
                errors.push(unit_spans(left, right, a, b, if current_line() == 0 { line } else { current_line() }, op, msg));
            }
            Ty::Bool
        }
        And | Or => Ty::Bool,
    }
}

/// The canonical unit for a dimension, used as the static type of an
/// arithmetic result. None for a derived dimension with no single spelling.
fn unit_for(d: Dim) -> Option<Unit> {
    match d.quantity() {
        Some("plain number") => None,
        Some("length") => Some(Unit::Meter),
        Some("time") => Some(Unit::Sec),
        Some("mass") => Some(Unit::Kg),
        Some("angle") => Some(Unit::Deg),
        Some("temperature") => Some(Unit::Kelvin),
        Some("pressure") => Some(Unit::Pascal),
        Some("current") => Some(Unit::Ampere),
        Some("velocity") => Some(Unit::Mps),
        Some("acceleration") => Some(Unit::Mps2),
        _ => None,
    }
}

fn bin_result(op: BinOp, lt: Ty, rt: Ty) -> Ty {
    use BinOp::*;
    match op {
        Add | Sub | Mul | Div | Mod => {
            if (lt == Ty::Str || rt == Ty::Str) && op == Add {
                return Ty::Str;
            }
            if (lt == Ty::List || rt == Ty::List) && op == Add {
                return Ty::List;
            }
            let unit = match (lt, rt) {
                (Ty::Num(a), Ty::Num(b)) => match (a, b) {
                    (Some(x), Some(y)) if x != y => {
                        return Ty::Num(Some(x));
                    }
                    (Some(x), _) => Some(x),
                    (_, Some(y)) => Some(y),
                    _ => None,
                },
                (Ty::Num(a), _) => a,
                (_, Ty::Num(b)) => b,
                _ => None,
            };
            Ty::Num(unit)
        }
        Lt | Le | Gt | Ge | Eq | Ne => Ty::Bool,
        And | Or => Ty::Bool,
    }
}

/// Does every path through this body end in `allow`, `reject` or `return`?
fn terminates(stmts: &[Stmt]) -> bool {
    let Some(last) = stmts.last() else { return false };
    match last {
        Stmt::Return(..) => true,
        Stmt::Eval(Expr::Call { recv, op, .. }, _) => {
            caps::family_of(recv) == "policy" && (op == "allow" || op == "reject")
        }
        Stmt::If { then, otherwise, .. } => {
            !otherwise.is_empty() && terminates(then) && terminates(otherwise)
        }
        // Either the body finishes normally and terminates, or it fails and the
        // (also terminating) recovery block finishes. Both paths must terminate,
        // because a try body that falls through still reaches the code after it.
        Stmt::Try { body, otherwise, .. } => terminates(body) && terminates(otherwise),
        _ => false,
    }
}

/// Unit-compatibility errors that `bin_result` cannot report on its own: a
/// comparison between two concrete, different units is almost always a bug in
/// a hardware script, so it is refused rather than guessed at.
pub fn unit_conflict(left: &Expr, right: &Expr) -> Option<(Unit, Unit)> {
    let unit_of = |x: &Expr| match x {
        Expr::Num(_, Unit::None) => None,
        Expr::Num(_, u) => Some(*u),
        _ => None,
    };
    match (unit_of(left), unit_of(right)) {
        (Some(a), Some(b)) if a != b => Some((a, b)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn check_src(src: &str) -> Result<(), Vec<CheckError>> {
        check(&parse(src).expect("parse"))
    }

    fn first_error(src: &str) -> String {
        match check_src(src) {
            Ok(()) => panic!("expected a check error"),
            Err(errs) => errs[0].msg.clone(),
        }
    }

    #[test]
    fn the_four_plan_examples_typecheck() {
        for src in [
            "needs: drone, log\n\naction patrol(points):\n    log \"go\"\n    arm drone\n    takeoff to 15 meters\n    for each point in points:\n        fly to point at 15 meters\n    return to launch\n\nwhen battery < 20%:\n    log warning \"low\"\n    return to launch\n",
            "needs: rover, lidar, log\n\nevery 100ms:\n    let front = lidar.front_distance() otherwise 10 meters\n    if front < 0.5 meters:\n        log warning \"obstacle\"\n        stop rover\n        turn right by 90 degrees\n    otherwise:\n        drive forward at 1.2 m/s\n",
            "needs: sensor, fan, mister, alert\n\ncheck climate(t: Celsius, h: percent):\n    let temperature = sensor.temperature() otherwise 20 Celsius\n    let humidity = sensor.humidity() otherwise 50%\n    if temperature > t:\n        turn on fan\n        send alert \"cooling\"\n    otherwise:\n        turn off fan\n    if humidity < h:\n        pulse mister for 5 seconds\n",
            "needs: memory, policy\n\nfilter user_prompt(prompt):\n    if length of prompt > 2000:\n        reject with \"too long\"\n    if policy detects secrets in prompt:\n        reject with \"secrets\"\n    remember prompt as \"last\"\n    allow prompt\n",
        ] {
            check_src(src).unwrap_or_else(|e| panic!("{}", e[0].msg));
        }
    }

    #[test]
    fn an_undeclared_capability_is_named_with_the_fix() {
        let msg = first_error("needs: log\naction a():\n    arm drone\n");
        assert!(msg.contains("not declared"), "{}", msg);
        assert!(msg.contains("needs: drone"), "{}", msg);
    }

    #[test]
    fn an_unknown_operation_lists_the_real_ones() {
        let msg = first_error("needs: drone\naction a():\n    drone.flyaway()\n");
        assert!(msg.contains("not something a script can ask for"), "{}", msg);
        assert!(msg.contains("takeoff"), "{}", msg);
    }

    #[test]
    fn an_unknown_name_is_refused() {
        let msg = first_error("needs: log\naction a():\n    log missing_name\n");
        assert!(msg.contains("not defined"), "{}", msg);
    }

    #[test]
    fn a_filter_that_can_fall_off_the_end_is_refused() {
        let msg = first_error("needs: policy\n\nfilter f(p):\n    if 1 < 2:\n        allow p\n");
        assert!(msg.contains("without allowing or rejecting"), "{}", msg);
    }

    #[test]
    fn a_filter_with_both_branches_is_accepted() {
        check_src("needs: policy\n\nfilter f(p):\n    if 1 < 2:\n        allow p\n    otherwise:\n        reject with \"no\"\n")
            .expect("balanced filter");
    }

    #[test]
    fn wrong_arity_is_reported() {
        let msg = first_error("needs: drone\naction a():\n    drone.takeoff()\n");
        assert!(msg.contains("takes 1 value"), "{}", msg);
    }

    #[test]
    fn a_non_boolean_condition_is_refused() {
        let msg = first_error("needs: log\naction a():\n    if 5:\n        log \"x\"\n");
        assert!(msg.contains("must be true or false"), "{}", msg);
    }

    #[test]
    fn a_dotted_receiver_must_resolve_to_a_declared_family() {
        // `lidar` is an instance of `sensor`, so declaring the instance is enough…
        check_src("needs: lidar\naction a():\n    let d = lidar.front_distance() otherwise 1 meter\n")
            .expect("alias");
        // …but using a family nobody declared is refused.
        let msg = first_error("needs: drone\naction a():\n    lidar.front_distance() otherwise 1 meter\n");
        assert!(msg.contains("not declared"), "{}", msg);
        assert!(msg.contains("needs: sensor"), "{}", msg);
    }
}
