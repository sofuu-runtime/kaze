# Division 03: Static Type Checker, Physical Units & Range Contracts (K03)

> **Maturity:** Core  
> **Parent Blueprint:** [kaze-map/README.md](README.md)  
> **Primary Locations:** `kaze/src/checker.rs`, `crates/sofuu-core/src/domains/kaze/checker.rs`, `kaze/compiler/checker.kaze` (bootstrap roadmap)

---

## 1. Mission & Scope

Division 03 provides the mathematical static verification engine for Kaze. Its mission is to eliminate entire classes of software failures at compile time, guaranteeing that any program that passes the type checker can never experience undefined behavior, unexpected panics, array out-of-bounds violations, or dimensional calculation errors in production.

Unlike general-purpose languages that treat physical quantities as raw, dimensionless floats, Kaze incorporates a first-class compile-time dimensional unit algebra. Furthermore, routine contracts (`requires`, `ensures`) establish explicit preconditions and postconditions that are verified statically whenever possible and backed by zero-cost assertions at external system boundaries.

---

## 2. Code Manifest & Owned Files

| Path | Purpose |
|---|---|
| `kaze/src/checker.rs` | Static type checker, capability grant analyzer, dimensional unit solver, contract verifier |
| `crates/sofuu-core/src/domains/kaze/checker.rs` | Embedded static checker mirror in Sofuu core |
| `kaze/compiler/checker.kaze` | Self-hosted pure Kaze semantic checker (in active bootstrap roadmap) |

---

## 3. Architectural Specifications & Invariants

1. **Absolute Zero-Panic Mandate**:
   - All operations that can theoretically fail must have explicit, compile-time enforced handling:
     - Integer division and modulo must have an `otherwise <fallback>` clause or a contract proving the denominator is non-zero.
     - Array indexing must supply an `otherwise` fallback value or prove index boundedness statically.
     - Map lookups must return an optional type or handle missing keys via `otherwise`.
2. **Compile-Time Dimensional Physics Algebra**:
   - Dimensions are checked at compile time with zero runtime storage or instruction overhead:
     $$\text{velocity} = \frac{\text{length}}{\text{time}}, \quad \text{acceleration} = \frac{\text{velocity}}{\text{time}}, \quad \text{force} = \text{mass} \times \text{acceleration}$$
   - Attempting to add meters to seconds, or assign radians directly to degrees without an explicit unit conversion, triggers a compile-time rejection (`E_KAZE_UNIT_MISMATCH`).
3. **Routine Preconditions & Postconditions (Contracts)**:
   - Routines declare domain constraints using `requires <cond>` and `ensures <cond>`.
   - The type checker propagates known bounds. If a caller supplies an argument that violates the precondition, compilation fails (`E_KAZE_CONTRACT_VIOLATION`).
4. **Structured Error Diagnostic Protocol**:
   - Every semantic error is assigned a deterministic diagnostic code (`E_KAZE_TYPE`, `E_KAZE_UNIT_MISMATCH`, `E_KAZE_CONTRACT`, `E_KAZE_PARAM`, `E_KAZE_DIV0`).
   - Diagnostic reports include file path, line, column, multiline visual carets, explanation of violated rules, and suggested remediation.

---

## 4. Implementation Details & Dimension Solver

### Dimensional Unit Algebra Verification Rules

```
[Length]     L (meter, km, cm, mm)
[Time]       T (second, ms, hour)
[Mass]       M (kg, gram)
[Angle]      A (rad, deg)
[Temperature]K (Kelvin, Celsius)
[Pressure]   P (Pascal, mbar, hPa)

Rule 1 (Addition/Subtraction):
  dim(a + b) requires dim(a) == dim(b)
  dim(a - b) requires dim(a) == dim(b)

Rule 2 (Multiplication):
  dim(a * b) = dim(a) * dim(b)

Rule 3 (Division):
  dim(a / b) = dim(a) * dim(b)^(-1)
```

### Static Diagnostic Caret Rendering Example

```
error[E_KAZE_UNIT_MISMATCH]: dimensional unit mismatch in arithmetic expression
  --> nav/waypoint.kaze:14:18
   |
14 |   let distance = altitude + ground_speed
   |                  ^^^^^^^^   ^^^^^^^^^^^^
   |                  |          |
   |                  |          unit is 'velocity' (meter/second)
   |                  unit is 'length' (meter)
   |
   = help: cannot add 'length' to 'velocity'; multiply speed by a time duration or convert units
```

---

## 5. Verification & Testing

```bash
# Verify type checker and capability analyzer
cargo test --manifest-path kaze/Cargo.toml --lib checker

# Verify compile-time rejection of dimensional mismatches
cargo test --manifest-path kaze/Cargo.toml --test type_checker_units
```

---

## 6. Active & Completed Tasks

- [x] Implement static capability grant analysis (`needs: ...`).
- [x] Implement structured diagnostic rendering with masked carets (`E_KAZE_*`).
- [x] Enforce mandatory `otherwise` fallback handling on fallible arithmetic.
- [x] Dimensional unit checking for basic kinematics (meter, second, rad, deg).
- [ ] Port complete type inference and dimension solver to self-hosted Kaze (`compiler/checker.kaze`).
- [ ] Add static interval arithmetic solver for automatic contract verification.

## Roadmap phase 1-6 update (2026-11-22)

- `kaze/src/checker.rs` now runs `dim_result`, a real dimensional algebra over `Dim` (length, time, mass, angle, temperature, pressure, current): add/subtract need one dimension, multiply adds exponents, divide subtracts them, comparisons need one dimension, and a mismatch is `E_KAZE_UNIT_MISMATCH` naming both quantities.
- `kaze/src/strict.rs` is the new strictness pass: fallibility proofs (`E_KAZE_MISSING_FALLBACK`, `E_KAZE_BOUNDS`), termination (`E_KAZE_LOOP_UNBOUNDED`, `E_KAZE_RECURSION_UNBOUNDED`), contracts (`E_KAZE_CONTRACT`, `E_KAZE_UNTRUSTED`) and capability attenuation (`E_CAP_UNDECLARED`).
- Proofs it accepts: non-zero literal divisor; constant index inside a known or promised length; an index guarded by `if`/`while`/`guard`/`requires`; `length of xs - k` under `guard k < length of xs`; a callee's `ensures length of result == k`.
- Every diagnostic carries its own code instead of collapsing into `E_KAZE_CHECK`.
- Diagnostics render as blocks (`kaze/src/diag.rs`): `error[CODE]`, `--> path:line:col`, the source line, a caret under the construct and a suggestion; `kaze/tests/diagnostics.rs` pins nine codes and the language server publishes structured ranges (`diagnose_in`).
- Verification: `cargo test --manifest-path kaze/Cargo.toml --test strict_semantics --test units --test contracts --test diagnostics`.
