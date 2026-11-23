# Phase 1-6 landing report (2026-11-22)

This is the evidence bundle for roadmap phases 1 through 6 of
[00-DETAILED-MASTER-ROADMAP.md](kaze-plan/00-DETAILED-MASTER-ROADMAP.md). Every row
names the artifact, the gate that proves it, and what was actually observed on
this machine. Where a gate could not run here, the row says so.

Language version stays **1.0**; the module format moved **4 to 5** because Phase 2
added the per-item recursion ceiling the VM enforces.

## Phase 1 - native systems foundation and the C99 runtime substrate

| Artifact | What it is |
| :--- | :--- |
| `kaze/dist/kaze_rt.h` | Portable C99 runtime: cross-architecture types (`kaze_int_t`, `kaze_uint_t`, `kaze_float_t`, `kaze_size_t`, fixed-width aliases), `kaze_str_t`, `kaze_vec_t`, `kaze_map_t`, the `kaze_val_t` union, trap codes, watchdog and cleanup declarations |
| `kaze/dist/kaze_rt.c` | Linear bump arena (`O(1)` allocate, 8-byte alignment, bulk reset, sticky `E_KAZE_OOM`), checked add/sub/mul/div/mod/neg with a registered fallback handler (`E_KAZE_OVERFLOW`, `E_KAZE_DIV0`, `E_KAZE_MATH`), linear-probing map, monotonic `kaze_time_ms`, 1500 ms heartbeat watchdog, async-signal-safe actuator de-assert on SIGINT/SIGTERM/SIGSEGV |
| `kaze/scripts/rt-check.sh` | The gate: strict compile, behavioural suite, sanitizer run |
| `kaze/tests/rt_runtime.rs` | Runs the gate from `cargo test` (skips cleanly with no C compiler) |
| `kaze/docs/KAZE-RT.md` | API reference, trap codes, 32/64-bit story, escape hatches |

Observed: `rt-check: PASS - 15 passed, 2 skipped, 0 failed`.

- Zero warnings under `clang -std=c99 -Wall -Wextra -Werror -pedantic`, 131 behavioural checks.
- ASan + UBSan clean over 1,000,000 arena allocate+reset cycles and 100 container builds.
- LeakSanitizer is unsupported on this platform; the gate substitutes an allocator probe (heap bytes identical before init and after free) and reports `SKIP` for LSan itself.
- 32-bit: this host cannot link a 32-bit executable (`ld: unknown -arch name: armv4t`). Verified instead: `-m32 -c` produces a 32-bit object, a compile-time probe asserts 4-byte pointer/int/float, `KAZE_USE_DOUBLE` builds at `-m32`, and both files parse for `i386-none-elf -ffreestanding`. A 32-bit *behavioural* run is still unproven here.
- GNU gcc is Apple clang on this machine; the gate labels it `gcc-alias(clang)` and prints a note.
- **Scope of the integration.** Phase 1 authors and gates the substrate; the Rust C
  emitter still emits its own inline `kz_*` runtime. Wiring the emitter onto
  `kaze_rt.h` is Phase 7's job (the roadmap has the pure-Kaze generator insert
  `kaze_arena_alloc` and the checked-arithmetic macros), so nothing here claims
  that emitted code already links against `kaze_rt.c`.

## Phase 2 - ultra-strict semantics and zero-panic invariants

| Rule | Code | Where |
| :--- | :--- | :--- |
| Fallible expression with no proovable safety and no inline fallback | `E_KAZE_MISSING_FALLBACK` | `kaze/src/strict.rs` |
| Constant index outside a known length | `E_KAZE_BOUNDS` | `kaze/src/strict.rs` |
| `while` with no iteration ceiling | `E_KAZE_LOOP_UNBOUNDED` | `kaze/src/strict.rs` |
| Routine on a call cycle with no `recursion bounded` | `E_KAZE_RECURSION_UNBOUNDED` | `kaze/src/strict.rs` |
| A call that breaks its callee's precondition at compile time | `E_KAZE_CONTRACT` | `kaze/src/strict.rs` |
| Declared ceiling exceeded at run time, or the 256-frame bound | `E_KAZE_RECURSION_DEPTH` | `kaze/src/vm.rs` (format 5) |
| Declared loop ceiling exceeded at run time | `E_KAZE_LOOP_BOUND` | `kaze/src/compiler.rs` (trap) |
| A `guard` that is false | `E_KAZE_GUARD` | `kaze/src/compiler.rs` (trap) |

Every one of those renders as a block with the path, line, **column**, the source
line, a caret under the construct and a suggestion (`kaze/src/diag.rs`), for
example:

```
error[E_KAZE_UNIT_MISMATCH]: cannot add length (meter) and time (second)
  --> flight.kaze:22:15
   |
22 |     let target = altitude + current_speed
   |                             ^ here: length (meter) and time (second)
```

The column is located by searching the offending line for the construct (the
operator, the bracket, the name), so the AST does not need to carry columns;
`kaze/tests/diagnostics.rs` pins the shape for nine codes. The language server
publishes the same problems as structured diagnostics with a real range
(`kaze/src/lib.rs::diagnose_in`), not as a rendered string.

Proofs the checker accepts instead of demanding a fallback: a non-zero literal
divisor; a constant index inside a list/text/point literal or a value with a
promised length; an index guarded by `while i < length of xs`, `if i < length of
xs`, `guard`, or a `requires` clause; an index of the form `length of xs - k`
under `guard k < length of xs`; and a length a callee promises with
`ensures length of result == 2`.

Observed: `kaze/tests/strict_semantics.rs` -> 13 tests over **72 fixtures**
(56 rejected with an exact code, 16 positive controls that compile and run), plus
an orphan check that every fixture is classified, a determinism check (same code
and message twice) and a panic-freedom check over the whole corpus. `tests/fuzz.rs`
still feeds 2,500 mutated sources and 2,500 mutated artifacts through the engine.

## Phase 3 - compile-time dimensional physics algebra

- `Dim` carries seven exponents (length, time, mass, angle, temperature,
  pressure, current) and `Unit` names 22 spellings; every literal is normalised
  into its dimension's base unit at parse time, so no unit ever reaches an
  instruction.
- Arithmetic: add/subtract require the same dimension, multiply adds exponents,
  divide subtracts them, comparisons require the same dimension; a mismatch is
  `E_KAZE_UNIT_MISMATCH` with both quantities named.
- Base units: meter, second, kilogram, **degree** (so a compass bearing and a
  `degrees` literal agree), kelvin (`Celsius` carries the 273.15 offset),
  pascal, ampere. Percent stays a whole number.
- Trig: `sin`/`cos` of an angle-dimension literal gets the degrees-to-radians
  conversion folded in at compile time.

Observed: `kaze/tests/units.rs` -> 7 tests, including `1 km + 500 meters == 1500`,
`2 m/s * 3 seconds == 6`, `20 Celsius == 293.15`, `sin(90 degrees) == 1`, and a
proof that the same program written with units compiles to the **same number of
instructions** as its plain-number twin.

## Phase 4 - routine contracts and untrusted boundaries

- Signature: `action|routine|check|filter name(a: int, b: percent = 20) -> bool:`.
- Body-top declarations: `requires <expr>`, `ensures <expr>`, `recursion bounded <n>`, `needs: <families>`.
- `requires` becomes an entry assertion and `ensures` an exit assertion over
  `result`, both trapping with `E_KAZE_CONTRACT`; a constant argument that
  violates a precondition is refused at check time instead.
- `untrusted<T>` parameters must be validated by a `requires` clause
  (`E_KAZE_UNTRUSTED` otherwise).
- 12-factor resolution: `--arg` > `AGENT_<PARAM>`/`SOFUU_<PARAM>` > `--config`
  document (default `./agent.config.json`) > signature default. A typed
  parameter with no value anywhere is `E_KAZE_PARAM` naming all three fixes; a
  value of the wrong type is refused with the type named.

Observed: `kaze/tests/contracts.rs` -> 11 tests covering the compile-time refusal,
the runtime trap, `ensures` seeing `result`, untrusted validation, the missing
parameter diagnostic, every rung of the hierarchy, wrong-typed values and `guard`.

## Phase 5 - capability gates, security policy and effector isolation

See [CAPABILITY-SECURITY.md](CAPABILITY-SECURITY.md) for the current contract.
What landed:

| Item | Implementation | Evidence |
| :--- | :--- | :--- |
| Zero-ambient authority | Routing is explicit in `src/host.rs`: only `log`/`policy` reach the in-process implementation, `drone`/`device`/`alert` have named backends, `rover` needs `rover=sim`, and `sensor`/`memory`/`ai`/`tool` are refused by name. No family can fall through to a silent success | `effector_safety.rs::every_registered_operation_crosses_every_gate_stage` (40 registry pairs x H-1/H-2/policy/missing-policy), `every_registered_operation_acts_or_fails_closed_with_no_backend` |
| Fail-closed arbiter + audit | Every gate decision goes through one audited wrapper; an unwritable audit sink denies the call (`H-5/audit`) | `the_audit_trail_is_off_by_default_and_records_one_line_per_decision`, `an_audit_line_carries_no_credential_and_a_broken_sink_fails_closed` |
| Real alert sinks | `alert=stdout`/`stderr`/`file:<path>`, one RFC3339 line per alert, 1024-byte ceiling truncated on a character boundary, credential redaction (headers, `secret`/`token`/`api_key` shapes and `sk-`/`ghp_`/`AKIA`/`AIza`/`glpat-` token prefixes) applied to the message **and** to the traced argument | `an_alert_sink_writes_a_timestamped_line_to_stdout_stderr_and_a_file`, `an_alert_redacts_credentials_and_stays_under_the_byte_ceiling`, 6 unit tests |
| Monotonic attenuation | Language-level (routine `needs:`, checker) with the runtime gate as a second lock; this crate has no sub-run path, which the doc states plainly | `a_routine_that_needs_drone_cannot_run_with_only_a_log_grant`, `a_module_whose_declaration_was_stripped_is_refused_at_runtime` |
| 1500 ms effector watchdog | `SAFETY_FRESHNESS_MS` = 1500 in `src/link.rs`; per-field reception stamps; a dependency table decides which fields each command needs; a stale or never-heard heartbeat/field refuses with uncatchable `E_SAFETY_VIOLATION`; live actuation re-enabled. Also fixed a real bug: the link discarded its first datagram (usually the heartbeat) | `live.rs`: arm over a real UDP socket, never-sends-heartbeat refused, stale-field partial target refused, 1.75 s silence refused with nothing sent, lossy stream still completes a mission; plus 3 unit tests |
| Cleanup on abnormal termination | `src/gpio.rs` cleanup table registering pre-opened descriptors before an output goes high; SIGINT/SIGTERM de-assert with async-signal-safe writes then re-raise; normal exit de-asserts GPIO and disables PWM channels | `sigint_de_asserts_a_gpio_line`, `sigterm_de_asserts_gpio_and_pwm_outputs` (real binary, fake sysfs, signal death observed), 6 unit tests |

Behaviour deliberately changed: live actuation enabled (the old test that
asserted it was disabled now asserts a silent vehicle is refused for the real
reason), normal exit de-asserts outputs, live telemetry older than 1500 ms is not
returned, alert trace arguments are redacted, and a broken audit sink denies the
capability. Verified against scripted backends only: no real autopilot, SITL or
GPIO/PWM hardware was exercised here, and the Rust cleanup covers SIGINT/SIGTERM
(the C substrate also covers SIGSEGV).

## Phase 6 - pure-Kaze self-hosted compiler frontend

See [SELF-HOSTING.md](SELF-HOSTING.md). What landed:

| Artifact | What it is |
| :--- | :--- |
| `kaze/compiler/checker.kaze` | The pure-Kaze semantic checker: undeclared capability family, unknown operation, unknown name, filter fall-through, division/modulo without a fallback (and without a literal non-zero divisor), dynamic index without a fallback, `while` without a ceiling, recursion cycles without a declared ceiling, and non-boolean `requires`/`ensures` |
| `kaze/compiler/machine.kaze` | The dual-syntax engine in pure Kaze: canonical machine S-expression emission and parsing |
| `kaze/compiler/{token,ast,lexer,parser,codegen,main}.kaze` | Extended to the strict surface: inline `otherwise`, `bounded` loops, `guard`, contract and routine-`needs:` declarations, typed parameters with defaults and a result type, records with instantiation, the widened unit lexicon |
| `kaze/scripts/self-sync.sh` | The monolith is generated from the eight modules and `--check` proves it is current |
| `kaze/tests/frontend.rs` | Drives the pure-Kaze frontend through the Rust engine: the monolith matches its parts, the checker accepts the corpus and refuses flawed programs, and the machine round trip is check-preserving over **100+ sources** (the ~35 files in the tree plus generated per-construct samples) |

Observed on this machine, independently of the test that wraps it:

```
$ ./kaze emit-c compiler/kaze_self.kaze -o /tmp/s1.c     # Rust Stage 0 -> 308,093 bytes of C
$ cc -std=c99 -O2 /tmp/s1.c -o /tmp/s1
$ /tmp/s1 compiler/kaze_self.kaze > /tmp/s2.c            # Stage 1 compiles itself
$ cc -std=c99 -O2 /tmp/s2.c -o /tmp/s2
$ /tmp/s2 compiler/kaze_self.kaze > /tmp/s3.c            # Stage 2 compiles itself
$ diff /tmp/s2.c /tmp/s3.c                               # 0 lines, 299,600 bytes each
```

The fixed point holds. The pure-Kaze frontend also checks the whole corpus:

```
$ for f in examples lib compiler; do /tmp/s1 check <file>; done   # 0 failures over every .kaze file
$ /tmp/s1 check /tmp/flaw_div.kaze    -> E_KAZE_MISSING_FALLBACK at line 3
$ /tmp/s1 check /tmp/flaw_loop.kaze   -> E_KAZE_LOOP_UNBOUNDED at line 2
$ /tmp/s1 check /tmp/flaw_cap.kaze    -> E_CAP_UNDECLARED at line 3
$ /tmp/s1 check examples/hello.kaze   -> check: ok
```

What it does not do: full type inference, dimensional algebra, or the contract
checks — those stay in the Rust checker, and `SELF-HOSTING.md` says so. The
pure-Kaze C generator still covers the pre-strict subset, which is Phase 7's job.

**Memory, measured.** The first tokenizer appended every token to one list,
which is quadratic when concatenation copies; on the monolith that reached
**7.5 GB and was killed**, and a background corpus sweep drove the host into tens
of gigabytes of swap. The lexer now accumulates in 256-token chunks and flattens
once, and the recursion check builds the call graph once per module as index
lists. Peak RSS is now **171 MB** for `emit-c` and for Stage 1 compiling the
monolith, **97 MB** for Stage 2, and **103 MB** to check a 38 KB module. Checking
the combined 165 KB monolith still costs ~4.8 GB: the pure-Kaze checker walks a
large file with a C runtime that never frees an intermediate list, which is the
same defect Phase 7 fixes by rebuilding the generator on `kaze_rt.h` with an
arena. The memory table is in `SELF-HOSTING.md`.

## Known issues outside this round's scope

- `crates/sofuu-core/tests/` is **gitignored** (`.gitignore` line 165), so those
  embedded-engine integration tests are not part of the committed contract. On
  this tree they fail before reaching any Kaze behavior: the product host now
  requires an explicit grant and a policy for every family, and those tests supply
  neither for the `log` calls their examples make (`E_CAP_MISSING` / then
  `E_CAP_DENIED: no policy is configured for log.info`). They need
  `grants: [.., "log"]` and a permissive `policy`; the file's own helper at line 81
  shows the shape. No phase-1-to-6 change touches them, and the tracked
  cross-engine gate (`scripts/kaze-drift-check.sh --strict`) is green.
- The embedded Sofuu engine still reads formats 1, 2 and 4; it has not been
  extended to format 5 or to the strict surface. That is a separate alignment
  decision, recorded in `kaze/docs/VERSION-ALIGNMENT.md`.

## Gates run for this report

```bash
cargo test --manifest-path kaze/Cargo.toml          # the whole standalone engine
bash scripts/kaze-drift-check.sh --strict           # two engines, zero drift
cargo run --bin kaze -- selftest                    # 11/11 scorecard
bash kaze/scripts/rt-check.sh                       # the C runtime substrate
bash kaze/scripts/self-sync.sh --check               # the monolith matches its parts
cargo run --bin kaze -- emit-c kaze/compiler/kaze_self.kaze -o /tmp/s1.c
cc -std=c99 -O2 /tmp/s1.c -o /tmp/s1 && /tmp/s1 kaze/compiler/kaze_self.kaze > /tmp/s2.c
cc -std=c99 -O2 /tmp/s2.c -o /tmp/s2 && /tmp/s2 kaze/compiler/kaze_self.kaze > /tmp/s3.c
diff /tmp/s2.c /tmp/s3.c                            # must be empty
```

## Phase 1–6 requirement audit (2026-11-22, round 13)

Every component of the roadmap's Phase 1–6 breakdown was checked against the
code, and every acceptance gate was run or explicitly marked as unverifiable on
this host.

### Phase 1 — native substrate

| Requirement | Evidence |
| :--- | :--- |
| `kaze_arena_t`, O(1) bump, bulk reset, 8-byte alignment, `E_KAZE_OOM` | `dist/kaze_rt.h`; alignment asserted per allocation in `dist/arena_stress.c` |
| 1,000,000 alloc + bulk-reset cycles clean under a sanitizer | **new gate**: `arena_stress.c` run under `-fsanitize=address,undefined` by `tests/rt_runtime.rs` |
| Cross-architecture typedefs | `kaze_int_t`, `kaze_uint_t`, `kaze_float_t` in `dist/kaze_rt.h` |
| Checked arithmetic with intrinsics + portable fallback | `kaze_add_checked` uses `__builtin_add_overflow` under `KAZE_HAVE_BUILTIN_OVERFLOW`, `kaze_add_overflows` otherwise |
| `kaze_str_t`, `kaze_vec_t`, `kaze_map_t` | all three in `dist/kaze_rt.h` (string is ptr+len after round 11) |
| Watchdog hooks, signal cleanup | `kaze_time_ms`, `kaze_register_deassert`, SIGINT/SIGTERM/SIGSEGV handler |
| Strict compile: `-Wall -Wextra -Werror -pedantic -std=c99` | clang: clean. `-m32` and `-m64`: clean. **gcc: not installed on this host** — unverified here |

### Phase 2 — strict semantics

All five components have a rule, a code and a test: `otherwise` on every
fallible operator (`tests/strict_semantics.rs`, 72 fixtures), index proofs,
bounded loops, recursion ceilings with `E_KAZE_RECURSION_DEPTH`, and structured
caret diagnostics (`tests/diagnostics.rs`). The "50+ intentionally flawed
programs" gate is met by 72 fixtures plus 15 effector-safety cases.

### Phase 3 — dimensional algebra

Seven-dimension vectors; **all 20 unit spellings** from the roadmap are present
(plus aliases); addition/subtraction require equal dimensions, multiplication and
division add/subtract vectors; constants fold (`1.5 km` emits `kz_num(1500.0)`) so
the generated C contains **zero** runtime conversion code; mismatches render with
two labelled carets (round 9).

### Phase 4 — contracts

`requires`/`ensures`/recursion ceilings, compile-time checking of constant
arguments, entry assertions that are **elided when every call site is proved**
(round 3), `untrusted<T>` barriers, and the four-level 12-factor resolution
(`--arg` > `AGENT_*` > `--config` > signature default) with `E_KAZE_PARAM`
guidance.

### Phase 5 — capabilities

`needs:` per routine (`E_CAP_UNDECLARED`), fail-closed runtime (`E_CAP_MISSING`),
monotonic attenuation, alert sinks with a 1024-record ceiling and `<redacted>`
substitution for credential-bearing prompt fields, and the 1500 ms telemetry
watchdog with `kaze_register_deassert` cleanup.

### Phase 6 — pure-Kaze frontend

Indentation stack, dual syntax with auto-detection, AST records, precedence
climbing, the checker (scope, strictness, unit algebra, contracts), and lossless
`to-machine`/`to-human` (100+ files round-tripped in `tests/frontend.rs`).
**Gap closed this round**: tokens carried line and column but not the byte offset
the roadmap asks for; `Token` now carries `at`, set at every construction site.

### Emitted C under strict warnings (added in round 14)

The generated C for the compiler's own 430 KB monolith now compiles **warning-free**
under `clang -std=c99 -O2 -Wall -Wextra -Werror -pedantic`. Turning the flags on
found a genuine bug: a routine with an `ensures` clause and its own local named
`result` initialised the return value from itself (undefined behaviour that
happened to work). The value is now bound to `_kz_ret_value` first. The gate is a
test (`emitted_c_compiles_with_zero_warnings_under_strict_flags`), so it cannot
regress silently.

### Engine parity (added in round 15)

`scripts/diff-check.sh` runs every program both engines can execute and compares the
returned value and the sequence of logged values. It found the native backend
answering **0** for the spatial maths (`distance between`, `slant distance`,
`bearing from`, `bearing of`) and for point arithmetic (`point − point`,
`point × number`, `length of <point>`), because `Expr::Math`'s lowering ended in a
`kz_num(0.0)` catch-all and the runtime's arithmetic had no point cases. Both are
implemented now, following the interpreter's `geo` module exactly (WGS84 mean
sphere, radius 6 371 008.8 m), and `MathKind` is matched exhaustively so a future
builtin cannot silently become zero. Gate: 5 examples agree, 0 differ, 4 skipped
(hook-only programs with no entry point).

### Hooks in the native backend (added in round 16)

`when` and `every` items were **not compiled at all** by the C emitter — the item
loop handled only `action`/`check`/`filter`, so a hook body was dropped and the
binary exited 0 after printing "no main entry". They are now emitted as
`kz_fn_hook_when_<line>`/`kz_fn_hook_every_<line>` and driven by a generated
`kz_run_hooks(argc, argv)` loop that accepts the interpreter's `--ticks N` (and
`--ticks=N`, `KAZE_TICKS`, `AGENT_TICKS`). A `when` predicate that needs telemetry the
host cannot supply is false rather than assumed true, so hooks stay fail-closed.
Verified against the interpreter on a hardware-free program: both engines fire
`hook fired, tick, hook fired, tick` with `--ticks 2` and nothing with no ticks.

### Parameter typing in the compiled binary (added in rounds 18–19)

Phase 4.4 asks for four resolution layers and strongly typed parameters. The
emitted runtime had three layers and **no coercion**: `--arg duty=abc` for a
`percent` reached the routine as the text `"abc"`, and a missing value became `0`.
Both are fixed — the config layer was added (`--config`, `./agent.config.json`), a
missing *typed* parameter is `E_KAZE_PARAM` with guidance, and every declared type
is coerced with the interpreter's rules (numeric, boolean and text). Verified
across the matrix in `compat`/`emitted_c` tests: the two engines now agree on
valid values, on values spelled as text, and on junk.

### The input surface, end to end (added in rounds 18–22)

Every layer the roadmap names for Phase 4.4 now behaves identically in the
interpreter and in a compiled binary: `--arg` (and a `name=value` positional), the
`AGENT_*`/`SOFUU_*` environment, the config file (`--config` or
`./agent.config.json`), and a pipe. A missing *typed* parameter is
`E_KAZE_PARAM` with the exact ways to supply it in both; a value of the wrong type
is `E_KAZE_PARAM` in both; and a field read on something that is not a record is
`E_KAZE_TYPE` in both. Each was a divergence found by comparing the engines, and
each has a test.

### Differential fuzzing (added in round 23)

`scripts/diff-fuzz.py` generates programs that use only constructs both engines
define, and `scripts/diff-fuzz.sh` compares their output. The first run disagreed on
11 of 25 — all of them the last printed digit, because the native printer used a
fixed `%.16g` while the interpreter prints the shortest round-tripping form. The
printer now tries 15, 16 and 17 significant digits and keeps the first that parses
back to the same double, so the text matches too. 25/25, 60/60 and 40/40 agree
across seed ranges, and the gate is a test.

### The fuzz generator covers the language (rounds 24–27)

Twelve flavours cycled by seed — arithmetic, text, lists with `for each`, records,
points (distance, slant range, bearing, difference, scaling, magnitude),
conditionals, logging, a denied capability, hooks with ticks, `try`/`otherwise with
e:`, guards after a bounded loop, and nested bounded loops. The driver compares the
returned value, the logged lines and, for a program both engines refuse, the error
code. **120 generated programs agree, 0 differ, 0 unusable.** The defects of
rounds 15, 25 and 26 (spatial maths answering zero, the capability fail-open, FP
contraction) are all in this gate's coverage.

Earlier seven-flavour description, kept for the record: cycled by seed — arithmetic, text, lists with `for each`, records,
points (distance, slant range, bearing, difference, scaling, magnitude),
conditionals and logging — with the driver comparing both the returned value and
the logged lines. 70 of 70 agree across all flavours. Writing it confirmed three
language rules by refusing to compile: field access is one level deep, and an angle
can be neither added to a plain number nor compared with one without a unit.

### Capability calls in the compiled binary (round 25)

The emitted runtime's `kz_cap_call` was a placeholder that printed
`[cap:family.op]` and returned `nothing`, so a script needing a sensor the host never
granted **succeeded with fabricated data** and exited 0 — the fail-open `RULES.md`
§1.9 forbids. It now refuses with `E_CAP_MISSING`, naming the family and operation,
and exits 70, matching the interpreter. `log` and `alert` (implemented in the
runtime) are unaffected. The fuzz driver compares refusals as well as values, so a
program both engines refuse must be refused with the same code.

### The two C emitters, compared (round 29)

A gate that had never run: emit one program with the Rust emitter and with the
pure-Kaze emitter, build both, compare behaviour. It found four divergences in the
**pure-Kaze** emitter (wrong capability code, flat-Euclidean distances, missing
hooks, and a missing entry refusal). Fixed and verified: the capability code, the
haversine/slant/bearing formulas, `#pragma STDC FP_CONTRACT OFF` and the
shortest-round-trip number formatter. Point arithmetic initially had to be reverted, because that
emitter represented points as three-element numeric lists, which collide with its own
tuple-shaped data (the change broke the bootstrap: stage 2 trapped with
`E_KAZE_LOOP_BOUND`). Round 30 gave it a real `KZ_VAL_POINT` kind — enum, union,
constructor, component reader, point arithmetic, magnitude, tuple printing and field
access — and ported the geo helpers onto it. `scripts/emitter-triangle.sh` now
compares the two emitters' binaries directly; it now reports **7 agree, 0 differ, 1 skipped**.

Round 31 closed the last gap: `emit_hook` turns each `when`/`every` item into
`kz_hook_when_<line>`/`kz_hook_every_<line>` (the item is `[condition, body]` — the
first attempt read the wrong child and emitted `kz_nil()` as the predicate), and
`emit_hook_driver` emits `kz_ticks` (from `--ticks`/`KAZE_TICKS`) plus
`kz_run_hooks`, which `main` calls both after an entry value and on the no-entry
path. That emitter's capability stub now also `exit(70)`s after reporting
`E_CAP_MISSING` instead of returning a value.

### Lossless dual syntax, proven over the corpus (rounds 32–35)

Phase 6 requires `AST(H) ≡ AST(to_human(to_machine(H)))`. Running it over every
example and compiler module found **six** defects, all now fixed and covered by
`tests/frontend.rs::the_human_and_machine_forms_round_trip_losslessly`:

| defect | consequence |
| :--- | :--- |
| unit literals printed from their base-unit value | `1.5 km` came back as `1500 km` (1 500 000 m) |
| capability calls printed as `log.info(…)` | source the parser rejects |
| `for x in xs` without `each` | source the parser rejects |
| inline `otherwise` printed unparenthesised | `a and b otherwise c` changed meaning |
| `length of x - 1` printed unparenthesised | parsed as `length of (x - 1)` |
| `record` missing from the machine reader | records could be written, never read |

Corpus status: **20 stable, 0 unstable, 0 incomplete**.

### Gates added in rounds 36–37

- **CLI matrix**: every command (`check`, `fmt`, `to-machine`, `to-human`, `emit-c`,
  `compile`) against every example — 60 invocations, no crashes, and consistent exit
  codes (1 for any diagnostic; 0 from the parse-only tools on a parse-valid program;
  diagnostics on stderr, values on stdout).
- **Documented programs parse**: every ```kaze block in the product docs that
  declares a top-level item must compile. Fourteen did not — fragments presented as
  programs, one fence defining a routine twice, good/bad pairs sharing a block, and
  map-file snippets showing planned syntax. Now `scripts/doc-samples.py`, wired in as
  `every_documented_program_parses`.

### Verification surface (regenerated 2026-11-23)

Counts taken from the files, not from memory:

| File | Area | Tests |
| :--- | :--- | ---: |
| `tests/strict_semantics.rs` | strict semantics | 13 |
| `tests/units.rs` | units | 7 |
| `tests/contracts.rs` | contracts | 11 |
| `tests/diagnostics.rs` | diagnostics | 10 |
| `tests/emitted_c.rs` | emitted C | 29 |
| `tests/frontend.rs` | frontend | 8 |
| `tests/capability_safety.rs` | capability safety | 12 |
| `tests/effector_safety.rs` | effector safety | 15 |
| `tests/modules.rs` | modules | 31 |
| `tests/lsp.rs` | LSP | 12 |
| `tests/compat.rs` | compat | 5 |
| `tests/fixtures/strict/` | intentionally flawed programs | 72 |

Plus the gates that are scripts rather than unit tests: `scripts/diff-check.sh`
(interpreter vs compiled binary over the examples), `scripts/diff-fuzz.sh` (18-120
generated programs across 12 language flavours), `scripts/emitter-triangle.sh` (the
Rust emitter vs the pure-Kaze emitter), `scripts/doc-samples.py` (documented
programs), `scripts/self-sync.sh --check` (the monolith matches its parts),
`scripts/kaze-drift-check.sh --strict` (the embedded engine), and
`scripts/panic-audit.sh` (exactly the six documented panic sites).

### Remaining unverified on this host

- `gcc` (Apple clang stands in) — the strict-compile gate is proven for clang only.
- Real drone / flight-controller hardware, real GPIO/PWM, and 32-bit *runtime*
  behaviour (the 32-bit *compile* of the runtime passes).
- LeakSanitizer (unsupported on this host; ASan's leak check is included in the
  arena gate via `-fsanitize=address`).
