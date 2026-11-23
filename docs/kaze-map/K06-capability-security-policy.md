# Division 06: Capability Gates, Security Policy & Sandbox (K06)

> **Maturity:** Core  
> **Parent Blueprint:** [kaze-map/README.md](README.md)  
> **Primary Locations:** `kaze/src/caps.rs`, `kaze/src/host.rs`, `crates/sofuu-core/src/domains/kaze/caps.rs`, `crates/sofuu-core/src/domains/kaze/host.rs`

---

## 1. Mission & Scope

Division 06 enforces the zero-ambient authority security model of Kaze. In Kaze, no program, routine, or agent possesses implicit access to system resources. Every capability to interact with the external world—whether reading from the filesystem, opening a network socket, calling an external AI model, querying an MCP tool, or driving a physical actuator—must be explicitly declared via `needs:` clauses in the routine source and authorized by an active security policy.

The core philosophy of Division 06 is fail-closed determinism: any unauthorized request, missing configuration, unconfigured backend, or network anomaly immediately results in a clean, typed error (`E_CAP_DENIED` or `E_CAP_MISSING`). Under no circumstances will the runtime fabricate a mock pass, return canned dummy data, or silently bypass policy gates.

---

## 2. Code Manifest & Owned Files

| Path | Purpose |
|---|---|
| `kaze/src/caps.rs` | Capability declarations, grant bitmasks, policy evaluator, token verification |
| `kaze/src/host.rs` | Host environment dispatcher, fail-closed capability gates, real alert sinks |
| `crates/sofuu-core/src/domains/kaze/caps.rs` | Embedded capability mirror with Sofuu host runtime bindings |
| `crates/sofuu-core/src/domains/kaze/host.rs` | Embedded host dispatcher routing to real tools, memory, and alert sinks |

---

## 3. Architectural Specifications & Invariants

1. **Zero-Ambient Authority**:
   - Routines possess zero implicit authority. File IO, networking, memory persistence, and effector actions are strictly inaccessible unless granted by explicit `needs: ...` signatures.
2. **Fail-Closed Security by Default**:
   - An empty, malformed, or missing policy causes all capability operations to fail closed with `E_CAP_DENIED`.
   - A granted capability that lacks a configured physical or virtual host backend must fail closed with `E_CAP_MISSING`; it may never return a simulated or fabricated success (`RULES.md` §1.9).
3. **Monotonic Capability Attenuation**:
   - Sub-routines and delegated tasks can only hold a subset of the parent routine's capabilities:
     $$\text{Caps}(\text{Child}) \subseteq \text{Caps}(\text{Parent})$$
   - It is mathematically impossible for a child routine to escalate privileges beyond what its caller possesses.
4. **Real Alert Sinks & Credential Scrubbing**:
   - Alert operations (`alert=stdout`, `alert=stderr`, `alert=file:<path>`) stream raw bytes, enforce strict length bounds ($\le 1024$ bytes per message), and redact sensitive credentials (API tokens, private keys, authorization headers) before output.

---

## 4. Implementation Details & Capability Matrix

### Capability Hierarchy & Declaration

```text
# Planned spelling: v1.0 declares a resource family, not a device id.
# Routine declaring explicit capability boundaries
routine update_telemetry(reading: float) -> bool:
  needs: hardware.sensor("imu0")
  needs: alert.sink("telemetry_log")
  
  let ok = hardware.sensor.read("imu0") otherwise 0.0
  alert.sink.send("telemetry_log", "IMU update received") otherwise false
  return true
```

### Host Backend Verification Flow

```
┌────────────────────────────────────────────────────────┐
│                   ROUTINE INVOCATION                   │
│          Operation: hardware.sensor.read(...)          │
└───────────────────────────┬────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────┐
│             STATIC / RUNTIME GRANT CHECK (K06)         │
│       Is 'hardware.sensor' declared in 'needs:'?       │
└─────────────┬────────────────────────────┬─────────────┘
              │ YES                        │ NO
              ▼                            ▼
┌───────────────────────────┐ ┌──────────────────────────┐
│     POLICY ARBITER (K06)  │ │ REJECT: E_CAP_DENIED     │
│  Does active policy allow?│ │ (Zero execution)         │
└──────┬─────────────┬──────┘ └──────────────────────────┘
       │ ALLOW       │ DENY
       ▼             ▼
┌──────────────┐ ┌──────────────────────────┐
│ BACKEND CHECK│ │ REJECT: E_CAP_DENIED     │
│ Real backend?│ └──────────────────────────┘
└──┬────────┬──┘
   │ YES    │ NO
   ▼        ▼
┌───────┐ ┌──────────────────────────┐
│EXECUTE│ │ REJECT: E_CAP_MISSING    │
│ REAL  │ │ (No fabricated success)  │
└───────┘ └──────────────────────────┘
```

---

## 5. Verification & Testing

```bash
# Verify capability checks and policy enforcement
cargo test --manifest-path kaze/Cargo.toml --lib caps

# Run capability safety integration test suite
cargo test --manifest-path kaze/Cargo.toml --test capability_safety

# Verify real alert sinks with redaction and fail-closed bounds
python3 scripts/test-kaze-fail-closed.py
python3 scripts/test-kaze-alert-real.py
```

---

## 6. Active & Completed Tasks

- [x] Implement fail-closed capability verification on missing or empty policies (`F-04`).
- [x] Implement real alert sinks (`stdout`, `stderr`, `file:<path>`) with $\le 1024$ B limit (`bb98aaa65`).
- [x] Comprehensive capability safety test suite in `kaze/tests/capability_safety.rs`.
- [x] Credential and sensitive path redaction in alert and diagnostic logs.
- [ ] Add cryptographic signature verification for third-party capability provider modules.

## Roadmap phase 1-6 update (2026-11-22)

- The language gained routine-level `needs:` with monotonic attenuation: a routine may only declare a capability the module declared, may only use what it declared, and a caller must hold everything its callee declares. Escalation is `E_CAP_UNDECLARED`.
- The host side is audited in `kaze/docs/CAPABILITY-SECURITY.md`: every family fails closed, alert sinks enforce a 1024-byte ceiling with credential redaction, the 1500 ms freshness veto gates live actuation, and actuator lines de-assert on abnormal termination.
- 15 effector-safety tests and 19 live tests now prove it, including an exhaustive sweep of all 40 registry (family, op) pairs across every gate stage and a fail-closed sweep with no backend.
- Verification: `cargo test --manifest-path kaze/Cargo.toml --test capability_safety --test effector_safety --test live`.
