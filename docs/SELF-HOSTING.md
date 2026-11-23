# Self-hosting the Kaze front end (Phase 6)

This page describes the compiler that is written in Kaze itself, in
`kaze/compiler/`. It is the stage that makes the bootstrap real: the Rust
engine (`kaze/src`) compiles these files to C, the resulting native binary
compiles the same files again, and the two outputs are byte-identical.

## The files

| File | What it is |
| :--- | :--- |
| `compiler/token.kaze` | the token record (kind, text, line, column) |
| `compiler/ast.kaze` | one `AstNode` shape for every construct, plus the item/contract/parameter constructors, the capability-name vocabulary and the unit lexicon |
| `compiler/lexer.kaze` | the tokenizer: indentation (off-side rule), strings with escapes, comments, numbers, identifiers, two-character operators, and bracket tracking (indentation inside `[` `(` is layout, not structure) |
| `compiler/parser.kaze` | the recursive-descent parser: items, routine headers, contract lines, statements, the sentence verbs, and expression precedence climbing |
| `compiler/checker.kaze` | the semantic checker: twelve rules, reported as `Diagnostic` records |
| `compiler/machine.kaze` | the dual-syntax engine: emit the canonical machine S-expression form, read it back, and print human syntax |
| `compiler/codegen.kaze` | the native C99 emitter, including the phase 1 runtime header it carries |
| `compiler/main.kaze` | the entry point and the mode switch |
| `compiler/kaze_self.kaze` | **generated**: the eight modules above concatenated into the single file the bootstrap compiles. Never edit it by hand; run `bash scripts/self-sync.sh`. |

`bash scripts/self-sync.sh --check` proves the monolith is current. The frontend
test runs that check, so a stale monolith fails the suite.

## The surface the front end reads

Everything the Rust engine accepts today, in one pass, with recovery instead of
panics:

1. **Inline fallbacks** — `let v = a / b otherwise 0`, `xs[i] otherwise 0`,
   `reading.value otherwise 0`. Parsed into a `FALLBACK` node at the loosest
   precedence level, exactly like `kaze/src/parser.rs`; `otherwise` followed by
   `:` stays the else-block keyword.
2. **Bounded loops** — `while cond bounded 1000:`. The ceiling is recorded on the
   `WHILE` node as a `BOUND` child.
3. **Recursion ceilings** — `recursion bounded 200` as the first line of a
   routine body.
4. **Contracts** — `requires <expr>` and `ensures <expr>` lines at the top of a
   routine body.
5. **Routine-level needs** — `needs: sensor, log` inside a routine body.
6. **Executed assertions** — `guard <expr>` as a statement.
7. **Typed signatures** — `action f(a: int, b: Celsius = 20 Celsius) -> bool:`,
   with `routine` and `func` as synonyms for `action`.
8. **Record-typed parameters** — `action emit_c_expr(node: AstNode):`.
9. **The wider unit lexicon** — meter/km/cm/mm, second/ms/hour/minute, kg/gram,
   degrees/radians, Celsius/Kelvin, Pascal/mbar/hPa, ampere, percent, plus the
   composite m/s, km/h and m/s2, and `<expr> in radians|degrees`.

Items 3, 4, 5 and 6 are **declarations, not statements**: they live on the item
(`children[1] = CONTRACTS`) or on the `WHILE` node, so no backend has to filter
them out of a body, and they cannot be mistaken for code.

Beyond that list the parser also reads the whole sentence-verb layer the engine
has (`arm`, `takeoff to`, `fly to … at`, `stop`, `turn on|off|left|right`,
`drive forward|backward`, `pulse … for`, `send alert`, `reject with`, `allow`,
`remember … as`, `recall memories matching`, `return to launch`, `log [level]`),
the `when` / `every` hooks (`when battery < 20%`, `when link lost for 3 seconds`,
`every 100 ms`), `try:` / `otherwise with err:`, named call arguments,
multi-line list literals and the geometry sentences (`distance between`,
`slant distance between`, `bearing from … to`, `bearing of`).

## The rules the pure-Kaze checker implements

`check_program` returns a list of `Diagnostic` records (`code`, `line`, `msg`);
`main` prints them and never prints a pass it did not compute.

| Rule | Code |
| :--- | :--- |
| capability family used but not declared in `needs:` (H-1) | `E_CAP_UNDECLARED` |
| capability operation the registry does not know | `E_KAZE_CAP_OP` |
| name that is not a parameter, a `let`, a loop/try binding or a known value | `E_KAZE_UNKNOWN_NAME` |
| `filter` whose body can fall off the end without `allow`/`reject` | `E_KAZE_FILTER_FALLTHROUGH` |
| `/` or `%` with no fallback and a divisor that is not a non-zero literal | `E_KAZE_MISSING_FALLBACK` |
| dynamic index with no fallback and no enclosing `i < length of xs` proof | `E_KAZE_MISSING_FALLBACK` |
| constant index outside a literal's known length | `E_KAZE_BOUNDS` |
| `while` with no `bounded n` (or a ceiling below 1) | `E_KAZE_LOOP_UNBOUNDED` |
| recursive routine (directly or through a call cycle) with no ceiling | `E_KAZE_RECURSION_UNBOUNDED` |
| `requires`/`ensures` clause that is not true-or-false | `E_KAZE_CONTRACT_NOT_BOOL` |
| `needs:` entry that is not a family or a known instance | `E_KAZE_CAP_UNKNOWN` |
| expression or machine form the front end could not read | `E_KAZE_PARSE` |

The index proof accepts three shapes, all from the AST alone: an explicit
`otherwise` fallback, `requires i < length of xs` on the enclosing routine, and
an enclosing `if`/`while`/`guard` of the same comparison — including through a
local alias such as `let len = length of src` followed by `if i < len:`. A
routine can also promise its result's length with `ensures length of result == 2`,
which is how `let r = parse_expr(...)` makes `r[0]` provable.

## What it does **not** cover

This is the honest list. None of these are silently passed; each is either
reported or simply left to the Rust front end.

* **No type inference.** Types on parameters, results and record fields are
  parsed, recorded and re-emitted, but not enforced. `let x: int = "text"`
  is not refused.
* **No unit algebra.** The unit lexicon is parsed and preserved, and an angle
  written in degrees is folded to radians on the way to C (`kz_math_to_radians`,
  the same lowering the reference engine applies, so `sin(90 degrees)` is 1).
  But dimensional arithmetic is not checked: `meters + seconds` is not refused
  here, and metre/kilogram/Celsius literals are emitted at their written value
  rather than normalised to base units. `E_KAZE_UNIT_MISMATCH` belongs to
  `kaze/src/checker.rs`.
* **No optimisation.** The C emitter lowers the AST directly. It does not fold
  constants, elide contracts or share literals.
* **No host-record field rule.** `E_KAZE_MISSING_FALLBACK` for a field read on a
  value that came from outside the routine is not implemented; the dynamic
  index and division/modulo forms are.
* **No capability arity checking.** An operation is checked for existence
  (`E_KAZE_CAP_OP`), not for its argument count; `op_arity` lives in
  `kaze/src/caps.rs`.
* **No contract satisfiability or interval propagation.** A clause is checked for
  being boolean, and its `requires` facts are used for index proofs, but it is
  not proved callable or contradictory.
* **No module loader.** `use` lines are recorded and re-emitted, not resolved.
  A file with any `use` line is therefore treated as open-world: an unknown
  call target is accepted (it may be imported), while unknown *values* are still
  reported.
* **Exact syntax diagnostics belong to the Rust parser.** The pure-Kaze parser
  recovers: whatever it cannot read becomes an `UNKNOWN` node, which the checker
  then reports as `E_KAZE_PARSE` with a line. It does not say "expected ')' here"
  with a column the way `kaze/src/parser.rs` does.
* **The native C backend is not the VM.** `try`/`otherwise` bodies are emitted
  but the recovery block is unreachable (marked in the C), `when`/`every` hooks
  have no scheduler (named in a comment), capability calls fail closed through
  `kz_cap_call` (no host in a standalone binary), and a fallback around anything
  other than a division, modulo or index is emitted as its inner expression —
  the same limitation `kaze/src/c_codegen.rs` has.
* **Geometry in the C backend is its own simple implementation** (points are
  three-element lists, distance is Euclidean, bearing is `atan2`), not the
  reference engine's.

## Building and running it

The bootstrap, from `kaze/`:

```sh
bash scripts/self-sync.sh                       # regenerate compiler/kaze_self.kaze
./target/debug/kaze check compiler/kaze_self.kaze

# stage 0 -> stage 1: the Rust engine emits C, cc builds the first native compiler
./target/debug/kaze emit-c compiler/kaze_self.kaze -o /tmp/s1.c
cc -std=c99 -O2 -Wno-unused-function /tmp/s1.c -o /tmp/s1

# stage 1 -> stage 2: the native compiler compiles itself
/tmp/s1 compiler/kaze_self.kaze > /tmp/s2.c
cc -std=c99 -O2 -Wno-unused-function /tmp/s2.c -o /tmp/s2

# stage 2 -> stage 3: fixed point
/tmp/s2 compiler/kaze_self.kaze > /tmp/s3.c
diff /tmp/s2.c /tmp/s3.c && echo "fixed point"
```

The last `diff` is empty: 300,048-byte stage-2 and stage-3 sources, byte for byte,
both with `sha256 f47d64b70698b110fa11f55faffb87f93e41234ee10d222b3f8ba6a089c6000a`.
(Stage 1 is 308,438 bytes: it is the Rust emitter's own C for the same program, and
the fixed point is the pure-Kaze compiler reproducing itself.)

The one binary has four modes, selected by the first positional argument:

```sh
/tmp/s1 emit-c     examples/hello.kaze            # source in, C out (the default)
/tmp/s1           examples/hello.kaze             # the same, one-argument form
/tmp/s1 check      examples/hello.kaze            # "check: ok", or one line per diagnostic
/tmp/s1 to-machine examples/hello.kaze            # the canonical S-expression form
/tmp/s1 to-human   /tmp/hello.machine             # machine input, human output
```

The one-argument form is why the bootstrap keeps working: when the first
positional argument is not a mode name it is the source itself, so `<bin>
file.kaze` still means "source in, C out".

## The machine form

`to-machine` writes one form per line; `to-human` reads it back. The round trip
is check-preserving: for all 24 `.kaze` files under `examples/`, `lib/` and
`compiler/`, `check` gives the identical verdict before and after.

```
(kaze
  (format 1)
  (needs "log")
  (action "main"
    (params (param "mode" "text"))
    (contracts (requires (bin ">" (name "n") (number "0"))) (recursion 64))
    (body (let "x" (fallback (bin "/" (name "a") (name "b")) (number "0")))
          (guard (bin ">" (name "x") (number "0")))
          (while (bin "<" (name "n") (number "5")) (bounded 100) (body ...))
          (return (name "total")))))
```

Expressions are `(number TEXT UNIT?)`, `(string ..)`, `(bool ..)`, `(name ..)`,
`(bin OP a b)`, `(fallback a b)`, `(field a NAME)`, `(index a i)`, `(len a)`,
`(in_unit UNIT a)`, `(list ..)`, `(call NAME ..)`, `(named NAME a)`.
Statements are `(let|set NAME e)`, `(return e?)`, `(eval e)`, `(guard e)`,
`(if c (then ..) (otherwise ..))`, `(while c (bounded n)? (body ..))`,
`(for_each NAME e (body ..))`, `(try (body ..) (otherwise (with NAME)? ..))`.

**Honesty rule:** the emitter writes `(unsupported …)` and the reader lowers what
it does not know to an `UNKNOWN` node or an `UNKNOWN_ITEM`. `to-human` marks
those with a `# E_KAZE_UNSUPPORTED` comment line, and `check` reports them as
`E_KAZE_PARSE`. Nothing is dropped in silence; `the_machine_reader_reports_what_it_cannot_lower`
pins that behaviour.

## Gate commands

From the repository root:

```sh
cargo test --manifest-path kaze/Cargo.toml --test frontend
cargo test --manifest-path kaze/Cargo.toml --test compat
cargo test --manifest-path kaze/Cargo.toml --all-features
cargo clippy --manifest-path kaze/Cargo.toml --all-targets --all-features -- -D warnings
bash kaze/scripts/self-sync.sh --check
```

`tests/frontend.rs` builds the stage-1 compiler the same way the bootstrap does,
then asserts:

* the monolith matches its parts (`self-sync.sh --check`);
* all **24** `.kaze` files under `examples/`, `lib/` and `compiler/` check clean
  (`check: ok`), which includes the compiler checking itself;
* **10** single-fault programs are each refused with exactly one diagnostic —
  division without a fallback, an unbounded `while`, unbounded recursion, an
  undeclared capability, a dynamic index, a constant index out of bounds, a
  non-boolean `requires`, an unknown capability operation, a filter that falls
  through, and an unknown name — and **9** legal spellings of the same programs
  are accepted;
* the machine round trip over all 24 files preserves the check result, and a
  machine form the reader does not know is reported;

If `cc` is unavailable the tests print a skip and return; they never pass
without evidence.

## Memory: what the compiler costs, measured

The first version of this frontend appended every token to one list
(`tokens = tokens + [tok]`). Concatenating a list copies it, so that loop was
quadratic, and the generated C never frees an intermediate list. On this
compiler's own monolith it grew to **7.5 GB and was killed**; a corpus sweep in
the background is what pushed the machine into tens of gigabytes of swap.

Measured on 2026-11-22 (macOS arm64, `cc -O2`, peak RSS via `/usr/bin/time -l`
and `ps` sampling):

| Operation | Input | Peak RSS |
| :--- | :--- | ---: |
| `emit-c` on the monolith (Stage 0) | 165 KB | 171 MB |
| Stage 1 compiling the monolith | 165 KB | 171 MB |
| Stage 2 compiling the monolith | 165 KB | 97 MB |
| `check` on a 38 KB module (`parser.kaze`) | 38 KB | 103 MB |
| `check` on the monolith | 165 KB | 555 MB |

Wall-clock on the same machine, all times for the Stage 1 binary:

| Operation | Before | After |
| :--- | ---: | ---: |
| `emit-c` of the monolith (parse, check, generate) | 2.16 s | **0.08 s** |
| `check` of the monolith | 2.23 s | **0.58 s** |
| `check` of a 307 KB comments-only file | 7.38 s | **0.01 s** |

The comments-only row is what found the bug: the lexer walks the source one
character at a time, and indexing a string called `strlen` on the whole string
**per character**, so lexing was O(n^2) and a 307 KB file scanned tens of
gigabytes of bytes. Strings now carry their length
(`struct { const char* data; size_t len; }`), computed once at construction, so
`length of s`, `s[i]` and the bounds checks are O(1).

What changed to get there:

- The lexer accumulates tokens in **256-token chunks** and flattens once: the
  concatenation cost is O(tokens x chunks) instead of O(tokens^2).
- The recursion check builds the **call graph once per module**, as two parallel
  lists of routine indices, instead of walking the AST (or scanning name-keyed
  edges) inside every reachability query.

Three more fixes brought the monolith from 4.8 GB down to 383 MB:

- **The recursion check's work list no longer pushes a callee that is already
  visited or already waiting.** A dense call graph pushed the same name over and
  over, and a Kaze list is copied on append, so the work list grew
  exponentially: 4,329 MB to 818 MB on its own.
- **The declared recursion ceiling counts a routine's own live frames**, not the
  global depth, and every return path leaves the frame. Before this, a `return`
  inside a loop or a `try` never decremented, so the counter only grew and the
  checker eventually hit the hard 256-frame bound.
- **Operator chains are walked iteratively** in the checker and printed flat by
  the machine emitter (a 200-term chain used to cost 200 parser frames).

The remaining cost is list copies the generated C cannot remove without a
language primitive. `KAZE_STATS=1` now attributes every append to the script routine
that performed it (`kaze-site:` lines), and the profile of checking the monolith is
unambiguous: `push_tok` 136 MiB and `tokenize` 73 MiB — **the lexer is half the
memory**, because each token copies a 256-token chunk and each sealed chunk copies
the chunk list. The chunk size that killed the original quadratic is now near the
theoretical optimum for copy-on-append (`N·C/2` versus `N²/2C` balance at
`C ≈ sqrt(N)`). The in-place append rule covers `set xs = xs + [x]` when the
compiler can prove the list unshared, but the front end's helpers build their
result with `return out + [diag]`, and a value rebuilt on return is no longer the
caller's list -- consuming it in place would duplicate the appended element. An
inter-procedural "consuming parameter" rule was implemented with a fixpoint and
is sound, but it applies to nothing in this code:

```
consumed_params(kaze/compiler/checker.kaze) == []
```

That is recorded as a test, not a wish. Removing the remaining 543 MB needs a
growable-list primitive in the language (a `push` with a documented ownership
rule), which is a language change, or the Phase 7 arena/ownership work, which
reclaims the copies instead of avoiding them.
