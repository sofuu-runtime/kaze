# Division 01: Language Core, Bytecode ISA & Instructions (K01)

> **Maturity:** Core  
> **Parent Blueprint:** [kaze-map/README.md](README.md)  
> **Primary Locations:** `kaze/src/isa.rs`, `kaze/src/ast.rs`, `kaze/src/value.rs`, `kaze/compiler/token.kaze`, `kaze/compiler/ast.kaze`, `crates/sofuu-core/src/domains/kaze/isa.rs`, `crates/sofuu-core/src/domains/kaze/ast.rs`

---

## 1. Mission & Scope

Division 01 defines the deterministic Instruction Set Architecture (ISA), low-level virtual machine opcodes, AST data representations, and value typing for the Kaze language. It serves as the immutable structural contract shared between the virtual machine runtime, the self-hosted pure Kaze compiler, and the native ANSI C99 code generator.

Kaze is architected as a systems-level language combining high-level human readability with zero-overhead native execution. Division 01 guarantees that whether an AST is interpreted by the bytecode VM or compiled to native C99 machine code, the operational semantics, precision, and error conditions remain mathematically identical.

---

## 2. Code Manifest & Owned Files

| Path | Purpose |
|---|---|
| `kaze/src/isa.rs` | Unified 49-opcode specification, bytecode format 4 binary encoder and decoder, statement debug maps |
| `kaze/src/ast.rs` | Core AST node definitions, statement and expression types, source span tracking |
| `kaze/src/value.rs` | Tagged union value representations (`Val`): integers, floats, booleans, strings, vectors, maps |
| `kaze/compiler/token.kaze` | Pure Kaze enumeration of lexical tokens for self-hosted compiler frontend |
| `kaze/compiler/ast.kaze` | Pure Kaze AST node structures used by the self-hosted parser and code generator |
| `crates/sofuu-core/src/domains/kaze/isa.rs` | Sofuu embedded engine mirror of the 49-opcode ISA |
| `crates/sofuu-core/src/domains/kaze/ast.rs` | Sofuu embedded engine mirror of AST node representations |

---

## 3. Architectural Specifications & Invariants

1. **Deterministic 47-Opcode Contract**:
   - The virtual instruction set consists of exactly 47 unified opcodes.
   - Opcode numbering and execution semantics must remain in 100% lockstep between standalone Kaze and embedded Sofuu Kaze (`scripts/kaze-drift-check.sh --strict`).
2. **Bytecode Format Version 4**:
   - Bytecode serialization follows format 4 specifications: magic header `KZBC`, format version `4`, constant table, routine metadata table, and statement-line source mapping.
   - Line numbers are preserved for every instruction to ensure zero-cost stack traces and multiline source caret diagnostic rendering.
3. **Value Tagging & Memory Representation**:
   - Values (`Val`) are represented as compact tagged unions. Immediate primitives (integers, floats, booleans) require zero heap allocation.
   - Aggregates (strings, vectors, maps) allocate into the active linear bump arena (`arena.rs`) and are never managed by a garbage collector.
4. **Zero-Panic Arithmetic Semantics**:
   - Division by zero and modulo by zero are trapped as explicit runtime errors (`E_KAZE_DIV0`), never inducing an OS signal or CPU divide error.
   - Integer overflow behavior is strictly defined: debug builds trap, release builds use defined two's-complement wrapping with optional static overflow analysis.

---

## 4. Implementation Details & Algorithms

### Unified 47-Opcode Table

| Opcode | Hex | Mnemonic | Operands | Description |
|---|---|---|---|---|
| `0x00` | `00` | `NOP` | - | No operation |
| `0x01` | `01` | `CONST` | `idx: u16` | Push constant value from pool |
| `0x02` | `02` | `LOAD` | `slot: u16` | Load local variable onto evaluation stack |
| `0x03` | `03` | `STORE` | `slot: u16` | Store top of stack into local variable |
| `0x04` | `04` | `ADD` | - | Binary addition (type/unit checked) |
| `0x05` | `05` | `SUB` | - | Binary subtraction |
| `0x06` | `06` | `MUL` | - | Binary multiplication |
| `0x07` | `07` | `DIV` | - | Binary division with non-zero check |
| `0x08` | `08` | `MOD` | - | Binary modulo with non-zero check |
| `0x09` | `09` | `NEG` | - | Unary negation |
| `0x0A` | `0A` | `NOT` | - | Logical negation |
| `0x0B` | `0B` | `EQ` | - | Equality comparison |
| `0x0C` | `0C` | `NEQ` | - | Inequality comparison |
| `0x0D` | `0D` | `LT` | - | Less-than comparison |
| `0x0E` | `0E` | `LTE` | - | Less-than-or-equal comparison |
| `0x0F` | `0F` | `GT` | - | Greater-than comparison |
| `0x10` | `10` | `GTE` | - | Greater-than-or-equal comparison |
| `0x11` | `11` | `JMP` | `offset: i16` | Unconditional relative jump |
| `0x12` | `12` | `JMP_IF` | `offset: i16` | Jump if top of stack is truthy |
| `0x13` | `13` | `JMP_NOT` | `offset: i16` | Jump if top of stack is falsy |
| `0x14` | `14` | `CALL` | `routine: u16, argc: u8` | Call routine with bounded stack frame |
| `0x15` | `15` | `RET` | - | Return from routine |
| `0x16` | `16` | `VEC_NEW` | `cap: u16` | Allocate vector in linear bump arena |
| `0x17` | `17` | `VEC_GET` | - | Indexed vector access with bounds check |
| `0x18` | `18` | `VEC_SET` | - | Indexed vector assignment with bounds check |
| `0x19` | `19` | `MAP_NEW` | `cap: u16` | Allocate hash map in arena |
| `0x1A` | `1A` | `MAP_GET` | - | Key lookup with fail-safe fallback |
| `0x1B` | `1B` | `MAP_SET` | - | Key assignment in arena |
| `0x1C` | `1C` | `GEO_DIST` | - | Geodesy distance calculation |
| `0x1D` | `1D` | `GEO_BEAR` | - | Geodesy bearing calculation |
| `0x1E` | `1E` | `CAP_CHECK`| `cap_id: u16` | Verify runtime capability grant |
| `0x1F` | `1F` | `EFF_DISP` | `eff_id: u16` | Dispatch authorized hardware effector |
| `0x20..0x2E`| `20..` | `EXT_OPS` | - | Reserved extensions (MAVLink, async bridge, telemetry) |

---

## 5. Verification & Testing

```bash
# Test ISA opcode serialization and deserialization
cargo test --manifest-path kaze/Cargo.toml --lib isa

# Test AST data structure construction and cloning
cargo test --manifest-path kaze/Cargo.toml --lib ast

# Verify cross-engine opcode alignment
bash scripts/kaze-drift-check.sh --strict
```

---

## 6. Active & Completed Tasks

- [x] Standardize 47 unified opcodes across standalone and embedded engines.
- [x] Implement Format 4 relocatable binary bytecode serialization.
- [x] Add statement-line debug maps to opcode stream.
- [x] Replicate token and AST definitions in pure Kaze (`compiler/token.kaze`, `compiler/ast.kaze`).
- [ ] Add compact constant-pool deduplication to reduce `.kazebc` binary size.
- [ ] Implement compressed instruction stream encoding for memory-constrained 32-bit targets.

## Roadmap phase 1-6 update (2026-11-22)

- The instruction set is **49 opcodes (0-48)**; `CallRoutine` (47) and `NewMap` (48) arrived with records and intra-module calls.
- **Module format 5**: every item carries a `u16 recursion_bound` (0 = not declared). The reader accepts 4 and 5; a format-4 artifact loads with bound 0.
- `kaze/tests/verifier.rs::opcode_numbers_are_locked` and `the_module_format_is_locked` (5) are the locks; `kaze/tests/compat.rs` holds a format-4 fixture that must keep loading and a format-5 fixture that must not drift.
- New opcode-free language surface lowered with existing instructions: inline `otherwise` (temporary-local + fused compare-and-branch), `bounded` loops (hidden counter + trap), `guard` (trap).
- Verification: `cargo test --manifest-path kaze/Cargo.toml --test verifier --test compat`.
