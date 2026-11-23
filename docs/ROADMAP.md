# Kaze Future Upgrades Roadmap

> **Strategic Architecture & Capability Roadmap for Post-v1.0 Evolution**
> *This roadmap is strictly capability- and milestone-driven. It contains no calendar deadlines or timeline estimates.*

## Current progress: capability safety

This section tracks the standalone Kaze 1.0 source. The separately maintained
Kaze engine embedded in Sofuu has its own status and is not covered by these
results. The [tagged reference](KAZE-1.0-REFERENCE.md) records the current
syntax, capabilities, limits, errors, and configuration in detail.

| Work | Status | Evidence or remaining work |
| --- | --- | --- |
| Standalone capability gates | In progress | Declarations, explicit grants, valid policies, link-loss monitoring, and selected physical argument checks are enforced. Malformed policy, including duplicate keys, is refused. |
| Backend behavior | Done | Unavailable non-robotics capabilities fail; drone and rover simulation require explicit selection and labelled results; live actuation is enabled behind the freshness veto. |
| Heartbeat and telemetry freshness | Done | The link stamps the heartbeat and every field; a command is refused with an uncatchable `E_SAFETY_VIOLATION` when the heartbeat or a field it reads is older than 1500 ms. Verified against a scripted autopilot over UDP; physical hardware has not been tested. |
| Tests and release gates | Partial | 142 selected all-features integration tests, two focused unit tests, Clippy, and Kaze formatting passed locally. Full suites and preflight contain non-robotics stand-ins that require migration under the Sofuu workspace's `RULES.md` §1.9; distribution verification also reported existing artifact failures. |
| Compatibility | Open | Format-4 bytecode compatibility tests pass, but capability behavior and the Rust run report changed. Decide the release/version treatment before claiming a compatible release. |
| Sofuu embedded engine | Separate | The standalone changes do not establish policy or backend behavior for the engine embedded in Sofuu. |

The immediate next steps are freshness enforcement, compliant testing, physical
hardware verification when available, and a release compatibility decision.
The proposed upgrades below remain future work.

---

## Completed: unified Kaze 1.0 language version

Standalone Kaze and Sofuu's embedded engine both report language version **1.0**
and implement the unified 47-opcode contract with 0 drift (spatial kinematics,
vectors, trigonometric math, and try/otherwise recovery).

The [version alignment plan](VERSION-ALIGNMENT.md) records the verified state,
reconciliation of opcodes, Format 1/2/4 bytecode deserialization, and all passing
gates across both engines.

The shared language is also intended to have human-oriented and
machine-oriented source syntaxes. Both must share applicable semantics and
authorization; machine syntax and conversion are planned. Grammar,
initial engine, compatibility and release scope remain open in the Sofuu
workspace's local `docs/plans/PLAN-KAZE-DUAL-SYNTAX.md`.

- [x] Define the shared 1.0 contract and reconcile the existing language gaps.
- [ ] Design both syntaxes against that contract and verify equivalent behavior
      and authority without claiming parity ahead of conformance evidence.
- [ ] Evaluate machine candidates and address human grammar ambiguities,
      composition and structured data under an explicit compatibility plan.
- [x] Integrate the agreed core and fail-closed capability behavior in Sofuu.
- [x] Decide and test bytecode and API migration for existing consumers.
- [x] Verify the same language contract on both surfaces and release artifacts
      before changing Sofuu's reported language version.

---

## Overview & Guiding Philosophy

Kaze's intended scope is general-purpose within AI, across AI software and
AI-powered physical systems, rather than general-purpose across all computing.
Agents and robotics are examples, not the limits of the vision. This roadmap
describes proposed work and does not establish support for every AI workload.

The Kaze language core is independently designed and implemented without
copying other projects' code or exact implementation. Documented external OS,
hardware, security, build, and host providers remain permitted dependencies,
not Kaze-owned code. Review existing core dependencies individually and
consider verified Kaze-owned replacements where useful, without prematurely
removing working dependencies.

**Long-term self-hosting direction:** Explore an independently authored
Kaze-written compiler bootstrapped by the existing Rust implementation after
the language can support it. Establish conformance before claiming
self-hosting. A Kaze-written VM or other components are separate decisions;
this roadmap does not claim any of that work is implemented.

The standalone project identifies its frozen baseline as **Kaze v1.0.0**. Its
source has a fixed-width ISA, a specification, an arena-based VM, and
compile-time modularity. Current release and hardware readiness are subject to
the open work above. Future engineering is organized around five strategic
pillars:

1. **AI Agent Tool-Execution & SDK Ecosystem**
2. **Language Ergonomics & Advanced Expressions**
3. **Multi-Agent Coordination & Swarm Primitives**
4. **Embedded, Bare-Metal (`no_std`) & Bus Expansions**
5. **High-Assurance Formal Verification & Developer Tooling**

Every future upgrade must strictly preserve Kaze's core invariants:
* **Zero Ambient I/O:** Every external effect must remain gated behind explicit `needs:` declarations.
* **Deterministic Execution:** No garbage collector, zero stop-the-world pauses, and fuel-bounded always.
* **Reject, Never Clamp:** Invariant violations (`E_KAZE_RANGE`, `E_SAFETY_VIOLATION`) must never be silently swallowed or coerced.
* **Single-Maintainer Governance:** Maintained exclusively by the developer. External pull requests are not accepted.

---

## Pillar 1: AI Agent Substrate & Ecosystem Integration

This pillar focuses on autonomous AI agents within Kaze's broader AI scope.
Future upgrades aim to streamline how LLMs synthesize, validate, and execute
Kaze programs in-process; safety and supported capabilities require evidence.

### 1. Native Language Bindings (FFI SDKs)
* **Python Bindings (`kaze-py` via PyO3):**
  * Direct in-process execution for LangChain, AutoGen, CrewAI, and LlamaIndex without CLI subprocess overhead.
  * Native translation between Python dictionaries and Kaze maps/records.
  * In-process `CapHost` implementation allowing Python functions to be exposed as Kaze tools.
* **TypeScript / Node.js Bindings (`kaze-node` via napi-rs):**
  * In-process VM evaluation for Node.js, Bun, and Deno agent runtimes.
  * Async tool execution bridges for web-agent architectures.

### 2. Constrained Decoding Grammars for Local LLMs
* **Canonical GBNF Grammar (`kaze.gbnf`):**
  * Formal GBNF grammar derived directly from `docs/SPEC.md` for `llama.cpp`, `vLLM`, and Hugging Face TGI.
  * Enforces grammar-constrained sampling, mathematically eliminating syntax errors during LLM script generation.
* **JSON Schema Tool Descriptors:**
  * Automated conversion between MCP / OpenAPI tool definitions and Kaze capability declarations.

### 3. Agent Standard Library Extensions
* **`lib/agent.kaze`:**
  * Multi-step decision wrappers, intermediate confidence scoring, and structured output formatting.
* **`lib/memory.kaze`:**
  * In-script short-term conversational context buffers and vector store retrieval adapters.

---

## Pillar 2: Language Ergonomics & Expressiveness

Refining language syntax while maintaining simple, zero-shot LLM generation and strict determinism.

### 1. User-Defined Routine Invocation
* Currently, routines in libraries are reachable via `--entry` or supervisor hooks (`when`/`every`).
* **Upgrade:** Direct internal routine calls (`let res = calculate_bounds(p1, p2)`) with clean call-frame stack limits, preventing recursive stack smashing while allowing shared helper logic.

### 2. First-Class Record & Map Literals
* Native syntax for structured key-value maps:
  ```kaze
  let config = { timeout: 500ms, retry: 3, alert_channel: "ops" }
  ```
* Bounded allocation inside the existing collection heap with compile-time key validation.

### 3. String Interpolation & Formatting
* Type-safe, bounded string templates without dynamic buffer reallocation:
  ```kaze
  log "Vehicle reached waypoint {index} at {altitude} meters"
  ```
* Evaluated strictly within the linear arena limit (`Limits::arena_bytes`).

### 4. Deterministic Collection Helpers
* Pure, non-allocating functional iterators over lists and waypoints:
  ```kaze
  let safe_points = points.filter(p => p.z < 100 meters)
  ```
* Bounded loops backed by instruction fuel decrementing.

---

## Pillar 3: Multi-Agent & Swarm Primitives

Extending Kaze from single-vehicle/single-agent supervisors to decentralized multi-agent teams.

### 1. Swarm Rendezvous & Synchronisation
* Native synchronization primitives for drone swarms and rover fleets:
  ```kaze
  needs: swarm, drone

  action sync_patrol():
      swarm.wait_for_peers(4, timeout: 10 seconds)
      drone.goto(formation_point)
  ```
* Bounded timeout semantics ensuring that network dropouts between peers do not stall mission execution.

### 2. Peer-to-Peer Telemetry Broadcast
* Cryptographically authenticated, lightweight inter-agent messaging over UDP and radio mesh:
  ```kaze
  swarm.broadcast({ status: "inspecting", target: waypoint_id })
  ```
* Gated under capability security (`needs: swarm`).

---

## Pillar 4: Embedded, Bare-Metal (`no_std`) & Bus Expansions

Extending Kaze deeper into microcontrollers, avionics buses, and flight controller silicon.

### 1. Bare-Metal `no_std` Target
* Compile the core Kaze bytecode virtual machine for bare-metal ARM Cortex-M4/M7, ESP32, and RISC-V microcontrollers.
* **Zero-Heap Static Mode:** Pre-allocate static memory buffers (e.g. 32 KiB total) without standard library allocator dependencies.

### 2. Peripheral Avionics Bus Drivers
* **CAN Bus / DroneCAN / Cyphal:**
  * Direct interaction with actuator nodes, ESC telemetry, and CAN battery monitors.
* **I2C & SPI Hardware Gates:**
  * Low-level peripheral reads for environmental sensors, payload releases, and custom avionics boards (`needs: i2c`, `needs: spi`).

---

## Pillar 5: High-Assurance Formal Verification & Tooling

Pushing Kaze to mission-critical aerospace and safety-critical industrial standards.

### 1. SMT-Based Static Invariant Verification
* Pre-flight static proof engine utilizing Z3 / SMT solvers.
* Automatically proves that a compiled `.kazebc` module **cannot** command coordinates outside a polygon geofence or exceed battery limits under any execution branch, *before* uploading bytecode to the vehicle.

### 2. Interactive Bytecode Debugger (`kaze debug`)
* Step-by-step bytecode execution with opcode disassembler, stack inspection, and register inspection.
* Integration of the Debug Adapter Protocol (DAP) into `kaze-lsp` for visual debugging in VS Code and Cursor.

### 3. Continuous Fuzzing & Mutation Bench
* Dedicated continuous fuzzing infrastructure using `cargo-fuzz` and `libFuzzer`.
* Multi-million iteration mutation testing over bytecode deserialization, parser error recovery, and host call boundaries.

---

## Summary Matrix of Future Upgrades

| Upgrade Area | Key Capabilities | Primary Target |
|:---|:---|:---|
| **FFI SDKs** | `kaze-py` (PyO3) & `kaze-node` (napi-rs) native bindings | Autonomous AI Agent Frameworks |
| **Constrained Decoding** | Formal GBNF grammar for local SLM/LLM zero-shot synthesis | Local AI & Edge Inference |
| **Language Expressions** | User-routine calls, map literals, string templates | General Developer Ergonomics |
| **Swarm Primitives** | Peer-to-peer sync, multi-agent coordination, broadcast | Autonomous Drone & Rover Fleets |
| **Bare-Metal (`no_std`)** | Zero-heap static VM for Cortex-M and microcontrollers | Deep Edge Micro-Avionics |
| **Hardware Buses** | DroneCAN, Cyphal, I2C, SPI | Avionics & Industrial Hardware |
| **Formal Verification** | SMT/Z3 geofence & safety invariant mathematical proofs | Flight-Critical Mission Assurance |
| **Interactive Debugger** | Step-debugging and DAP support in `kaze-lsp` | Developer Experience |

---

*Maintained exclusively by the developer.*
