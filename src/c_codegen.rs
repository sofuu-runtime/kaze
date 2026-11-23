// c_codegen.rs — ANSI/ISO C99 Native Code Generator for Kaze.
//
// Translates a validated Kaze AST (Program) into self-contained, portable C99 code.
// Operates with zero external dependencies and supports both 32-bit and 64-bit architectures.

use super::ast::*;

pub struct CodegenOptions {
    pub standalone: bool,
    pub entry: Option<String>,
}

impl Default for CodegenOptions {
    fn default() -> Self {
        CodegenOptions {
            standalone: true,
            entry: None,
        }
    }
}

/// How a declared parameter type reads in a diagnostic. Debug formatting cannot
/// be used here: a record type formats as Record("Point"), and those quotes would
/// land inside a C string literal and break the generated program.
fn type_word(ty: &crate::ast::TypeRef) -> &'static str {
    use crate::ast::TypeRef;
    match ty {
        TypeRef::Int | TypeRef::Number | TypeRef::Percent => "a number",
        TypeRef::Length => "a length",
        TypeRef::Time => "a time",
        TypeRef::Angle => "an angle",
        TypeRef::Temperature => "a temperature",
        TypeRef::Bool => "true or false",
        TypeRef::Text => "text",
        TypeRef::Record(_) => "a record",
        TypeRef::Untrusted(_) => "untrusted input",
    }
}

pub fn emit_c(program: &Program, opts: &CodegenOptions) -> String {
    let mut out = String::with_capacity(8192);

    // 1. Header & C99 runtime
    out.push_str(C_RUNTIME_HEADER);

    // 2. Forward declarations for records and routines
    out.push_str("/* --- Forward Declarations --- */\n");
    for item in &program.items {
        match &item.kind {
            ItemKind::Record(fields) => {
                let rec_name = sanitize_id(&item.name);
                let params = fields
                    .iter()
                    .map(|f| format!("KzValue {}", sanitize_id(&f.name)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sig_params = if params.is_empty() { "void".to_string() } else { params };
                out.push_str(&format!("KZ_MAYBE_UNUSED static inline KzValue kz_record_new_{}({});\n", rec_name, sig_params));
            }
            ItemKind::Action | ItemKind::Check | ItemKind::Filter | ItemKind::When(_) | ItemKind::Every(_) => {
                let fn_name = match &item.kind {
                    ItemKind::When(_) => format!("hook_when_{}", item.line),
                    ItemKind::Every(_) => format!("hook_every_{}", item.line),
                    _ => sanitize_id(&item.name),
                };
                let params = item
                    .params
                    .iter()
                    // A routine need not read every parameter.
                    .map(|p| format!("KZ_MAYBE_UNUSED KzValue {}", sanitize_id(p)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sig_params = if params.is_empty() { "void".to_string() } else { params };
                out.push_str(&format!("KZ_MAYBE_UNUSED static KzValue kz_fn_{}({});\n", fn_name, sig_params));
            }
        }
    }
    out.push('\n');

    // 3. Record Constructors
    out.push_str("/* --- Record Constructors --- */\n");
    for item in &program.items {
        if let ItemKind::Record(fields) = &item.kind {
            let rec_name = sanitize_id(&item.name);
            let n = fields.len();
            let params = fields
                .iter()
                .map(|f| format!("KzValue {}", sanitize_id(&f.name)))
                .collect::<Vec<_>>()
                .join(", ");
            let sig_params = if params.is_empty() { "void".to_string() } else { params };

            out.push_str(&format!("KZ_MAYBE_UNUSED static inline KzValue kz_record_new_{}({}) {{\n", rec_name, sig_params));
            if n == 0 {
                out.push_str(&format!("    return kz_record_create(\"{}\", 0, NULL, NULL);\n", item.name));
            } else {
                let keys = fields
                    .iter()
                    .map(|f| format!("\"{}\"", f.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                let vals = fields
                    .iter()
                    .map(|f| sanitize_id(&f.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("    static const char* _keys[] = {{ {} }};\n", keys));
                out.push_str(&format!("    KzValue _vals[] = {{ {} }};\n", vals));
                out.push_str(&format!("    return kz_record_create(\"{}\", {}, _keys, _vals);\n", item.name, n));
            }
            out.push_str("}\n\n");
        }
    }

    // 4. Routine Implementations
    out.push_str("/* --- Routines --- */\n");
    // Which parameters a routine may consume in place, computed once from the
    // bodies so the call-site check below cannot be circular.
    let consumed = consumed_params(&program.items);
    for item in &program.items {
        if matches!(
            item.kind,
            ItemKind::Action | ItemKind::Check | ItemKind::Filter | ItemKind::When(_) | ItemKind::Every(_)
        ) {
            let fn_name = match &item.kind {
                ItemKind::When(_) => format!("hook_when_{}", item.line),
                ItemKind::Every(_) => format!("hook_every_{}", item.line),
                _ => sanitize_id(&item.name),
            };
            let params = item
                .params
                .iter()
                // A routine need not read every parameter, so the definition
                // marks them all as possibly unused.
                .map(|p| format!("KZ_MAYBE_UNUSED KzValue {}", sanitize_id(p)))
                .collect::<Vec<_>>()
                .join(", ");
            let sig_params = if params.is_empty() { "void".to_string() } else { params };

            // A script routine nobody calls is not an error: annotate it so a
            // strict build of the generated C stays warning-free.
            out.push_str(&format!("KZ_MAYBE_UNUSED static KzValue kz_fn_{}({}) {{\n", fn_name, sig_params));
            // The declared recursion ceiling is a runtime bound, exactly as in
            // the interpreter (0 means "not declared": the global 256 applies).
            CURRENT_FN.with(|c| *c.borrow_mut() = item.name.clone());
            let appendable = appendable_names(&program.items, item, &consumed);
            let plan = append_plan(item);
            let mut order = 0usize;
            out.push_str(&format!("    static int kz_own_{} = 0;\n", fn_name));
            out.push_str(&format!(
                "    kz_enter_frame(\"{}\", &kz_own_{}, {});\n",
                item.name,
                fn_name,
                item.contracts.recursion_bound.unwrap_or(0)
            ));
            // Preconditions are entry assertions, not documentation.
            for clause in &item.contracts.requires {
                out.push_str(&format!(
                    "    if (!kz_is_truthy({})) kz_trap(\"E_KAZE_CONTRACT\", \"the requires clause at line {} is false for these arguments\");\n",
                    emit_expr(&clause.expr),
                    clause.line
                ));
            }
            for stmt in &item.body {
                emit_stmt_contracts(stmt, &mut out, 1, &item.contracts.ensures, &fn_name, &appendable, &plan, &mut order);
            }
            out.push_str(&format!("    kz_leave_frame(&kz_own_{});\n", fn_name));
            out.push_str("    return kz_nil();\n");
            out.push_str("}\n\n");
        }
    }


    // 4b. The hook driver: `when` and `every` items fire per host tick.
    let mut hooks = String::new();
    let mut has_hooks = false;
    for item in &program.items {
        match &item.kind {
            ItemKind::When(cond) => {
                has_hooks = true;
                let pred = emit_expr(cond);
                hooks.push_str(&format!(
                    "        if (kz_is_truthy({})) {{ kz_fn_hook_when_{}(); }}\n",
                    pred, item.line
                ));
            }
            ItemKind::Every(_) => {
                has_hooks = true;
                hooks.push_str(&format!("        kz_fn_hook_every_{}();\n", item.line));
            }
            _ => {}
        }
    }
    if has_hooks {
        out.push_str("/* --- Hook driver --- */\n");
        out.push_str("KZ_MAYBE_UNUSED static KzValue kz_run_hooks(int argc, char** argv) {\n");
        out.push_str("    int ticks = kz_ticks_from_args(argc, argv);\n");
        out.push_str("    for (int t = 0; t < ticks; t++) {\n");
        out.push_str(&hooks);
        out.push_str("    }\n");
        out.push_str("    return kz_nil();\n");
        out.push_str("}\n\n");
    } else {
        // No hooks: a call that does nothing, so the generated main stays uniform.
        out.push_str("KZ_MAYBE_UNUSED static KzValue kz_run_hooks(int argc, char** argv) {\n");
        out.push_str("    (void)argc; (void)argv; return kz_nil();\n");
        out.push_str("}\n\n");
    }

    // 5. Entry point main if standalone
    if opts.standalone {
        let entry_name = opts.entry.as_deref().unwrap_or("main");
        let entry_item = program.items.iter().find(|it| it.name == entry_name);
        out.push_str("/* --- Main Entry Point --- */\n");
        out.push_str("int main(int argc, char** argv) {\n");
        out.push_str("    kz_memory_limit_from_env();\n    atexit(kz_print_stats);
    atexit(kz_print_concat_sites);\n");
        if let Some(item) = entry_item {
            if item.params.is_empty() {
                out.push_str("    (void)argc; (void)argv;\n");
                out.push_str(&format!("    KzValue res = kz_fn_{}();\n", sanitize_id(entry_name)));
            } else {
                for (i, p) in item.params.iter().enumerate() {
                    out.push_str(&format!("    KzValue arg_{} = kz_resolve_cli_arg(argc, argv, \"{}\", {});\n", i, p, i));
                    // A *typed* parameter with no value from any layer is a
                    // refusal, not a zero — the same rule the interpreter applies.
                    // An untyped one keeps the legacy "nothing", because scripts
                    // such as the compiler's own main() accept a missing argument.
                    // Coerce to the declared type, or refuse — the interpreter's
                    // rule, so a text value in a numeric parameter cannot slip
                    // through the compiled binary either.
                    let kind = match item.contracts.param_types.get(i).and_then(|t| t.clone()) {
                        Some(TypeRef::Int)
                        | Some(TypeRef::Number)
                        | Some(TypeRef::Percent)
                        | Some(TypeRef::Length)
                        | Some(TypeRef::Time)
                        | Some(TypeRef::Angle)
                        | Some(TypeRef::Temperature) => "KZ_PARAM_NUM",
                        Some(TypeRef::Bool) => "KZ_PARAM_BOOL",
                        Some(TypeRef::Text) => "KZ_PARAM_TEXT",
                        Some(TypeRef::Untrusted(inner)) => match *inner {
                            TypeRef::Int
                            | TypeRef::Number
                            | TypeRef::Percent
                            | TypeRef::Length
                            | TypeRef::Time
                            | TypeRef::Angle
                            | TypeRef::Temperature => "KZ_PARAM_NUM",
                            TypeRef::Bool => "KZ_PARAM_BOOL",
                            TypeRef::Text => "KZ_PARAM_TEXT",
                            _ => "KZ_PARAM_ANY",
                        },
                        Some(TypeRef::Record(_)) | None => "KZ_PARAM_ANY",
                    };
                    if kind != "KZ_PARAM_ANY" {
                        out.push_str(&format!(
                            "    arg_{} = kz_coerce_param(arg_{}, {}, \"{}\", \"{}\");\n",
                            i, i, kind, p, entry_name
                        ));
                    }
                    if let Some(Some(ty)) = item.contracts.param_types.get(i).map(|t| t.as_ref()) {
                        out.push_str(&format!(
                            "    if (arg_{}.kind == KZ_VAL_NIL) kz_trap(\"E_KAZE_PARAM\", \"parameter '{}' of '{}' is declared as {} and has no value; provide --arg {}=<value>, the environment variable AGENT_{}, or \\\"{}\\\" in the config file (--config <path> or ./agent.config.json)\");\n",
                            i,
                            p,
                            entry_name,
                            type_word(ty),
                            p,
                            p.to_uppercase(),
                            p
                        ));
                    }
                }
                let args_call = (0..item.params.len()).map(|i| format!("arg_{}", i)).collect::<Vec<_>>().join(", ");
                out.push_str(&format!("    KzValue res = kz_fn_{}({});\n", sanitize_id(entry_name), args_call));
            }
            out.push_str("    if (res.kind == KZ_VAL_NUM) {\n");
            out.push_str("        char numbuf[64];\n");
            out.push_str("        kz_format_number(numbuf, sizeof(numbuf), res.as.number);\n");
            out.push_str("        printf(\"%s\\n\", numbuf);\n");
            out.push_str("    } else if (res.kind == KZ_VAL_STR) {\n");
            out.push_str("        printf(\"%s\\n\", res.as.str.data ? res.as.str.data : \"\");\n");
            out.push_str("    } else if (res.kind == KZ_VAL_BOOL) {\n");
            out.push_str("        printf(\"%s\\n\", res.as.boolean ? \"true\" : \"false\");\n");
            out.push_str("    }\n");
            // A program with an entry point also runs its hooks afterwards, the
            // same order the interpreter uses.
            out.push_str("    kz_run_hooks(argc, argv);\n");
        } else {
            // No entry point: the hooks are the whole program.
            out.push_str("    kz_run_hooks(argc, argv);\n");
            out.push_str("    printf(\"Kaze module loaded (no main entry)\\n\");\n");
        }
        out.push_str("    return 0;\n");
        out.push_str("}\n");
    }

    out
}

// ── in-place list append ─────────────────────────────────────────────────

/// Names inside one routine for which `set v = v + [x]` may append in place.
///
/// A Kaze list is copied on append, and the front end appends in loops, so
/// this is the single largest allocation cost in a compiler written in Kaze.
/// Mutating in place is only sound when nothing else can hold the same list,
/// so a name qualifies only when every mention is one of: the append pattern
/// itself, a read that copies (index, length, for-each, arithmetic,
/// comparison), or the routine's own return value. Any other use -- giving the
/// list to a call, storing it in another list or record, binding it to another
/// name -- disqualifies it.
//
/// A parameter the callee may append to in place: its body only appends to it,
/// reads it without retaining it, and returns it.
/// Parameters a routine may consume in place, as (routine, index) pairs. Built
/// from routine bodies alone, so the analysis is acyclic: bodies decide which
/// parameters are consumable, and callers are then checked against that.
type Consumed = Vec<(String, usize)>;

fn is_consumed(consumed: &Consumed, callee: &str, k: usize) -> bool {
    consumed.iter().any(|(n, i)| n == callee && *i == k)
}

fn consumed_params(items: &[Item]) -> Consumed {
    // A body that hands its accumulator on to a helper is only append-only if
    // the helper consumes it too, so grow the set until it stops changing. The
    // rule is grow-only, so the least fixpoint is sound: every exemption is
    // justified by another one that is itself justified.
    let mut consumed: Consumed = Vec::new();
    for _ in 0..8 {
        let mut next: Consumed = Vec::new();
        for item in items {
            for k in 0..item.params.len() {
                if param_is_append_only(item, k, &consumed) {
                    next.push((item.name.clone(), k));
                }
            }
        }
        if next == consumed {
            break;
        }
        consumed = next;
    }
    consumed
}

fn param_is_append_only(item: &Item, k: usize, consumed: &Consumed) -> bool {
    let Some(name) = item.params.get(k) else {
        return false;
    };
    // The accumulator idiom is `let out = list` followed by appends to `out`,
    // so one alias of the parameter is followed. The alias binding itself is
    // the only allowed mention of the parameter.
    let alias = first_binding_of(&item.body, name);
    let subject = alias.clone().unwrap_or_else(|| name.clone());
    let mut appended = false;
    let mut returns_it = false;
    if param_usage(&item.body, name, alias.as_deref(), consumed, &mut appended, &mut returns_it).is_none() {
        return false;
    }
    if alias.is_some() && subject == *name {
        return false;
    }
    appended && returns_it
}

/// The first `let x = param` in the body, if any.
fn first_binding_of(stmts: &[Stmt], param: &str) -> Option<String> {
    for stmt in stmts {
        match stmt {
            Stmt::Let(target, value, _) => {
                if matches!(value, Expr::Name(n) if n == param) {
                    return Some(target.clone());
                }
            }
            Stmt::If { then, otherwise, .. } => {
                if let Some(a) = first_binding_of(then, param) {
                    return Some(a);
                }
                if let Some(a) = first_binding_of(otherwise, param) {
                    return Some(a);
                }
            }
            Stmt::ForEach { body, .. } | Stmt::While { body, .. } => {
                if let Some(a) = first_binding_of(body, param) {
                    return Some(a);
                }
            }
            Stmt::Try { body, otherwise, .. } => {
                if let Some(a) = first_binding_of(body, param) {
                    return Some(a);
                }
                if let Some(a) = first_binding_of(otherwise, param) {
                    return Some(a);
                }
            }
            _ => {}
        }
    }
    None
}

/// Walk the body for a parameter (and its one alias). `None` means the value is
/// used in a way that would alias it: rebound, bound to a second name, handed
/// to a call, or stored inside another value.
fn param_usage(
    stmts: &[Stmt],
    param: &str,
    alias: Option<&str>,
    consumed: &Consumed,
    appended: &mut bool,
    returns_it: &mut bool,
) -> Option<()> {
    let subject = alias.unwrap_or(param);
    for stmt in stmts {
        match stmt {
            Stmt::Let(target, value, _) => {
                // The alias binding itself: `let out = list`.
                if Some(target.as_str()) == alias && matches!(value, Expr::Name(n) if n == param) {
                    continue;
                }
                if target == param || target == subject {
                    return None;
                }
                if matches!(value, Expr::Name(n) if n == param || n == subject) {
                    // only `let <alias> = <param>` is allowed, and only once
                    if !(matches!(value, Expr::Name(n) if n == param)
                        && Some(target.as_str()) == alias)
                    {
                        return None;
                    }
                }
            }
            Stmt::Set(target, value, _) => {
                if target == param || target == subject {
                    if is_append_pattern(target, value).is_some() {
                        *appended = true;
                    } else if let Expr::CallRoutine { name, args, .. } = value {
                        // A consuming rebind: the callee appends in place and
                        // hands the same list back.
                        let position = args
                            .iter()
                            .position(|a| matches!(a, Expr::Name(n) if n == subject));
                        match position {
                            Some(k) if is_consumed(consumed, name, k) => {
                                // The chain grows the list further down, so this
                                // counts as an append for the caller's purposes.
                                *appended = true;
                            }
                            _ => return None,
                        }
                    } else {
                        return None;
                    }
                }
            }
            Stmt::Return(Some(v), _) => {
                if matches!(v, Expr::Name(n) if n == subject || n == param) {
                    *returns_it = true;
                }
            }
            Stmt::If { then, otherwise, .. } => {
                param_usage(then, param, alias, consumed, appended, returns_it)?;
                param_usage(otherwise, param, alias, consumed, appended, returns_it)?;
            }
            Stmt::ForEach { body, .. } | Stmt::While { body, .. } => {
                param_usage(body, param, alias, consumed, appended, returns_it)?;
            }
            Stmt::Try { body, otherwise, .. } => {
                param_usage(body, param, alias, consumed, appended, returns_it)?;
                param_usage(otherwise, param, alias, consumed, appended, returns_it)?;
            }
            _ => {}
        }
    }
    Some(())
}


fn unique_locals(item: &Item, consumed: &Consumed) -> std::collections::HashSet<String> {
    let mut bound: std::collections::HashSet<String> = std::collections::HashSet::new();
    collect_bound_names(&item.body, &mut bound);
    let mut escaped: std::collections::HashSet<String> = std::collections::HashSet::new();
    scan_escapes_stmts(&item.body, &bound, &mut escaped, consumed);
    bound.retain(|n| !escaped.contains(n) && !item.params.iter().any(|p| p == n));
    bound
}

fn collect_bound_names(stmts: &[Stmt], out: &mut std::collections::HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let(name, _, _) => {
                out.insert(name.clone());
            }
            Stmt::If { then, otherwise, .. } => {
                collect_bound_names(then, out);
                collect_bound_names(otherwise, out);
            }
            Stmt::ForEach { var, body, .. } => {
                out.insert(var.clone());
                collect_bound_names(body, out);
            }
            Stmt::While { body, .. } => collect_bound_names(body, out),
            Stmt::Try { body, otherwise, error_name, .. } => {
                if let Some(n) = error_name {
                    out.insert(n.clone());
                }
                collect_bound_names(body, out);
                collect_bound_names(otherwise, out);
            }
            _ => {}
        }
    }
}

/// Every call site must pass a *name* at that position whose list is unshared in
/// the calling routine; otherwise the callee would grow a list someone else can
/// still see as unchanged.
fn param_call_sites_share_nothing(
    program: &[Item],
    callee: &Item,
    k: usize,
    consumed: &Consumed,
) -> bool {
    let mut sites = 0usize;
    let mut ok = true;
    for caller in program {
        let local = unique_locals(caller, consumed);
        walk_call_args(&caller.body, &callee.name, &mut |args| {
            sites += 1;
            match args.get(k) {
                Some(Expr::Name(v)) if local.contains(v) => {}
                _ => ok = false,
            }
        });
    }
    sites > 0 && ok
}

fn walk_call_args(stmts: &[Stmt], callee: &str, f: &mut impl FnMut(&[Expr])) {
    for stmt in stmts {
        match stmt {
            Stmt::Let(_, v, _) | Stmt::Set(_, v, _) | Stmt::Eval(v, _) | Stmt::Guard(v, _) => {
                walk_call_args_expr(v, callee, f)
            }
            Stmt::Return(Some(v), _) => walk_call_args_expr(v, callee, f),
            Stmt::Return(None, _) => {}
            Stmt::If { cond, then, otherwise, .. } => {
                walk_call_args_expr(cond, callee, f);
                walk_call_args(then, callee, f);
                walk_call_args(otherwise, callee, f);
            }
            Stmt::ForEach { list, body, .. } => {
                walk_call_args_expr(list, callee, f);
                walk_call_args(body, callee, f);
            }
            Stmt::While { cond, body, .. } => {
                walk_call_args_expr(cond, callee, f);
                walk_call_args(body, callee, f);
            }
            Stmt::Try { body, otherwise, .. } => {
                walk_call_args(body, callee, f);
                walk_call_args(otherwise, callee, f);
            }
        }
    }
}

fn walk_call_args_expr(expr: &Expr, callee: &str, f: &mut impl FnMut(&[Expr])) {
    match expr {
        Expr::CallRoutine { name, args, .. } => {
            if name == callee {
                f(args);
            }
            for a in args {
                walk_call_args_expr(a, callee, f);
            }
        }
        Expr::Call { args, .. } | Expr::Math { args, .. } | Expr::List(args) => {
            for a in args {
                walk_call_args_expr(a, callee, f);
            }
        }
        Expr::RecordInit { fields, .. } => {
            for (_, v) in fields {
                walk_call_args_expr(v, callee, f);
            }
        }
        Expr::Bin(_, l, r) => {
            walk_call_args_expr(l, callee, f);
            walk_call_args_expr(r, callee, f);
        }
        Expr::Not(inner) | Expr::Len(inner) => walk_call_args_expr(inner, callee, f),
        Expr::Index(c, i) => {
            walk_call_args_expr(c, callee, f);
            walk_call_args_expr(i, callee, f);
        }
        Expr::Fallback { inner, fallback, .. } => {
            walk_call_args_expr(inner, callee, f);
            walk_call_args_expr(fallback, callee, f);
        }
        _ => {}
    }
}

/// Locals only: the proof that never depends on another routine.
fn local_appendable_names(item: &Item, consumed: &Consumed) -> std::collections::HashSet<String> {
    let mut candidates: std::collections::HashSet<String> = std::collections::HashSet::new();
    collect_appends(&item.body, &mut candidates);
    let mut escaped: std::collections::HashSet<String> = std::collections::HashSet::new();
    scan_escapes_stmts(&item.body, &candidates, &mut escaped, consumed);
    // A name bound from a parameter is an alias of the caller's list: appending
    // to it in place would grow a list the caller still holds. Only a parameter
    // the callee is allowed to consume (see appendable_names) may be mutated.
    let mut aliases_of_params: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (_, bound) in bound_from_param(&item.body, &item.params) {
        aliases_of_params.insert(bound);
    }
    candidates.retain(|n| {
        !escaped.contains(n) && !item.params.iter().any(|p| p == n) && !aliases_of_params.contains(n)
    });
    candidates
}

/// Every `let x = <param>` binding in the body.
fn bound_from_param(stmts: &[Stmt], params: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for stmt in stmts {
        match stmt {
            Stmt::Let(target, Expr::Name(n), _) if params.iter().any(|p| p == n) => {
                out.push((n.clone(), target.clone()));
            }
            Stmt::If { then, otherwise, .. } => {
                out.extend(bound_from_param(then, params));
                out.extend(bound_from_param(otherwise, params));
            }
            Stmt::ForEach { body, .. } | Stmt::While { body, .. } => {
                out.extend(bound_from_param(body, params));
            }
            Stmt::Try { body, otherwise, .. } => {
                out.extend(bound_from_param(body, params));
                out.extend(bound_from_param(otherwise, params));
            }
            _ => {}
        }
    }
    out
}

/// Locals plus parameters the callee may consume in place.
fn appendable_names(
    program: &[Item],
    item: &Item,
    consumed: &Consumed,
) -> std::collections::HashSet<String> {
    let mut out = local_appendable_names(item, consumed);
    for (k, p) in item.params.iter().enumerate() {
        if param_is_append_only(item, k, consumed)
            && param_call_sites_share_nothing(program, item, k, consumed)
        {
            out.insert(p.clone());
            // The accumulator idiom writes through a local bound from the
            // parameter: `let out = list` then appends to `out`. When the
            // parameter itself is consumable, its alias is the same list.
            for (from, to) in bound_from_param(&item.body, &item.params) {
                if &from == p {
                    out.insert(to);
                }
            }
        }
    }
    out
}

fn is_append_pattern<'a>(name: &str, expr: &'a Expr) -> Option<&'a Expr> {
    if let Expr::Bin(BinOp::Add, left, right) = expr {
        if matches!(&**left, Expr::Name(n) if n == name) {
            if let Expr::List(items) = &**right {
                if items.len() == 1 {
                    return Some(&items[0]);
                }
            }
        }
    }
    None
}

fn collect_appends(stmts: &[Stmt], out: &mut std::collections::HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Set(name, value, _) => {
                if is_append_pattern(name, value).is_some() {
                    out.insert(name.clone());
                }
                collect_appends_expr(value, out);
            }
            Stmt::Let(_, v, _) | Stmt::Eval(v, _) | Stmt::Guard(v, _) => collect_appends_expr(v, out),
            Stmt::Return(Some(v), _) => collect_appends_expr(v, out),
            Stmt::Return(None, _) => {}
            Stmt::If { cond, then, otherwise, .. } => {
                collect_appends_expr(cond, out);
                collect_appends(then, out);
                collect_appends(otherwise, out);
            }
            Stmt::ForEach { list, body, .. } => {
                collect_appends_expr(list, out);
                collect_appends(body, out);
            }
            Stmt::While { cond, body, .. } => {
                collect_appends_expr(cond, out);
                collect_appends(body, out);
            }
            Stmt::Try { body, otherwise, .. } => {
                collect_appends(body, out);
                collect_appends(otherwise, out);
            }
        }
    }
}

#[allow(clippy::only_used_in_recursion)]
fn collect_appends_expr(expr: &Expr, out: &mut std::collections::HashSet<String>) {
    match expr {
        Expr::Bin(BinOp::Add, l, r) => {
            // a pattern inside a larger expression is not a statement append
            collect_appends_expr(l, out);
            collect_appends_expr(r, out);
        }
        Expr::Bin(_, l, r) => {
            collect_appends_expr(l, out);
            collect_appends_expr(r, out);
        }
        Expr::Call { args, .. } | Expr::CallRoutine { args, .. } | Expr::Math { args, .. } => {
            for a in args {
                collect_appends_expr(a, out);
            }
        }
        Expr::List(items) => {
            for a in items {
                collect_appends_expr(a, out);
            }
        }
        Expr::RecordInit { fields, .. } => {
            for (_, v) in fields {
                collect_appends_expr(v, out);
            }
        }
        Expr::Not(inner) | Expr::Len(inner) => collect_appends_expr(inner, out),
        Expr::Index(c, i) => {
            collect_appends_expr(c, out);
            collect_appends_expr(i, out);
        }
        Expr::Fallback { inner, fallback, .. } => {
            collect_appends_expr(inner, out);
            collect_appends_expr(fallback, out);
        }
        _ => {}
    }
}

fn scan_escapes_stmts(
    stmts: &[Stmt],
    candidates: &std::collections::HashSet<String>,
    escaped: &mut std::collections::HashSet<String>,
    consumed: &Consumed,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Set(name, value, _) => {
                if let Some(item) = is_append_pattern(name, value) {
                    // the appended element is stored inside the list
                    scan_retained(item, true, escaped, consumed);
                } else {
                    // set other = v also aliases the list
                    if let Expr::Name(n) = value {
                        escaped.insert(n.clone());
                    }
                    scan_retained(value, false, escaped, consumed);
                }
            }
            Stmt::Let(name, value, _) => {
                // A second binding of the same list aliases it.
                if let Expr::Name(n) = value {
                    let _ = name;
                    escaped.insert(n.clone());
                }
                scan_retained(value, false, escaped, consumed);
            }
            Stmt::Eval(v, _) => scan_retained(v, false, escaped, consumed),
            Stmt::Guard(v, _) => scan_retained(v, false, escaped, consumed),
            Stmt::Return(Some(v), _) => scan_retained(v, false, escaped, consumed),
            Stmt::Return(None, _) => {}
            Stmt::If { cond, then, otherwise, .. } => {
                scan_retained(cond, false, escaped, consumed);
                scan_escapes_stmts(then, candidates, escaped, consumed);
                scan_escapes_stmts(otherwise, candidates, escaped, consumed);
            }
            Stmt::ForEach { body, .. } => scan_escapes_stmts(body, candidates, escaped, consumed),
            Stmt::While { cond, body, .. } => {
                scan_retained(cond, false, escaped, consumed);
                scan_escapes_stmts(body, candidates, escaped, consumed);
            }
            Stmt::Try { body, otherwise, .. } => {
                scan_escapes_stmts(body, candidates, escaped, consumed);
                scan_escapes_stmts(otherwise, candidates, escaped, consumed);
            }
        }
    }
    let _ = candidates;
}

/// Walk an expression, marking a candidate name only where the value would be
/// retained by something else. `retained` says whether this position hands the
/// value on: a call argument, a list or record element, a record field.
fn scan_retained(
    expr: &Expr,
    retained: bool,
    escaped: &mut std::collections::HashSet<String>,
    consumed: &Consumed,
) {
    match expr {
        Expr::Name(n) => {
            if retained {
                escaped.insert(n.clone());
            }
        }
        // Reads that cannot retain the value.
        Expr::Len(inner) | Expr::Not(inner) => scan_retained(inner, false, escaped, consumed),
        Expr::Index(c, i) => {
            scan_retained(c, false, escaped, consumed);
            scan_retained(i, false, escaped, consumed);
        }
        Expr::Bin(_, l, r) => {
            scan_retained(l, false, escaped, consumed);
            scan_retained(r, false, escaped, consumed);
        }
        // Handing the value to something that can keep it -- except an argument
        // the callee consumes in place and hands back, which keeps the caller
        // the only holder.
        Expr::CallRoutine { name, args, .. } => {
            for (k, a) in args.iter().enumerate() {
                if is_consumed(consumed, name, k) {
                    scan_retained(a, false, escaped, consumed);
                } else {
                    scan_retained(a, true, escaped, consumed);
                }
            }
        }
        Expr::Call { args, .. } | Expr::Math { args, .. } => {
            for a in args {
                scan_retained(a, true, escaped, consumed);
            }
        }
        Expr::List(items) => {
            for a in items {
                scan_retained(a, true, escaped, consumed);
            }
        }
        Expr::RecordInit { fields, .. } => {
            for (_, v) in fields {
                scan_retained(v, true, escaped, consumed);
            }
        }
        Expr::Fallback { inner, fallback, .. } => {
            scan_retained(inner, retained, escaped, consumed);
            scan_retained(fallback, retained, escaped, consumed);
        }
        Expr::Field { base, .. } => {
            escaped.insert(base.clone());
        }
        _ => {}
    }
}

/// Emit one statement, knowing the routine's postconditions: a `return` binds
/// `result`, checks every `ensures` clause, leaves the recursion frame and only
/// then returns.
#[allow(clippy::too_many_arguments)]
fn emit_stmt_contracts(
    stmt: &Stmt,
    out: &mut String,
    indent: usize,
    ensures: &[Clause],
    counter: &str,
    appendable: &std::collections::HashSet<String>,
    plan: &std::collections::HashMap<usize, bool>,
    order: &mut usize,
) {
    let here = *order;
    *order += 1;
    let ind = "    ".repeat(indent);
    match stmt {
        Stmt::Return(value, _) if !ensures.is_empty() => {
            let expr = value
                .as_ref()
                .map(emit_expr)
                .unwrap_or_else(|| "kz_nil()".to_string());
            // Braces and a name no script can collide with: a routine may have
            // its own local called `result`, and two returns may share a block.
            out.push_str(&format!("{}{{\n", ind));
            // Bind the returned value first: a script may have its own local
            // named result, and it has to be read before the name is shadowed.
            out.push_str(&format!("{}    KzValue _kz_ret_value = {};\n", ind, expr));
            out.push_str(&format!("{}    KzValue result = _kz_ret_value;\n", ind));
            let scope = format!("{}    ", ind);
            for clause in ensures {
                out.push_str(&format!(
                    "{}if (!kz_is_truthy({})) kz_trap(\"E_KAZE_CONTRACT\", \"the ensures clause at line {} is false for this result\");\n",
                    scope,
                    emit_expr(&clause.expr),
                    clause.line
                ));
            }
            out.push_str(&format!("{}    kz_leave_frame(&kz_own_{});\n", ind, counter));
            out.push_str(&format!("{}    return result;\n", ind));
            out.push_str(&format!("{}}}\n", ind));
        }
        Stmt::Return(value, _) => {
            // The value may be a fallback: build it lazily, then bind it before
            // leaving the frame (a tail call must run at this depth).
            let mut block = String::new();
            let mut lazy: Option<String> = None;
            if let Some(Expr::Fallback { inner, fallback, .. }) = value.as_ref() {
                lazy = lazy_fallback(inner, fallback, &ind, &mut block);
            }
            let mut prelude = String::new();
            let expr = match &lazy {
                Some(t) => t.clone(),
                None => {
                    let (code, pre) = with_hoisting(|| {
                        value
                            .as_ref()
                            .map(emit_expr)
                            .unwrap_or_else(|| "kz_nil()".to_string())
                    });
                    prelude = pre;
                    code
                }
            };
            if lazy.is_some() {
                out.push_str(&block);
            } else if !prelude.is_empty() {
                out.push_str(&block);
                out.push_str(&prelude);
                out.push_str(&format!("{}{{\n", ind));
                out.push_str(&format!("{}    KzValue _ret = {};\n", ind, expr));
                out.push_str(&format!("{}    kz_leave_frame(&kz_own_{});\n", ind, counter));
                out.push_str(&format!("{}    return _ret;\n", ind));
                out.push_str(&format!("{}}}\n", ind));
                let _ = &ensures;
                return;
            }
            let pad = if lazy.is_some() { "    " } else { "" };
            out.push_str(&format!("{}{}{{\n", ind, pad));
            out.push_str(&format!("{}{}    KzValue _ret = {};\n", ind, pad, expr));
            out.push_str(&format!("{}{}    kz_leave_frame(&kz_own_{});\n", ind, pad, counter));
            out.push_str(&format!("{}{}    return _ret;\n", ind, pad));
            out.push_str(&format!("{}{}}}\n", ind, pad));
            if lazy.is_some() {
                // close the block that holds the operand temporaries
                out.push_str(&format!("{}}}\n", ind));
            }
            let _ = &ensures;
        }
        // Every other statement is emitted as usual, but the branches of an if
        // must also see the postconditions.
        Stmt::If { cond, then, otherwise, .. } => {
            let c = emit_expr(cond);
            out.push_str(&format!("{}if (kz_is_truthy({})) {{\n", ind, c));
            for s in then {
                emit_stmt_contracts(s, out, indent + 1, ensures, counter, appendable, plan, order);
            }
            if !otherwise.is_empty() {
                out.push_str(&format!("{}}} else {{\n", ind));
                for s in otherwise {
                    emit_stmt_contracts(s, out, indent + 1, ensures, counter, appendable, plan, order);
                }
            }
            out.push_str(&format!("{}}}\n", ind));
        }
        other => emit_stmt_impl(here, other, out, indent, Some(counter), appendable, plan, order),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_stmt(
    stmt: &Stmt,
    out: &mut String,
    indent: usize,
    counter: Option<&str>,
    appendable: &std::collections::HashSet<String>,
    plan: &std::collections::HashMap<usize, bool>,
    order: &mut usize,
) {
    let here = *order;
    *order += 1;
    emit_stmt_impl(here, stmt, out, indent, counter, appendable, plan, order);
}

#[allow(clippy::too_many_arguments)]
fn emit_stmt_impl(
    here: usize,
    stmt: &Stmt,
    out: &mut String,
    indent: usize,
    counter: Option<&str>,
    appendable: &std::collections::HashSet<String>,
    plan: &std::collections::HashMap<usize, bool>,
    order: &mut usize,
) {
    let ind = "    ".repeat(indent);
    match stmt {
        Stmt::Let(var, expr, _) => {
            let id = sanitize_id(var);
            if let Expr::Fallback { inner, fallback, .. } = expr {
                // Declare the binding outside the block: the synthetic block
                // only exists to hold the operand temporaries.
                let mut block = String::new();
                if let Some(ternary) = lazy_fallback(inner, fallback, &ind, &mut block) {
                    out.push_str(&format!("{}KzValue {};\n", ind, id));
                    out.push_str(&block);
                    out.push_str(&format!("{}    {} = {};\n", ind, id, ternary));
                    out.push_str(&format!("{}}}\n", ind));
                    return;
                }
            }
            let (val, prelude) = with_hoisting(|| emit_expr(expr));
            if prelude.is_empty() {
                out.push_str(&format!("{}KZ_MAYBE_UNUSED KzValue {} = {};\n", ind, id, val));
            } else {
                out.push_str(&format!("{}KzValue {};\n", ind, id));
                let body = format!("{} = {};
", id, val);
                out.push_str(&wrap_with_prelude(&prelude, &ind, &body));
            }
        }
        Stmt::Set(var, expr, _) => {
            let id = sanitize_id(var);
            // Assigning a name to itself changes nothing.
            if matches!(expr, Expr::Name(n) if n == var) {
                return;
            }
            if let Expr::Fallback { inner, fallback, .. } = expr {
                if let Some(ternary) = lazy_fallback(inner, fallback, &ind, out) {
                    out.push_str(&format!("{}    {} = {};\n", ind, id, ternary));
                    out.push_str(&format!("{}}}\n", ind));
                    return;
                }
            }
            // A proved unique owner appends in place instead of copying the list
            // on every iteration.
            if appendable.contains(var) || plan.get(&here).copied().unwrap_or(false) {
                if let Some(item) = is_append_pattern(var, expr) {
                    out.push_str(&format!(
                        "{}kz_list_push_own(&{}, {});\n",
                        ind,
                        id,
                        emit_expr(item)
                    ));
                    return;
                }
            }
            let (val, prelude) = with_hoisting(|| emit_expr(expr));
            let body = format!("{} = {};
", id, val);
            out.push_str(&wrap_with_prelude(&prelude, &ind, &body));
        }
        Stmt::Eval(expr, _) => {
            let val = emit_expr(expr);
            out.push_str(&format!("{}(void)({});\n", ind, val));
        }
        Stmt::If { cond, then, otherwise, .. } => {
            let c = emit_expr(cond);
            out.push_str(&format!("{}if (kz_is_truthy({})) {{\n", ind, c));
            for s in then {
                emit_stmt(s, out, indent + 1, counter, appendable, plan, order);
            }
            if !otherwise.is_empty() {
                out.push_str(&format!("{}}} else {{\n", ind));
                for s in otherwise {
                    emit_stmt(s, out, indent + 1, counter, appendable, plan, order);
                }
            }
            out.push_str(&format!("{}}}\n", ind));
        }
        Stmt::While { cond, body, bound, .. } => {
            let c = emit_expr(cond);
            out.push_str(&format!("{}{{\n", ind));
            out.push_str(&format!("{}    long _kz_loop_guard = 0;\n", ind));
            if let Some(n) = bound {
                out.push_str(&format!(
                    "{}    const long _kz_loop_bound = {}L;\n",
                    ind, n
                ));
            } else {
                out.push_str(&format!("{}    const long _kz_loop_bound = -1L;\n", ind));
            }
            out.push_str(&format!("{}    while (kz_is_truthy({})) {{\n", ind, c));
            out.push_str(&format!(
                "{}        if (_kz_loop_bound >= 0 && ++_kz_loop_guard > _kz_loop_bound) {{\n",
                ind
            ));
            out.push_str(&format!(
                "{}            fprintf(stderr, \"E_KAZE_LOOP_BOUND: loop exceeded its declared bound\\n\");\n",
                ind
            ));
            out.push_str(&format!("{}            return kz_nil();\n{}        }}\n", ind, ind));
            for s in body {
                emit_stmt(s, out, indent + 2, counter, appendable, plan, order);
            }
            out.push_str(&format!("{}    }}\n", ind));
            out.push_str(&format!("{}}}\n", ind));
        }
        Stmt::ForEach { var, list, body, .. } => {
            let id = sanitize_id(var);
            let l = emit_expr(list);
            out.push_str(&format!("{}{{\n", ind));
            out.push_str(&format!("{}    KzValue _iter_target = {};\n", ind, l));
            out.push_str(&format!("    {}if (_iter_target.kind == KZ_VAL_LIST && _iter_target.as.list) {{\n", ind));
            out.push_str(&format!("    {}    size_t _len = _iter_target.as.list->len;\n", ind));
            out.push_str(&format!("    {}    for (size_t _i = 0; _i < _len; _i++) {{\n", ind));
            out.push_str(&format!("    {}        KzValue {} = _iter_target.as.list->items[_i];\n", ind, id));
            for s in body {
                emit_stmt(s, out, indent + 2, counter, appendable, plan, order);
            }
            out.push_str(&format!("    {}    }}\n", ind));
            out.push_str(&format!("    {}}}\n", ind));
            out.push_str(&format!("{}}}\n", ind));
        }
        Stmt::Try { body, otherwise, error_name, .. } => {
            out.push_str(&format!("{}{{\n", ind));
            out.push_str(&format!("{}    /* try block */\n", ind));
            for s in body {
                emit_stmt(s, out, indent + 1, counter, appendable, plan, order);
            }
            if !otherwise.is_empty() {
                out.push_str(&format!("{}    if (0) {{\n", ind));
                if let Some(err_n) = error_name {
                    let eid = sanitize_id(err_n);
                    out.push_str(&format!("{}        KzValue {} = kz_nil();\n", ind, eid));
                }
                for s in otherwise {
                    emit_stmt(s, out, indent + 2, counter, appendable, plan, order);
                }
                out.push_str(&format!("{}    }}\n", ind));
            }
            out.push_str(&format!("{}}}\n", ind));
        }
        Stmt::Guard(cond, line) => {
            let c = emit_expr(cond);
            out.push_str(&format!(
                "{}if (!kz_is_truthy({})) {{ fprintf(stderr, \"E_KAZE_GUARD: the invariant at line {}\\n\"); return kz_nil(); }}\n",
                ind, c, line
            ));
        }
        Stmt::Return(expr, _) => {
            // A fallback in a return value is lazy too: operands once, fallback
            // only when the operation fails. The temporaries live in a block of
            // their own, then the value is bound and the frame is left.
            let mut block = String::new();
            let mut lazy: Option<String> = None;
            if let Some(Expr::Fallback { inner, fallback, .. }) = expr.as_ref() {
                lazy = lazy_fallback(inner, fallback, &ind, &mut block);
            }
            let v = match &lazy {
                Some(t) => t.clone(),
                None => expr
                    .as_ref()
                    .map(emit_expr)
                    .unwrap_or_else(|| "kz_nil()".to_string()),
            };
            if lazy.is_some() {
                out.push_str(&block);
            }
            // A return nested in a loop or a try still has to leave the frame,
            // or a routine's own-frame counter would only ever grow.
            match counter {
                Some(name) => {
                    out.push_str(&format!("{}    KzValue _ret = {};\n", ind, v));
                    out.push_str(&format!("{}    kz_leave_frame(&kz_own_{});\n", ind, name));
                    out.push_str(&format!("{}    return _ret;\n", ind));
                }
                None => out.push_str(&format!("{}{}return {};\n", ind, if lazy.is_some() { "    " } else { "" }, v)),
            }
            if lazy.is_some() {
                out.push_str(&format!("{}}}\n", ind));
            }
        }
    }
}


// ── hoisting effectful operands ─────────────────────────────────────────
//
// A fallback nested inside a larger expression is lazy when its operands are
// pure (evaluating them twice is unobservable). When an operand has effects,
// the statement form is rewritten instead: the operands are evaluated once
// into temporaries declared before the statement, and the fallback stays
// inside a ternary. That keeps every fallback lazy without GNU statement
// expressions.

thread_local! {
    static HOIST: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// The routine currently being emitted, so an allocation can be attributed to
    /// the script routine that caused it.
    static CURRENT_FN: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn hoisting_active() -> bool {
    HOIST.with(|h| h.borrow().is_some())
}

/// Declare `code` as a temporary in the enclosing statement's prelude and
/// return the temporary's name.
fn hoist_temp(code: String) -> String {
    HOIST.with(|h| {
        let mut borrow = h.borrow_mut();
        let Some(buf) = borrow.as_mut() else {
            return code;
        };
        let name = format!("_kz_h{}", buf.matches("_kz_h").count());
        buf.push_str(&format!("    KzValue {} = {};\n", name, code));
        name
    })
}

/// Run `f` with operand hoisting enabled, returning its result and the prelude
/// of temporaries it produced.
fn with_hoisting<T>(f: impl FnOnce() -> T) -> (T, String) {
    HOIST.with(|h| *h.borrow_mut() = Some(String::new()));
    let value = f();
    let prelude = HOIST.with(|h| h.borrow_mut().take().unwrap_or_default());
    (value, prelude)
}

/// Wrap a statement's text in a block when a prelude was produced.
fn wrap_with_prelude(prelude: &str, ind: &str, body: &str) -> String {
    if prelude.is_empty() {
        return body.to_string();
    }
    let mut out = String::new();
    out.push_str(&format!("{}{{\n", ind));
    out.push_str(prelude);
    for line in body.lines() {
        out.push_str(&format!("    {}\n", line));
    }
    out.push_str(&format!("{}}}\n", ind));
    out
}

// ── flow-sensitive ownership ────────────────────────────────────────────
//
// The set-based rule can only append in place when a name never escapes at all.
// Most real code appends while building a value and hands it over *afterwards*
// (`set kids to kids + [c]` in a loop, then `make_node(..., kids, ...)`), so
// the useful rule is ordered: a local owns its list from creation until its
// first escape, and the appends before that point can mutate in place.
//
// `append_plan` returns, for each statement in emission order, whether the
// append pattern at that statement may mutate in place.

fn append_plan(item: &Item) -> std::collections::HashMap<usize, bool> {
    use std::collections::{HashMap, HashSet};
    let params: HashSet<&str> = item.params.iter().map(|s| s.as_str()).collect();
    // order of the first escaping use of each name
    let mut escapes: HashMap<String, usize> = HashMap::new();
    // order of the last append-pattern use of each name
    let mut last_append: HashMap<String, usize> = HashMap::new();
    let mut appends: HashMap<String, Vec<usize>> = HashMap::new();
    let mut order = 0usize;

    plan_walk_stmts(&item.body, &params, &mut order, &mut escapes, &mut last_append, &mut appends);

    let mut plan: HashMap<usize, bool> = HashMap::new();
    for (name, positions) in appends {
        let first_escape = escapes.get(&name).copied().unwrap_or(usize::MAX);
        for at in positions {
            // Everything before the first escape happens while the local is the
            // only holder; later appends fall back to the copying form.
            plan.insert(at, at < first_escape);
        }
    }
    plan
}

fn plan_walk_stmts(
    stmts: &[Stmt],
    params: &std::collections::HashSet<&str>,
    order: &mut usize,
    escapes: &mut std::collections::HashMap<String, usize>,
    last_append: &mut std::collections::HashMap<String, usize>,
    appends: &mut std::collections::HashMap<String, Vec<usize>>,
) {
    for stmt in stmts {
        let here = *order;
        *order += 1;
        match stmt {
            Stmt::Set(name, value, _) => {
                // A parameter is never in-place appended by this rule: the caller
                // may hold another name for the same list. Only locals are owned
                // from creation, and only until their first escape.
                if is_append_pattern(name, value).is_some() && !params.contains(name.as_str()) {
                    appends.entry(name.clone()).or_default().push(here);
                    last_append.insert(name.clone(), here);
                    // the appended element may escape something else
                    plan_walk_expr(value, params, here, escapes);
                } else {
                    // a rebind to a non-append value: only fresh values keep
                    // ownership, so treat any name use as an escape
                    plan_walk_expr(value, params, here, escapes);
                    escapes.entry(name.clone()).or_insert(here);
                }
            }
            Stmt::Let(name, value, _) => {
                plan_walk_expr(value, params, here, escapes);
                if !matches!(value, Expr::List(_) | Expr::RecordInit { .. }) {
                    // `let x = f()` may alias something else
                    escapes.entry(name.clone()).or_insert(here);
                }
            }
            Stmt::Eval(v, _) | Stmt::Guard(v, _) => plan_walk_expr(v, params, here, escapes),
            Stmt::Return(Some(v), _) => plan_walk_expr(v, params, here, escapes),
            Stmt::Return(None, _) => {}
            Stmt::If { cond, then, otherwise, .. } => {
                plan_walk_expr(cond, params, here, escapes);
                plan_walk_stmts(then, params, order, escapes, last_append, appends);
                plan_walk_stmts(otherwise, params, order, escapes, last_append, appends);
            }
            Stmt::ForEach { list, body, .. } => {
                plan_walk_expr(list, params, here, escapes);
                plan_walk_stmts(body, params, order, escapes, last_append, appends);
            }
            Stmt::While { cond, body, .. } => {
                plan_walk_expr(cond, params, here, escapes);
                plan_walk_stmts(body, params, order, escapes, last_append, appends);
            }
            Stmt::Try { body, otherwise, .. } => {
                plan_walk_stmts(body, params, order, escapes, last_append, appends);
                plan_walk_stmts(otherwise, params, order, escapes, last_append, appends);
            }
        }
    }
}

fn plan_walk_expr(
    expr: &Expr,
    params: &std::collections::HashSet<&str>,
    here: usize,
    escapes: &mut std::collections::HashMap<String, usize>,
) {
    let mark = |n: &String, escapes: &mut std::collections::HashMap<String, usize>| {
        if !params.contains(n.as_str()) {
            escapes.entry(n.clone()).or_insert(here);
        }
    };
    match expr {
        Expr::Name(n) => mark(n, escapes),
        Expr::Len(inner) | Expr::Not(inner) => plan_walk_expr(inner, params, here, escapes),
        Expr::Index(c, i) => {
            plan_walk_expr(c, params, here, escapes);
            plan_walk_expr(i, params, here, escapes);
        }
        Expr::Bin(_, l, r) => {
            plan_walk_expr(l, params, here, escapes);
            plan_walk_expr(r, params, here, escapes);
        }
        Expr::Call { args, .. } | Expr::CallRoutine { args, .. } | Expr::Math { args, .. } => {
            for a in args {
                if let Expr::Name(n) = a {
                    mark(n, escapes);
                }
                plan_walk_expr(a, params, here, escapes);
            }
        }
        Expr::List(items) => {
            for a in items {
                if let Expr::Name(n) = a {
                    mark(n, escapes);
                }
                plan_walk_expr(a, params, here, escapes);
            }
        }
        Expr::RecordInit { fields, .. } => {
            for (_, v) in fields {
                plan_walk_expr(v, params, here, escapes);
            }
        }
        Expr::Fallback { inner, fallback, .. } => {
            plan_walk_expr(inner, params, here, escapes);
            plan_walk_expr(fallback, params, here, escapes);
        }
        _ => {}
    }
}

/// Emit a *lazy* fallback for a statement whose whole value is
/// `<fallible> otherwise <fallback>`. The operands are evaluated once into
/// temporaries, the fallback only when the operation would fail, and the result
/// is returned as an expression over those temporaries. Returns `None` for a
/// shape this does not cover (the caller then keeps the eager helper).
/// True when evaluating the expression twice cannot be observed: names,
/// literals, and reads built out of them. Used to make an in-expression
/// fallback lazy without hoisting temporaries.
fn is_pure(expr: &Expr) -> bool {
    match expr {
        Expr::Num(_, _) | Expr::Str(_) | Expr::Bool(_) | Expr::Name(_) => true,
        Expr::Len(inner) | Expr::Not(inner) => is_pure(inner),
        Expr::Field { base, .. } => !base.is_empty(),
        Expr::Index(c, i) => is_pure(c) && is_pure(i),
        Expr::Bin(_, l, r) => is_pure(l) && is_pure(r),
        _ => false,
    }
}

fn lazy_fallback(inner: &Expr, fallback: &Expr, ind: &str, out: &mut String) -> Option<String> {
    match inner {
        Expr::Index(coll, idx) => {
            out.push_str(&format!("{}{{\n", ind));
            out.push_str(&format!("{}    KzValue _kz_c = {};\n", ind, emit_expr(coll)));
            out.push_str(&format!("{}    KzValue _kz_i = {};\n", ind, emit_expr(idx)));
            Some(format!(
                "kz_index_ok(_kz_c, _kz_i) ? kz_index(_kz_c, _kz_i) : ({})",
                emit_expr(fallback)
            ))
        }
        Expr::Bin(op @ (BinOp::Div | BinOp::Mod), left, right) => {
            out.push_str(&format!("{}{{\n", ind));
            out.push_str(&format!("{}    KzValue _kz_a = {};\n", ind, emit_expr(left)));
            out.push_str(&format!("{}    KzValue _kz_b = {};\n", ind, emit_expr(right)));
            let opfn = if *op == BinOp::Div { "kz_div" } else { "kz_mod" };
            Some(format!(
                "kz_num_nonzero(_kz_b) ? {}(_kz_a, _kz_b) : ({})",
                opfn,
                emit_expr(fallback)
            ))
        }
        _ => None,
    }
}

fn emit_expr(expr: &Expr) -> String {
    match expr {
        Expr::Fallback { inner, fallback, .. } => match inner.as_ref() {
            Expr::Bin(op @ (BinOp::Div | BinOp::Mod), left, right) => {
                let opfn = if *op == BinOp::Div { "kz_div" } else { "kz_mod" };
                // Inside a larger expression the fallback is still lazy as long
                // as reading the operands twice is unobservable: the ternary
                // then evaluates the fallback only on failure.
                if is_pure(left) && is_pure(right) {
                    return format!(
                        "(kz_num_nonzero({r}) ? {op}({l}, {r}) : ({fb}))",
                        r = emit_expr(right),
                        op = opfn,
                        l = emit_expr(left),
                        fb = emit_expr(fallback)
                    );
                }
                if hoisting_active() {
                    let l = hoist_temp(emit_expr(left));
                    let r = hoist_temp(emit_expr(right));
                    return format!(
                        "(kz_num_nonzero({r}) ? {op}({l}, {r}) : ({fb}))",
                        r = r,
                        op = opfn,
                        l = l,
                        fb = emit_expr(fallback)
                    );
                }
                let helper = if *op == BinOp::Div { "kz_fb_div" } else { "kz_fb_mod" };
                format!(
                    "{}({}, {}, {})",
                    helper,
                    emit_expr(left),
                    emit_expr(right),
                    emit_expr(fallback)
                )
            }
            Expr::Index(coll, idx) => {
                if is_pure(coll) && is_pure(idx) {
                    return format!(
                        "(kz_index_ok({c}, {i}) ? kz_index({c}, {i}) : ({fb}))",
                        c = emit_expr(coll),
                        i = emit_expr(idx),
                        fb = emit_expr(fallback)
                    );
                }
                if hoisting_active() {
                    let c = hoist_temp(emit_expr(coll));
                    let i = hoist_temp(emit_expr(idx));
                    return format!(
                        "(kz_index_ok({c}, {i}) ? kz_index({c}, {i}) : ({fb}))",
                        c = c,
                        i = i,
                        fb = emit_expr(fallback)
                    );
                }
                format!(
                    "kz_fb_index({}, {}, {})",
                    emit_expr(coll),
                    emit_expr(idx),
                    emit_expr(fallback)
                )
            }
            // A capability fallback needs a host failure path; the native
            // backend has none yet (its try block is a placeholder), so the
            // call is emitted as it is. The VM implements the recovery.
            other => emit_expr(other),
        },
        Expr::Num(n, _) => {
            if n.is_nan() {
                "kz_num(0.0)".to_string()
            } else if n.is_infinite() {
                if *n > 0.0 { "kz_num(1e300)".to_string() } else { "kz_num(-1e300)".to_string() }
            } else {
                format!("kz_num({:?})", n)
            }
        }
        Expr::Str(s) => {
            format!("kz_str({})", escape_c_string(s))
        }
        Expr::Bool(b) => {
            format!("kz_bool({})", if *b { "true" } else { "false" })
        }
        Expr::Name(name) => sanitize_id(name),
        Expr::Ambient { field, family } => {
            format!("kz_cap_get(\"{}\", \"{}\")", family, field)
        }
        Expr::Call { recv, op, args, .. } => {
            let a_strs: Vec<String> = args.iter().map(emit_expr).collect();
            if recv == "log" {
                let arg_v = a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into());
                format!("kz_builtin_log({})", arg_v)
            } else if recv == "alert" {
                let arg_v = a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into());
                format!("kz_builtin_alert({})", arg_v)
            } else {
                format!("kz_cap_call(\"{}\", \"{}\", {}, (KzValue[]){{ {} }})",
                    recv, op, args.len(), if a_strs.is_empty() { "kz_nil()".to_string() } else { a_strs.join(", ") })
            }
        }
        Expr::Field { base, field, .. } => {
            format!("kz_get_field({}, \"{}\")", sanitize_id(base), field)
        }
        Expr::CallRoutine { name, args, .. } => {
            let fn_name = sanitize_id(name);
            let arg_str = args.iter().map(emit_expr).collect::<Vec<_>>().join(", ");
            format!("kz_fn_{}({})", fn_name, arg_str)
        }
        Expr::RecordInit { name, fields, .. } => {
            let rec_name = sanitize_id(name);
            let arg_str = fields.iter().map(|(_, e)| emit_expr(e)).collect::<Vec<_>>().join(", ");
            format!("kz_record_new_{}({})", rec_name, arg_str)
        }
        Expr::Bin(op, left, right) => {
            let l = emit_expr(left);
            let r = emit_expr(right);
            match op {
                BinOp::Add => {
                    let who = CURRENT_FN.with(|c| c.borrow().clone());
                    format!("kz_add_at(\"{}\", {}, {})", who, l, r)
                }
                BinOp::Sub => format!("kz_sub({}, {})", l, r),
                BinOp::Mul => format!("kz_mul({}, {})", l, r),
                BinOp::Div => format!("kz_div({}, {})", l, r),
                BinOp::Mod => format!("kz_mod({}, {})", l, r),
                BinOp::Eq => format!("kz_eq({}, {})", l, r),
                BinOp::Ne => format!("kz_ne({}, {})", l, r),
                BinOp::Lt => format!("kz_lt({}, {})", l, r),
                BinOp::Le => format!("kz_le({}, {})", l, r),
                BinOp::Gt => format!("kz_gt({}, {})", l, r),
                BinOp::Ge => format!("kz_ge({}, {})", l, r),
                BinOp::And => format!("kz_and({}, {})", l, r),
                BinOp::Or => format!("kz_or({}, {})", l, r),
            }
        }
        Expr::Not(inner) => format!("kz_not({})", emit_expr(inner)),
        Expr::Len(inner) => format!("kz_len({})", emit_expr(inner)),
        Expr::Index(coll, idx) => {
            let c = emit_expr(coll);
            let i = emit_expr(idx);
            format!("kz_index({}, {})", c, i)
        }
        Expr::List(items) => {
            if items.is_empty() {
                "kz_list_new(0)".to_string()
            } else {
                let args = items.iter().map(emit_expr).collect::<Vec<_>>().join(", ");
                format!("kz_list_from({}, (KzValue[]){{ {} }})", items.len(), args)
            }
        }
        Expr::Math { kind, args, .. } => {
            let a_strs: Vec<String> = args.iter().map(emit_expr).collect();
            match kind {
                MathKind::Vector => {
                    format!("kz_point({}, {}, {})",
                        a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into()),
                        a_strs.get(1).cloned().unwrap_or_else(|| "kz_nil()".into()),
                        a_strs.get(2).cloned().unwrap_or_else(|| "kz_nil()".into())
                    )
                }
                MathKind::Sqrt => format!("kz_math_sqrt({})", a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into())),
                MathKind::Sin => format!("kz_math_sin({})", a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into())),
                MathKind::Cos => format!("kz_math_cos({})", a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into())),
                MathKind::Atan2 => format!("kz_math_atan2({}, {})",
                    a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into()),
                    a_strs.get(1).cloned().unwrap_or_else(|| "kz_nil()".into())
                ),
                MathKind::ToRadians => format!("kz_math_to_radians({})", a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into())),
                MathKind::ToDegrees => format!("kz_math_to_degrees({})", a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into())),
                MathKind::Distance => format!(
                    "kz_num(kz_haversine_m(kz_as_point({}), kz_as_point({})))",
                    a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into()),
                    a_strs.get(1).cloned().unwrap_or_else(|| "kz_nil()".into())
                ),
                MathKind::SlantDistance => format!(
                    "kz_num(kz_slant_m(kz_as_point({}), kz_as_point({})))",
                    a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into()),
                    a_strs.get(1).cloned().unwrap_or_else(|| "kz_nil()".into())
                ),
                MathKind::BearingFrom => format!(
                    "kz_num(kz_bearing_deg(kz_as_point({}), kz_as_point({})))",
                    a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into()),
                    a_strs.get(1).cloned().unwrap_or_else(|| "kz_nil()".into())
                ),
                MathKind::BearingOf => format!(
                    "kz_num(kz_bearing_of_point(kz_as_point({})))",
                    a_strs.first().cloned().unwrap_or_else(|| "kz_nil()".into())
                ),
            }
        }
        Expr::LinkLost { .. } => "kz_bool(false)".to_string(),
    }
}

fn sanitize_id(name: &str) -> String {
    match name {
        "auto" | "break" | "case" | "char" | "const" | "continue" | "default" | "do" | "double"
        | "else" | "enum" | "extern" | "float" | "for" | "goto" | "if" | "inline" | "int"
        | "long" | "register" | "restrict" | "return" | "short" | "signed" | "sizeof" | "static"
        | "struct" | "switch" | "typedef" | "union" | "unsigned" | "void" | "volatile" | "while"
        | "main" => format!("_kz_{}", name),
        other => {
            let mut s = String::with_capacity(other.len());
            for c in other.chars() {
                if c.is_ascii_alphanumeric() || c == '_' {
                    s.push(c);
                } else {
                    s.push('_');
                }
            }
            s
        }
    }
}

fn escape_c_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

const C_RUNTIME_HEADER: &str = r#"/* Generated by Kaze C99 Native Emitter (Stage 0) */
#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <math.h>

/* The interpreter evaluates a*b - c*d exactly as written. A C compiler may
   contract that into a fused multiply-add, which rounds differently and made the
   two engines disagree in the last bits (a bearing between two identical points
   came out 180 instead of 0). Turning contraction off keeps the native result
   bit-identical to the interpreter's. */
#if defined(__clang__) || defined(__GNUC__)
#pragma STDC FP_CONTRACT OFF
#endif

/* Helpers a given program may not call: annotated so a strict build
   (-Wall -Wextra -Werror) stays clean. */
#if defined(__GNUC__) || defined(__clang__)
#define KZ_MAYBE_UNUSED __attribute__((unused))
#else
#define KZ_MAYBE_UNUSED
#endif

/* Defined further down; declared here so any helper can stop the program. */
static void kz_trap(const char* code, const char* what);

#if !defined(_WIN32)
#include <unistd.h>
#else
#include <io.h>
#define isatty _isatty
#define fileno _fileno
#endif

/* 32-bit & 64-bit portable value representation */
typedef enum {
    KZ_VAL_NIL = 0,
    KZ_VAL_BOOL,
    KZ_VAL_NUM,
    KZ_VAL_STR,
    KZ_VAL_LIST,
    KZ_VAL_RECORD,
    KZ_VAL_POINT,
} KzValKind;

struct KzValue;

typedef struct {
    size_t len;
    size_t cap;
    struct KzValue* items;
} KzList;

typedef struct {
    const char* type_name;
    size_t field_count;
    const char** keys;
    struct KzValue* values;
} KzRecord;

typedef struct {
    double x, y, z;
} KzPoint;

typedef struct KzValue {
    KzValKind kind;
    union {
        bool boolean;
        double number;
        struct { const char* data; size_t len; } str;
        KzList* list;
        KzRecord* record;
        KzPoint point;
    } as;
} KzValue;

/* ── memory budget ──────────────────────────────────────────────────────
   The generated program is standalone and its list building copies values, so
   a pathological source can allocate far more than it looks like it should. A
   bounded program fails with a diagnostic instead of taking the machine into
   swap. The budget is counted at every allocation; 1 GiB is the default and
   KAZE_MAX_MEMORY (bytes) overrides it, so a game or a mission can raise it
   deliberately. */
static size_t kz_live_bytes = 0;
static size_t kz_max_bytes = (size_t)1024 * 1024 * 1024;
static size_t kz_peak_bytes = 0;

KZ_MAYBE_UNUSED static inline void kz_memory_limit_from_env(void) {
    const char* env = getenv("KAZE_MAX_MEMORY");
    if (env && env[0]) {
        long long v = atoll(env);
        if (v > 0) kz_max_bytes = (size_t)v;
    }
}

/* Allocation accounting by site, so KAZE_STATS=1 answers "where did the
   memory go" with data instead of a guess. */
#define KZ_SITES 8
static const char* kz_site_name[KZ_SITES];
static size_t kz_site_bytes[KZ_SITES];
static size_t kz_site_calls[KZ_SITES];

KZ_MAYBE_UNUSED static inline void kz_note_site(const char* site, size_t bytes) {
    for (int i = 0; i < KZ_SITES; i++) {
        if (kz_site_name[i] == site || kz_site_name[i] == NULL) {
            if (kz_site_name[i] == NULL) kz_site_name[i] = site;
            kz_site_bytes[i] += bytes;
            kz_site_calls[i] += 1;
            return;
        }
    }
}

KZ_MAYBE_UNUSED static inline void kz_print_stats(void) {
    const char* env = getenv("KAZE_STATS");
    if (!env || env[0] != '1') return;
    fprintf(stderr, "kaze-stats: peak %.1f MiB, live %.1f MiB\n",
            (double)kz_peak_bytes / (1024.0 * 1024.0),
            (double)kz_live_bytes / (1024.0 * 1024.0));
    for (int i = 0; i < KZ_SITES; i++) {
        if (!kz_site_name[i]) continue;
        fprintf(stderr, "kaze-stats: %-16s %8.1f MiB in %llu allocations\n",
                kz_site_name[i],
                (double)kz_site_bytes[i] / (1024.0 * 1024.0),
                (unsigned long long)kz_site_calls[i]);
    }
}

KZ_MAYBE_UNUSED static inline void* kz_alloc(size_t bytes, size_t count);

KZ_MAYBE_UNUSED static inline void* kz_alloc_at(const char* site, size_t bytes, size_t count) {
    size_t total = bytes * count;
    kz_note_site(site, total);
    return kz_alloc(bytes, count);
}

KZ_MAYBE_UNUSED static inline void* kz_alloc(size_t bytes, size_t count) {
    size_t total = bytes * count;
    if (kz_live_bytes + total > kz_max_bytes) {
        fprintf(stderr,
                "E_KAZE_MEMORY: the program exceeded its memory budget (%llu MiB); "
                "raise it with KAZE_MAX_MEMORY=<bytes>\n",
                (unsigned long long)(kz_max_bytes / (1024 * 1024)));
        exit(70);
    }
    void* p = malloc(total);
    if (!p) {
        fprintf(stderr, "E_KAZE_MEMORY: the host refused %llu bytes\n", (unsigned long long)total);
        exit(70);
    }
    kz_live_bytes += total;
    if (kz_live_bytes > kz_peak_bytes) kz_peak_bytes = kz_live_bytes;
    return p;
}

KZ_MAYBE_UNUSED static inline void* kz_realloc(void* p, size_t total) {
    if (kz_live_bytes + total > kz_max_bytes) {
        fprintf(stderr,
                "E_KAZE_MEMORY: the program exceeded its memory budget (%llu MiB); "
                "raise it with KAZE_MAX_MEMORY=<bytes>\n",
                (unsigned long long)(kz_max_bytes / (1024 * 1024)));
        exit(70);
    }
    void* q = realloc(p, total);
    if (!q) {
        fprintf(stderr, "E_KAZE_MEMORY: the host refused %llu bytes\n", (unsigned long long)total);
        exit(70);
    }
    kz_live_bytes += total;
    if (kz_live_bytes > kz_peak_bytes) kz_peak_bytes = kz_live_bytes;
    return q;
}

KZ_MAYBE_UNUSED static inline void kz_free(void* p, size_t bytes) {
    if (!p) return;
    free(p);
    if (kz_live_bytes >= bytes) kz_live_bytes -= bytes;
}

KZ_MAYBE_UNUSED static inline KzValue kz_add(KzValue a, KzValue b);

KZ_MAYBE_UNUSED static inline KzValue kz_nil(void) {
    KzValue v; v.kind = KZ_VAL_NIL; v.as.number = 0.0; return v;
}
KZ_MAYBE_UNUSED static inline KzValue kz_bool(bool b) {
    KzValue v; v.kind = KZ_VAL_BOOL; v.as.boolean = b; return v;
}
KZ_MAYBE_UNUSED static inline KzValue kz_num(double n) {
    KzValue v; v.kind = KZ_VAL_NUM; v.as.number = n; return v;
}
KZ_MAYBE_UNUSED static inline KzValue kz_str_n(const char* s, size_t len) {
    KzValue v; v.kind = KZ_VAL_STR; v.as.str.data = s; v.as.str.len = len; return v;
}
KZ_MAYBE_UNUSED static inline KzValue kz_str(const char* s) {
    const char* d = s ? s : "";
    return kz_str_n(d, strlen(d));
}
KZ_MAYBE_UNUSED static inline KzValue kz_point(KzValue x, KzValue y, KzValue z) {
    KzValue v; v.kind = KZ_VAL_POINT;
    v.as.point.x = (x.kind == KZ_VAL_NUM) ? x.as.number : 0.0;
    v.as.point.y = (y.kind == KZ_VAL_NUM) ? y.as.number : 0.0;
    v.as.point.z = (z.kind == KZ_VAL_NUM) ? z.as.number : 0.0;
    return v;
}

/* ── spherical geometry ──────────────────────────────────────────────────
   The same formulas as the interpreter's geo module: a point is
   [lat_degrees, lon_degrees, altitude_m], `distance between` is the great-circle
   ground track on the WGS84 mean sphere, `slant distance` adds the altitude
   change, and bearings are degrees clockwise from north. These used to be
   lowered to 0 in the native backend, which silently produced wrong answers. */
#define KZ_EARTH_RADIUS_M 6371008.8
#define KZ_PI 3.14159265358979323846

KZ_MAYBE_UNUSED static inline double kz_rad(double d) { return d * (KZ_PI / 180.0); }
KZ_MAYBE_UNUSED static inline double kz_deg(double r) { return r * (180.0 / KZ_PI); }
KZ_MAYBE_UNUSED static inline double kz_normalise_deg(double d) {
    double m = fmod(d, 360.0);
    if (m < 0.0) m += 360.0;
    return m;
}

KZ_MAYBE_UNUSED static inline double kz_haversine_m(KzPoint a, KzPoint b) {
    double lat1 = kz_rad(a.x), lon1 = kz_rad(a.y);
    double lat2 = kz_rad(b.x), lon2 = kz_rad(b.y);
    double dlat = lat2 - lat1, dlon = lon2 - lon1;
    double s1 = sin(dlat / 2.0), s2 = sin(dlon / 2.0);
    double h = s1 * s1 + cos(lat1) * cos(lat2) * s2 * s2;
    if (h > 1.0) h = 1.0;
    return 2.0 * KZ_EARTH_RADIUS_M * asin(sqrt(h));
}

KZ_MAYBE_UNUSED static inline double kz_slant_m(KzPoint a, KzPoint b) {
    double ground = kz_haversine_m(a, b);
    double dz = b.z - a.z;
    return sqrt(ground * ground + dz * dz);
}

KZ_MAYBE_UNUSED static inline double kz_bearing_deg(KzPoint a, KzPoint b) {
    double lat1 = kz_rad(a.x), lat2 = kz_rad(b.x);
    double dlon = kz_rad(b.y - a.y);
    double y = sin(dlon) * cos(lat2);
    double x = cos(lat1) * sin(lat2) - sin(lat1) * cos(lat2) * cos(dlon);
    return kz_normalise_deg(kz_deg(atan2(y, x)));
}

KZ_MAYBE_UNUSED static inline double kz_bearing_of_point(KzPoint v) {
    return kz_normalise_deg(kz_deg(atan2(v.x, v.y)));
}

KZ_MAYBE_UNUSED static inline double kz_magnitude(KzPoint v) {
    return sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
}

KZ_MAYBE_UNUSED static inline KzPoint kz_as_point(KzValue v) {
    KzPoint p; p.x = 0.0; p.y = 0.0; p.z = 0.0;
    if (v.kind == KZ_VAL_POINT) return v.as.point;
    return p;
}
KZ_MAYBE_UNUSED static inline bool kz_is_truthy(KzValue v) {
    switch (v.kind) {
        case KZ_VAL_BOOL: return v.as.boolean;
        case KZ_VAL_NUM: return v.as.number != 0.0;
        case KZ_VAL_STR: return v.as.str.len > 0;
        case KZ_VAL_LIST: return v.as.list && v.as.list->len > 0;
        case KZ_VAL_RECORD: return true;
        case KZ_VAL_POINT: return true;
        default: return false;
    }
}

KZ_MAYBE_UNUSED static inline char* kz_str_alloc_copy(const char* s) {
    if (!s) return "";
    size_t l = strlen(s);
    char* out = (char*)kz_alloc_at("str_copy", 1, l + 1);
    if (!out) return "";
    memcpy(out, s, l + 1);
    return out;
}

KZ_MAYBE_UNUSED static inline char* kz_str_concat(const char* a, const char* b) {
    size_t la = a ? strlen(a) : 0;
    size_t lb = b ? strlen(b) : 0;
    char* out = (char*)kz_alloc_at("str_concat", 1, la + lb + 1);
    if (!out) return "";
    if (la) memcpy(out, a, la);
    if (lb) memcpy(out + la, b, lb);
    out[la + lb] = '\0';
    return out;
}

/* Shortest representation that round-trips, the rule the interpreter follows:
   try 15, 16 then 17 significant digits and keep the first that parses back to
   the same double. Integral values print without a fractional part. */
KZ_MAYBE_UNUSED static inline void kz_format_number(char* buf, size_t cap, double v) {
    if (isnan(v)) { snprintf(buf, cap, "NaN"); return; }
    if (isinf(v)) { snprintf(buf, cap, v > 0 ? "inf" : "-inf"); return; }
    if (floor(v) == v && fabs(v) < 1e17) { snprintf(buf, cap, "%.0f", v); return; }
    for (int prec = 15; prec <= 17; prec++) {
        snprintf(buf, cap, "%.*g", prec, v);
        if (strtod(buf, NULL) == v) return;
    }
}

KZ_MAYBE_UNUSED static inline const char* kz_to_string(KzValue v) {
    if (v.kind == KZ_VAL_STR) return v.as.str.data ? v.as.str.data : "";
    if (v.kind == KZ_VAL_BOOL) return v.as.boolean ? "true" : "false";
    if (v.kind == KZ_VAL_NUM) {
        char buf[64];
        kz_format_number(buf, sizeof(buf), v.as.number);
        return kz_str_alloc_copy(buf);
    }
    if (v.kind == KZ_VAL_NIL) return "nil";
    if (v.kind == KZ_VAL_RECORD) return v.as.record ? v.as.record->type_name : "record";
    if (v.kind == KZ_VAL_LIST) return "list";
    if (v.kind == KZ_VAL_POINT) {
        // The interpreter prints a point as a tuple of its components.
        char buf[160];
        snprintf(buf, sizeof(buf), "(%g, %g, %g)", v.as.point.x, v.as.point.y, v.as.point.z);
        return kz_str_alloc_copy(buf);
    }
    return "";
}

KZ_MAYBE_UNUSED static inline KzValue kz_list_new(size_t cap) {
    KzList* l = (KzList*)kz_alloc_at("list_new", sizeof(KzList), 1);
    l->len = 0;
    l->cap = cap > 4 ? cap : 4;
    l->items = (KzValue*)kz_alloc_at("list_items", sizeof(KzValue), l->cap);
    KzValue v; v.kind = KZ_VAL_LIST; v.as.list = l; return v;
}

/* Append into a slot the compiler proved is the only owner of its list: this is
   what turns the front end's set xs = xs + [x] loops from O(n^2) copies into an
   amortized O(1) push. */
KZ_MAYBE_UNUSED static inline void kz_list_push_own(KzValue* slot, KzValue item);

KZ_MAYBE_UNUSED static inline void kz_list_push(KzValue* list_val, KzValue item) {
    if (list_val->kind != KZ_VAL_LIST || !list_val->as.list) return;
    KzList* l = list_val->as.list;
    if (l->len >= l->cap) {
        l->cap = l->cap < 8 ? 8 : l->cap * 2;
        l->items = (KzValue*)kz_realloc(l->items, sizeof(KzValue) * l->cap);
    }
    l->items[l->len++] = item;
}

KZ_MAYBE_UNUSED static inline void kz_list_push_own(KzValue* slot, KzValue item) {
    if (slot->kind != KZ_VAL_LIST || !slot->as.list) {
        KzValue one = kz_list_new(1);
        kz_list_push(&one, item);
        *slot = one;
        return;
    }
    kz_list_push(slot, item);
}

/* Every list append is attributed to the routine that performed it, so a profile
   says *which* script routine is copying rather than only that copies happen. */
#define KZ_SITE_LIMIT 24
static const char* kz_site_label[KZ_SITE_LIMIT];
static size_t kz_site_total[KZ_SITE_LIMIT];
static size_t kz_site_count[KZ_SITE_LIMIT];

KZ_MAYBE_UNUSED static void kz_note_concat(const char* who, size_t bytes) {
    const char* env = getenv("KAZE_STATS");
    if (!env || env[0] != '1') return;
    for (int i = 0; i < KZ_SITE_LIMIT; i++) {
        if (kz_site_label[i] == who || kz_site_label[i] == NULL) {
            if (kz_site_label[i] == NULL) kz_site_label[i] = who;
            kz_site_total[i] += bytes;
            kz_site_count[i] += 1;
            return;
        }
    }
}

#define kz_list_concat(a, b) kz_list_concat_at(__func__, (a), (b))

KZ_MAYBE_UNUSED static void kz_print_concat_sites(void) {
    const char* env = getenv("KAZE_STATS");
    if (!env || env[0] != '1') return;
    for (int i = 0; i < KZ_SITE_LIMIT; i++) {
        if (!kz_site_label[i] || kz_site_total[i] == 0) continue;
        fprintf(stderr, "kaze-site: %-28s %8.1f MiB in %llu appends\n",
                kz_site_label[i],
                (double)kz_site_total[i] / (1024.0 * 1024.0),
                (unsigned long long)kz_site_count[i]);
    }
}

KZ_MAYBE_UNUSED static inline KzValue kz_add_at(const char* who, KzValue a, KzValue b) {
    if (a.kind == KZ_VAL_LIST || b.kind == KZ_VAL_LIST) {
        size_t la = (a.kind == KZ_VAL_LIST && a.as.list) ? a.as.list->len : 0;
        size_t lb = (b.kind == KZ_VAL_LIST && b.as.list) ? b.as.list->len : 0;
        kz_note_concat(who, sizeof(KzValue) * (la + lb));
    }
    return kz_add(a, b);
}

KZ_MAYBE_UNUSED static inline KzValue kz_list_concat_at(const char* kz_concat_caller, KzValue a, KzValue b) {
    size_t la = (a.kind == KZ_VAL_LIST && a.as.list) ? a.as.list->len : 0;
    size_t lb = (b.kind == KZ_VAL_LIST && b.as.list) ? b.as.list->len : 0;
    kz_note_site(lb == 1 ? "concat_append1" : "concat_other", sizeof(KzValue) * (la + lb));
    (void)kz_concat_caller;
    KzValue res = kz_list_new(la + lb);
    KzList* l = res.as.list;
    if (la) {
        memcpy(l->items, a.as.list->items, la * sizeof(KzValue));
        l->len += la;
    }
    if (lb) {
        memcpy(l->items + l->len, b.as.list->items, lb * sizeof(KzValue));
        l->len += lb;
    }
    return res;
}

KZ_MAYBE_UNUSED static inline KzValue kz_list_from(size_t n, const KzValue* items) {
    KzValue res = kz_list_new(n);
    if (n > 0 && items) {
        memcpy(res.as.list->items, items, n * sizeof(KzValue));
        res.as.list->len = n;
    }
    return res;
}

KZ_MAYBE_UNUSED static inline KzValue kz_record_create(const char* type_name, size_t n, const char** keys, const KzValue* vals) {
    KzRecord* r = (KzRecord*)kz_alloc_at("record_new", sizeof(KzRecord), 1);
    r->type_name = type_name;
    r->field_count = n;
    r->keys = keys;
    r->values = (n > 0) ? (KzValue*)kz_alloc_at("record_values", sizeof(KzValue), n) : NULL;
    if (n > 0 && vals && r->values) {
        memcpy(r->values, vals, n * sizeof(KzValue));
    }
    KzValue v; v.kind = KZ_VAL_RECORD; v.as.record = r; return v;
}

KZ_MAYBE_UNUSED static inline KzValue kz_get_field(KzValue obj, const char* field) {
    if (obj.kind == KZ_VAL_POINT) {
        if (strcmp(field, "x") == 0) return kz_num(obj.as.point.x);
        if (strcmp(field, "y") == 0) return kz_num(obj.as.point.y);
        if (strcmp(field, "z") == 0) return kz_num(obj.as.point.z);
        return kz_nil();
    }
    // Reading a field off something that is not a record is a type error in the
    // interpreter; answering "nothing" here would let a wrong value flow on.
    if (obj.kind != KZ_VAL_RECORD || !obj.as.record) {
        kz_trap("E_KAZE_TYPE", "a field read needs a record, but this value is not one");
    }
    KzRecord* r = obj.as.record;
    for (size_t i = 0; i < r->field_count; i++) {
        if (strcmp(r->keys[i], field) == 0) return r->values[i];
    }
    return kz_nil();
}

KZ_MAYBE_UNUSED static inline KzValue kz_add(KzValue a, KzValue b) {
    if (a.kind == KZ_VAL_NUM && b.kind == KZ_VAL_NUM) {
        return kz_num(a.as.number + b.as.number);
    }
    // point + point is a componentwise offset
    if (a.kind == KZ_VAL_POINT && b.kind == KZ_VAL_POINT) {
        KzPoint p;
        p.x = a.as.point.x + b.as.point.x;
        p.y = a.as.point.y + b.as.point.y;
        p.z = a.as.point.z + b.as.point.z;
        KzValue v; v.kind = KZ_VAL_POINT; v.as.point = p; return v;
    }
    if (a.kind == KZ_VAL_STR || b.kind == KZ_VAL_STR) {
        const char* sa = kz_to_string(a);
        const char* sb = kz_to_string(b);
        return kz_str(kz_str_concat(sa, sb));
    }
    if (a.kind == KZ_VAL_LIST && b.kind == KZ_VAL_LIST) {
        return kz_list_concat(a, b);
    }
    return kz_nil();
}

/* A stop the script cannot catch: print the code the interpreter would have
   raised and leave with status 70. The native path must not invent a value
   where the interpreter refuses one. */
static void kz_trap(const char* code, const char* what) {
    fprintf(stderr, "%s: %s\n", code, what ? what : "the program stopped");
    exit(70);
}

/* A finite result or a typed stop: the interpreter raises E_KAZE_OVERFLOW for a
   non-finite arithmetic result, so the native path does the same. */
KZ_MAYBE_UNUSED static inline KzValue kz_num_checked(double v) {
    if (isnan(v)) kz_trap("E_KAZE_MATH", "arithmetic produced not-a-number");
    if (isinf(v)) kz_trap("E_KAZE_OVERFLOW", "arithmetic overflowed");
    return kz_num(v);
}

KZ_MAYBE_UNUSED static inline KzValue kz_sub(KzValue a, KzValue b) {
    // point - point is the local offset between them
    if (a.kind == KZ_VAL_POINT && b.kind == KZ_VAL_POINT) {
        KzPoint p;
        p.x = a.as.point.x - b.as.point.x;
        p.y = a.as.point.y - b.as.point.y;
        p.z = a.as.point.z - b.as.point.z;
        KzValue v; v.kind = KZ_VAL_POINT; v.as.point = p; return v;
    }
    return kz_num_checked(((a.kind == KZ_VAL_NUM) ? a.as.number : 0.0) - ((b.kind == KZ_VAL_NUM) ? b.as.number : 0.0));
}
KZ_MAYBE_UNUSED static inline KzValue kz_mul(KzValue a, KzValue b) {
    // a point scales: point * number, number * point
    if (a.kind == KZ_VAL_POINT && b.kind == KZ_VAL_NUM) {
        KzPoint p;
        p.x = a.as.point.x * b.as.number;
        p.y = a.as.point.y * b.as.number;
        p.z = a.as.point.z * b.as.number;
        KzValue v; v.kind = KZ_VAL_POINT; v.as.point = p; return v;
    }
    if (a.kind == KZ_VAL_NUM && b.kind == KZ_VAL_POINT) {
        return kz_mul(b, a);
    }
    return kz_num_checked(((a.kind == KZ_VAL_NUM) ? a.as.number : 0.0) * ((b.kind == KZ_VAL_NUM) ? b.as.number : 0.0));
}
KZ_MAYBE_UNUSED static inline KzValue kz_div(KzValue a, KzValue b) {
    // a point divided by a number scales it
    if (a.kind == KZ_VAL_POINT && b.kind == KZ_VAL_NUM && b.as.number != 0.0) {
        KzPoint p;
        p.x = a.as.point.x / b.as.number;
        p.y = a.as.point.y / b.as.number;
        p.z = a.as.point.z / b.as.number;
        KzValue v; v.kind = KZ_VAL_POINT; v.as.point = p; return v;
    }
    double denom = (b.kind == KZ_VAL_NUM) ? b.as.number : 0.0;
    if (denom == 0.0) kz_trap("E_KAZE_DIV0", "division by zero");
    return kz_num_checked(((a.kind == KZ_VAL_NUM) ? a.as.number : 0.0) / denom);
}
KZ_MAYBE_UNUSED static inline KzValue kz_mod(KzValue a, KzValue b) {
    double denom = (b.kind == KZ_VAL_NUM) ? b.as.number : 0.0;
    if (denom == 0.0) kz_trap("E_KAZE_DIV0", "remainder by zero");
    return kz_num_checked(fmod((a.kind == KZ_VAL_NUM) ? a.as.number : 0.0, denom));
}

/* The declared recursion ceiling, enforced in native code as the VM does.
   A routine with no declared ceiling is bounded by the global frame limit. */
static int kz_call_depth = 0;

/* The declared ceiling counts a routine's own live frames, which is what a
   reader expects; the global 256-frame bound still applies to everyone. */
KZ_MAYBE_UNUSED static inline void kz_enter_frame(const char* name, int* own, int bound) {
    kz_call_depth += 1;
    if (kz_call_depth > 256) {
        char what[160];
        snprintf(what, sizeof(what), "the call stack reached 256 frames in '%s'", name);
        kz_trap("E_KAZE_RECURSION_DEPTH", what);
    }
    if (bound > 0) {
        *own += 1;
        if (*own > bound) {
            char what[160];
            snprintf(what, sizeof(what), "'%s' reached frame %d of its ceiling %d", name, *own, bound);
            kz_trap("E_KAZE_RECURSION_DEPTH", what);
        }
    }
}

KZ_MAYBE_UNUSED static inline void kz_leave_frame(int* own) {
    if (kz_call_depth > 0) kz_call_depth -= 1;
    if (own && *own > 0) *own -= 1;
}
KZ_MAYBE_UNUSED static inline KzValue kz_not(KzValue a) {
    return kz_bool(!kz_is_truthy(a));
}

KZ_MAYBE_UNUSED static inline bool kz_value_eq(KzValue a, KzValue b) {
    if (a.kind != b.kind) return false;
    switch (a.kind) {
        case KZ_VAL_NIL: return true;
        case KZ_VAL_BOOL: return a.as.boolean == b.as.boolean;
        case KZ_VAL_NUM: return a.as.number == b.as.number;
        case KZ_VAL_STR: return strcmp(a.as.str.data && b.as.str.data ? a.as.str.data : "", a.as.str.data && b.as.str.data ? b.as.str.data : "") == 0;
        default: return a.as.list == b.as.list;
    }
}
KZ_MAYBE_UNUSED static inline KzValue kz_eq(KzValue a, KzValue b) { return kz_bool(kz_value_eq(a, b)); }
KZ_MAYBE_UNUSED static inline KzValue kz_ne(KzValue a, KzValue b) { return kz_bool(!kz_value_eq(a, b)); }
KZ_MAYBE_UNUSED static inline KzValue kz_lt(KzValue a, KzValue b) {
    if (a.kind == KZ_VAL_NUM && b.kind == KZ_VAL_NUM) return kz_bool(a.as.number < b.as.number);
    if (a.kind == KZ_VAL_STR && b.kind == KZ_VAL_STR) return kz_bool(strcmp(a.as.str.data, b.as.str.data) < 0);
    return kz_bool(false);
}
KZ_MAYBE_UNUSED static inline KzValue kz_le(KzValue a, KzValue b) {
    if (a.kind == KZ_VAL_NUM && b.kind == KZ_VAL_NUM) return kz_bool(a.as.number <= b.as.number);
    if (a.kind == KZ_VAL_STR && b.kind == KZ_VAL_STR) return kz_bool(strcmp(a.as.str.data, b.as.str.data) <= 0);
    return kz_bool(false);
}
KZ_MAYBE_UNUSED static inline KzValue kz_gt(KzValue a, KzValue b) {
    if (a.kind == KZ_VAL_NUM && b.kind == KZ_VAL_NUM) return kz_bool(a.as.number > b.as.number);
    if (a.kind == KZ_VAL_STR && b.kind == KZ_VAL_STR) return kz_bool(strcmp(a.as.str.data, b.as.str.data) > 0);
    return kz_bool(false);
}
KZ_MAYBE_UNUSED static inline KzValue kz_ge(KzValue a, KzValue b) {
    if (a.kind == KZ_VAL_NUM && b.kind == KZ_VAL_NUM) return kz_bool(a.as.number >= b.as.number);
    if (a.kind == KZ_VAL_STR && b.kind == KZ_VAL_STR) return kz_bool(strcmp(a.as.str.data, b.as.str.data) >= 0);
    return kz_bool(false);
}
KZ_MAYBE_UNUSED static inline KzValue kz_and(KzValue a, KzValue b) { return kz_bool(kz_is_truthy(a) && kz_is_truthy(b)); }
KZ_MAYBE_UNUSED static inline KzValue kz_or(KzValue a, KzValue b) { return kz_bool(kz_is_truthy(a) || kz_is_truthy(b)); }

KZ_MAYBE_UNUSED static inline KzValue kz_len(KzValue a) {
    if (a.kind == KZ_VAL_STR) return kz_num((double)a.as.str.len);
    if (a.kind == KZ_VAL_LIST && a.as.list) return kz_num((double)a.as.list->len);
    // A point's length is its magnitude: the 3-D length of a local offset.
    if (a.kind == KZ_VAL_POINT) return kz_num(kz_magnitude(a.as.point));
    return kz_num(0.0);
}

KZ_MAYBE_UNUSED static inline KzValue kz_index(KzValue coll, KzValue idx) {
    // The interpreter refuses an out-of-range read with E_KAZE_BOUNDS; the
    // native path must not answer with nothing instead.
    double raw = (idx.kind == KZ_VAL_NUM) ? idx.as.number : -1.0;
    if (raw < 0.0) kz_trap("E_KAZE_BOUNDS", "a negative position is not an element");
    size_t i = (size_t)raw;
    if (coll.kind == KZ_VAL_LIST && coll.as.list) {
        if (i < coll.as.list->len) return coll.as.list->items[i];
        kz_trap("E_KAZE_BOUNDS", "position past the end of the list");
    }
    if (coll.kind == KZ_VAL_STR && coll.as.str.data) {
        size_t l = coll.as.str.len;
        if (i < l) {
            char* one = (char*)kz_alloc_at("str_char", 1, 2);
            one[0] = coll.as.str.data[i];
            one[1] = '\0';
            return kz_str_n(one, 1);
        }
        kz_trap("E_KAZE_BOUNDS", "position past the end of the text");
    }
    return kz_nil();
}

/* Phase 2 fallbacks. The native backend evaluates the fallback eagerly: these
   helpers take both the operand and the fallback value, so the operand is
   evaluated exactly once. The VM evaluates the fallback lazily; a fallback with
   a side effect is refused by the checker, which keeps the two engines equal. */
/* Predicates for the lazy fallback forms: the emitter evaluates a statement's
   operands once, then chooses between the operation and the fallback, so the
   fallback is only evaluated when it is actually used. */
KZ_MAYBE_UNUSED static inline bool kz_num_nonzero(KzValue v) {
    return v.kind == KZ_VAL_NUM && v.as.number != 0.0;
}

KZ_MAYBE_UNUSED static inline bool kz_index_ok(KzValue coll, KzValue idx) {
    if (idx.kind != KZ_VAL_NUM || idx.as.number < 0.0) return false;
    size_t i = (size_t)idx.as.number;
    if (coll.kind == KZ_VAL_LIST && coll.as.list) return i < coll.as.list->len;
    if (coll.kind == KZ_VAL_STR) return i < coll.as.str.len;
    return false;
}

KZ_MAYBE_UNUSED static inline KzValue kz_fb_div(KzValue a, KzValue b, KzValue fb) {
    double d = (b.kind == KZ_VAL_NUM) ? b.as.number : 0.0;
    if (d == 0.0) return fb;
    return kz_div(a, b);
}
KZ_MAYBE_UNUSED static inline KzValue kz_fb_mod(KzValue a, KzValue b, KzValue fb) {
    double d = (b.kind == KZ_VAL_NUM) ? b.as.number : 0.0;
    if (d == 0.0) return fb;
    return kz_mod(a, b);
}
KZ_MAYBE_UNUSED static inline KzValue kz_fb_index(KzValue coll, KzValue idx, KzValue fb) {
    double d = (idx.kind == KZ_VAL_NUM) ? idx.as.number : -1.0;
    if (d < 0.0) return fb;
    if (coll.kind == KZ_VAL_LIST && coll.as.list) {
        if ((size_t)d >= coll.as.list->len) return fb;
    } else if (coll.kind == KZ_VAL_STR) {
        if ((size_t)d >= coll.as.str.len) return fb;
    }
    return kz_index(coll, idx);
}


/* ── hook driver ─────────────────────────────────────────────────────────
   `when` and `every` items fire on host ticks, the same way the interpreter
   drives them. The compiled binary takes the tick count from --ticks (or
   KAZE_TICKS) and runs each tick's hooks in declaration order; a `when` body
   runs only while its predicate holds, and a predicate that needs telemetry the
   host cannot supply is false rather than assumed true. */


KZ_MAYBE_UNUSED static KzValue kz_run_hooks(int argc, char** argv);
KZ_MAYBE_UNUSED static inline void kz_builtin_log(KzValue msg) {
    printf("[info] %s\n", kz_to_string(msg));
}
KZ_MAYBE_UNUSED static inline void kz_builtin_alert(KzValue msg) {
    fprintf(stderr, "[alert] %s\n", kz_to_string(msg));
}

KZ_MAYBE_UNUSED static inline KzValue kz_math_sqrt(KzValue a) {
    return kz_num(sqrt((a.kind == KZ_VAL_NUM) ? a.as.number : 0.0));
}
KZ_MAYBE_UNUSED static inline KzValue kz_math_sin(KzValue a) {
    return kz_num(sin((a.kind == KZ_VAL_NUM) ? a.as.number : 0.0));
}
KZ_MAYBE_UNUSED static inline KzValue kz_math_cos(KzValue a) {
    return kz_num(cos((a.kind == KZ_VAL_NUM) ? a.as.number : 0.0));
}
KZ_MAYBE_UNUSED static inline KzValue kz_math_atan2(KzValue y, KzValue x) {
    double dy = (y.kind == KZ_VAL_NUM) ? y.as.number : 0.0;
    double dx = (x.kind == KZ_VAL_NUM) ? x.as.number : 0.0;
    return kz_num(atan2(dy, dx));
}
KZ_MAYBE_UNUSED static inline KzValue kz_math_to_radians(KzValue a) {
    double deg = (a.kind == KZ_VAL_NUM) ? a.as.number : 0.0;
    return kz_num(deg * 3.14159265358979323846 / 180.0);
}
KZ_MAYBE_UNUSED static inline KzValue kz_math_to_degrees(KzValue a) {
    double rad = (a.kind == KZ_VAL_NUM) ? a.as.number : 0.0;
    return kz_num(rad * 180.0 / 3.14159265358979323846);
}

KZ_MAYBE_UNUSED static inline KzValue kz_cap_get(const char* family, const char* field) {
    (void)family; (void)field;
    return kz_nil();
}

/* A standalone binary carries no host backends, so a capability call has nothing
   to talk to. It used to print a stub line and return nothing, which let a script
   that needs a sensor succeed with fabricated data — the fail-open RULES.md §1.9
   forbids. It refuses instead, with the same code the interpreter uses. */
KZ_MAYBE_UNUSED static inline KzValue kz_cap_call(const char* family, const char* op, size_t argc, const KzValue* args) {
    (void)argc; (void)args;
    char what[192];
    snprintf(what, sizeof(what),
             "this binary has no host backend for '%s' (needed by '%s.%s'); run the script with the interpreter and a granted backend, or build with one",
             family, family, op);
    kz_trap("E_CAP_MISSING", what);
    return kz_nil();
}

KZ_MAYBE_UNUSED static inline KzValue kz_read_stream(FILE* f) {
    size_t cap = 4096;
    size_t len = 0;
    char* buf = (char*)kz_alloc(1, cap);
    if (!buf) return kz_str("");
    size_t n;
    while ((n = fread(buf + len, 1, cap - len - 1, f)) > 0) {
        len += n;
        if (len + 1024 >= cap) {
            cap *= 2;
            char* nb = (char*)realloc(buf, cap);
            if (!nb) break;
            buf = nb;
        }
    }
    buf[len] = '\0';
    return kz_str(buf);
}

KZ_MAYBE_UNUSED static inline KzValue kz_read_file_or_string(const char* path_or_str) {
    if (!path_or_str) return kz_str("");
    FILE* f = fopen(path_or_str, "rb");
    if (!f) {
        return kz_str(path_or_str);
    }
    fseek(f, 0, SEEK_END);
    long sz = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (sz < 0) {
        fclose(f);
        return kz_str(path_or_str);
    }
    char* buf = (char*)kz_alloc(1, (size_t)sz + 1);
    if (!buf) {
        fclose(f);
        return kz_str(path_or_str);
    }
    size_t rd = fread(buf, 1, (size_t)sz, f);
    buf[rd] = '\0';
    fclose(f);
    return kz_str(buf);
}


/* ── configuration file layer ────────────────────────────────────────────
   Routine parameters resolve CLI > environment > config file > signature
   default, the four-level hierarchy of the 12-factor rule. The interpreter
   has had the config layer since Phase 4; the compiled binary only had three
   levels. The file is a flat JSON object ({"duty": 40} or {"args": {...}}),
   which is what the interpreter's config documents look like. */

KZ_MAYBE_UNUSED static char* kz_config_path(int argc, char** argv) {
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--config") == 0 && i + 1 < argc) return argv[i + 1];
        if (strncmp(argv[i], "--config=", 9) == 0) return argv[i] + 9;
    }
    return (char*)"agent.config.json";
}

KZ_MAYBE_UNUSED static char* kz_read_whole_file(const char* path) {
    FILE* f = fopen(path, "rb");
    if (!f) return NULL;
    if (fseek(f, 0, SEEK_END) != 0) { fclose(f); return NULL; }
    long n = ftell(f);
    if (n < 0) { fclose(f); return NULL; }
    rewind(f);
    char* buf = (char*)malloc((size_t)n + 1);
    if (!buf) { fclose(f); return NULL; }
    size_t rd = fread(buf, 1, (size_t)n, f);
    buf[rd] = '\0';
    fclose(f);
    return buf;
}

/* Parse the value that follows "name" : in a flat JSON object. Numbers, booleans
   and strings are understood; anything else is not a parameter value. */
KZ_MAYBE_UNUSED static KzValue kz_config_lookup(int argc, char** argv, const char* name) {
    char* text = kz_read_whole_file(kz_config_path(argc, argv));
    if (!text) return kz_nil();
    KzValue found = kz_nil();
    size_t nlen = strlen(name);
    char* p = text;
    while ((p = strchr(p, '"')) != NULL) {
        char* key = p + 1;
        char* close = strchr(key, '"');
        if (!close) break;
        p = close + 1;
        int matches = ((size_t)(close - key) == nlen) && (strncmp(key, name, nlen) == 0);
        if (!matches) continue;
        char* colon = strchr(close, ':');
        if (!colon) continue;
        char* v = colon + 1;
        while (*v == ' ' || *v == '\t' || *v == '\n' || *v == '\r') v++;
        if (*v == '"') {
            char* end = strchr(v + 1, '"');
            if (end) {
                size_t len = (size_t)(end - (v + 1));
                char* s = (char*)kz_alloc(1, len + 1);
                memcpy(s, v + 1, len);
                s[len] = '\0';
                found = kz_str_n(s, len);
            }
        } else if (strncmp(v, "true", 4) == 0) {
            found = kz_bool(true);
        } else if (strncmp(v, "false", 5) == 0) {
            found = kz_bool(false);
        } else {
            char* end = NULL;
            double d = strtod(v, &end);
            if (end && end != v) found = kz_num(d);
        }
        break;
    }
    free(text);
    return found;
}
/* ── parameter coercion ──────────────────────────────────────────────────
   A declared parameter type is a promise about the CLI, environment and config
   values that reach the routine. The interpreter refuses a value that does not
   fit (`E_KAZE_PARAM: declared as a number but was given ...`); the compiled
   binary used to pass the raw string through, so `--arg duty=abc` became the
   text "abc" inside a numeric parameter. */
#define KZ_PARAM_ANY 0
#define KZ_PARAM_NUM 1
#define KZ_PARAM_BOOL 2
#define KZ_PARAM_TEXT 3

KZ_MAYBE_UNUSED static void kz_param_refuse(const char* name, const char* routine, const char* want, KzValue got) {
    char what[192];
    const char* kind = "nothing";
    if (got.kind == KZ_VAL_NUM) kind = "a number";
    else if (got.kind == KZ_VAL_STR) kind = kz_to_string(got);
    else if (got.kind == KZ_VAL_BOOL) kind = got.as.boolean ? "true" : "false";
    snprintf(what, sizeof(what), "parameter '%s' of '%s' is declared as %s but was given %s", name, routine, want, kind);
    kz_trap("E_KAZE_PARAM", what);
}

KZ_MAYBE_UNUSED static inline KzValue kz_coerce_param(KzValue v, int kind, const char* name, const char* routine) {
    if (kind == KZ_PARAM_ANY || v.kind == KZ_VAL_NIL) return v;
    if (kind == KZ_PARAM_NUM) {
        if (v.kind == KZ_VAL_NUM) return v;
        if (v.kind == KZ_VAL_STR) {
            const char* s = v.as.str.data ? v.as.str.data : "";
            char* end = NULL;
            double d = strtod(s, &end);
            while (end && (*end == ' ' || *end == '\t' || *end == '\n' || *end == '\r')) end++;
            if (end != s && end && *end == '\0') return kz_num(d);
        }
        kz_param_refuse(name, routine, "a number", v);
    }
    if (kind == KZ_PARAM_BOOL) {
        if (v.kind == KZ_VAL_BOOL) return v;
        if (v.kind == KZ_VAL_STR) {
            const char* s = v.as.str.data ? v.as.str.data : "";
            if (strcmp(s, "true") == 0) return kz_bool(true);
            if (strcmp(s, "false") == 0) return kz_bool(false);
        }
        kz_param_refuse(name, routine, "true or false", v);
    }
    if (kind == KZ_PARAM_TEXT) {
        if (v.kind == KZ_VAL_STR) return v;
        kz_param_refuse(name, routine, "text", v);
    }
    return v;
}

KZ_MAYBE_UNUSED static inline KzValue kz_resolve_cli_arg(int argc, char** argv, const char* name, int index) {
    size_t nlen = strlen(name);
    for (int i = 1; i < argc; i++) {
        const char* a = argv[i];
        if (strncmp(a, "--", 2) == 0) {
            const char* opt = a + 2;
            if (strncmp(opt, name, nlen) == 0 && opt[nlen] == '=') {
                return kz_read_file_or_string(opt + nlen + 1);
            }
            if (strcmp(opt, "arg") == 0 && i + 1 < argc) {
                const char* next_arg = argv[i + 1];
                if (strncmp(next_arg, name, nlen) == 0 && next_arg[nlen] == '=') {
                    return kz_read_file_or_string(next_arg + nlen + 1);
                }
            }
            if (strncmp(opt, "arg=", 4) == 0) {
                const char* next_arg = opt + 4;
                if (strncmp(next_arg, name, nlen) == 0 && next_arg[nlen] == '=') {
                    return kz_read_file_or_string(next_arg + nlen + 1);
                }
            }
        }
    }
    char env_name[128];
    snprintf(env_name, sizeof(env_name), "AGENT_%s", name);
    for (size_t j = 6; env_name[j]; j++) {
        if (env_name[j] >= 'a' && env_name[j] <= 'z') env_name[j] -= 32;
    }
    char* ev = getenv(env_name);
    if (ev) return kz_read_file_or_string(ev);

    snprintf(env_name, sizeof(env_name), "SOFUU_%s", name);
    for (size_t j = 6; env_name[j]; j++) {
        if (env_name[j] >= 'a' && env_name[j] <= 'z') env_name[j] -= 32;
    }
    ev = getenv(env_name);
    if (ev) return kz_read_file_or_string(ev);

    // Third layer: the configuration file named by --config (or
    // ./agent.config.json). Below it are the positional argument, piped input
    // and finally the routine's own signature default.
    {
        KzValue cfg = kz_config_lookup(argc, argv, name);
        if (cfg.kind != KZ_VAL_NIL) return cfg;
    }

    int pos_idx = 0;
    for (int i = 1; i < argc; i++) {
        if (argv[i][0] != '-') {
            if (pos_idx == index) {
                // A positional may spell the parameter out (duty=500), which is how
                // people usually type a single value.
                if (strncmp(argv[i], name, nlen) == 0 && argv[i][nlen] == '=') {
                    return kz_read_file_or_string(argv[i] + nlen + 1);
                }
                return kz_read_file_or_string(argv[i]);
            }
            pos_idx++;
        }
    }

    // Piped input is a documented way to feed the first parameter, but an empty
    // pipe is not a value: say nothing was found so the caller can refuse.
    if (index == 0 && !isatty(fileno(stdin))) {
        KzValue piped = kz_read_stream(stdin);
        if (piped.kind == KZ_VAL_STR && piped.as.str.len > 0) {
            // A pipe ends with a newline that is not part of the value; the
            // interpreter trims it, so this does too.
            size_t n = piped.as.str.len;
            const char* d = piped.as.str.data;
            while (n > 0 && (d[n - 1] == '\n' || d[n - 1] == '\r')) n--;
            if (n > 0) return kz_str_n(d, n);
        }
    }

    return kz_nil();
}

KZ_MAYBE_UNUSED static int kz_ticks_from_args(int argc, char** argv) {
    // The interpreter takes --ticks N, so the compiled binary accepts the same
    // spelling (and --ticks=N) before falling back to the AGENT_-style lookup.
    for (int i = 1; i + 1 < argc; i++) {
        if (strcmp(argv[i], "--ticks") == 0) {
            long n = strtol(argv[i + 1], NULL, 10);
            if (n > 0) return (int)n;
        }
    }
    for (int i = 1; i < argc; i++) {
        if (strncmp(argv[i], "--ticks=", 8) == 0) {
            long n = strtol(argv[i] + 8, NULL, 10);
            if (n > 0) return (int)n;
        }
    }
    KzValue v = kz_resolve_cli_arg(argc, argv, "ticks", -1);
    if (v.kind == KZ_VAL_NUM && v.as.number > 0.0) return (int)v.as.number;
    const char* env = getenv("KAZE_TICKS");
    if (env && env[0]) {
        long n = strtol(env, NULL, 10);
        if (n > 0) return (int)n;
    }
    return 0;
}
"#;

#[cfg(test)]
mod append_tests {
    use super::*;

    fn program(src: &str) -> Program {
        crate::parser::parse(src).expect("parses")
    }

    #[test]
    fn an_alias_in_the_caller_blocks_the_in_place_append() {
        let good = program("action acc(list, n):\n    let out = list\n    set out to out + [1]\n    return out\n\naction main():\n    let xs = []\n    set xs to acc(xs, 3)\n    return length of xs\n");
        let acc = good.items.iter().find(|i| i.name == "acc").expect("acc");
        let main = good.items.iter().find(|i| i.name == "main").expect("main");
        eprintln!("good: unique_locals(main) = {:?}", unique_locals(main, &consumed_params(&good.items)));
        eprintln!("good: appendable(acc) = {:?}", appendable_names(&good.items, acc, &consumed_params(&good.items)));

        let bad = program("action acc(list, n):\n    let out = list\n    set out to out + [1]\n    return out\n\naction main():\n    let xs = []\n    let keep = xs\n    set xs to acc(xs, 3)\n    return length of keep\n");
        let acc_bad = bad.items.iter().find(|i| i.name == "acc").expect("acc");
        let main_bad = bad.items.iter().find(|i| i.name == "main").expect("main");
        eprintln!("bad: unique_locals(main) = {:?}", unique_locals(main_bad, &consumed_params(&bad.items)));
        eprintln!("bad: appendable(acc) = {:?}", appendable_names(&bad.items, acc_bad, &consumed_params(&bad.items)));
        assert!(
            !appendable_names(&bad.items, acc_bad, &consumed_params(&bad.items)).contains("list"),
            "an aliasing caller must block the in-place append"
        );
    }

    #[test]
    fn the_current_front_end_style_does_not_qualify_for_consumed_parameters() {
        // Observation, not a wish: the front end's accumulator chains pass
        // their list through helpers that also *return* a new list
        // (\`return out + [diag]\`), and a value that is rebuilt on return is not
        // the caller's list any more, so consuming it in place would duplicate
        // the appended element. The rule is sound but has nothing to bite on
        // here; if this assertion starts failing, the style changed and the
        // optimisation is worth re-measuring.
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/compiler/checker.kaze"))
            .expect("read the checker source");
        let program = crate::parser::parse(src.as_str()).expect("parses");
        let consumed = consumed_params(&program.items);
        eprintln!("consumed parameters in checker.kaze: {:?}", consumed);
        assert!(
            consumed.is_empty(),
            "the checker now has consumable parameters: {:?} -- re-measure the memory",
            consumed
        );
    }
}
