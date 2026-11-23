# Division 10: Verification, Conformance & Test Suite (K10)

> **Maturity:** Core  
> **Parent Blueprint:** [kaze-map/README.md](README.md)  
> **Primary Locations:** `kaze/tests/`, `kaze/docs/CERTIFICATION.md`, `kaze/docs/FAULT-TOLERANCE-CHECKLIST.md`, `kaze/docs/COMPATIBILITY.md`, `scripts/kaze-drift-check.sh`, `scripts/kaze-sync.sh`

---

## 1. Mission & Scope

Division 10 establishes the rigorous testing harnesses, mathematical verification suites, drift prevention gates, and conformance standards for Kaze. A language built for autonomous systems and reliable AI must provide verifiable proof of correctness: programs must compile reproducibly, runtime invariants must hold under all edge conditions, and language definitions must never silently diverge across repositories.

Division 10 maintains automated regression suites that validate every capability boundary, verify fixed-point bootstrap identity, prevent cross-engine drift, and guarantee that zero simulated or fabricated passes can contaminate test reports.

---

## 2. Code Manifest & Owned Files

| Path | Purpose |
|---|---|
| `kaze/tests/` | Comprehensive test suite: unit tests, capability safety, format parsing, VM execution |
| `kaze/docs/CERTIFICATION.md` | Formal certification rules, verified safety invariants, assurance criteria |
| `kaze/docs/FAULT-TOLERANCE-CHECKLIST.md` | Audit checklist verifying fail-safe behavior across all 49 opcodes |
| `kaze/docs/COMPATIBILITY.md` | Bytecode version compatibility register and migration guide |
| `scripts/kaze-drift-check.sh` | Automated drift detection asserting 0 verb and 0 opcode drift across engines |
| `scripts/kaze-sync.sh` | Two-way automated synchronization tool between standalone Kaze and Sofuu core |

---

## 3. Architectural Specifications & Invariants

1. **Zero-Drift Gatekeeper**:
   - The test harness runs `scripts/kaze-drift-check.sh --strict` before any release or CI sign-off.
   - Any difference between standalone and embedded implementations halts CI immediately with exit code 1.
2. **Fixed-Point Compiler Bootstrap Reproducibility**:
   - The self-hosted compiler bootstrap pipeline is subject to bitwise verification:
     $$\text{sha256}(\text{kaze\_stage2.c}) \equiv \text{sha256}(\text{kaze\_stage3.c})$$
   - Ensures that the compiler emits deterministic, reproducible machine code independent of the host compiler toolchain.
3. **No Fabricated Test Results**:
   - In accordance with `RULES.md` §1.9, no test may mock, stub, or fabricate an external capability pass. If an external backend (e.g., physical drone or remote provider) is unconfigured, the test records `blocked` or `unverified`; it may never fabricate success.
4. **Local Model Invariant**:
   - All AI inference verification tests run exclusively against existing local GGUF models on disk (`~/Downloads/*.gguf`). Network downloads are strictly prohibited.

---

## 4. Implementation Details & Verification Gates

### Multi-Stage Verification Pipeline

```
┌────────────────────────────────────────────────────────┐
│               STAGE 1: STATIC DRIFT GATE               │
│          bash scripts/kaze-drift-check.sh --strict     │
│   Asserts 0 opcode drift, 0 verb drift, 0 ast drift    │
└───────────────────────────┬────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────┐
│             STAGE 2: CAPABILITY SAFETY SUITE           │
│   cargo test --manifest-path kaze/Cargo.toml --test \  │
│                   capability_safety                    │
│   Asserts fail-closed denial on empty/missing policies │
└───────────────────────────┬────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────┐
│            STAGE 3: BOOTSTRAP FIXED-POINT PROOF        │
│   Stage 0 -> Stage 1 -> Stage 2 -> Stage 3             │
│   Asserts sha256(stage2.c) == sha256(stage3.c)         │
└───────────────────────────┬────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────┐
│             STAGE 4: EMBEDDED CONFORMANCE              │
│          cargo test -p sofuu-core domains::kaze        │
│   Asserts complete parity in Sofuu runtime host        │
└────────────────────────────────────────────────────────┘
```

---

## 5. Verification & Testing

```bash
# Run strict drift check
bash scripts/kaze-drift-check.sh --strict

# Run standalone Kaze test suite
cargo test --manifest-path kaze/Cargo.toml

# Run capability safety integration test
cargo test --manifest-path kaze/Cargo.toml --test capability_safety

# Run embedded Kaze test suite in Sofuu
cargo test -p sofuu-core domains::kaze
```

---

## 6. Active & Completed Tasks

- [x] Establish strict cross-engine drift gate script (`scripts/kaze-drift-check.sh --strict`).
- [x] Implement comprehensive capability safety test harness (`kaze/tests/capability_safety.rs`).
- [x] Establish documentation metrics checker (`scripts/check-doc-metrics.sh`).
- [x] Document fault-tolerance checklist across all 49 opcodes (`kaze/docs/FAULT-TOLERANCE-CHECKLIST.md`).
- [ ] Implement automated CI runner for multi-stage Stage 2 / Stage 3 bootstrap hash comparison.
- [ ] Add fuzz-testing harness for dual-syntax parser (`cargo fuzz`).

## Roadmap phase 1-6 update (2026-11-22)

- New gates: `kaze/tests/strict_semantics.rs` (72 fixtures), `units.rs`, `contracts.rs`, `frontend.rs`, `effector_safety.rs`, `emitted_c.rs`, `diagnostics.rs`, `rt_runtime.rs`, and `scripts/self-sync.sh --check`.
- `emitted_c.rs` proves the native path refuses what the interpreter refuses (`E_KAZE_DIV0`, `E_KAZE_OVERFLOW`, `E_KAZE_BOUNDS`, `E_KAZE_RECURSION_DEPTH`, `E_KAZE_CONTRACT`) and honours its memory budget.
- Golden traces now lock behaviour (entry value, actions, logs) and only the shape of the instrumentation line, because instruction counts move whenever the lowering changes; the artifact bytes stay locked by the format-5 fixture.
- `kaze/tests/compat.rs` keeps two fixtures: a format-4 artifact that must keep loading and a format-5 artifact that must not drift.
- Verification: `cargo test --manifest-path kaze/Cargo.toml` and `bash scripts/kaze-drift-check.sh --strict`.
