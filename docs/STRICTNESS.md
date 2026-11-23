# Strictness rules (Phase 2)

Phase 2 of the language roadmap removes the constructs that can panic at run
time from the standalone engine (`kaze/src`). Every rule below is enforced
**before** a program is compiled, except the recursion ceiling, which the VM
also enforces while running. A program that breaks a rule is refused with a
stable diagnostic code, never with a crash.

The acceptance corpus for these rules lives in
`kaze/tests/fixtures/strict/` and is driven by
`kaze/tests/strict_semantics.rs`.

## Running the corpus

From the repository root:

```sh
cargo test --manifest-path kaze/Cargo.toml --test strict_semantics
```

or, from inside `kaze/`:

```sh
cargo test --test strict_semantics
```

## What the corpus covers

| Rule | Diagnostic | Rejected fixtures |
| :--- | :--- | ---: |
| Division / modulo without a fallback | `E_KAZE_MISSING_FALLBACK` | 16 |
| Dynamic list / text / point index without a fallback | `E_KAZE_MISSING_FALLBACK` | 7 |
| Host-record field read without a fallback | `E_KAZE_MISSING_FALLBACK` | 4 |
| Capability value in expression position without a fallback | `E_KAZE_MISSING_FALLBACK` | 5 |
| Capability family used but not declared | `E_KAZE_CAP_UNDECLARED` | 6 |
| Constant index outside a literal's bounds | `E_KAZE_BOUNDS` | 6 |
| `while` without a fuel ceiling | `E_KAZE_LOOP_UNBOUNDED` | 6 |
| Recursion without a declared ceiling | `E_KAZE_RECURSION_UNBOUNDED` | 4 |
| Declared ceiling outside `1..=256` | `E_KAZE_PARSE` | 1 |
| Declared recursion ceiling exceeded at run time | `E_KAZE_RECURSION_DEPTH` | 1 |

72 fixtures: those 56 rejected programs plus 16 positive controls — the
constant-divisor and constant-index controls that need no fallback (5), a
dynamic index proved by the loop's length guard (1), the `otherwise` fix in
place (4), a bounded `while` (2), `for each` over a list (2), and bounded
direct and mutual recursion (2).

Every fixture is listed in exactly one table in the runner, and the tables are
cross-checked against the files on disk, so the corpus cannot rot: adding a
fixture without listing it, or deleting one that is still listed, fails
`the_corpus_is_large_enough_and_every_fixture_is_classified`.

Each fixture names the code it expects in its own header (`# expect: …`) and
marks the offending line with `# BAD`. The runner asserts the exact code, and
that the diagnostic names the marked line.

## `E_KAZE_MISSING_FALLBACK` — every fallible expression states its fallback

A fallible expression must carry an inline fallback: `<expr> otherwise
<fallback-expr>`. Fallible expressions are division, modulo, a dynamic index
(`list[i]`, `text[i]`, `point[i]`), a dynamic field read on a host record,
and a value-returning capability call (`sensor`, `device`, `memory`) in
expression position.

Two cases are provably safe and need no fallback: a **literal non-zero
divisor**, and a **constant index inside a literal's known length**.

```kaze
action good(total: num, parts: num, items: list, i: num, gap: num):
    # accepted: every fallible operation has a fallback
    let share = total / parts otherwise 0
    let chosen = items[i] otherwise "none"
    let speed = gap / 3
    let second = [10, 20, 30][1]

action bad(total: num, parts: num, items: list, i: num, reading: record, sensor: record):
    # refused with E_KAZE_MISSING_FALLBACK
    let share = total / parts
    let chosen = items[i]
    let q = total / 0
    let v = reading.value
    let t = sensor.temperature()
```

A capability call used as a **statement** is still an effect, not a value, so
it needs no fallback; only its value form does.

## `E_KAZE_BOUNDS` — constant indices are checked at compile time

When the indexed value is a literal, the index is a constant and it is
compared against the literal's length while checking. Out of bounds is a
compile-time error, not a run-time trap.

```text
# statement fragments, not a whole program
# good
return [10, 20, 30][1]

# bad — refused with E_KAZE_BOUNDS
let v = [1, 2, 3][7]
let v = [10, 20][2]
let v = [][0]
```

## `E_KAZE_LOOP_UNBOUNDED` — a `while` loop declares its fuel ceiling

Every `while` must declare how many iterations it may take:
`while cond bounded <positive integer>:`. The ceiling is a plain positive
integer literal. `for each` over a list literal or a list value is bounded by
the list and needs no declaration.

```text
# statement fragments, not a whole program
# good
while i < 3 bounded 1000:
    set i = i + 1

for each point in points:
    fly to point

# bad — refused with E_KAZE_LOOP_UNBOUNDED
while i < 3:
    set i = i + 1

while true:
    let x = 1
```

A ceiling on an outer loop does not bound an inner loop: each `while` in the
nesting declares its own.

## `E_KAZE_RECURSION_UNBOUNDED` — a recursive routine declares its depth

A routine that reaches itself directly, or through a cycle of calls, must
declare its own ceiling on its own line at the top of the body (with any
`requires`/`ensures` clauses, before the first statement):
`recursion bounded <positive integer literal <= 256>`. Mutual recursion needs
the declaration on **every** routine in the cycle.

```kaze
# good
action down(n):
    recursion bounded 64
    if n <= 0:
        return 0
    return down(n - 1)

# bad — refused with E_KAZE_RECURSION_UNBOUNDED
action countdown(n):
    if n <= 0:
        return 0
    return countdown(n - 1)
```

A declared ceiling outside `1..=256` cannot be honoured, so the declaration
itself is refused while parsing (`E_KAZE_PARSE`: "'recursion bounded' needs a
whole number of frames from 1 to 256"). A non-recursive routine needs no
declaration.

At run time the VM honours the declared ceiling: a call that would exceed it
aborts with `E_KAZE_RECURSION_DEPTH` instead of exhausting the process stack.

```kaze
action main():
    return down(8)

action down(n):
    recursion bounded 3        # eight calls deep: E_KAZE_RECURSION_DEPTH
    if n <= 0:
        return 0
    return down(n - 1)
```

## `E_KAZE_CAP_UNDECLARED` — a capability family is declared before use

Every capability family a script touches must be named in the `needs:` line.
This rule is unchanged by Phase 2; the corpus pins it so the stricter
expression rules cannot quietly replace it.

```kaze
# good: the routine's capability is declared
needs: drone, log

action lift():
    arm drone
```

```text
# bad — refused with E_KAZE_CAP_UNDECLARED: the routine uses `drone`
# but the script only declares `log`
needs: log

action lift():
    arm drone
```

## Points the corpus leaves open

Three places where the contract, the grammar and the checker do not yet meet
cleanly. The corpus does not pretend to cover them:

1. **Proofs beyond the two the contract names.** The corpus pins exactly the
   proofs the Phase 2 contract requires — a literal index inside a literal's
   length, and an explicit `otherwise` — plus one stronger form that the engine
   accepts today: a dynamic index inside a loop whose condition bounds it
   (`while i < length of items bounded 100:`, fixture
   `index_loop_guarded_ok.kaze`). Two other forms that the diagnostic text
   suggests do **not** discharge the rule yet, so no fixture claims them: an
   `if i < length of xs:` guard, and a `requires i < length of xs` contract.
2. **Modulo with a literal dividend.** `10 % 4` does not lex as modulo: a `%`
   directly after a number is the percent unit, so the source means `10%`
   followed by a stray `4`. Modulo fixtures therefore use a named dividend
   (`total % 4`, `total % 0`).
3. **Constant index into a point literal.** `vector(1, 2, 3)[1]` has a known
   width, but the contract only names list and text literals as provable, so
   no fixture asserts whether that index needs a fallback.

## Corrections and additions after the rules landed (2026-11-22)

Three details changed while the rules were implemented, and this file is the
reference for them:

1. **The capability code is `E_CAP_UNDECLARED`**, the code the roadmap and
   SPEC §7 name. The corpus fixtures and their table use that spelling.
2. **A host-record field read is refused only when the base is an untyped
   parameter of the routine**, because that is the value the host may fill with
   a shape the script did not describe. Declare the parameter with a record type
   (`action f(node: AstNode):`) and its fields are checked against the record
   instead. A field read on a `let` binding is not refused: the script owns the
   value it just built.
3. **A field rule cannot be removed by a `guard`**: `guard n > 0` proves an
   index, not a field. Use a record type, or write the fallback.

### Proofs the checker accepts (and how to write them)

| To use | Write |
| :--- | :--- |
| `total / parts` with a dynamic divisor | `total / parts otherwise 0` |
| `src[i]` in a scan | keep the loop guard `while i < len`, where `len = length of src` |
| `src[i]` after mutating `i` | `guard i < (length of src)` before the read |
| `tokens[pos]` inside a helper routine | `requires pos >= 0` and `requires pos < (length of tokens)` |
| `node.children[0]` on an AST node | `guard 0 < (length of node.children)` (or index a value whose length a callee promised) |
| a routine that returns a pair | `ensures length of result == 2`, then `res[0]`/`res[1]` are provable at every call site |
| a recursive routine | `recursion bounded 200` as its first line |
| a `while` loop | `while cond bounded 1000:` |
| a sensor read | `let t = sensor.temperature() otherwise 0 Celsius` (the fallback must be the whole statement value) |

### Known quirks

- `10 % 4` is **not** writable: `%` directly after a number lexes as the percent
  unit, so the source means `10%` followed by a stray `4`. Use a named dividend
  (`total % 4`) or parentheses (`(10) % 4`).
- A capability fallback recovers operational failures only. `E_CAP_*` and
  `E_SAFETY_VIOLATION` stay fatal, so a missing grant is never swallowed by a
  fallback value.
- `sin`/`cos` read radians. An angle-dimension literal is converted at compile
  time, so `sin(90 degrees)` is 1, but a variable that holds a bearing must be
  converted with `x in radians`.
