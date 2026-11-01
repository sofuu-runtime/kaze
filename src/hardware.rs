// hardware.rs — R7 hardware capability and safety firewall for Kaze.
//
// Every capability call crosses this gate:
//   H-1: family declared in script's needs:         (E_CAP_UNDECLARED)
//   H-2: effector family has explicit backend grant (E_CAP_MISSING)
//   H-3: host policy does not deny operation        (E_CAP_DENIED)
//   H-4: physical argument ranges (reject, not clamp) (E_KAZE_RANGE)

use serde_json::Value as Json;
use crate::arena::Arena;
use crate::value::Value;

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

fn num_arg(args: &[Value], idx: usize) -> Option<f64> {
    args.get(idx).and_then(|v| v.as_num())
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
    let Some(n) = num_arg(args, idx) else { return Ok(()) };
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

pub fn validate_arguments(family: &str, op: &str, args: &[Value], _arena: &Arena) -> Result<(), Denial> {
    match (family, op) {
        ("drone", "takeoff") | ("drone", "goto") => {
            let idx = if family == "drone" && op == "goto" { 1 } else { 0 };
            range(family, op, idx, args, 0.0, 500.0, "meters")?
        }
        ("rover", "forward") | ("rover", "backward") => range(family, op, 0, args, 0.0, 10.0, "m/s")?,
        ("sensor", "distance") => range(family, op, 0, args, 0.0, 100.0, "meters")?,
        ("device", "pulse") => range(family, op, 1, args, 0.0, 60.0, "seconds")?,
        _ => {}
    }
    Ok(())
}
