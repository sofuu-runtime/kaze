# Kaze Language Master Roadmap: The Self-Hosted Systems Architecture

> **The Core Maxim:** *"If it compiles, it runs without problems in an absolute sense."*  
> **Execution Strategy:** Phase-Ordered, Milestone-Driven, Zero Timelines. Progression occurs strictly upon passing mathematical acceptance gates.

---

## 1. Roadmap Architecture & Execution Principles

This roadmap outlines the complete transformation of Kaze from its current Stage 0 Rust reference implementation into a fully self-hosted, ultra-strict systems programming language. Kaze combines the high-level readability of human sentence syntax with the raw speed, minimal memory footprint, and predictability of hand-optimized ANSI C99, accompanied by compile-time safety that eliminates runtime panics.

### Governing Principles:
1. **Zero Timelines, Pure Capability Gates**:
   - Milestones are not scheduled against arbitrary calendar dates. A phase is complete only when its acceptance tests and verification gates pass with zero warnings, zero memory leaks, and zero drift.
2. **Total Compile-Time Correctness**:
   - Runtime panics, null-pointer dereferences, array out-of-bounds, modulo/division by zero, unhandled capability failures, and dimensional physics errors must be eliminated at compile time.
3. **Self-Hosting Independence**:
   - Rust serves exclusively as the Stage 0 bootstrap engine. At the conclusion of this roadmap, the Rust reference implementation will be retired, and Kaze will compile itself directly to native machine code via ANSI C99.
4. **Target Portability (32-Bit & 64-Bit)**:
   - Every phase must maintain architecture portability across bare-metal 32-bit microcontrollers (ARM Cortex-M, RISC-V RV32) and high-performance 64-bit platforms (x86_64, aarch64 Apple Silicon / Linux).
5. **Zero Domain Hardcoding**:
   - The compiler, lexer, parser, checker, and runtime must remain 100% domain-agnostic. All configuration enters via 12-factor hierarchy.

---

## 2. Master Phase Overview

```
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 1: NATIVE SYSTEMS FOUNDATION & C99 RUNTIME           │
│   • Portable kaze_rt.h / kaze_rt.c     • Zero-GC Linear Bump Arena     │
│   • 32-bit & 64-bit Architecture Types • Checked Arithmetic Primitives │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 2: ULTRA-STRICT SEMANTICS & ZERO-PANIC CORE          │
│   • Mandatory otherwise Fallbacks      • Static Array Bounds & Indexing│
│   • Bounded Loop & Recursion Proofs    • Comprehensive Caret Diagnostic│
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 3: COMPILE-TIME DIMENSIONAL PHYSICS ALGEBRA          │
│   • First-Class Physical Dimensions    • Compile-Time Unit Solver      │
│   • Kinematics & Geodesy Algebra       • Zero-Cost Runtime Lowering    │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 4: ROUTINE CONTRACTS & UNTRUSTED BOUNDARIES          │
│   • Preconditions (requires)           • Postconditions (ensures)      │
│   • Static Range Interval Propagation  • 12-Factor Schema Sanitization │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 5: CAPABILITY GATES & EFFECTOR ISOLATION             │
│   • Zero-Ambient Authority Enforcer    • Fail-Closed Policy Evaluator  │
│   • Real Alert Sinks & Audit Trails    • MAVLink & 1500ms Safety Gates │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 6: PURE-KAZE SELF-HOSTED COMPILER FRONTEND           │
│   • Lexer in Pure Kaze (Off-Side)      • Parser in Pure Kaze (Dual-Syn)│
│   • Semantic Checker in Pure Kaze      • AST & Token Monolith          │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 7: PURE-KAZE NATIVE C99 CODE GENERATOR               │
│   • AST -> ANSI C99 Native Lowering    • Stack & Arena Emission        │
│   • Checked Arithmetic C Macro Lowering• 32/64-Bit Abstract Emitter    │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 8: BOOTSTRAP PIPELINE & FIXED-POINT PROOF            │
│   • Stage 0 (Rust) -> Stage 1 Native   • Stage 1 -> Stage 2 Native     │
│   • Stage 2 -> Stage 3 Native          • Bitwise sha256 Equivalence    │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 9: DEVELOPER TOOLING, LSP & CANONICAL FORMATTER      │
│   • Sub-Millisecond Native LSP Server  • Idempotent kaze fmt           │
│   • CLI Command Suite & Diagnostics    • Single-Binary Delivery        │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 10: RUST RETIREMENT & STANDALONE DISTRIBUTION        │
│   • Remove Rust Stage 0 References     • Pure C & Kaze Standalone Tree │
│   • Clean Single-Command C Build       • Embedded Sofuu Host C-ABI     │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Detailed Phase Specifications

---

### Phase 1: Native Systems Foundation & Portable C99 Runtime Substrate

* **Primary Objective**: Author the zero-dependency, ultra-fast ANSI C99 runtime header and implementation (`kaze_rt.h` and `kaze_rt.c`) that will serve as the execution substrate for all compiled Kaze binaries.
* **Component Breakdown**:
  1. **Linear Bump Arena Allocator (`kaze_arena_t`)**:
     - Fast $O(1)$ allocation by incrementing a pointer offset against a fixed or dynamically sized memory block.
     - Bulk deallocation: reset the offset to zero at routine exit. Zero garbage collection sweeps, zero pointer chasing, zero memory fragmentation.
     - Memory alignment: automatic 8-byte alignment across all allocated structures.
     - Out-of-memory handling: clean, structured trap (`E_KAZE_OOM`) without core dumping.
  2. **Cross-Architecture Type Definitions**:
     - Concrete integer and floating-point abstraction:
       ```c
       #if defined(__SIZEOF_POINTER__) && __SIZEOF_POINTER__ == 4
         typedef int32_t   kaze_int_t;
         typedef uint32_t  kaze_uint_t;
         typedef float     kaze_float_t;
       #else
         typedef int64_t   kaze_int_t;
         typedef uint64_t  kaze_uint_t;
         typedef double    kaze_float_t;
       #endif
       ```
  3. **Checked Arithmetic Primitives**:
     - Safe addition, subtraction, and multiplication using compiler intrinsics (`__builtin_add_overflow`, `__builtin_sub_overflow`, `__builtin_mul_overflow`) or portable bit-twiddling fallbacks on MSVC.
     - Safe division and modulo with explicit divisor checks: zero divisor invokes the registered fallback handler or emits `E_KAZE_DIV0`.
  4. **Core Value Representations**:
     - `kaze_str_t`: Immutable slice pointer and length (`uint8_t* ptr`, `size_t len`).
     - `kaze_vec_t`: Contiguous array header in arena memory (`void* data`, `size_t len`, `size_t cap`, `size_t elem_size`).
     - `kaze_map_t`: Linear probing open-addressed hash map allocated in the arena.
  5. **Hardware Safety Watchdog Hooks**:
     - Millisecond clock timer abstraction (`kaze_time_ms`).
     - Heartbeat watchdog structure tracking last packet timestamps.
     - Signal handlers (`SIGINT`, `SIGTERM`, `SIGSEGV`) triggering registered actuator de-assert functions.
* **Acceptance Gates**:
  - `kaze_rt.h` compiles with zero warnings under `clang -Wall -Wextra -Werror -pedantic -std=c99` and `gcc -Wall -Wextra -Werror -pedantic -std=c99`.
  - Tested clean under both 32-bit (`-m32`) and 64-bit (`-m64`) compilation targets.
  - Valgrind / ASan reports zero memory leaks on 1,000,000 consecutive arena allocation and bulk-reset cycles.

---

### Phase 2: Ultra-Strict Semantics & Zero-Panic Invariants (In Stage 0)

* **Primary Objective**: Upgrade the Stage 0 reference compiler (`kaze/src/checker.rs`, `parser.rs`) so that it statically forbids any construct capable of inducing an uncontrolled runtime panic.
* **Component Breakdown**:
  1. **Mandatory Fallback Enforcer (`otherwise`)**:
     - The parser and checker require every fallible operator to provide an explicit fallback value or code block:
       - Division: `a / b otherwise <fallback>`
       - Modulo: `a % b otherwise <fallback>`
       - Vector Indexing: `vec[i] otherwise <fallback>`
       - Map Lookup: `map[key] otherwise <fallback>`
       - Capability Calls: `eff.call() otherwise <fallback>`
     - Omitting the `otherwise` clause on a fallible expression triggers immediate compile-time rejection: `error[E_KAZE_MISSING_FALLBACK]`.
  2. **Compile-Time Index & Bounded Array Safety**:
     - Constant indices (`vec[3]`) are verified against known array literal bounds at compile time.
     - Dynamic indices (`vec[idx]`) must either be bound by an enclosing range loop (`for i in 0..vec.len`), a range contract (`requires idx < vec.len`), or supply an `otherwise` fallback.
  3. **Guaranteed Loop Termination**:
     - `for` loops iterate over known bounded ranges or collections.
     - `while` loops must declare an explicit fuel ceiling (`while cond bounded 1000:`) or a monotonic progress metric verified by the checker.
  4. **Strict Call Stack Recursion Ceiling**:
     - The type checker analyzes the call graph to detect direct and mutual recursion.
     - Recursive routines must declare bounded depth; the runtime enforces a strict ceiling of $\le 256$ stack frames (`KZ-095/099`), cleanly failing with `E_KAZE_RECURSION_DEPTH`.
  5. **Structured Visual Caret Diagnostics**:
     - Error messages display the exact file path, line, column, multiline context snippet, visual caret indicators (`^^^^`), and concrete suggestions for remediation.
* **Acceptance Gates**:
  - Test suite containing 50+ intentionally flawed programs (divisions without fallback, unbounded loops, raw out-of-bounds indexing) all fail compilation with deterministic `E_KAZE_*` error codes.
  - Zero uncontrolled panics or unhandled exceptions across all test suites.

---

### Phase 3: Compile-Time Dimensional Physics Algebra & Unit Solver

* **Primary Objective**: Integrate a first-class dimensional analysis engine into the Kaze type checker, ensuring that physical world computations are mathematically validated before generating machine code.
* **Component Breakdown**:
  1. **Dimension Vector Model**:
     - Every numeric type can carry an optional dimensional vector:
       $$\text{Dim} = [L: \text{Length}, T: \text{Time}, M: \text{Mass}, A: \text{Angle}, K: \text{Temperature}, P: \text{Pressure}, E: \text{Electrical}]$$
     - Standard dimensionless scalars have $\text{Dim} = [0, 0, 0, 0, 0, 0, 0]$.
  2. **Supported Unit Lexicon**:
     - Length: `meter`, `km`, `cm`, `mm`
     - Time: `second`, `ms`, `hour`, `minute`
     - Mass: `kg`, `gram`
     - Angle: `rad`, `deg`
     - Temperature: `Celsius`, `Kelvin`
     - Pressure: `Pascal`, `mbar`, `hPa`
     - Speed / Velocity: `m/s`, `km/h`
     - Acceleration: `m/s2`
  3. **Dimensional Arithmetic Rules**:
     - **Addition / Subtraction**: $\text{Dim}(A) \equiv \text{Dim}(B)$. The compiler automatically inserts constant conversion factors if units belong to the same dimension (e.g. adding `1.0 km` and `500.0 meter` lowers to `1500.0 meter`).
     - **Multiplication**: $\text{Dim}(A \times B) = \text{Dim}(A) + \text{Dim}(B)$. E.g., $\text{velocity} \times \text{time} = \text{length}$.
     - **Division**: $\text{Dim}(A / B) = \text{Dim}(A) - \text{Dim}(B)$. E.g., $\text{length} / \text{time} = \text{velocity}$.
  4. **Zero-Cost Runtime Lowering**:
     - Units exist purely in the compiler's type checker. Once verified, dimensional types lower directly to primitive `kaze_float_t` or `kaze_int_t` in generated C99 code with zero memory or latency penalty.
  5. **Diagnostic Error Reporting**:
     - Incompatible operations (e.g., adding altitude in `meter` to speed in `m/s`) produce precise diagnostic reports:
       ```
       error[E_KAZE_UNIT_MISMATCH]: cannot add 'meter' to 'm/s'
         --> flight/control.kaze:22:15
          |
       22 |   let target = altitude + current_speed
          |                ^^^^^^^^   ^^^^^^^^^^^^^
          |                |          |
          |                |          unit is 'velocity' (m/s)
          |                unit is 'length' (meter)
       ```
* **Acceptance Gates**:
  - 100% of unit mismatch tests fail at compile time with `E_KAZE_UNIT_MISMATCH`.
  - Generated C99 assembly contains zero unit conversion checks at runtime; all constant scale conversions are folded at compile time.

---

### Phase 4: Routine Contracts & Untrusted Input Boundary Validation

* **Primary Objective**: Provide verifiable pre- and post-condition contracts on routines, and establish strict sanitization barriers at untrusted system boundaries.
* **Component Breakdown**:
  1. **Contract Syntax & Declaration**:
     - Routines declare domain bounds via `requires` and `ensures` clauses:
       ```kaze
       routine set_throttle(channel: int, duty: percent) -> bool:
         requires channel >= 0 and channel <= 8
         requires duty >= 0 and duty <= 100
         ensures result == true
         ...
       ```
  2. **Static Range Interval Propagation**:
     - The type checker maintains numerical intervals for variables ($x \in [\min, \max]$).
     - If the compiler can mathematically prove that caller arguments satisfy the routine's `requires` clause, the call compiles with zero runtime checks.
     - If the caller supplies unverified or dynamic data, the compiler mandates an explicit boundary check or inserts an optimized branch with an `otherwise` fallback.
  3. **Untrusted Input Sanitization at Borders**:
     - Data originating from outside the process (CLI arguments, environment variables, network packets, MAVLink telemetry, configuration files) is typed as `untrusted<T>`.
     - Untrusted values cannot be passed to contracted routines without first passing through a schema parser or validator routine that asserts constraints.
  4. **12-Factor Configuration Resolution**:
     - Routine parameter defaults resolve according to the strict hierarchy:
       $$\text{CLI Flags } (\text{--arg}) \;>\; \text{Environment Variables } (\text{AGENT\_*}) \;>\; \text{Config File } (\text{--config}) \;>\; \text{Signature Defaults}$$
     - Missing required parameters emit structured guidance (`E_KAZE_PARAM`) with exact usage flags instead of crashing.
* **Acceptance Gates**:
  - All routines with contracted bounds reject out-of-range caller values at compile time or route cleanly to `otherwise`.
  - CLI flags and environment variables parse into strongly typed, contracted data structures with zero unhandled format crashes.

---

### Phase 5: Capability Gates, Security Policies & Effector Isolation

* **Primary Objective**: Enforce zero-ambient authority security across filesystem, network, memory, AI models, and robotics effectors, guaranteeing fail-closed execution.
* **Component Breakdown**:
  1. **Zero-Ambient Authority Enforcement**:
     - Routines possess zero implicit system access. Every side-effecting operation requires explicit declaration in the routine header:
       ```kaze
       routine log_and_actuate(val: int) -> bool:
         needs: hardware.gpio("pin12")
         needs: alert.sink("telemetry")
         ...
       ```
  2. **Fail-Closed Security Arbiter**:
     - Any capability invocation missing a corresponding `needs:` clause fails compilation (`E_CAP_UNDECLARED`).
     - At runtime, if a granted capability lacks a configured physical or virtual host backend, it immediately fails closed with `E_CAP_MISSING`. It is strictly forbidden to return dummy or fabricated success (`RULES.md` §1.9).
  3. **Monotonic Capability Attenuation**:
     - Routines calling sub-routines can only pass a subset of their own granted capabilities: $\text{Caps}(\text{Child}) \subseteq \text{Caps}(\text{Parent})$. Escalation of privilege is impossible.
  4. **Real Alert Sinks & Audit Trails**:
     - `alert=stdout`, `alert=stderr`, and `alert=file:<path>` stream raw bytes, enforce length ceilings ($\le 1024$ bytes), and automatically redact sensitive credentials (API keys, authorization headers).
  5. **Physical Robotics Effector Safety Watchdogs**:
     - Unified 1500 ms heartbeat safety gate: any telemetry frame older than $1500\text{ ms}$ triggers immediate fail-safe motor cut or return-to-launch (RTL).
     - Automatic OS exit cleanup: Linux GPIO character device lines and PWM timer registers hook signal handlers to ground actuators cleanly upon abnormal termination (`F-09`).
* **Acceptance Gates**:
  - `kaze/tests/capability_safety.rs` passes 100% of negative tests asserting fail-closed behavior on missing policies.
  - Telemetry freshness watchdog reliably aborts actuation when packet delay exceeds 1500 ms.

---

### Phase 6: Pure-Kaze Self-Hosted Compiler Frontend (`kaze/compiler/`)

* **Primary Objective**: Implement the complete compiler frontend—lexer, parser, dual-syntax engine, and AST data structures—written 100% in pure Kaze.
* **Component Breakdown**:
  1. **Pure-Kaze Lexical Tokenizer (`compiler/lexer.kaze`)**:
     - Indentation stack tracker implementing the off-side rule for human syntax.
     - Exact span tracking (byte offsets, line numbers, column numbers) preserved for every emitted token.
     - Comment and whitespace preservation mechanics for source-to-source tools.
  2. **Pure-Kaze AST Representations (`compiler/ast.kaze`)**:
     - Algebraic data types representing modules, imports, routines, contracts, statements, expressions, literals, and physical units.
  3. **Pure-Kaze Parser (`compiler/parser.kaze`)**:
     - Recursive-descent parser with precedence climbing for binary operators.
     - Transparent auto-detection between human natural syntax and machine S-expressions `(kaze (format 1) ...)`.
  4. **Pure-Kaze Semantic Checker (`compiler/checker.kaze`)**:
     - Scope resolution and symbol table management.
     - Strictness enforcer (checking mandatory `otherwise` on all fallible nodes).
     - Complete dimensional unit algebra solver.
     - Contract condition verifier.
  5. **Pure-Kaze Dual-Syntax Transpiler (`compiler/machine.kaze`)**:
     - Lossless bidirectional transpilation (`to-machine` and `to-human`) satisfying the invariant:
       $$\text{AST}(H) \equiv \text{AST}(\text{to\_human}(\text{to\_machine}(H)))$$
* **Acceptance Gates**:
  - Pure-Kaze frontend parses and type-checks the entire Kaze codebase, including itself (`compiler/kaze_self.kaze`).
  - Round-trip AST identity tests pass across 100+ sample source files.

---

### Phase 7: Pure-Kaze Native C99 Code Generator (`compiler/codegen.kaze`)

* **Primary Objective**: Implement the native ANSI C99 code generator in pure Kaze, transforming verified Kaze ASTs directly into standalone, high-performance C source code.
* **Component Breakdown**:
  1. **AST Lowering Engine (`compiler/codegen.kaze`)**:
     - Lowers Kaze routines into clean, readable C functions prefixed with `kaze_fn_*`.
     - Emits stack-allocated local variables and inserts `kaze_arena_alloc()` calls for dynamic structures.
     - Lowers checked arithmetic into `kaze_rt.h` macros with inlined `otherwise` fallbacks.
     - Lowers routine contracts into optimized entry assertions or elides them when statically proven.
  2. **32-Bit & 64-Bit Target Emitter**:
     - Generates portable code utilizing `kaze_int_t` and `kaze_float_t`.
     - Emits architecture-neutral integer literals with appropriate standard C macros (`INT64_C`, `UINT64_C`).
  3. **Constant Pool & Literal Serialization**:
     - Generates static string slice tables and deduplicated constant pools.
  4. **Main CLI Entrypoint Driver (`compiler/main.kaze`)**:
     - CLI entrypoint handling arguments: `compile`, `emit-c`, `check`, `fmt`, `to-machine`, `to-human`.
* **Acceptance Gates**:
  - The C code emitted by `codegen.kaze` compiles with zero warnings under `clang` and `gcc` with `-Wall -Wextra -Werror -pedantic -std=c99`.
  - Native executables produced from emitted C match the execution traces and outputs of the bytecode VM with microsecond-level latency.

---

### Phase 8: Multi-Stage Bootstrap Pipeline & Fixed-Point Verification

* **Primary Objective**: Execute the complete multi-stage bootstrap pipeline and prove mathematical self-hosting through bitwise fixed-point verification.
* **Component Breakdown**:
  1. **Multi-Stage Bootstrap Protocol**:
     - **Stage 0 (Rust Reference)**: The existing Rust compiler compiles `kaze/compiler/kaze_self.kaze` $\to$ emits `kaze_stage1.c`.
     - **Stage 1 (Bootstrapped Native Executable)**: Standard C compiler (`clang` or `gcc`) compiles `kaze_stage1.c` and `kaze_rt.c` $\to$ produces executable binary `kaze-stage1`.
     - **Stage 2 (Pure Kaze Compiler)**: `kaze-stage1` compiles `kaze/compiler/kaze_self.kaze` $\to$ emits `kaze_stage2.c`.
     - **Stage 3 (Fixed-Point Verification)**: `kaze_stage2.c` is compiled to `kaze-stage2`, which then compiles `kaze/compiler/kaze_self.kaze` $\to$ emits `kaze_stage3.c`.
  2. **Bitwise Fixed-Point Proof**:
     - Verification invariant:
       $$\text{sha256}(\text{kaze\_stage2.c}) \equiv \text{sha256}(\text{kaze\_stage3.c})$$
     - Proves that the compiler is mathematically deterministic and that the compiler written in Kaze correctly compiles itself.
  3. **Automated Verification Harness (`scripts/kaze-bootstrap-verify.sh`)**:
     - Shell script executing Stages 0 through 3, asserting hash equality, and running the full conformance test suite against `kaze-stage2`.
* **Acceptance Gates**:
  - `sha256(kaze_stage2.c)` matches `sha256(kaze_stage3.c)` exactly.
  - Zero differences in AST or bytecode output between Stage 1, Stage 2, and Stage 3 compilers.

---

### Phase 9: Developer Experience, Native LSP & Canonical Formatter

* **Primary Objective**: Deliver production-grade developer tooling written in pure Kaze, including a Language Server Protocol (LSP) daemon, an idempotent code formatter, and single-binary distribution.
* **Component Breakdown**:
  1. **Canonical Code Formatter (`kaze fmt`)**:
     - AST-based formatting enforcing official indentation (2 spaces), operator spacing, and normalized capability clauses.
     - Idempotency invariant: $\text{fmt}(\text{fmt}(\text{source})) \equiv \text{fmt}(\text{source})$.
  2. **Native Language Server Protocol (LSP) Daemon**:
     - Communicates over stdio JSON-RPC 2.0.
     - Sub-millisecond response time for diagnostics, completions, hover documentation, and definition jumping.
     - Semantic token highlighting for physical units and capability clauses.
  3. **Unified Single-Binary CLI**:
     - Standalone executable `kaze` providing:
       - `kaze run <file.kaze>`
       - `kaze check <file.kaze>`
       - `kaze compile <file.kaze> -o <out.kazebc>`
       - `kaze emit-c <file.kaze> -o <out.c>`
       - `kaze fmt <file.kaze>`
       - `kaze to-machine <file.kaze>`
       - `kaze to-human <file.kaze>`
       - `kaze lsp`
* **Acceptance Gates**:
  - `kaze fmt` passes idempotency verification on 100% of codebase files.
  - LSP daemon responds to definition and hover queries in under 5 milliseconds.

---

### Phase 10: Complete Retirement of Rust & Standalone Distribution

* **Primary Objective**: Permanently retire the Rust reference implementation from Kaze. The Kaze compiler and runtime become 100% self-hosted, distributed as a single pure C source file (`dist/kaze.c`) and header (`dist/kaze_rt.h`), building in under one second via any standard C compiler.
* **Component Breakdown**:
  1. **Rust Codebase Deprecation & Removal**:
     - Deprecate and remove `kaze/src/*.rs`.
     - The standalone Kaze repository relies exclusively on `kaze/compiler/*.kaze` and the pre-generated bootstrap files in `dist/`.
  2. **Single-Command Build (`Makefile`)**:
     - Compiling the entire Kaze compiler from source requires only:
       ```bash
       cc -O3 dist/kaze.c dist/kaze_rt.c -lm -o kaze
       ```
     - Zero Rust toolchain, zero cargo dependencies, sub-second build time.
  3. **Embedded Sofuu Host Bridge Update**:
     - The Sofuu runtime embeds Kaze via the clean C-ABI (`sofuu-capi` / `kaze_rt.h`) or directly executes `.kazebc` bytecode.
  4. **Official Standalone Release Packaging**:
     - Pre-compiled standalone binaries for:
       - macOS (arm64 Apple Silicon, x86_64)
       - Linux (x86_64, aarch64, armv7)
       - Bare-metal embedded archives (ARM Cortex-M, RISC-V)
* **Acceptance Gates**:
  - Complete Kaze compiler builds and passes all tests on a fresh machine with only `gcc` or `clang` installed (zero Rust toolchain).
  - Standalone release binary footprint is under 300 KB.
  - Zero cross-engine drift verified against the embedded Sofuu runtime bridge.

---

## 4. Phase Dependency Matrix

| Phase | Description | Prerequisites | Outputs |
|---|---|---|---|
| **Phase 1** | Native C99 Runtime Substrate | None | `kaze_rt.h`, `kaze_rt.c`, arena allocator, 32/64-bit types |
| **Phase 2** | Ultra-Strict Semantics & Zero-Panic Core | Phase 1 | Mandatory `otherwise`, loop bounding, caret diagnostics in Stage 0 |
| **Phase 3** | Compile-Time Dimensional Physics Units | Phase 2 | Dimensional unit solver, zero-cost unit lowering |
| **Phase 4** | Routine Contracts & Boundary Validation | Phase 2, 3 | `requires`/`ensures` contracts, interval propagation, 12-factor CLI |
| **Phase 5** | Capability Gates & Effector Isolation | Phase 2, 4 | Fail-closed capability gates, real alert sinks, 1500ms safety gate |
| **Phase 6** | Pure-Kaze Self-Hosted Frontend | Phase 2, 3, 4, 5 | `compiler/lexer.kaze`, `parser.kaze`, `ast.kaze`, `checker.kaze` |
| **Phase 7** | Pure-Kaze Native C99 Code Generator | Phase 1, 6 | `compiler/codegen.kaze`, `main.kaze`, native C emitter |
| **Phase 8** | Bootstrap Pipeline & Fixed-Point Proof | Phase 6, 7 | Stage 0 $\to$ 1 $\to$ 2 $\to$ 3, verified `sha256(stage2) == sha256(stage3)` |
| **Phase 9** | Developer Tooling, LSP & Formatter | Phase 8 | Native LSP daemon, idempotent `kaze fmt`, CLI toolchain |
| **Phase 10** | Complete Retirement of Rust | Phase 8, 9 | `dist/kaze.c`, sub-second C build, zero Rust dependency |

---

## 5. Continuous Verification & Change Discipline

On every task within any phase:
1. **Never Bypass Invariants**: Zero runtime panics, mandatory `otherwise`, fail-closed capabilities, zero simulated passes.
2. **Never Download Models**: Benchmarks and tests use local GGUFs on disk (`~/Downloads/*.gguf`).
3. **Always Run Drift Check**:
   ```bash
   bash scripts/kaze-drift-check.sh --strict
   ```
4. **Assert Fixed-Point Identity**: Any change to `kaze/compiler/` must pass the bootstrap verification script before merging.

## 6. Phase status (2026-11-22)

Phases 1 through 6 are implemented in the standalone reference engine. The evidence
for every acceptance gate, including the gates that could not run on this machine,
is in [`kaze/docs/PHASE-1-6-REPORT.md`](../kaze/docs/PHASE-1-6-REPORT.md).

| Phase | Status | Primary evidence |
| :--- | :--- | :--- |
| 1. Native C99 runtime substrate | Done | `kaze/dist/kaze_rt.{h,c}`, `kaze/scripts/rt-check.sh` (15 passed, 2 skipped), `kaze/tests/rt_runtime.rs` |
| 2. Ultra-strict semantics and zero panics | Done | `kaze/tests/strict_semantics.rs` (72 fixtures), `kaze/src/strict.rs`, format 5 recursion ceiling in `kaze/src/vm.rs` |
| 3. Compile-time dimensional algebra | Done | `kaze/tests/units.rs`, `Dim`/`Unit` in `kaze/src/ast.rs`, `dim_result` in `kaze/src/checker.rs` |
| 4. Contracts and untrusted boundaries | Done | `kaze/tests/contracts.rs`, `kaze/src/parser.rs` (`contract_block`), 12-factor resolution in `kaze/src/lib.rs` |
| 5. Capability gates and effector isolation | Done | `kaze/tests/capability_safety.rs`, `kaze/tests/effector_safety.rs`, `kaze/docs/CAPABILITY-SECURITY.md` |
| 6. Pure-Kaze self-hosted frontend | Done | `kaze/compiler/{checker,machine}.kaze`, `kaze/tests/frontend.rs`, `kaze/docs/SELF-HOSTING.md` |

Language version remains `1.0`; the module format is `5` (Phase 2 added the
per-item recursion ceiling). The embedded Sofuu engine still reads formats 1, 2
and 4 and keeps the same 49 opcodes and 17 verbs, so `kaze-drift-check.sh --strict`
stays green. The new language surface (fallbacks, bounded loops, guards,
contracts, typed signatures, the widened unit lexicon) is documented in
[`kaze/docs/SPEC.md`](../kaze/docs/SPEC.md) and is not claimed for the embedded
engine.

---

## 7. Post-Bootstrap Extension: Native AI Models, Tensors & Neural Computation

Following the completion of the core 10-phase systems roadmap and self-hosting bootstrap, Kaze expands into first-class neural network modeling, tensor algebra, and automatic differentiation as specified in [09-NATIVE-AI-MODELS-AND-TENSORS.md](./09-NATIVE-AI-MODELS-AND-TENSORS.md).

* **Milestone AI-1: Multidimensional Tensor Primitives (`tensor<T, [dims]>`)**: Linear bump arena allocated strided buffers with compile-time tensor shape inference and dimension mismatch rejection (`E_KAZE_SHAPE_MISMATCH`).
* **Milestone AI-2: CPU SIMD Acceleration Kernels**: ARM NEON and x86 AVX2/AVX-512 GEMM matrix multiplication and activations with quantized block decoding (`Q4_K`, `Q8_0`).
* **Milestone AI-3: Zero-Copy Weight Ingestion**: Direct memory-mapped streaming of GGUF and SafeTensors model weights without memory duplication (`~/Downloads/*.gguf`).
* **Milestone AI-4: Symbolic Autograd Engine**: Compile-time reverse-mode automatic differentiation emitting static C99 backward passes without runtime dynamic graph overhead.
* **Milestone AI-5: Metal GPU Compute Shaders**: Native Metal Shading Language (`.metal`) compute kernels for zero-copy unified memory execution on Apple Silicon M-series.
