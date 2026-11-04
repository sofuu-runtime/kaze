// hardware.rs — R7 hardware capability and safety firewall for Kaze.
//
// Every capability call crosses this gate:
//   H-1: family declared in script's needs:         (E_CAP_UNDECLARED)
//   H-2: effector family has explicit backend grant (E_CAP_MISSING)
//   H-3: host policy does not deny operation        (E_CAP_DENIED)
//   H-4: physical argument ranges (reject, not clamp) (E_KAZE_RANGE)

use serde_json::Value as Json;
use crate::value::{Heap, Value};

pub const EFFECTOR_FAMILIES: &[&str] = &["drone", "rover", "sensor", "device", "alert"];

pub fn needs_backend_grant(family: &str) -> bool {
    EFFECTOR_FAMILIES.contains(&family)
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

pub fn authorise(
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
                "the host did not grant a backend for '{}'. Grant one explicitly, e.g. --cap {}=sim",
                family, family
            ),
        ));
    }
    if let Some(p) = policy {
        if let Some(rules) = p.get("deny").and_then(|d| d.as_array()) {
            let target = format!("{}.{}", family, op);
            for rule in rules {
                if let Some(pattern) = rule.as_str() {
                    if pattern == target || pattern == format!("{}.*", family) || pattern == "*" {
                        return Err(Denial::new(
                            "E_CAP_DENIED",
                            "H-3/policy",
                            format!("the host policy denies capability '{}' (R7.6)", target),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// A value where a number is required is a type denial, never something the
/// host silently coerces. A non-numeric duty that reached `PwmBus` used to be
/// refused under `E_KAZE_LINK`; with `try` in the language that would have made
/// a range refusal catchable, so H-4 stops it here instead.
fn require_num(family: &str, op: &str, idx: usize, args: &[Value]) -> Result<(), Denial> {
    match args.get(idx) {
        None | Some(Value::Num(_)) => Ok(()),
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
    match (family, op) {
        ("drone", "takeoff") => range(family, op, 0, args, 0.0, 500.0, "meters")?,
        ("drone", "goto") => {
            // The bounded value is the altitude. Which argument carries it
            // depends on the call form: a point may carry it in z, a point
            // may be followed by an explicit altitude, and the numeric forms
            // are goto(lat), goto(lat, alt) and goto(lat, lon, alt).
            match args.first() {
                Some(Value::Vec(v)) => {
                    if args.len() >= 2 {
                        range(family, op, 1, args, 0.0, 500.0, "meters")?;
                    } else if let Some(p) = heap.vec3(*v) {
                        range_value(family, op, p[2], 0.0, 500.0, "meters")?;
                    }
                }
                Some(Value::Map(_)) => {
                    if args.len() >= 2 {
                        range(family, op, 1, args, 0.0, 500.0, "meters")?;
                    }
                }
                _ => {
                    if args.len() >= 3 {
                        require_num(family, op, 0, args)?;
                        require_num(family, op, 1, args)?;
                        range(family, op, 2, args, 0.0, 500.0, "meters")?;
                    } else if args.len() == 2 {
                        require_num(family, op, 0, args)?;
                        range(family, op, 1, args, 0.0, 500.0, "meters")?;
                    } else {
                        require_num(family, op, 0, args)?;
                    }
                }
            }
        }
        ("rover", "forward") | ("rover", "backward") => range(family, op, 0, args, 0.0, 10.0, "m/s")?,
        ("sensor", "distance") => range(family, op, 0, args, 0.0, 100.0, "meters")?,
        ("device", "pulse") => range(family, op, 1, args, 0.0, 60.0, "seconds")?,
        // A duty cycle outside 0-100% is a script bug, not something to clamp:
        // a fan running at a speed nobody asked for is a safety problem.
        ("device", "set_duty") => range(family, op, 1, args, 0.0, 100.0, "percent")?,
        _ => {}
    }
    Ok(())
}
