# Division 05: Virtual Machine, Linear Arena & Memory (K05)

> **Maturity:** Core  
> **Parent Blueprint:** [kaze-map/README.md](README.md)  
> **Primary Locations:** `kaze/src/vm.rs`, `kaze/src/arena.rs`, `kaze/src/deadline.rs`, `crates/sofuu-core/src/domains/kaze/vm.rs`, `crates/sofuu-core/src/domains/kaze/arena.rs`

---

## 1. Mission & Scope

Division 05 provides the high-performance bytecode interpreter, linear arena memory allocator, and runtime resource budgeting engine for Kaze. While the native C99 generator (Division 04) produces standalone machine binaries, Division 05 powers fast development iterations, interactive scripting, live routine evaluation, and sandboxed embedded execution inside larger host applications.

The virtual machine is engineered for deterministic, real-time safety. It eliminates unpredictable garbage collection latency through bulk-reclaimed linear arenas and guarantees that runaway loops or malicious inputs cannot exhaust system memory or CPU time.

---

## 2. Code Manifest & Owned Files

| Path | Purpose |
|---|---|
| `kaze/src/vm.rs` | Virtual machine bytecode interpreter, register evaluation stack, instruction dispatch loop |
| `kaze/src/arena.rs` | High-speed linear bump allocator, memory segment management, bulk arena reset |
| `kaze/src/deadline.rs` | High-resolution wall-clock execution deadline watchdog |
| `crates/sofuu-core/src/domains/kaze/vm.rs` | Embedded virtual machine mirror in Sofuu core |
| `crates/sofuu-core/src/domains/kaze/arena.rs` | Embedded linear arena allocator mirror in Sofuu core |

---

## 3. Architectural Specifications & Invariants

1. **Zero-GC Linear Bump Allocator**:
   - Dynamic allocations (strings, vectors, maps, closures) are sequentially appended to a contiguous linear buffer.
   - Microsecond allocation cost: allocating $N$ bytes is an addition and bounds check ($O(1)$).
   - Instant bulk reclamation: When a routine or execution cycle completes, the arena offset is reset to zero. There is zero memory fragmentation, zero mark-and-sweep phases, and zero reference counting overhead.
2. **Strict Call Stack Recursion Ceiling**:
   - Call stack depth is strictly limited to $\le 256$ frames (`KZ-095/099`).
   - Exceeding this boundary cleanly aborts execution with `E_KAZE_RECURSION_DEPTH`, preventing native thread stack overflow or undefined crash states.
3. **Deterministic Fuel Accounting**:
   - Every opcode execution deducts calibrated fuel units from an execution budget.
   - When the fuel budget drops to zero, the interpreter immediately halts with `E_KAZE_FUEL_EXHAUSTED`, bounding unbounded loops.
4. **Hard Real-Time Deadline Watchdogs**:
   - Routines executed under real-time constraints configure a microsecond deadline (`deadline.rs`).
   - If execution time exceeds the deadline, the VM yields or triggers a deterministic fallback handler.

---

## 4. Implementation Details & Allocation Topology

### Linear Arena Memory Layout

```
┌────────────────────────────────────────────────────────────────────────┐
│                        LINEAR BUMP ARENA BUFFER                        │
│                                                                        │
│ ┌───────────────┬─────────────────────────┬──────────────┬───────────┐ │
│ │ Vector Data   │ String Buffer           │ Map Buckets  │ Unused    │ │
│ │ (Allocated)   │ (Allocated)             │ (Allocated)  │ Capacity  │ │
│ └───────────────┴─────────────────────────┴──────────────┴───────────┘ │
│ ▲                                                        ▲           ▲ │
│ │                                                        │           │ │
│ Base Offset (0)                                    Active Offset  Capacity
│                                                                        │
│ [Bulk Reset]: Active Offset -> 0 (Instantaneous O(1) Memory Reclaim)   │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 5. Verification & Testing

```bash
# Verify virtual machine bytecode execution loop
cargo test --manifest-path kaze/Cargo.toml --lib vm

# Verify linear bump allocator and reset semantics
cargo test --manifest-path kaze/Cargo.toml --lib arena

# Verify recursion ceiling limits
cargo test --manifest-path kaze/Cargo.toml --test recursion_bounds
```

---

## 6. Active & Completed Tasks

- [x] Implement linear bump arena memory management (`arena.rs`).
- [x] Enforce hard call recursion depth limit of $\le 256$ frames.
- [x] Implement modulo-by-zero and division-by-zero traps (`KZ-035`).
- [x] Implement high-resolution deadline timer (`deadline.rs`).
- [ ] Add per-opcode fuel deduction and quota enforcement in standalone VM.
- [ ] Implement thread-local arena pooling for multi-core worker threads.

## Roadmap phase 1-6 update (2026-11-22)

- The VM enforces the ceiling a routine declared: a call that would break `recursion bounded <n>` stops with `E_KAZE_RECURSION_DEPTH`, and the hard bound for every routine is 256 frames.
- `bounded` loops carry a hidden counter; passing the declared ceiling stops the run with `E_KAZE_LOOP_BOUND`, and fuel still bounds everything else.
- Trap messages may name their own code (`E_KAZE_LOOP_BOUND`, `E_KAZE_CONTRACT`, `E_KAZE_GUARD`), so a stop is diagnosable without guessing.
- Verification: `cargo test --manifest-path kaze/Cargo.toml --test strict_semantics --test verifier --test limits`.
