# Kaze C99 runtime substrate (kaze_rt)

Phase 1 of the master roadmap: the portable ANSI C99 runtime that every lowered
Kaze program will link against. Two files, no generated code:

| File | Role |
| :--- | :--- |
| [kaze_rt.h](../dist/kaze_rt.h) | Declarations, cross-architecture types, value model, trap codes |
| [kaze_rt.c](../dist/kaze_rt.c) | Arena, checked arithmetic, containers, clock, watchdog, signal cleanup |

The Rust reference model it mirrors is [arena.rs](../src/arena.rs) (one linear
arena per run, bulk free) and [value.rs](../src/value.rs) (a tagged value enum
with unit/bool/number/text/list/map/point). The C version keeps the same
semantics and the same vocabulary; it drops the borrow checker, so ownership is
replaced by one rule: every pointer into an arena dies at `kaze_arena_reset`.

## 1. Design rules

1. **ANSI/ISO C99.** No C11 features, no extensions required. Compiler
   intrinsics (`__builtin_*_overflow`) are used when present, never required.
2. **Eight headers, nothing else.** `<stdint.h> <stdbool.h> <stddef.h>
   <string.h> <stdio.h> <math.h> <time.h>` and, unless `KAZE_NO_SIGNALS` is
   defined, `<signal.h>`. The gate counts them and fails on a ninth.
3. **One heap entry point.** `malloc` appears twice in `kaze_rt.c`: its manual
   declaration and the call inside `kaze_arena_init_heap`. Containers allocate
   from the arena they are handed.
4. **No panic paths.** No `abort`, no `longjmp`, no `exit` on a trap path (the
   signal handler's `_exit` is the only process-ending call, and it runs only on
   SIGINT/SIGTERM/SIGSEGV). A failure is a code string plus a message handed to
   a registered fallback handler; the caller keeps running with a documented
   fallback value.
5. **Nothing undefined.** Signed overflow, division by zero and `INT_MIN / -1`
   are checked before the operation, never executed.

Example build, no `-lm` required (the runtime references no libm symbol):

```bash
cc -std=c99 -Wall -Wextra -Werror -pedantic -I kaze/dist app.c kaze/dist/kaze_rt.c -o app
```

## 2. Gate commands

The Phase 1 gate is a shell script because everything it asserts is a toolchain
fact rather than a cargo fact.

```bash
# Full gate: strict builds, behaviour, 32-bit status, ASan/UBSan.
cd kaze
./scripts/rt-check.sh

# Same, printing every check line and every compiler log.
./scripts/rt-check.sh --verbose

# Rust-side entry point (from the repository root). Skips with a printed
# reason when no C compiler is on PATH.
cargo test --manifest-path kaze/Cargo.toml --test rt_runtime
```

Exit status: `0` every check that ran passed, `1` at least one check failed,
`2` no C compiler was found. A `SKIP` line is printed for any property the host
cannot verify; a skip is not a pass.

### What the gate checks

| Gate | What it asserts |
| :--- | :--- |
| `clang 64-bit build` | `-std=c99 -Wall -Wextra -Werror -pedantic` with zero warnings |
| `clang 64-bit behaviour` | 131 behavioural checks (see section 12) |
| `gcc 64-bit build` / `behaviour` | The same, under gcc; SKIP when no gcc is on PATH |
| `header self-contained` | `kaze_rt.h` parses twice in one translation unit (include guard) |
| `header C++ safety` | `extern "C"` and every typedef compile as C++11 |
| `include allowlist` | Exactly eight `#include` lines in `kaze_rt.h` |
| `heap discipline` | `malloc` appears exactly twice in `kaze_rt.c` and both lines are printed |
| `no abort/longjmp/exit` | Comment-stripped scan finds no process-ending call |
| `escape hatches` | `KAZE_NO_SIGNALS` + `KAZE_NO_HEAP` still compile |
| `clang -m32 build + run` | A real 32-bit executable; SKIP when the host cannot link 32-bit |
| `clang -m32 codegen` | `kaze_rt.c` emits a 32-bit object, and a compile-time probe asserts the 4-byte type block |
| `KAZE_USE_DOUBLE at -m32` | `kaze_float_t` is 8 bytes when the hatch is defined |
| `i386 freestanding syntax` | `-target i386-none-elf -ffreestanding -fsyntax-only` against stub libc headers |
| `ASan/UBSan behaviour` | The behavioural suite under `-fsanitize=address,undefined` |
| `ASan/UBSan 1e6 arena cycles` | 1,000,000 `kmalloc` + `kaze_arena_reset` cycles plus 100 container builds |
| `leak detection (LSan)` | `ASAN_OPTIONS=detect_leaks=1`; SKIP on hosts without LeakSanitizer |

## 3. Cross-architecture types

```c
#if defined(__SIZEOF_POINTER__) && __SIZEOF_POINTER__ == 4
  typedef int32_t  kaze_int_t;
  typedef uint32_t kaze_uint_t;
  typedef float    kaze_float_t;   /* double when KAZE_USE_DOUBLE is defined */
#else
  typedef int64_t  kaze_int_t;
  typedef uint64_t kaze_uint_t;
  typedef double   kaze_float_t;
#endif
```

MSVC does not define `__SIZEOF_POINTER__`, so a 32-bit MSVC build is detected
through `_WIN32`/`_WIN64` instead of silently getting the 64-bit types.

| Type | Meaning |
| :--- | :--- |
| `kaze_int_t`, `kaze_uint_t` | Pointer-width signed/unsigned integer |
| `kaze_float_t` | Wide float: `float` on 32-bit, `double` on 64-bit, `double` with `KAZE_USE_DOUBLE` |
| `kaze_size_t`, `kaze_ptrdiff_t` | Synonyms for `size_t` and `ptrdiff_t` |
| `u8 u16 u32 u64` | `uint8_t … uint64_t` |
| `i8 i16 i32 i64` | `int8_t … int64_t` |
| `f32 f64` | `float` / `double` |
| `kaze_bool_t` | `bool` from `<stdbool.h>`. The bare name `bool` is a macro in C99, so a typedef cannot be called that. |
| `KAZE_INT_BITS` | `32` or `64`, selected with the type block |
| `KAZE_INT_MAX`, `KAZE_INT_MIN`, `KAZE_UINT_MAX` | The matching limits, cast to `kaze_int_t`/`kaze_uint_t` |
| `KAZE_STATIC_ASSERT(cond, name)` | C99 static assertion (negative array length); the name must be unique per translation unit |

The header fails the build rather than miscompiling on an ABI it does not
support: it asserts that `kaze_int_t` matches the pointer width and that
`float`/`double` are 4/8 bytes. Measured sizes on the reference host
(macOS arm64; ILP32 numbers from the compile-time probe described in section 13):

| Type | 64-bit (measured at runtime) | 32-bit armv4t (compile-time probe and symbol sizes) |
| :--- | :--- | :--- |
| `kaze_str_t` | 16 | 8 |
| `kaze_vec_t` | 32 | 16 |
| `kaze_val_payload_t` | 24 | 12 (float) / 24 (`KAZE_USE_DOUBLE`) |
| `kaze_val_t` | 32 | 16 (float) / 28 (`KAZE_USE_DOUBLE`) |
| `kaze_map_entry_t` | 56 | 32 (float) / 44 (`KAZE_USE_DOUBLE`) |
| `kaze_arena_t` | 40 | 20 |

Exact padding is the ABI's business; the gate asserts budgets instead of
offsets (`kaze_str_t` is two words, the value is tag plus payload, the arena
header is at most 32 bytes on ILP32). The armv4t numbers also show why: that ABI
lays a `double` out with 4-byte alignment, so `kaze_val_t` with
`KAZE_USE_DOUBLE` is 28 bytes there, not 32.

## 4. Traps

Every failure the runtime can detect is reported as a code string and a
one-line message. Nothing prints by default except the default handler, and
nothing aborts.

| `kaze_code_t` | Code string | Raised when |
| :--- | :--- | :--- |
| `KAZE_OK` | `KAZE_OK` | Not a trap: the success value of `kaze_watchdog_abort_if_stale` |
| `KAZE_ERR_OOM` | `E_KAZE_OOM` | The arena cannot satisfy a request; the map or vector cannot grow; the de-assert registry is full |
| `KAZE_ERR_OVERFLOW` | `E_KAZE_OVERFLOW` | Checked integer overflow, `INT_MIN / -1`, `INT_MIN % -1`, `-INT_MIN` |
| `KAZE_ERR_DIV0` | `E_KAZE_DIV0` | Integer division or modulo by zero; float division by zero |
| `KAZE_ERR_MATH` | `E_KAZE_MATH` | A float result that is not finite (IEEE inf or NaN) |
| `KAZE_ERR_SAFETY_VIOLATION` | `E_SAFETY_VIOLATION` | The heartbeat watchdog is stale (fail-closed) |
| `KAZE_ERR_BADARG` | `E_KAZE_BADARG` | NULL arena, NULL out-parameter, NULL callback, zero element size |

| Function | Contract |
| :--- | :--- |
| `typedef int (*kaze_fallback_fn)(const char *code, const char *message)` | Receives the code string and the message, returns an int that the trapping wrapper hands back to its caller. Must not allocate or abort unless the embedder wants that. |
| `void kaze_set_fallback_handler(kaze_fallback_fn fn)` | Installs a handler; `NULL` restores the default. Global, not thread-safe; set it during startup. |
| `kaze_fallback_fn kaze_get_fallback_handler(void)` | Returns the active handler (never `NULL`). |
| `int kaze_default_fallback(const char *code, const char *message)` | Prints `CODE: message` to `stderr` and returns `1`. |
| `const char *kaze_code_name(kaze_code_t code)` | The code string for an enum value; never `NULL`. |
| `int kaze_trap(kaze_code_t code, const char *message)` | Raises a trap through the active handler and returns its status. Public so lowered code and hosts can raise the same codes the runtime does. |

A trapping wrapper ignores the handler's return value and returns its documented
fallback (`0` for integers, `0.0` for floats, `NULL` or `false` for pointers and
booleans). A handler that returns never resumes the failed operation: it has
already been abandoned.

## 5. Linear bump arena

`kaze_arena_t` is one buffer plus a byte cursor. Allocation bumps the cursor;
there is no per-object free, no fragmentation, and reset returns the whole run's
memory at once.

| Field | Meaning |
| :--- | :--- |
| `base` | First byte the cursor may use; aligned up from the caller's buffer |
| `cap` | Usable bytes from `base` |
| `offset` | Next free byte. May be unaligned after an odd-sized allocation; every returned pointer is not. |
| `failed` | Sticky: some allocation in this run did not fit |
| `owned` | `base` came from `kaze_arena_init_heap` |
| `raw` | The pointer `kaze_arena_free` hands back to `free` |

| Function | Contract |
| :--- | :--- |
| `void kaze_arena_init(kaze_arena_t *arena, void *buffer, size_t capacity)` | Bind caller storage (stack array, static buffer, slice of a larger block). A misaligned buffer costs at most 7 leading bytes; everything handed out is still 8-byte aligned. A `NULL` buffer yields an empty arena whose allocations trap `E_KAZE_BADARG`. |
| `bool kaze_arena_init_heap(kaze_arena_t *arena, size_t capacity)` | The only heap path: allocates `capacity` usable bytes (`+16` alignment headroom internally) and marks the arena as owned. Returns `false` after trapping `E_KAZE_OOM`. |
| `void kaze_arena_free(kaze_arena_t *arena)` | Frees an owned block and zeroes the header. Safe on a caller-buffer arena (it frees nothing). |
| `void *kmalloc(kaze_arena_t *arena, size_t size)` | O(1) bump allocation, 8-byte aligned. Returns `NULL` and sets `failed` when the request does not fit; the cursor is unchanged in that case, so a smaller request afterwards still succeeds. `size == 0` consumes nothing and returns the current aligned cursor. |
| `void kaze_arena_reset(kaze_arena_t *arena)` | Bulk free: `offset = 0`, capacity retained, `failed` cleared. Every pointer into the arena is dead afterwards. |
| `bool kaze_arena_require(kaze_arena_t *arena, size_t size)` | Pre-flight check including worst-case alignment padding. On failure it records `E_KAZE_OOM` and sets `failed`, so a caller can pick a smaller plan before allocating. |
| `size_t kaze_arena_used(const kaze_arena_t *arena)` | Bytes consumed so far. |
| `size_t kaze_arena_capacity(const kaze_arena_t *arena)` | Usable bytes. |
| `bool kaze_arena_failed(const kaze_arena_t *arena)` | The sticky OOM flag. |

Why `failed` is sticky and cleared only by reset: it is a diagnostic for the run
that ran out of memory, not a lock. A controller that hit OOM mid-tick should
still be able to log or de-assert actuators with a small allocation, and the
next tick starts from a reset arena anyway.

## 6. Checked arithmetic

The `*_checked` family never traps. It returns `false` and stores `0` in `*out`,
so a caller that ignores the result cannot read a torn value. The bare
`kaze_add` … `kaze_float_div` wrappers are the trapping forms.

| Checked function | Failure |
| :--- | :--- |
| `bool kaze_add_checked(kaze_int_t a, kaze_int_t b, kaze_int_t *out)` | `false` on overflow |
| `bool kaze_sub_checked(kaze_int_t a, kaze_int_t b, kaze_int_t *out)` | `false` on overflow |
| `bool kaze_mul_checked(kaze_int_t a, kaze_int_t b, kaze_int_t *out)` | `false` on overflow |
| `bool kaze_div_checked(kaze_int_t a, kaze_int_t b, kaze_int_t *out)` | `false` on `b == 0` or `INT_MIN / -1` |
| `bool kaze_mod_checked(kaze_int_t a, kaze_int_t b, kaze_int_t *out)` | `false` on `b == 0` or `INT_MIN % -1` |
| `bool kaze_neg_checked(kaze_int_t a, kaze_int_t *out)` | `false` on `-INT_MIN` |
| `bool kaze_float_div_checked(kaze_float_t a, kaze_float_t b, kaze_float_t *out)` | `false` on a zero divisor or a non-finite result |

| Trapping wrapper | Code | Fallback |
| :--- | :--- | :--- |
| `kaze_int_t kaze_add(kaze_int_t a, kaze_int_t b)` | `E_KAZE_OVERFLOW` | `0` |
| `kaze_int_t kaze_sub(kaze_int_t a, kaze_int_t b)` | `E_KAZE_OVERFLOW` | `0` |
| `kaze_int_t kaze_mul(kaze_int_t a, kaze_int_t b)` | `E_KAZE_OVERFLOW` | `0` |
| `kaze_int_t kaze_div(kaze_int_t a, kaze_int_t b)` | `E_KAZE_DIV0`, or `E_KAZE_OVERFLOW` for `INT_MIN / -1` | `0` |
| `kaze_int_t kaze_mod(kaze_int_t a, kaze_int_t b)` | `E_KAZE_DIV0`, or `E_KAZE_OVERFLOW` for `INT_MIN % -1` | `0` |
| `kaze_int_t kaze_neg(kaze_int_t a)` | `E_KAZE_OVERFLOW` | `0` |
| `kaze_float_t kaze_float_div(kaze_float_t a, kaze_float_t b)` | `E_KAZE_DIV0` for a zero divisor, `E_KAZE_MATH` for a non-finite result | `0.0` |

Implementation notes that matter:

* With clang, or GCC 5 and newer, the checked helpers use
  `__builtin_add_overflow` / `__builtin_sub_overflow` / `__builtin_mul_overflow`.
  MSVC (through VS2022 17.x) has none of them, so both it and any older compiler
  take the CERT C integer-range recipes, which never evaluate an overflowing
  expression.
* Finiteness is tested arithmetically (`x == x` and `x - x == 0`) instead of
  with `isfinite`, because MSVC lacked the C99 macro for years and the
  arithmetic form is exactly what IEEE-754 guarantees. No libm is involved.
* `INT_MIN % -1` is caught even though the mathematical result is `0`: the
  C divide instruction traps on some targets, so it is treated as overflow
  rather than left to the hardware.

## 7. Value representations

### Strings

| Type / function | Contract |
| :--- | :--- |
| `typedef struct kaze_str_t { const uint8_t *ptr; size_t len; } kaze_str_t` | Immutable byte slice. `len == 0` means `ptr == NULL` (the empty string is the null slice). |
| `kaze_str_t str_from_cstr(kaze_arena_t *arena, const char *cstr)` | Copies the bytes into the arena; a `NULL` argument yields the empty string. |
| `kaze_str_t str_from_bytes(kaze_arena_t *arena, const void *bytes, size_t len)` | Copies `len` bytes; `NULL` or `len == 0` yields the empty string. |
| `kaze_str_t str_concat(kaze_arena_t *arena, kaze_str_t head, kaze_str_t tail)` | Copies both slices into one new arena block. A `size_t` overflow traps `E_KAZE_OOM`. |
| `bool str_eq(kaze_str_t a, kaze_str_t b)` | Length then byte comparison; two empty strings are equal. |
| `const char *str_cstr(kaze_arena_t *arena, kaze_str_t s)` | NUL-terminated arena copy for diagnostics; returns `""` for the empty string and on OOM. |

The empty string allocating nothing is deliberate: a missing map value and an
empty text are the same size on the wire, so a lowered program never pays an
allocation for `""`.

### Vectors

| Type / function | Contract |
| :--- | :--- |
| `typedef struct kaze_vec_t { void *data; size_t len; size_t cap; size_t elem_size; } kaze_vec_t` | Contiguous array header in arena memory |
| `kaze_vec_t *vec_new(kaze_arena_t *arena, size_t elem_size, size_t cap_hint)` | Allocates the header and, when `cap_hint > 0`, the initial block. Returns `NULL` after an OOM trap. `elem_size == 0` is coerced to 1. |
| `bool vec_push(kaze_arena_t *arena, kaze_vec_t *v, const void *elem)` | Copies one element; grows to 4 then doubles. `false` on OOM. |
| `void *vec_get(const kaze_vec_t *v, size_t index)` | Pointer to the element, or `NULL` when out of range (no trap: an out-of-range index is a Phase 2 checker error, not a runtime crash). |
| `size_t vec_len(const kaze_vec_t *v)` | Element count; `0` for `NULL`. |
| `size_t vec_cap(const kaze_vec_t *v)` | Capacity in elements; `0` for `NULL`. |
| `vec_new_vals(arena, cap_hint)` | Macro for `vec_new(arena, sizeof(kaze_val_t), cap_hint)` |

Growth copies into a fresh arena block and abandons the old one, which is the
accepted cost of growing without a free list: total waste is bounded by the sum
of the old capacities (under 2× the final size). A run that knows its element
count should pass it as `cap_hint` and never grow.

### Maps

Open addressing with linear probing, power-of-two capacity, cached FNV-1a hash,
tombstones for deletion, growth at 70% occupancy.

| Type / function | Contract |
| :--- | :--- |
| `typedef struct kaze_map_entry_t { kaze_str_t key; kaze_val_t val; uint32_t hash; kaze_slot_t slot; } kaze_map_entry_t` | One slot. `slot` is `KAZE_SLOT_EMPTY` (stop probing), `KAZE_SLOT_USED` or `KAZE_SLOT_TOMBSTONE` (keep probing). |
| `typedef struct kaze_map_t { kaze_map_entry_t *entries; size_t cap; size_t len; size_t tombs; } kaze_map_t` | Table header. `cap` is a power of two, at least `KAZE_MAP_MIN_CAP` (8). |
| `kaze_map_t *map_new(kaze_arena_t *arena)` | Header plus the initial 8-slot table; `NULL` on OOM. |
| `bool map_get(const kaze_map_t *m, kaze_str_t key, kaze_val_t *out)` | `true` and the value when the key is present; `false` when absent (never traps). `out` may be `NULL`. |
| `bool map_set(kaze_arena_t *arena, kaze_map_t *m, kaze_str_t key, kaze_val_t val)` | Inserts or replaces. The key bytes are copied into the arena, so a literal or stack buffer is safe. `false` on OOM, with no half-inserted entry. |
| `bool map_del(kaze_map_t *m, kaze_str_t key)` | Removes the key (leaves a tombstone) and returns `true`; `false` when absent. |
| `size_t map_len(const kaze_map_t *m)` | Live key count. |
| `bool map_contains(const kaze_map_t *m, kaze_str_t key)` | `map_get` without the value. |

Details worth knowing:

* Growth is triggered by `(len + tombs + 1) * 10 >= cap * 7` and rehashes into a
  fresh arena block; the previous entry array is abandoned like a grown vector's
  block.
* The probe run is bounded by `cap`. A table that is somehow full returns
  `SIZE_MAX` to `map_set`, which grows instead of looping forever — defensive,
  because occupancy keeps at least 30% of the slots empty by construction.
* Tombstones are reused by later inserts (the tombstone count is decremented),
  so a delete/insert cycle does not grow the table forever.

### Tagged values

`kaze_val_t` mirrors `Value` in [value.rs](../src/value.rs):

| `kaze_kind_t` | `kaze_val_kind_name` | Payload member | Rust `Value` variant |
| :--- | :--- | :--- | :--- |
| `KAZE_KIND_UNIT` | `nothing` | none | `Value::Unit` |
| `KAZE_KIND_BOOL` | `true/false` | `as.b` | `Value::Bool` |
| `KAZE_KIND_NUMBER` | `number` | `as.num` | `Value::Num` |
| `KAZE_KIND_TEXT` | `text` | `as.text` | `Value::Str` |
| `KAZE_KIND_LIST` | `list` | `as.list` | `Value::Arr` |
| `KAZE_KIND_MAP` | `record` | `as.map` | `Value::Map` |
| `KAZE_KIND_POINT` | `point` | `as.point` | `Value::Vec` |

| Function | Contract |
| :--- | :--- |
| `kaze_val_t kaze_val_unit(void)` | The no-value constant |
| `kaze_val_t kaze_val_bool(bool b)` | Boolean value |
| `kaze_val_t kaze_val_number(kaze_float_t n)` | Numeric value |
| `kaze_val_t kaze_val_text(kaze_str_t s)` | Text value (slice must live in the same arena) |
| `kaze_val_t kaze_val_list(kaze_vec_t *v)` | List value |
| `kaze_val_t kaze_val_map(kaze_map_t *m)` | Map value |
| `kaze_val_t kaze_val_point(kaze_float_t x, kaze_float_t y, kaze_float_t z)` | Inline point/offset |
| `bool kaze_val_truthy(kaze_val_t v)` | unit false, bool as stored, number `!= 0`, text `len > 0`, list/map/point always true |
| `const char *kaze_val_kind_name(kaze_val_t v)` | The words the Rust VM prints in a type error |

`kaze_val_t` is 32 bytes on a 64-bit host and 16 on a 32-bit host with the
narrow float. That is larger than Rust's 16-byte `Value` on purpose: the point
is inline (three floats, no indirection) and the text slice is a real pointer
plus length instead of an arena offset pair. A later phase can move the point
into a side table if a memory profile asks for the smaller slot.

## 8. Monotonic clock

| Function | Contract |
| :--- | :--- |
| `uint64_t kaze_time_ms(void)` | Milliseconds from a monotonic source: `CLOCK_MONOTONIC` when available, `clock()` otherwise. Only differences are meaningful; returns `0` if no clock can be read. |

`kaze_rt.c` defines `_POSIX_C_SOURCE 200809L` before its includes so glibc
declares `clock_gettime` under `-std=c99`. On glibc older than 2.17 that symbol
lives in `librt`, so such a host may additionally need `-lrt`. The `clock()`
fallback measures processor time rather than wall time; it is monotonic, which
is what the watchdog needs, but it under-reports while a controller is blocked
on I/O.

## 9. Heartbeat watchdog

| Type / function | Contract |
| :--- | :--- |
| `KAZE_HEARTBEAT_TIMEOUT_MS` | `1500`, the vehicle-safety cadence from the roadmap |
| `typedef struct kaze_watchdog_t { uint64_t last_packet_ms; uint64_t timeout_ms; } kaze_watchdog_t` | One window. `timeout_ms == 0` means the 1500 ms default. |
| `void kaze_watchdog_init(kaze_watchdog_t *w, uint64_t timeout_ms)` | Starts the window at the current time; `0` selects the default |
| `void kaze_watchdog_packet(kaze_watchdog_t *w)` | Records a heartbeat at the current time |
| `bool kaze_watchdog_stale(const kaze_watchdog_t *w)` | `true` when `now - last_packet_ms >= timeout_ms` |
| `kaze_code_t kaze_watchdog_abort_if_stale(const kaze_watchdog_t *w)` | `KAZE_OK` while fresh; otherwise traps `E_SAFETY_VIOLATION` with an age/limit message and returns that code |

Two fail-closed rules: a `NULL` watchdog is stale, and a clock that appears to
run backwards (`now < last_packet_ms`) is stale. Granting a fresh window to a
link whose timestamps moved backwards would be exactly the wrong direction for a
safety gate.

```c
kaze_watchdog_t wd;
kaze_watchdog_init(&wd, 0);            /* 1500 ms window */
/* ... on every accepted packet: */
kaze_watchdog_packet(&wd);
/* ... once per control tick: */
if (kaze_watchdog_abort_if_stale(&wd) != KAZE_OK) {
    /* land or hold: the link is gone */
}
```

## 10. Signal-time actuator de-assert

| Type / function | Contract |
| :--- | :--- |
| `KAZE_MAX_DEASSERTS` | `16` registration slots |
| `typedef void (*kaze_cleanup_fn)(void)` | A de-assert callback. Runs inside a signal handler, so it may only do async-signal-safe work: write a memory-mapped register or a pre-opened descriptor, set a `volatile sig_atomic_t`. No `malloc`, no stdio, no locks. |
| `bool kaze_register_deassert(kaze_cleanup_fn fn)` | Registers a callback; `false` for `NULL` or when all 16 slots are used (traps `E_KAZE_BADARG` / `E_KAZE_OOM`). Not thread-safe: register during startup. |
| `size_t kaze_deassert_count(void)` | Number of registered callbacks |
| `void kaze_run_cleanups(void)` | Runs every callback in reverse registration order, then returns. The signal path calls it and exits; a host that unwinds without a signal calls it directly. |
| `bool kaze_install_cleanup_signal_handlers(void)` | Installs one handler for SIGINT, SIGTERM and SIGSEGV; `false` if any install fails |

The handler does exactly two things: call the registered callbacks and
`_exit(128 + signal)`. It never returns, because returning from SIGSEGV would
resume the faulting instruction with actuators still armed. A `volatile
sig_atomic_t` guard sends a callback that faults straight to `_exit` instead of
looping on a broken stack. SIGKILL cannot be handled; the watchdog (or a
hardware one) is the only backstop there.

`_exit` is declared by hand in `kaze_rt.c` rather than by including
`<unistd.h>`, for the same reason `malloc` is: the include allowlist is
deliberate. `KAZE_NO_SIGNALS` removes this whole section, including the
`<signal.h>` include.

## 11. Escape hatches

| Macro | Effect | When to use it |
| :--- | :--- | :--- |
| `KAZE_USE_DOUBLE` | Forces `kaze_float_t` to `double` on a 32-bit target | Geodesy or control work that needs the extra mantissa on an MCU. Costs a software double on cores without an FPU. |
| `KAZE_NO_SIGNALS` | Removes `kaze_install_cleanup_signal_handlers`, `kaze_register_deassert`, `kaze_deassert_count`, `kaze_run_cleanups` and the `<signal.h>` include | Freestanding and embedded builds with no POSIX signal model |
| `KAZE_NO_HEAP` | Removes `kaze_arena_init_heap` and `kaze_arena_free` (and the `malloc`/`free` declarations) | A build that must not link a heap at all; use `kaze_arena_init` with a static buffer |

```bash
# 32-bit target with double-precision floats
clang -m32 -DKAZE_USE_DOUBLE -std=c99 -Wall -Wextra -Werror -pedantic -c kaze_rt.c

# Bare-metal style build: no signals, no heap
clang -DKAZE_NO_SIGNALS -DKAZE_NO_HEAP -std=c99 -Wall -Wextra -Werror -pedantic -c kaze_rt.c
```

The gate builds both of those, plus the freestanding i386 parse, so neither
hatch can rot unnoticed.

## 12. Reference test inventory

The behavioural program (131 checks) covers:

* **Types** — pointer-width selection, fixed widths, payload capacity, measured
  sizes printed on one line.
* **Arena** — misaligned caller buffer, 8-byte alignment with odd sizes,
  non-overlap, cursor accounting, zero-size request, OOM (`NULL`, sticky flag,
  `E_KAZE_OOM`, cursor untouched, smaller request afterwards), `require`
  pre-flight, reset semantics (offset 0, capacity retained, flag cleared), heap
  init/free.
* **Arithmetic** — every boundary for add/sub/mul/div/mod/neg plus float
  division, the exact trap code for each trapping wrapper, `E_KAZE_MATH` for an
  overflowed float result.
* **Strings** — copy, equality, empty slice, concatenation of equal and unequal
  lengths, NUL-terminated diagnostics.
* **Vectors** — growth to 100 elements, capacity, every element in order,
  out-of-range `NULL`, `kaze_val_t` round-trips including text and point.
* **Maps** — 200 keys across rehashes, all values returned, absent key without a
  trap, delete half then find the rest behind tombstones, reinsert over
  tombstones, key bytes copied from a stack buffer, empty key.
* **Clock and watchdog** — monotonic advance, 1499 ms fresh versus exactly
  1500 ms stale, `E_SAFETY_VIOLATION` from `abort_if_stale`, refresh on packet,
  and a real 1600 ms gap.
* **Signals** — `NULL` rejection, registration, newest-first order, fill to 16,
  17th refused, handler installation.

The sanitizer program runs 1,000,000 `kmalloc` + `kaze_arena_reset` cycles with
varying (odd) sizes, checks alignment every cycle, builds a 32-key map and a
32-element vector every 10,000 cycles, and asserts the arena heap bytes return
to their baseline after `kaze_arena_free`.

## 13. What this host actually verified

Reference host: macOS 15.7.3 arm64, Apple clang 17.0.0. Last gate output:

```text
rt-check: PASS - 15 passed, 2 skipped, 0 failed
rt-check: 32-bit: no 32-bit binary executed on this host; 32-bit object codegen passed; freestanding i386 syntax passed
rt-check: leak detection: LeakSanitizer unsupported on this host; the arena's heap bytes were counted instead (see the asan heap line above)
```

Honest accounting of the gaps, because a gate that overstates itself is worse
than no gate:

* **No 32-bit binary ran on this host.** `clang -m32` on arm64 macOS targets
  armv4t and the linker refuses it (`ld: unknown -arch name: armv4t`); the
  script prints that reason verbatim and skips the run. What *was* verified:
  `kaze_rt.c` compiles to a 32-bit Mach-O object, a compile-time probe asserts
  the 32-bit type block (4-byte pointer/int/float, and 8-byte float under
  `KAZE_USE_DOUBLE`), and the same sources parse for `i386-none-elf` in
  freestanding mode against stub libc headers. A 32-bit *behavioural* run
  remains unproven here.
* **GNU gcc was not exercised.** On macOS `gcc` is Apple clang. The script
  notes this and labels the gate `gcc-alias(clang)`; a real GNU gcc log will
  appear on a Linux host.
* **LeakSanitizer is unavailable on this host.** Asking for
  `ASAN_OPTIONS=detect_leaks=1` aborts with "detect_leaks is not supported on
  this platform". The script reports that as a SKIP and proves the same property
  directly: with the sanitizer allocator interface, the resident heap bytes
  before `kaze_arena_init_heap` and after `kaze_arena_free` are identical
  (the exact figure varies with the sanitizer runtime; the gate compares
  equality and prints both numbers), and any growth fails the gate. On a host
  with LSan the leak option is used instead.
* **No embedded hardware was involved.** Everything above ran on a hosted
  64-bit machine; the freestanding checks are parse-level only.
`