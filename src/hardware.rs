// hardware.rs — R7 hardware capability and safety firewall for Kaze.
//
// Every capability call crosses this gate:
//   H-1: family declared in script's needs:         (E_CAP_UNDECLARED)
//   H-2: every family has an explicit host grant   (E_CAP_MISSING)
//   H-3: host policy does not deny operation        (E_CAP_DENIED)
//   H-4: physical argument ranges (reject, not clamp) (E_KAZE_RANGE)

use std::io::Write;
use std::sync::{Mutex, OnceLock};

use serde_json::Value as Json;
use crate::value::{Heap, Value};

pub const EFFECTOR_FAMILIES: &[&str] = &["drone", "rover", "sensor", "device", "alert"];

pub fn needs_backend_grant(_family: &str) -> bool {
    true
}

#[derive(Debug, Clone)]
pub struct Denial {
    pub code: &'static str,
    pub rule: &'static str,
    pub reason: String,
}

impl Denial {
    pub fn new(code: &'static str, rule: &'static str, reason: String) -> Self {
        Denial { code, rule, reason }
    }
}

// ── Structured audit trail (Phase 5) ─────────────────────────────────────
//
// One line per gate decision, off by default. An operator who wants the trail
// sets `KAZE_AUDIT=file:<path>` (a bare path is accepted). A line records the
// family, the operation, the decision and the reason — never an argument value,
// so a credential a script happens to pass to a capability cannot reach it. If
// a trail was asked for and cannot be written, capability calls are denied:
// security telemetry that silently disappears is worse than no telemetry.

/// Environment variable naming the audit sink. Off when absent or empty.
pub const AUDIT_ENV: &str = "KAZE_AUDIT";
/// The longest reason one audit line carries, in bytes.
pub const AUDIT_REASON_BYTES: usize = 240;

enum AuditTarget {
    Off,
    File { file: Mutex<std::fs::File>, label: String },
    Broken(String),
}

static AUDIT: OnceLock<AuditTarget> = OnceLock::new();

fn audit_target() -> &'static AuditTarget {
    AUDIT.get_or_init(|| {
        let spec = match std::env::var(AUDIT_ENV) {
            Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return AuditTarget::Off,
        };
        let path = spec.strip_prefix("file:").unwrap_or(&spec).trim().to_string();
        if path.is_empty() {
            return AuditTarget::Broken(format!(
                "{} names no file; use {}=file:<path>",
                AUDIT_ENV, AUDIT_ENV
            ));
        }
        match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => AuditTarget::File { file: Mutex::new(file), label: path },
            Err(e) => AuditTarget::Broken(format!(
                "cannot open the audit sink '{}': {} (fix the path or unset {})",
                path, e, AUDIT_ENV
            )),
        }
    })
}

/// Append one decision line: `<utc> capability <family>.<op> decision=<d> reason=<r>`.
pub fn record_audit(family: &str, op: &str, decision: &str, reason: &str) -> Result<(), String> {
    match audit_target() {
        AuditTarget::Off => Ok(()),
        AuditTarget::Broken(why) => Err(why.clone()),
        AuditTarget::File { file, label } => {
            let reason: String = crate::caps::redact_secrets(reason)
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            let (reason, _) = crate::caps::truncate_utf8(&reason, AUDIT_REASON_BYTES);
            let line = format!(
                "{} capability {}.{} decision={} reason={}\n",
                crate::caps::utc_timestamp(),
                family,
                op,
                decision,
                reason
            );
            let mut guard = file
                .lock()
                .map_err(|_| format!("the audit sink '{}' cannot be locked", label))?;
            guard
                .write_all(line.as_bytes())
                .map_err(|e| format!("cannot write the audit sink '{}': {}", label, e))?;
            guard
                .flush()
                .map_err(|e| format!("cannot flush the audit sink '{}': {}", label, e))
        }
    }
}

/// Record a host-side refusal (the 1500 ms freshness watchdog) in the trail.
/// The refusal stands whether or not the trail can be written.
pub fn record_safety_veto(family: &str, op: &str, reason: &str) -> Result<(), String> {
    record_audit(family, op, "deny", reason)
}

/// Fold one gate outcome into the audit trail. A trail that cannot be written
/// turns the call into an `E_CAP_DENIED`: the gate fails closed, not open.
fn audited(
    result: Result<(), Denial>,
    family: &str,
    op: &str,
    allow_reason: &str,
) -> Result<(), Denial> {
    let recording = match &result {
        Ok(()) => record_audit(family, op, "allow", allow_reason),
        Err(d) => record_audit(family, op, "deny", &format!("{}/{}", d.code, d.rule)),
    };
    match recording {
        Ok(()) => result,
        Err(why) => Err(Denial::new(
            "E_CAP_DENIED",
            "H-5/audit",
            format!(
                "{}; '{}.{}' is refused because the audit trail cannot be written",
                why, family, op
            ),
        )),
    }
}

pub fn authorise(
    declared: &[String],
    grants: &[String],
    policy: Option<&Json>,
    family: &str,
    op: &str,
) -> Result<(), Denial> {
    audited(
        authorise_gate(declared, grants, policy, family, op),
        family,
        op,
        "H-1..H-3 passed",
    )
}

fn authorise_gate(
    declared: &[String],
    grants: &[String],
    policy: Option<&Json>,
    family: &str,
    op: &str,
) -> Result<(), Denial> {
    let declared_families = || declared.iter().any(|d| crate::caps::family_of(d) == family);
    if !declared_families() {
        return Err(Denial::new(
            "E_CAP_UNDECLARED",
            "H-1/needs",
            format!(
                "'{}' is not declared. Add it to the script's first line: needs: {}",
                family, family
            ),
        ));
    }
    if needs_backend_grant(family)
        && !grants.iter().any(|g| crate::caps::family_of(g) == family)
    {
        return Err(Denial::new(
            "E_CAP_MISSING",
            "H-2/backend",
            format!(
                "the host did not grant '{}'. Grant it explicitly with --cap {} and configure a real backend when needed",
                family, family
            ),
        ));
    }
    let p = policy.ok_or_else(|| {
        Denial::new(
            "E_CAP_DENIED",
            "H-3/policy",
            format!(
                "the host policy is missing; capability '{}.{}' denied. Provide a valid policy.json with a deny array",
                family, op
            ),
        )
    })?;
    {
        if p.get("__missing").is_some() {
            return Err(Denial::new(
                "E_CAP_DENIED",
                "H-3/policy",
                format!("the host policy is missing; capability '{}.{}' denied (R7.6 fail-closed)", family, op),
            ));
        }
        if p.get("__malformed").is_some() || !p.is_object() {
            return Err(Denial::new(
                "E_CAP_DENIED",
                "R-HW/badRequest",
                format!("malformed policy file rejected for '{}.{}' (R7.6 fail-closed)", family, op),
            ));
        }
        let Some(obj) = p.as_object() else {
            return Err(Denial::new("E_CAP_DENIED", "R-HW/badRequest", "policy must be an object".to_string()));
        };
        if obj.keys().any(|key| key != "deny") {
            return Err(Denial::new(
                "E_CAP_DENIED",
                "R-HW/badRequest",
                "policy contains an unsupported field; only 'deny' is supported".to_string(),
            ));
        }
        let rules = obj.get("deny").and_then(Json::as_array).ok_or_else(|| {
            Denial::new(
                "E_CAP_DENIED",
                "R-HW/badRequest",
                "policy must contain a 'deny' array".to_string(),
            )
        })?;
        let target = format!("{}.{}", family, op);
        for rule in rules {
            let pattern = rule.as_str().ok_or_else(|| {
                Denial::new(
                    "E_CAP_DENIED",
                    "R-HW/badRequest",
                    "every policy 'deny' entry must be text".to_string(),
                )
            })?;
            let valid = pattern == "*" || pattern.split_once('.').is_some_and(|(fam, operation)| {
                crate::caps::FAMILIES.contains(&fam)
                    && (operation == "*" || crate::caps::known_op(fam, operation)
                        || (fam == "drone" && operation == "link_lost"))
            });
            if !valid {
                return Err(Denial::new(
                    "E_CAP_DENIED",
                    "R-HW/badRequest",
                    format!("unrecognized policy deny pattern '{}'", pattern),
                ));
            }
            if pattern == target || pattern == format!("{}.*", family) || pattern == "*" {
                return Err(Denial::new(
                    "E_CAP_DENIED",
                    "H-3/policy",
                    format!("the host policy denies capability '{}' (R7.6)", target),
                ));
            }
        }
    }
    Ok(())
}

fn require_text(family: &str, op: &str, idx: usize, args: &[Value]) -> Result<(), Denial> {
    match args.get(idx) {
        Some(Value::Str(_)) => Ok(()),
        other => Err(Denial::new(
            "E_KAZE_TYPE",
            "H-4/type",
            format!(
                "{}.{} needs text for argument {}, but it is {}",
                family,
                op,
                idx + 1,
                other.map(Value::type_name).unwrap_or("missing")
            ),
        )),
    }
}

fn range_value(
    family: &str,
    op: &str,
    n: f64,
    lo: f64,
    hi: f64,
    unit: &str,
) -> Result<(), Denial> {
    if !n.is_finite() || n < lo || n > hi {
        return Err(Denial::new(
            "E_KAZE_RANGE",
            "H-4/range",
            format!(
                "{}.{} got {} {}, which is outside the permitted range {} to {} {} (rejected, not clamped)",
                family,
                op,
                crate::arena::fmt_num(n),
                unit,
                crate::arena::fmt_num(lo),
                crate::arena::fmt_num(hi),
                unit
            ),
        ));
    }
    Ok(())
}

fn range(
    family: &str,
    op: &str,
    idx: usize,
    args: &[Value],
    lo: f64,
    hi: f64,
    unit: &str,
) -> Result<(), Denial> {
    match args.get(idx) {
        None => Ok(()),
        Some(Value::Num(n)) => range_value(family, op, *n, lo, hi, unit),
        Some(other) => Err(Denial::new(
            "E_KAZE_TYPE",
            "H-4/type",
            format!(
                "{}.{} needs a number for argument {}, but it is {} (rejected, not coerced)",
                family,
                op,
                idx + 1,
                other.type_name()
            ),
        )),
    }
}

pub fn validate_arguments(
    family: &str,
    op: &str,
    args: &[Value],
    heap: &Heap,
) -> Result<(), Denial> {
    let result = validate_arguments_gate(family, op, args, heap);
    // The gate above already recorded its allow; a refusal here is a second,
    // later decision and is recorded as such, so the last line for a call is
    // always the effective one.
    if let Err(d) = &result {
        if let Err(why) = record_audit(family, op, "deny", &format!("{}/{}", d.code, d.rule)) {
            return Err(Denial::new(
                "E_CAP_DENIED",
                "H-5/audit",
                format!("{}; the call is refused because the audit trail cannot be written", why),
            ));
        }
    }
    result
}

fn validate_arguments_gate(
    family: &str,
    op: &str,
    args: &[Value],
    heap: &Heap,
) -> Result<(), Denial> {
    match (family, op) {
        ("drone", "takeoff") => range(family, op, 0, args, 0.0, 500.0, "meters")?,
        ("drone", "goto") => {
            // The bounded value is the altitude. Which argument carries it
            // depends on the call form: a point may carry it in z, a point
            // may be followed by an explicit altitude, and the numeric forms
            // are goto(lat), goto(lat, alt) and goto(lat, lon, alt).
            match args.first() {
                Some(Value::Vec(v)) => {
                    if args.len() > 2 {
                        return Err(Denial::new("E_KAZE_TYPE", "H-4/type", "drone.goto(point) accepts at most one additional altitude".to_string()));
                    }
                    let point = heap.vec3(*v).ok_or_else(|| {
                        Denial::new("E_KAZE_BADCODE", "H-4/type", "the target point is missing from the heap".to_string())
                    })?;
                    range_value(family, op, point[0], -90.0, 90.0, "degrees latitude")?;
                    range_value(family, op, point[1], -180.0, 180.0, "degrees longitude")?;
                    if args.len() >= 2 {
                        range(family, op, 1, args, 0.0, 500.0, "meters")?;
                    } else {
                        range_value(family, op, point[2], 0.0, 500.0, "meters")?;
                    }
                }
                Some(Value::Map(_)) => {
                    return Err(Denial::new("E_KAZE_TYPE", "H-4/type", "drone.goto does not support record targets; provide a vector or numeric coordinates".to_string()));
                }
                _ => {
                    if args.len() >= 3 {
                        range(family, op, 0, args, -90.0, 90.0, "degrees latitude")?;
                        range(family, op, 1, args, -180.0, 180.0, "degrees longitude")?;
                        range(family, op, 2, args, 0.0, 500.0, "meters")?;
                    } else if args.len() == 2 {
                        range(family, op, 0, args, -90.0, 90.0, "degrees latitude")?;
                        range(family, op, 1, args, 0.0, 500.0, "meters")?;
                    } else {
                        range(family, op, 0, args, -90.0, 90.0, "degrees latitude")?;
                    }
                }
            }
        }
        ("rover", "forward") | ("rover", "backward") => range(family, op, 0, args, 0.0, 10.0, "m/s")?,
        ("sensor", "distance") => range(family, op, 0, args, 0.0, 100.0, "meters")?,
        ("device", "set_relay") => {
            require_text(family, op, 0, args)?;
            match args.get(1) {
                Some(Value::Bool(_)) => {}
                Some(Value::Num(n)) if n.is_finite() => {}
                Some(Value::Num(_)) => {
                    return Err(Denial::new("E_KAZE_RANGE", "H-4/range", "device.set_relay requires a finite numeric state".to_string()));
                }
                _ => {
                    return Err(Denial::new("E_KAZE_TYPE", "H-4/type", "device.set_relay requires a boolean or number for its state".to_string()));
                }
            }
        }
        ("device", "read_gpio") => require_text(family, op, 0, args)?,
        ("device", "pulse") => {
            require_text(family, op, 0, args)?;
            range(family, op, 1, args, 0.0, 60.0, "seconds")?;
        }
        // A duty cycle outside 0-100% is a script bug, not something to clamp:
        // a fan running at a speed nobody asked for is a safety problem.
        ("device", "set_duty") => {
            require_text(family, op, 0, args)?;
            range(family, op, 1, args, 0.0, 100.0, "percent")?;
        }
        _ => {}
    }
    Ok(())
}
