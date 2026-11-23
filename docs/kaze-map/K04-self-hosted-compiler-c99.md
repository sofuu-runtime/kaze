# Division 04: Self-Hosted Compiler & Native C99 Code Gen (K04)

> **Maturity:** Core / Bootstrap Active  
> **Parent Blueprint:** [kaze-map/README.md](README.md)  
> **Primary Locations:** `kaze/src/compiler.rs`, `kaze/src/c_codegen.rs`, `kaze/compiler/codegen.kaze`, `kaze/compiler/kaze_self.kaze`, `kaze/compiler/main.kaze`, `crates/sofuu-core/src/compiler.rs`

---

## 1. Mission & Scope

Division 04 is responsible for compiling Kaze source code directly to ultra-fast, zero-overhead, portable ANSI C99 native executables, and establishing complete compiler self-hosting where Kaze is written entirely in Kaze itself.

The mission of Division 04 is to give developers a language that reads like high-level human phrasing but executes with the raw speed, minimal memory footprint, and predictability of hand-optimized C and C++. Division 04 achieves this by lowering Kaze ASTs directly to clean, idiomatic C99 code that can be compiled by any standard C compiler (`clang`, `gcc`, `zig cc`, or embedded cross-compilers) without requiring a heavy runtime, virtual machine, or garbage collector.

---

## 2. Code Manifest & Owned Files

| Path | Purpose |
|---|---|
| `kaze/src/c_codegen.rs` | Rust-based Stage 0 native ANSI C99 code generator emitting standalone C source |
| `kaze/src/compiler.rs` | Bytecode compiler emitting compact, relocatable format 4 `.kazebc` modules |
| `kaze/compiler/codegen.kaze` | Pure Kaze self-hosted code generator emitting C99 and bytecode |
| `kaze/compiler/kaze_self.kaze` | Complete unified self-hosted compiler monolith in pure Kaze |
| `kaze/compiler/main.kaze` | Self-hosted compiler CLI entry point in pure Kaze |
| `crates/sofuu-core/src/compiler.rs` | Sofuu standalone binary compiler bundling runtime and bytecode |

---

## 3. Architectural Specifications & Invariants

1. **Three-Stage Multi-Stage Bootstrap Pipeline**:
   - **Stage 0**: Existing Rust reference compiler compiles the pure-Kaze compiler source (`kaze/compiler/kaze_self.kaze`) $\to$ produces `kaze_stage1.c`.
   - **Stage 1**: Standard C compiler compiles `kaze_stage1.c` $\to$ produces native executable `kaze-stage1`.
   - **Stage 2**: `kaze-stage1` compiles `kaze_self.kaze` $\to$ produces `kaze_stage2.c`, compiled to `kaze-stage2`.
   - **Stage 3 (Fixed-Point Verification)**: `kaze-stage2` compiles `kaze_self.kaze` $\to$ produces `kaze_stage3.c`.
   - Invariant: $\text{sha256}(kaze\_stage2.c) \equiv \text{sha256}(kaze\_stage3.c)$.
2. **Strict ANSI C99 Portability**:
   - The emitted C code requires only standard headers (`<stdint.h>`, `<stdbool.h>`, `<stdio.h>`, `<stdlib.h>`, `<string.h>`, `<math.h>`).
   - Zero vendor-specific extensions or non-standard compiler pragmas.
3. **Cross-Target 32-Bit & 64-Bit Parity**:
   - Target architecture pointer width is abstracted via types: `kaze_int_t` (`int32_t` on 32-bit, `int64_t` on 64-bit) and `kaze_uint_t`.
   - Allows seamless targeting of bare-metal microcontrollers (ARM Cortex-M, RISC-V RV32) as well as modern 64-bit servers.
4. **Zero GC Memory Architecture in Generated C**:
   - All temporary working values reside strictly on the native C stack.
   - Dynamic allocations utilize a linear arena buffer passed explicitly as `kaze_arena_t*`. Bulk release occurs upon routine exit with zero fragmentation and zero pointer chasing.

---

## 4. Implementation Details & Generated C Patterns

### Low-Level Memory and Execution Model in Generated C

```c
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>

typedef int64_t kaze_int_t;
typedef double  kaze_float_t;

typedef struct {
  uint8_t* buffer;
  size_t   capacity;
  size_t   offset;
} kaze_arena_t;

static inline void* kaze_arena_alloc(kaze_arena_t* arena, size_t bytes) {
  size_t aligned = (bytes + 7) & ~7;
  if (arena->offset + aligned > arena->capacity) {
    fprintf(stderr, "error[E_KAZE_OOM]: linear arena exhausted\n");
    exit(1);
  }
  void* ptr = arena->buffer + arena->offset;
  arena->offset += aligned;
  return ptr;
}

// Emitted routine calculate_velocity
kaze_float_t kaze_fn_calculate_velocity(kaze_float_t distance, kaze_float_t duration) {
  // Preconditions checked
  if (duration <= 0.0) {
    return 0.0; // Handled otherwise fallback
  }
  return distance / duration;
}
```

---

## 5. Verification & Testing

```bash
# Test Rust-based ANSI C99 code generator
cargo test --manifest-path kaze/Cargo.toml --lib c_codegen

# Emit C source for an example agent routine and compile natively
cargo run --manifest-path kaze/Cargo.toml -- emit-c examples/simple.kaze -o /tmp/simple.c
clang -O3 -Wall -Wextra /tmp/simple.c -o /tmp/simple_bin
/tmp/simple_bin

# Run self-hosted compiler benchmark in Kaze
cargo run --manifest-path kaze/Cargo.toml -- run kaze/compiler/bench.kaze
```

---

## 6. Active & Completed Tasks

- [x] Implement Rust-based C99 native code generator (`kaze/src/c_codegen.rs`).
- [x] Author pure-Kaze self-hosted compiler frontend (`compiler/lexer.kaze`, `compiler/parser.kaze`).
- [x] Author pure-Kaze self-hosted code generator (`compiler/codegen.kaze`).
- [x] Author unified compiler monolith (`compiler/kaze_self.kaze`).
- [x] Add CLI command `kaze emit-c <file.kaze> -o <file.c>`.
- [ ] Automate Stage 1 $\to$ Stage 2 $\to$ Stage 3 bootstrap fixed-point verification script.
- [ ] Implement SIMD vectorization lowering in generated C for vector operations.

## Roadmap phase 1-6 update (2026-11-22)

- The ANSI C99 substrate is real: `kaze/dist/kaze_rt.h` and `kaze_rt.c` (arena, checked arithmetic with a fallback handler, str/vec/map, clock, 1500 ms watchdog, signal cleanup), gated by `kaze/scripts/rt-check.sh` and `kaze/tests/rt_runtime.rs`.
- The Rust emitter lowers the new constructs: `kz_fb_div`/`kz_fb_mod`/`kz_fb_index` for fallbacks (eager fallback, single evaluation of the operands), a counter guard for `bounded` loops, and a trap for `guard`.
- The pure-Kaze front end gained `compiler/checker.kaze` and `compiler/machine.kaze` (Phase 6) and the monolith is generated, not hand-maintained: `scripts/self-sync.sh` derives `compiler/kaze_self.kaze` from the eight modules and `--check` proves it is current.
- The emitted C counts every allocation against a budget (1 GiB default, `KAZE_MAX_MEMORY` to raise it) and stops with `E_KAZE_MEMORY` and exit 70 instead of being OOM-killed; `kaze/tests/emitted_c.rs` proves both the refusal and the raised-budget run.
- Verification: `bash kaze/scripts/rt-check.sh`, `cargo test --test compat --test frontend --test emitted_c`, `bash kaze/scripts/self-sync.sh --check`.
