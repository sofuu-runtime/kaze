# Kaze Language Specification — v1.0

> **Status: FROZEN 2026-11-05.** This is the normative specification of the
> standalone Kaze prototype in this repository. It is *not* the specification of
> the Kaze engine shipped inside `sofuu` (`crates/sofuu-core/src/domains/kaze/`,
> language v0.1, frozen separately in `docs/KAZE-SPEC.md`). The two surfaces
> have deliberately diverged; see `docs/COMPATIBILITY.md`.
>
> Changes to this document require a minor/major release decision and a format
> bump where they alter the artifact or the ISA. See `docs/STABILITY.md`.

## 1. Conformance

An implementation conforms to v1.0 if it:

1. accepts every program described by §3 and rejects malformed input with a
   typed error (§7);
2. produces the observable behaviour of §4–§6, including the safety refusals;
3. reads and writes module format 4 (§6.2) and verifies it before execution
   (§6.4);
4. produces byte-identical artifacts for the same source, root name and module
   layout.

## 2. Lexical structure

- Source is UTF-8. Lines end with `\n` or `\r\n`.
- Indentation is spaces only. A tab anywhere is a lexical error. The canonical
  form is four spaces per level; the parser accepts any consistent widths, and
  `kaze fmt` rewrites them to four.
- A `#` outside a string starts a comment to end of line. Blank and
  comment-only lines produce no tokens and do not open or close a block.
- Newlines inside `(`/`[` are continuations, not statement ends.
- Strings are double-quoted, single-line. Escapes: `\n`, `\t`, `\"`, `\\`.
  Any other escape is an error.
- Numbers are decimal digits with at most one `.` and digits after it. No sign,
  exponent, hex or leading `.5`; a leading `-` is the unary operator. A literal
  that is not finite in `f64` is a lexical error.
- Identifiers are ASCII `[A-Za-z_][A-Za-z0-9_]*`.
- Punctuation: `: ( ) , . % + - * / < > = [ ]`; two-character `<= >= == !=`.
- Units (only immediately after a numeric literal): `m`/`meters`, `m/s`,
  `degrees`, `s`/`seconds`, `ms`/`milliseconds`, `%`/`percent`. Time units
  normalize to seconds (`500 ms` → `0.5 s`); `%` stays a whole number (`20%` is
  20). Angle conversion is the separate postfix form `x in radians` /
  `x in degrees`, not a unit.

## 3. Grammar

```ebnf
program     = header* item+ ;
header      = needs | use ;
needs       = "needs" ":" ident ("," ident)* newline ;
use         = "use" string newline ;              (* .kaze path *)

item        = action | check | filter | when | every ;
action      = ("action" | "check" | "filter") ident "(" params? ")" ":" block ;
when        = "when" expr ":" block ;
every       = "every" number timeunit ":" block ;
params      = ident ("," ident)* ;

block       = newline indent stmt+ dedent ;

stmt        = let | set | if | for | while | try | return | eval ;
let         = "let" ident "=" expr ;
set         = "set" ident ("=" | "to") expr ;
if          = "if" expr ":" block ("otherwise" ":" block)? ;
for         = "for" "each" ident "in" expr ":" block ;
while       = "while" expr ":" block ;
try         = "try" ":" block
              "otherwise" ("with" ident)? ":" block ;
return      = "return" expr? | "return" "to" "launch" ;
eval        = verb | expr ;

expr        = or_expr ;
or_expr     = and_expr ("or" and_expr)* ;
and_expr    = not_expr ("and" not_expr)* ;
not_expr    = "not" not_expr | comparison ;
comparison  = additive (("<"|"<="|">"|">="|"=="|"!=") additive)? ;
additive    = multiplicative (("+"|"-") multiplicative)* ;
multiplicative = unary (("*"|"/"|"%") unary)* ;
unary       = "-" unary | postfix ;
postfix     = primary (("[" expr "]") | ("in" ("radians"|"degrees")))* ;
primary     = number [unit] | string | "true" | "false" | "(" expr ")"
            | "[" (expr ("," expr)*)? "]"
            | "length" "of" additive
            | math_call | "distance" "between" additive "and" additive
            | "slant" "distance" "between" additive "and" additive
            | "bearing" ("of" additive | "to" additive
                         | "from" additive "to" additive)
            | "link" "lost" "for" number timeunit
            | "policy" ("detects" | "detect") ident "in" additive
            | "recall" "memories" "matching" expr
            | ident ("." ident ("(" args? ")")?)?
            | telemetry_name ;

math_call   = ("vector" | "sin" | "cos" | "atan2" | "sqrt")
              "(" args? ")" ;
args        = expr ("," expr)* ;
```

`verb` is the sentence layer: `log [level] expr`, `send alert expr`,
`remember expr as expr`, `allow [expr]`, `reject with expr`,
`pulse ident for expr`, `turn on|off ident`, `turn left|right by expr`,
`arm ident`, `takeoff to expr`, `fly to expr [at expr]`, `land [ident]`,
`hold [position]`, `stop ident`, `drive [forward|backward] [at expr]`,
`recall memories matching expr`. Verbs lower to capability calls (see §5.7).

## 4. Types and values

| Type | Meaning |
| :--- | :--- |
| number | `f64`, optionally tagged with a unit |
| text | an arena-backed UTF-8 string |
| true/false | booleans |
| list | an ordered heap array |
| point | `[lat_or_x, lon_or_y, alt_or_z]` (three `f64`) |
| record | a heap map supplied by a host (readable with `.field`) |
| nothing | unit |

Units are checked loosely: numeric literals carry a unit so `meters + m/s` is
a checker error, and a point used as a **place** must have `lat` in `[-90, 90]`
and `lon` in `[-180, 180]` (`E_KAZE_RANGE`). Units have no runtime effect.
Points additionally support `p.x`/`p.lat`, `p.y`/`p.lon`,
`p.z`/`p.alt`/`p.altitude` and `p[0..2]`.

## 5. Semantics

### 5.1 Memory

Strings live in a linear arena; lists/maps/points in heap side tables. Nothing
is individually freed. The supervisor reclaims both at each tick boundary; no
value survives a tick. A single run is bounded by `Limits.arena_bytes` (string
bytes) **and** `Limits.heap_values` (default 65,536 values); an allocation over
either budget is `E_KAZE_MEMORY` and is never catchable. Fuel bounds work, not
memory; the heap budget is the memory bound.

### 5.2 Entry

`--entry` wins. Otherwise the **root file's** `main` is selected; else the only
`action`/`check`/`filter` in the root; else none (hooks only). Imported routines
are never the implicit entry. More than one root routine without `main` is
`E_KAZE_ENTRY`.

### 5.3 Hooks and ticks

Supervisor ticks run after the entry. Each tick evaluates every `when` item and
every `every` item whose `period_ms` divides the virtual time. Ticks are virtual
unless `--pace` is set. A hook may run on every tick; recovery must be
idempotent. `when link lost for <time>:` reads the live link's silence window
and requires `needs: drone`; the window is at least 1 ms.

### 5.4 Filters

A `filter` must end every path in `allow [value]` or `reject with reason`. The
verdict is `Allow(rendered value)` or `Reject(reason)`. A `try` short-circuit
restores the verdict that was in force at `try`, so an abandoned body path
cannot approve.

### 5.5 Fault tolerance

`try: … otherwise [with err]: …` recovers from exactly these operational codes:

```
E_KAZE_LINK  E_KAZE_LINK_TIMEOUT  E_COMMAND_REJECTED
E_ARMING_DENIED  E_KAZE_HOST
```

Everything else is fatal: `E_KAZE_RANGE`, every `E_CAP_*`,
`E_SAFETY_VIOLATION`, `E_TIMEOUT`, `E_KAZE_MEMORY`, type/maths/bytecode errors.
`otherwise with err` binds a record with `code` (text), `message` (text) and
`line` (number). Catching unwinds the operand stack to the depth at `try`; locals
are left as they were.

### 5.6 Modules

`use "path.kaze"` is a compile-time include. Paths resolve relative to the
importing file (source text: the current directory). Dependencies are inlined
depth-first before the importer's items; a diamond includes a file once; a
cycle, missing file, non-`.kaze` path, duplicate item name or header-only module
is an error. `needs:` merge and deduplicate. Each item keeps an origin relative
to the root directory. A compiled module has no imports.

### 5.7 Capabilities

A script declares families in `needs:`; the host grants backends with `--cap`.
Every call crosses the R7 gate in order:

- **H-1** family declared (`E_CAP_UNDECLARED`);
- **H-2** effector family has a grant (`E_CAP_MISSING`);
- **H-3** host policy allows it (`E_CAP_DENIED`);
- **H-4** physical arguments are in range and numeric (`E_KAZE_RANGE` /
  `E_KAZE_TYPE`), rejected never clamped.

The registry (family: operations) is:

| Family | Operations |
| :--- | :--- |
| drone | `arm`, `takeoff(alt m)`, `goto(point[, alt m] \| lat[, lon, alt])`, `land`, `rtl`, `hold`, `stop`, `telemetry(field)` |
| rover | `forward [m/s]`, `backward [m/s]`, `turn_left/right(deg)`, `stop`, `telemetry(field)` |
| sensor | `temperature`, `humidity`, `distance`, `front_distance`, `read(name/pin)`, `telemetry(field)` |
| device | `set_relay(name, on)`, `pulse(name, sec)`, `read_gpio(name)`, `set_duty(name, %)` |
| memory | `save(label, value)`, `recall(query)` |
| policy | `detects(kind, text)`, `detect(kind, text)`, `allow [value]`, `reject(reason)` |
| log | `info`, `warn`, `error`, `debug` |
| alert | `send(text)` |

`policy detects <kind> in <text>` is sugar for `policy.detects(<kind>, <text>)`
and yields a boolean. Bare telemetry (`battery`, `altitude`, `temperature`, …)
is resolved **per file** at parse time and requires exactly one hardware family
(`drone`/`rover`/`sensor`) in that file.

### 5.8 Fuel, deadline and interrupts

Every instruction costs one fuel unit. At zero the run stops with `E_TIMEOUT`
(not catchable). A loop head (`FUEL_TICK`) also polls a process-global host
deadline; an expired deadline stops the loop with `E_TIMEOUT`. A host call may
block; fuel is not charged for wall-clock time.

### 5.9 Determinism

For a given script, arguments, initial `SimState` and fuel, the simulator
produces the same actions, logs, value and fuel counts on every run. Compiling
the same source twice produces the same bytes; origins are relative to the root
directory (an import outside it is recorded as written), and the tables use
ordered insertion, never hashing.

## 6. Bytecode and module format

### 6.1 Encoding

One instruction is `opcode:8 | a:8 | b:8 | c:8` little-endian. Instructions
with an immediate are followed by one 32-bit word. Jumps are relative to the
end of the instruction, signed.

### 6.2 Module format 4

```
KAZEBC1\0            8 bytes magic
u16                  format version (4)
text                 root origin
u32 + text[]         needs
u32 + const[]        constants: u8 kind (0 num f64, 1 text, 2 bool), value
u32 + cap[]          capabilities: text family, text op, u8 argc
u32 + item[]         item:
                       u8 tag (0 action,1 check,2 filter,3 when,4 every)
                       f64 period_ms
                       u32 line
                       u8 slots
                       text name
                       text origin
                       u32 + text[] params
                       u32 + u32[] code
```

`text` is a `u32` byte length followed by UTF-8 bytes. The artifact contains no
timestamps, hashes or map iteration order.

### 6.3 ISA (frozen numbers)

| # | Op | Stack effect | # | Op | Stack effect |
| :-- | :--- | :--- | :-- | :--- | :--- |
| 0 | CONST | +1 | 24 | RET | −1 |
| 1 | LOAD | +1 | 25 | RET_NOTHING | — |
| 2 | STORE | −1 | 26 | FUEL_TICK | — |
| 3 | NEWARR(a) | −a,+1 | 27 | TRAP_ERR | — |
| 4 | GETINDEX | −2,+1 | 28 | HALT | — |
| 5 | LEN | −1,+1 | 29 | JMP_IF_TRUE | −1 |
| 6 | POP | −1 | 30 | GET_FIELD | −2,+1 |
| 7–11 | ADD/SUB/MUL/DIV/MOD | −2,+1 | 31–32 | JEQ/JNE | −2 |
| 12 | NEG | −1,+1 | 33 | VECNEW(a) | −a,+1 |
| 13–18 | CMP_* | −2,+1 | 34 | VECDIST | −2,+1 |
| 19 | NOT | −1,+1 | 35 | VECBEARING | −1,+1 |
| 20 | JMP | — | 36 | VECBEARING_BETWEEN | −2,+1 |
| 21 | JMP_IF_FALSE | −1 | 37–38 | SIN/COS | −1,+1 |
| 22 | CALL_CAP(a) | −a,+1 | 39 | ATAN2 | −2,+1 |
| 23 | POLICY_STOP | −1 | 40 | SQRT | −1,+1 |
| 41–42 | DEG_TO_RAD/RAD_TO_DEG | −1,+1 | 43 | VECSLANT_DIST | −2,+1 |
| 44 | TRY_BEGIN | — | 45 | TRY_END | — |
| 46 | LINK_LOST | +1 | | | |

Immediates: CONST (const index), JMP/JMP_IF_*/JEQ/JNE/TRY_BEGIN (relative
offset), CALL_CAP (cap index), TRAP_ERR (const index), FUEL_TICK (source line),
LINK_LOST (milliseconds of silence).

### 6.4 Verifier

Before execution (and inside `deserialize`), every item is checked: known
opcodes and immediates; constant/capability indices; `CALL_CAP` argc matches
its table entry, lies inside the registry's arity, and names a registered op;
local slots exist; every jump lands on an instruction boundary or the end; and
a stack-depth dataflow proves no underflow, consistent merge depths, and ≤256
stack entries. `TRY_BEGIN` must occur at stack depth 0 (the compiler emits it
at a statement boundary), because the VM's recovery unwinds by truncation and
cannot restore a deeper stack. A module that fails is refused
(`E_KAZE_BADCODE`).

## 7. Errors

`CODE: message` (`file:line[:col]` is added by the CLI). Stable codes:

`E_KAZE_PARSE`, `E_KAZE_CHECK`, `E_KAZE_COMPILE`, `E_KAZE_ENTRY`, `E_KAZE_IO`,
`E_KAZE_IMPORT`, `E_KAZE_FMT`, `E_KAZE_BADCODE`, `E_KAZE_TYPE`,
`E_KAZE_RANGE`, `E_KAZE_BOUNDS`, `E_KAZE_DIV0`, `E_KAZE_MATH`,
`E_KAZE_OVERFLOW`, `E_KAZE_STACK`, `E_KAZE_MEMORY`, `E_KAZE_TRAP`,
`E_TIMEOUT`, `E_CAP_UNDECLARED`, `E_CAP_MISSING`, `E_CAP_DENIED`,
`E_SAFETY_VIOLATION`, `E_KAZE_LINK`, `E_KAZE_LINK_TIMEOUT`,
`E_COMMAND_REJECTED`, `E_ARMING_DENIED`, `E_KAZE_HOST`.

## 8. Known limits (v1.0)

- No user-routine call syntax; routines are reached by `--entry` or hooks.
- Modules are global: no namespaces, conditional imports or aliases.
- Bare telemetry is per-file, not merged across modules.
- Nesting is bounded to **64 expression levels** and **64 blocks**; deeper
  input is `E_KAZE_PARSE`, not a process stack overflow.
- Import identity uses canonical paths; reads and nested resolution use the
  path as written (a symlinked module's imports resolve next to the symlink).
- The built-in media/inference features of the Sofuu product are not part of
  this prototype's language.
