# Division 09: Embedded Sofuu Bridge & C-FFI Integrations (K09)

> **Maturity:** Core  
> **Parent Blueprint:** [kaze-map/README.md](README.md)  
> **Primary Locations:** `crates/sofuu-core/src/domains/kaze/`, `crates/sofuu-core/src/script_cmd.rs`, `crates/sofuu-core/src/compiler.rs`, `crates/sofuu-capi/`

---

## 1. Mission & Scope

Division 09 provides the seamless architectural bridge between Kaze and host application environments—most prominently the Sofuu AI-native runtime substrate, QuickJS JavaScript engine, and foreign language C-ABIs. Division 09 ensures that Kaze functions both as an autonomous standalone systems language and as an embeddable, secure execution engine inside higher-level platforms.

Division 09 owns the asynchronous continuation bridge that allows pure Kaze routines to invoke host-managed capabilities (such as asynchronous AI model queries and MCP tool calls) without blocking OS threads, while preserving 100% syntactic and semantic lockstep with standalone Kaze.

---

## 2. Code Manifest & Owned Files

| Path | Purpose |
|---|---|
| `crates/sofuu-core/src/domains/kaze/` | In-tree embedded mirror of the Kaze compiler, parser, checker, and VM (`crate::sfs`) |
| `crates/sofuu-core/src/script_cmd.rs` | Sofuu CLI commands for Kaze: `sofuu script check`, `compile`, `run`, `repl` |
| `crates/sofuu-core/src/compiler.rs` | Standalone binary compiler packaging runtime, bytecode, and configuration |
| `crates/sofuu-capi/` | Exported C ABI symbols for hosting Kaze in iOS, Android, and C++ runtimes |

---

## 3. Architectural Specifications & Invariants

1. **Zero-Drift Synchronization Mandate**:
   - The embedded Kaze codebase (`crates/sofuu-core/src/domains/kaze/`) and the standalone repository (`kaze/src/`) must remain in 100% lexical and behavioral parity.
   - Any commit modifying Kaze must pass `scripts/kaze-drift-check.sh --strict` with zero diffs in opcodes, verbs, or grammar structures.
2. **Asynchronous Continuation Bridge (`sofuu.kaze.runAsync`)**:
   - When a Kaze routine invokes an asynchronous external capability (`ai.prompt`, `tool.execute`), execution cleanly suspends at the VM boundary.
   - Host adapters evaluate security policy, dispatch the async request, and resume the Kaze VM continuation without blocking the event loop thread (`544c680c8`).
3. **Standalone Binary Embedding & Size Budget**:
   - `sofuu compile <agent.kaze>` bundles the runtime, configuration, and compiled `.kazebc` into a single standalone binary.
   - All binaries produced must comply with the repository size ceiling of $\le 5,872,025\text{ bytes}$ ($5.6\text{ MiB}$).
4. **Clean C-ABI Exports**:
   - Foreign runtimes interact with Kaze through a clean, opaque-pointer C ABI (`sofuu-capi`), enabling seamless integration into Android (via JNI) and iOS (via Swift Package Manager).

---

## 4. Implementation Details & Async Suspension Flow

```
┌────────────────────────────────────────────────────────┐
│              KAZE VM EXECUTION (EMBEDDED)              │
│       Routine reaches authorized 'ai.prompt(...)'      │
└───────────────────────────┬────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────┐
│               VM SUSPENSION CONTINUATION               │
│   • Serialize current VM register & call stack frame   │
│   • Yield execution to host event loop (libuv)         │
└───────────────────────────┬────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────┐
│              SOFUU HOST ASYNC DISPATCH                 │
│   • Validate request against active security policy    │
│   • Execute local GGUF Metal kernel or remote provider │
└───────────────────────────┬────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────┐
│                   VM RESUME & RETURN                   │
│   • Push model result onto evaluation stack            │
│   • Resume bytecode execution seamlessly               │
└────────────────────────────────────────────────────────┘
```

---

## 5. Verification & Testing

```bash
# Verify embedded Kaze domain unit tests
cargo test -p sofuu-core --lib domains::kaze

# Verify strict cross-engine drift gate
bash scripts/kaze-drift-check.sh --strict

# Verify real local GGUF AI execution through embedded bridge
python3 scripts/test-kaze-local-gguf-real.py
```

---

## 6. Active & Completed Tasks

- [x] Unify 49 opcodes and 17 verbs between standalone and embedded engines (`kaze/docs/VERSION-ALIGNMENT.md`).
- [x] Implement asynchronous continuation bridge (`sofuu.kaze.runAsync`, commit `544c680c8`).
- [x] Enforce automated cross-engine drift check in CI (`scripts/kaze-drift-check.sh --strict`).
- [x] Standalone binary compilation with 12-factor argument resolution (`sofuu compile`).
- [ ] Add zero-copy memory buffer sharing between QuickJS strings and Kaze VM arena.

## Roadmap phase 1-6 update (2026-11-22)

- No change in this round: the embedded engine implements the v1.0 surface. The two engines share 49 opcodes and 17 verbs and `scripts/kaze-drift-check.sh --strict` is green.
- The strict surface is deliberately not claimed for the embedded engine; bringing it across is a separate alignment decision recorded in `kaze/docs/VERSION-ALIGNMENT.md`.
- Verification: `cargo test -p sofuu-core domains::kaze` in the root workspace and `bash scripts/kaze-drift-check.sh --strict`.
