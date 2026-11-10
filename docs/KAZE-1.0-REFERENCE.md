# Kaze 1.0 — tagged language and runtime reference

**Snapshot:** 2026-11-10. This inventory describes the current standalone
`kaze/` source and its CLI after the capability-safety changes. It does
not establish the contents of a published release, the behavior of a different
checkout, or successful operation on physical hardware.

Standalone **Kaze is 1.0** (`kaze --version`); the Rust package is `1.0.0` and
its bytecode format is **4**. Sofuu also has a separate embedded Kaze engine
that reports **0.1**; its current source writes format **2** and reads legacy
format **1**. Their features and bytecode are not interchangeable. This document is for standalone Kaze 1.0. The embedded
engine has a separate `docs/KAZE-CURRENT-SURFACE.md` inventory in the Sofuu workspace.

## Tag key

Every inventory row has at least one status tag. Tags describe this snapshot;
`IMPLEMENTED` alone is a source-code observation, not proof of deployment.

| Tag | Meaning |
| --- | --- |
| `[IMPLEMENTED]` | Present in the inspected standalone source for the stated scope. |
| `[VERIFIED]` | Additionally checked locally; the evidence section identifies pure checks, real local operations, or explicitly labelled robotics simulation. |
| `[PARTIAL]` | Present with a stated gap, restriction, or incomplete enforcement. |
| `[LIVE-PATH]` | A real hardware code path exists; a configured device and appropriate permissions are still required. |
| `[UNVERIFIED]` | This inventory did not verify the stated physical device, external integration, or published artifact. |
| `[SIMULATED]` | The built-in path supplies simulated state or an imitation rather than a real external operation. |
| `[REGISTRY-ONLY]` | The name and arity are recognized, but the inspected built-in host has no matching implementation. |
| `[POST-BASELINE]` | This detail differs from the frozen standalone v1.0 baseline; the release compatibility decision remains open. |
| `[UNSUPPORTED]` | No corresponding general feature is implemented in this standalone surface. |
| `[TARGET]` | A repository requirement or intended behavior, not a verified implementation. |

**Policy note:** `RULES.md` §1.9 permits explicitly selected and labelled
robotics/drone simulation for testing. In this source snapshot the standalone host
refuses unavailable non-robotics backends instead of simulating them. Simulator
selection is explicit and reported; physical hardware has not been verified.

## 1. Syntax

### 1.1 Source structure and lexical rules

| Item | Tag | Exact form and behavior |
| --- | --- | --- |
| File encoding | `[IMPLEMENTED]` | UTF-8 source. `kaze run -` and `kaze check -` read source from stdin. |
| Program | `[IMPLEMENTED]` | Optional headers followed by at least one `action`, `check`, `filter`, `when`, or `every` item. A header-only file is rejected. |
| Header order | `[IMPLEMENTED]` | `needs:` and `use` lines can appear in either order before the first item. Each file permits at most one `needs:` line. |
| Block | `[IMPLEMENTED]` `[VERIFIED]` | A colon ends the header; the body starts on the next line with at least one indented statement. Dedents must match a previous indentation level. |
| Indentation | `[IMPLEMENTED]` | Spaces are used; the formatter uses four spaces per level. Tabs in code outside strings/comments are rejected. |
| Comments | `[IMPLEMENTED]` | `#` outside a string starts a comment through the end of the line. |
| Continuation | `[IMPLEMENTED]` | Newlines inside `(...)` or `[...]` continue the expression. |
| Identifier | `[IMPLEMENTED]` | ASCII `[A-Za-z_][A-Za-z0-9_]*`; names are case-sensitive. |
| Text literal | `[IMPLEMENTED]` | Double-quoted, single-line text. Supported escapes: `\n`, `\t`, `\"`, `\\`. |
| Number literal | `[IMPLEMENTED]` | Decimal integer or fraction such as `12` or `1.25`; the literal must fit a finite `f64`. Leading `-` is an operator. Exponents and hexadecimal are not numeric literals. |
| Punctuation | `[IMPLEMENTED]` | `:`, `(`, `)`, `,`, `.`, `%`, `+`, `-`, `*`, `/`, `<`, `>`, `=`, `[`, `]`; plus `<=`, `>=`, `==`, `!=`. |
| Semicolon and braces | `[UNSUPPORTED]` | No general semicolon statement separator, `{...}` block, or map literal syntax. |

### 1.2 Header and declaration syntax

| Syntax | Tag | Meaning and constraints |
| --- | --- | --- |
| `needs: family, family` | `[IMPLEMENTED]` | Declare capability resources. Duplicate names in one header are rejected. A declaration is not itself a backend grant. |
| `use "path.kaze"` | `[IMPLEMENTED]` `[VERIFIED]` | Compile-time include. The path must end in `.kaze`; relative paths resolve from the importing file. |
| `action name(a, b):` | `[IMPLEMENTED]` | Declare an executable named routine with positional signature names. |
| `check name(a, b):` | `[IMPLEMENTED]` | Declare a named check routine. Runtime entry binding works like an action. |
| `filter name(a, b):` | `[PARTIAL]` `[VERIFIED]` | Declare a policy-oriented routine; the checker allows a plain `return`, which can yield no verdict. |
| `when condition:` | `[IMPLEMENTED]` | Declare a hook evaluated on each requested supervisor tick. |
| `every 100 ms:` | `[IMPLEMENTED]` | Declare a periodic hook with a numeric literal and time unit. It runs on ticks whose virtual time is divisible by its period. A zero period is never due. |
| Duplicate routine names | `[IMPLEMENTED]` | Rejected within one file and on module merge. Hook names are generated. |
| Parameter defaults / annotations | `[UNSUPPORTED]` | Routine signatures contain names only; no default values or source type annotations. |

### 1.3 Statements

| Syntax | Tag | Meaning |
| --- | --- | --- |
| `let name = expression` | `[IMPLEMENTED]` `[VERIFIED]` | Bind a value in the current scope. |
| `set name = expression` | `[IMPLEMENTED]` | Reassign an existing binding. |
| `set name to expression` | `[IMPLEMENTED]` `[VERIFIED]` | Alternative spelling for `set`. |
| `if condition:` | `[IMPLEMENTED]` | Run its indented block when the condition is true. |
| `otherwise:` after `if` | `[IMPLEMENTED]` | Optional alternative block. |
| `for each item in values:` | `[IMPLEMENTED]` `[VERIFIED]` | Iterate over a list; the loop variable is local to its body. |
| `while condition:` | `[IMPLEMENTED]` | Repeat while the condition is true, subject to the run's fuel and applicable deadline. |
| `try:` | `[IMPLEMENTED]` | Start a protected block for the specific recoverable host errors in §5. |
| `otherwise:` after `try` | `[IMPLEMENTED]` | Required recovery block; it can omit an error binding. |
| `otherwise with error:` | `[IMPLEMENTED]` | Recovery block with `error.code`, `error.message`, and `error.line`. |
| `return` | `[IMPLEMENTED]` | End the current item with the runtime value `nothing`. |
| `return expression` | `[IMPLEMENTED]` `[VERIFIED]` | End the current item with that value. |
| `return to launch` | `[IMPLEMENTED]` | Capability sentence for `drone.rtl()`; this is a host operation, not the ordinary `return` statement. |
| Expression statement | `[IMPLEMENTED]` | Evaluate an expression and discard its value unless it is a policy verdict. |
| `break` / `continue` | `[UNSUPPORTED]` | No general loop-control statements. |
| Field or index assignment | `[UNSUPPORTED]` | `set record.field ...` and `set list[index] ...` are not general assignment forms. |

### 1.4 Expressions and operators

| Form | Tag | Meaning and restrictions |
| --- | --- | --- |
| `name` | `[IMPLEMENTED]` | Read a parameter or local binding; some unbound names can resolve as bare telemetry. |
| `(expression)` | `[IMPLEMENTED]` | Group an expression. |
| `[a, b, ...]` | `[IMPLEMENTED]` | Ordered list literal; at most 255 elements per literal. |
| `value[index]` | `[IMPLEMENTED]` | Zero-based read from a list, text, or point; index must be a nonnegative integer in range. |
| `name.field` | `[IMPLEMENTED]` | Read a field from a named record or point. A missing record field returns `nothing`. |
| `receiver.operation(args)` | `[PARTIAL]` | Call a registered capability operation. The receiver is a resource/family name, not a general object method. At the beginning of a statement, `log.info(...)` is parsed as the `log` sentence and fails; use `log value` or place the call in an expression. |
| `log.info(...)` as a statement | `[PARTIAL]` `[VERIFIED]` | Rejected by the current parser because `log` starts a sentence. `return log.info(...)` passes the front end; `log value` is the usual sentence form. Other `log.<level>(...)` statements encounter the same dispatch. |
| `length of value` | `[IMPLEMENTED]` | Character count for text, element count for lists, and Euclidean magnitude for a point. |
| `-value` | `[IMPLEMENTED]` | Numeric negation; point negation is supported. |
| `not value` | `[IMPLEMENTED]` | Boolean negation; the checker expects boolean where it knows the type. |
| `a + b` | `[IMPLEMENTED]` | Add numbers or points, or concatenate two text values. |
| `a - b` | `[IMPLEMENTED]` | Subtract numbers or points. |
| `a * b` | `[IMPLEMENTED]` | Multiply numbers or scale a point by a number. |
| `a / b` | `[IMPLEMENTED]` | Divide numbers or divide a point by a number; zero divisor raises `E_KAZE_DIV0`. |
| `a % b` | `[PARTIAL]` `[VERIFIED]` | Numeric remainder. Immediately after a numeric literal `%` can instead be consumed as a percent unit: `return 5 % 2` returned `5` in a local probe. Use `return (5) % 2` or a variable for the left operand. |
| `a < b`, `a <= b` | `[IMPLEMENTED]` | Ordering comparisons for compatible numbers or text. |
| `a > b`, `a >= b` | `[IMPLEMENTED]` | Ordering comparisons for compatible numbers or text. |
| `a == b`, `a != b` | `[IMPLEMENTED]` | Equality / inequality. Points compare components; lists and records compare their heap identities, not deep structural contents. |
| `a and b` | `[IMPLEMENTED]` | Short-circuit conjunction; the right side is skipped if the left is false. |
| `a or b` | `[IMPLEMENTED]` | Short-circuit disjunction; the right side is skipped if the left is true. |
| `routine(args)` | `[UNSUPPORTED]` | No direct call from one user-defined Kaze routine to another. Use `--entry` to select a routine. |
| Chained comparisons | `[UNSUPPORTED]` | The grammar handles one comparison at a time; do not rely on `a < b < c`. |
| String interpolation | `[UNSUPPORTED]` | Build supported text using literals and `+` between text values. |

Operator precedence, from tighter to looser: postfix/indexing and angle
conversion; unary `-`; `*` `/` `%`; `+` `-`; comparison; `not`; `and`; `or`.
Use parentheses when the intended grouping is not obvious.

### 1.5 Mathematical and spatial forms

| Syntax | Tag | Result and constraints |
| --- | --- | --- |
| `vector(x, y)` | `[IMPLEMENTED]` | Create a point with `z = 0`. |
| `vector(x, y, z)` | `[IMPLEMENTED]` `[VERIFIED]` | Create a point from three numeric components. |
| `distance between a and b` | `[IMPLEMENTED]` | Great-circle ground distance in meters; `a` and `b` are geographic points. |
| `slant distance between a and b` | `[IMPLEMENTED]` | Ground distance combined with altitude difference. |
| `bearing of offset` | `[IMPLEMENTED]` | Compass bearing of a local vector in degrees: north 0, east 90. |
| `bearing from a to b` | `[IMPLEMENTED]` | Initial compass bearing between two geographic points. |
| `bearing to target` | `[PARTIAL]` | Uses ambient latitude/longitude from exactly one declared hardware resource in the file; backend availability controls whether the position is real. |
| `sin(x)` | `[IMPLEMENTED]` | Sine of a number in radians. |
| `cos(x)` | `[IMPLEMENTED]` | Cosine of a number in radians. |
| `atan2(y, x)` | `[IMPLEMENTED]` | Two-argument arctangent; result is radians. |
| `sqrt(x)` | `[IMPLEMENTED]` `[VERIFIED]` | Square root; a negative input raises `E_KAZE_MATH`. |
| `x in radians` | `[IMPLEMENTED]` | Convert degrees to radians. |
| `x in degrees` | `[IMPLEMENTED]` | Convert radians to degrees. |
| `link lost for 3 seconds` | `[PARTIAL]` | Boolean link-silence expression; requires a drone declaration, grant, valid policy, and configured backend. Window must be 1 through 4,294,967,295 ms. A real result requires a live link. |
| `policy detects kind in text` | `[PARTIAL]` | Lowers to `policy.detects(kind, text)`; detector behavior depends on the host. |
| `recall memories matching query` | `[REGISTRY-ONLY]` | Lowers to `memory.recall(query)`; the standalone built-in host currently refuses it with `E_CAP_MISSING`. |

### 1.6 Sentence-form capability calls

These forms are syntactic sugar. Their tags describe parsing; §4 gives the
separate real-backend status and required declaration/grant.

| Sentence | Tag | Lowers to |
| --- | --- | --- |
| `log value` | `[IMPLEMENTED]` | `log.info(value)`. |
| `log info value` | `[IMPLEMENTED]` | `log.info(value)`. |
| `log warn value` / `log warning value` | `[IMPLEMENTED]` | `log.warn(value)`. |
| `log error value` | `[IMPLEMENTED]` | `log.error(value)`. |
| `log debug value` | `[IMPLEMENTED]` | `log.debug(value)`. |
| `send alert value` | `[IMPLEMENTED]` | `alert.send(value)`. |
| `remember value as label` | `[IMPLEMENTED]` | `memory.save(label, value)`. |
| `recall memories matching query` | `[IMPLEMENTED]` | `memory.recall(query)`. |
| `allow` / `allow value` | `[IMPLEMENTED]` | `policy.allow()` / `policy.allow(value)` and terminate the item. |
| `reject with reason` | `[IMPLEMENTED]` | `policy.reject(reason)` and terminate the item. |
| `arm drone` | `[IMPLEMENTED]` | `drone.arm()`; the noun resolves to its family. |
| `takeoff to height` | `[IMPLEMENTED]` | `drone.takeoff(height)`. |
| `fly to point` | `[IMPLEMENTED]` | `drone.goto(point)`. |
| `fly to point at altitude` | `[IMPLEMENTED]` | `drone.goto(point, altitude)`. |
| `land` / `land drone` | `[IMPLEMENTED]` | `drone.land()`. |
| `hold` / `hold position` | `[IMPLEMENTED]` | `drone.hold()`. |
| `return to launch` | `[IMPLEMENTED]` | `drone.rtl()`. |
| `stop drone` / `stop rover` | `[IMPLEMENTED]` | `<family>.stop()`. |
| `drive` / `drive forward` | `[IMPLEMENTED]` | `rover.forward()`; `at speed` supplies a speed. |
| `drive backward` | `[IMPLEMENTED]` | `rover.backward()`; `at speed` supplies a speed. |
| `turn left by degrees` | `[IMPLEMENTED]` | `rover.turn_left(degrees)`. |
| `turn right by degrees` | `[IMPLEMENTED]` | `rover.turn_right(degrees)`. |
| `turn on name` | `[IMPLEMENTED]` | `device.set_relay("name", true)`. |
| `turn off name` | `[IMPLEMENTED]` | `device.set_relay("name", false)`. |
| `pulse name for duration` | `[IMPLEMENTED]` | `device.pulse("name", duration)`. |

## 2. Values and units

### 2.1 Runtime values

| Value | Tag | Creation, access, and behavior |
| --- | --- | --- |
| Number | `[IMPLEMENTED]` | Finite `f64`; for example `12`, `3.5`, or a numeric `--arg`. Arithmetic can raise overflow or division errors. |
| Text | `[IMPLEMENTED]` | UTF-8, for example `"hello"`; text concatenation uses `+`. Indexing and length use Unicode scalar values, not bytes or grapheme clusters. |
| Boolean | `[IMPLEMENTED]` | Source literals `true` and `false`. |
| List | `[IMPLEMENTED]` | `[1, "a", true]` or a JSON array argument; ordered, zero-based, and heap-backed. |
| Record/map | `[PARTIAL]` | A JSON object argument or host can supply a record. Read `name.field`; no record literal or field assignment. |
| Point/vector | `[IMPLEMENTED]` | `vector(x, y)` or `vector(x, y, z)`. Read indexes `0`, `1`, `2` or named components below. |
| Nothing/unit | `[IMPLEMENTED]` | Runtime value from an empty return, a missing field, JSON `null`, or a missing parameter. No general `nothing` source literal. |
| Error record | `[IMPLEMENTED]` | In a recovery block, `.code` and `.message` are text and `.line` is a number. |

| Point component | Tag | Equivalent names |
| --- | --- | --- |
| First component | `[IMPLEMENTED]` | `.x`, `.lat`, `.latitude`, or `[0]`. |
| Second component | `[IMPLEMENTED]` | `.y`, `.lon`, `.longitude`, or `[1]`. |
| Third component | `[IMPLEMENTED]` | `.z`, `.alt`, `.altitude`, or `[2]`. |
| Geographic place | `[IMPLEMENTED]` | Spatial distance/bearing checks latitude `-90..90` and longitude `-180..180`; invalid places raise `E_KAZE_RANGE`. |

### 2.2 Numeric literal units

| Unit | Tag | Accepted suffixes | Representation |
| --- | --- | --- | --- |
| Distance | `[IMPLEMENTED]` | `m`, `meter`, `meters` | Numeric value in meters. |
| Speed | `[IMPLEMENTED]` | `m/s` | Numeric value in meters per second. |
| Angle | `[IMPLEMENTED]` | `deg`, `degree`, `degrees` | Numeric value in degrees. |
| Time | `[IMPLEMENTED]` | `s`, `sec`, `second`, `seconds` | Numeric value in seconds. |
| Milliseconds | `[IMPLEMENTED]` | `ms`, `millisecond`, `milliseconds` | Normalized to seconds: `500 ms` becomes `0.5`. |
| Percentage | `[IMPLEMENTED]` | `%`, `percent` | Whole-number convention: `20%` is `20`, not `0.2`. |
| Dimensional mismatch checking | `[PARTIAL]` `[VERIFIED]` | Any unit combination | The current checker accepts `1 meters < 1 seconds`; units are not retained as runtime tags. Do not assume dimensional safety. |
| Arbitrary/user-defined units | `[UNSUPPORTED]` | — | No general unit declaration or conversion system. |

## 3. Routines, entries, and scheduling

| Item | Tag | Behavior |
| --- | --- | --- |
| Action | `[IMPLEMENTED]` | A named `action` may return a value or `nothing`. |
| Check | `[IMPLEMENTED]` | A named `check` is selected and bound like an action; it has no separate mandatory boolean-return rule. |
| Filter | `[PARTIAL]` `[VERIFIED]` | Intended to stop at `allow` or `reject`. A plain `return` also passes the current checker and can yield a null verdict. |
| Explicit entry | `[IMPLEMENTED]` `[VERIFIED]` | `--entry name` selects a named item, including an imported item. Missing names give `E_KAZE_ENTRY`. |
| Root `main` | `[IMPLEMENTED]` | Without `--entry`, a `main` from the root file is preferred. |
| Single root routine | `[IMPLEMENTED]` | Without root `main`, the sole root action/check/filter is selected. |
| Multiple root routines | `[IMPLEMENTED]` | Without an explicit entry or root `main`, ambiguous selection raises `E_KAZE_ENTRY`. |
| Hooks-only root | `[IMPLEMENTED]` | Can run with no entry; hooks require ticks to execute. |
| Imported implicit entry | `[UNSUPPORTED]` | Imported routines do not become the implicit root entry. |
| Parameters | `[PARTIAL]` | Bound by name or numeric key in `--arg`. Missing values become `nothing` instead of an early missing-parameter diagnostic. |
| `when` hook | `[IMPLEMENTED]` | Predicate runs for each requested tick; body runs when true. |
| `every` hook | `[IMPLEMENTED]` | Runs when period divides virtual tick time. Time begins at `0`, so a positive period is initially due. |
| Tick pacing | `[IMPLEMENTED]` | `--pace` sleeps between ticks; without it scheduling advances virtual time without a sleep. |
| Hook state | `[PARTIAL]` | VM locals/stack are recreated for each item; arena and heap are cleared after each tick. Host state can remain separate. |
| Routine calls | `[UNSUPPORTED]` | A routine cannot directly invoke another routine using ordinary call syntax. |
| Runtime imports / namespaces | `[UNSUPPORTED]` | `use` is an include at compilation, with a shared global item namespace. |
| Import cycle / collision | `[IMPLEMENTED]` | Cycles and duplicate item names fail. Shared dependencies are included once. |

### Example: pure action

```kaze
action main(amount):
    let total = 0
    for each value in [1, 2, 3]:
        set total to total + value
    return total + amount
```

`kaze run example.kaze --arg amount=4 --plain` returns `10`.

### Example: filter and error recovery

```kaze
needs: policy

filter limit_length(text):
    if length of text > 120:
        reject with "too long"
    otherwise:
        allow text
```

```kaze
needs: drone

action recover():
    try:
        arm drone
    otherwise with problem:
        return problem.code

when link lost for 3 seconds:
    return to launch
```

The examples were checked without executing their capabilities. Actual drone
operations need an authorized, configured backend.

## 4. Capabilities

### 4.1 Gate and backend tags

Every operation below requires a matching `needs:` declaration, an explicit
host grant, and a valid policy. Selected physical arguments are checked before
dispatch. An operation's tag describes the built-in standalone host, not every
possible custom `CapHost` supplied by an embedding application.

| Gate | Tag | Current behavior |
| --- | --- | --- |
| H-1: declaration | `[IMPLEMENTED]` | An undeclared family is rejected; the checker can reject it before execution. |
| H-2: grant | `[IMPLEMENTED]` `[POST-BASELINE]` `[VERIFIED]` | Requires an explicit host grant for every capability family. |
| H-3: policy | `[IMPLEMENTED]` `[POST-BASELINE]` `[VERIFIED]` | Missing, malformed, unsupported, or denying policy causes `E_CAP_DENIED`. `--no-policy` does not bypass this gate. |
| H-4: arguments | `[PARTIAL]` | Selected physical arguments have type/range checks and are rejected rather than clamped; coverage is not universal. |
| Default host | `[IMPLEMENTED]` `[POST-BASELINE]` | No physical or simulated backend is selected implicitly. Missing backends return `E_CAP_MISSING`. |
| Simulator selection | `[IMPLEMENTED]` `[POST-BASELINE]` `[VERIFIED]` | `drone=sim` and `rover=sim` must be selected explicitly for testing; run reports identify selected simulators. |
| Live host fallback | `[IMPLEMENTED]` `[POST-BASELINE]` `[VERIFIED]` | Unconfigured sensor, device, alert, memory, AI, and tool operations return unavailable errors rather than simulated success. |
| Link-loss gate | `[IMPLEMENTED]` `[POST-BASELINE]` `[VERIFIED]` | Link monitoring checks the drone declaration, grant, policy, and backend before reading the host. |
| Lazy live link | `[IMPLEMENTED]` `[POST-BASELINE]` `[VERIFIED]` | The live drone connection opens only after the relevant VM gate approves the operation. |
| Live telemetry freshness | `[PARTIAL]` `[TARGET]` | The live link uses a two-second timeout on received bytes, not a 1.5-second heartbeat and per-field telemetry freshness veto. Partial targets can use a previously reported value whose age is unknown. The `RULES.md` safety requirement is not yet met. |
| Live drone actuation | `[POST-BASELINE]` `[VERIFIED]` `[UNSUPPORTED]` | The built-in host returns `E_CAP_DENIED` for live actuation before opening the link until the required freshness checks are implemented. Explicit drone simulation remains available for testing. |
| Physical hardware execution | `[UNVERIFIED]` | The source has MAVLink and Linux GPIO/PWM paths; this inventory did not run a vehicle or device. |

### 4.2 Drone operations

These require `needs: drone`, a `drone` grant and a valid policy. MAVLink
actuation code exists but is disabled by the built-in host until freshness
checks are implemented. The explicitly selected simulator is for testing only;
the live path was not physically verified here.

| Operation | Tag | Arguments and behavior |
| --- | --- | --- |
| `drone.arm()` | `[LIVE-PATH]` `[UNVERIFIED]` | No arguments; request arming and wait for armed telemetry on the live path. |
| `drone.takeoff(height)` | `[LIVE-PATH]` `[UNVERIFIED]` | One number in meters; gate checks `0..500`. |
| `drone.goto(target)` | `[LIVE-PATH]` `[UNVERIFIED]` `[PARTIAL]` | One point or numeric latitude. The currently disabled partial live-target code requires previously reported telemetry rather than fabricated coordinates or altitude; freshness is not checked. |
| `drone.goto(target, altitude)` | `[LIVE-PATH]` `[UNVERIFIED]` | Two arguments; supported point/altitude or numeric forms have backend-specific interpretation. |
| `drone.goto(lat, lon, altitude)` | `[LIVE-PATH]` `[UNVERIFIED]` | Three numeric arguments; latitude is checked `-90..90`, longitude `-180..180`, and altitude `0..500` meters. |
| `drone.land()` | `[LIVE-PATH]` `[UNVERIFIED]` | No arguments; request landing. |
| `drone.rtl()` | `[LIVE-PATH]` `[UNVERIFIED]` | No arguments; request return to launch. |
| `drone.hold()` | `[LIVE-PATH]` `[UNVERIFIED]` | No arguments; request hold/loiter. |
| `drone.stop()` | `[LIVE-PATH]` `[UNVERIFIED]` | No arguments; live path maps to hold/loiter. |
| `drone.telemetry(field)` | `[LIVE-PATH]` `[UNVERIFIED]` | One field name; live telemetry can report a link error if no reading arrives. |

### 4.3 Rover and sensor operations

These require a family declaration and grant. Rover simulation requires an
explicit testing selection; no built-in live rover or sensor backend has been
established. Sensor calls now fail rather than return simulated readings.

| Operation | Tag | Arguments and behavior |
| --- | --- | --- |
| `rover.forward()` | `[SIMULATED]` | No speed argument; simulator uses its own default. |
| `rover.forward(speed)` | `[SIMULATED]` | One speed; supplied value is checked `0..10` m/s. |
| `rover.backward()` | `[SIMULATED]` | No speed argument; simulator uses its own default. |
| `rover.backward(speed)` | `[SIMULATED]` | One speed; supplied value is checked `0..10` m/s. |
| `rover.turn_left(angle)` | `[SIMULATED]` | One angle in degrees. |
| `rover.turn_right(angle)` | `[SIMULATED]` | One angle in degrees. |
| `rover.stop()` | `[SIMULATED]` | No arguments. |
| `rover.telemetry(field)` | `[SIMULATED]` | One field name. |
| `sensor.temperature()` | `[REGISTRY-ONLY]` | No arguments; built-in host returns `E_CAP_MISSING`. |
| `sensor.humidity()` | `[REGISTRY-ONLY]` | No arguments; built-in host returns `E_CAP_MISSING`. |
| `sensor.distance()` | `[REGISTRY-ONLY]` | No arguments; built-in host returns `E_CAP_MISSING`. |
| `sensor.front_distance()` | `[REGISTRY-ONLY]` | No arguments; built-in host returns `E_CAP_MISSING`. |
| `sensor.read(name)` | `[REGISTRY-ONLY]` | One name or pin; built-in host returns `E_CAP_MISSING`. |
| `sensor.telemetry(field)` | `[REGISTRY-ONLY]` | One field name; built-in host returns `E_CAP_MISSING`. |

### 4.4 Device operations

These require `needs: device` or a device alias and a `device` grant.

| Operation | Tag | Arguments and behavior |
| --- | --- | --- |
| `device.set_relay(name, on)` | `[LIVE-PATH]` `[UNVERIFIED]` | Name and boolean or finite numeric state; live Linux GPIO requires a named pin. |
| `device.pulse(name, seconds)` | `[LIVE-PATH]` `[UNVERIFIED]` | Name and duration; gate checks `0..60` seconds. |
| `device.read_gpio(name)` | `[LIVE-PATH]` `[PARTIAL]` `[UNVERIFIED]` | One name; live path returns a boolean, while the checker currently infers a number. The mismatch can reject an `if` condition. |
| `device.set_duty(name, percent)` | `[LIVE-PATH]` `[UNVERIFIED]` | Name and PWM percentage; gate checks `0..100`. A named PWM channel is required for the live PWM path. |
| Missing device mapping | `[IMPLEMENTED]` `[POST-BASELINE]` | Without a suitable live GPIO/PWM mapping, device operations return `E_CAP_MISSING`. |

### 4.5 Memory, policy, logging, alerts, AI, and tools

| Operation | Tag | Arguments and current behavior |
| --- | --- | --- |
| `memory.save(label, value)` | `[REGISTRY-ONLY]` | Two arguments; the standalone built-in host returns `E_CAP_MISSING`. |
| `memory.recall(query)` | `[REGISTRY-ONLY]` | One argument; the standalone built-in host returns `E_CAP_MISSING`. |
| `policy.detects(kind, text)` | `[PARTIAL]` | Two arguments; the built-in host uses a small local pattern detector for `secrets` and `prompt_injection`; unknown kinds return `E_CAP_MISSING`. |
| `policy.detect(kind, text)` | `[PARTIAL]` | Alias operation with the same built-in detection behavior. |
| `policy.allow()` | `[IMPLEMENTED]` | VM records an allow verdict with `nothing`. |
| `policy.allow(value)` | `[IMPLEMENTED]` | VM records an allow verdict with the rendered value. |
| `policy.reject(reason)` | `[IMPLEMENTED]` | VM records a reject verdict with the reason. |
| `log.info(value)` | `[IMPLEMENTED]` | One argument; captured in the run report's logs. |
| `log.warn(value)` | `[IMPLEMENTED]` | One argument; captured at warning level. |
| `log.error(value)` | `[IMPLEMENTED]` | One argument; captured at error level. |
| `log.debug(value)` | `[IMPLEMENTED]` | One argument; captured at debug level. |
| `alert.send(text)` | `[REGISTRY-ONLY]` | One argument; the built-in host returns `E_CAP_MISSING` instead of pretending to deliver. |
| `ai.prompt(text)` | `[POST-BASELINE]` `[REGISTRY-ONLY]` | One argument is recognized in this source snapshot; no built-in inference handler. |
| `ai.complete(text)` | `[POST-BASELINE]` `[REGISTRY-ONLY]` | One argument is recognized in this source snapshot; no built-in inference handler. |
| `ai.classify(text, classes)` | `[POST-BASELINE]` `[REGISTRY-ONLY]` | Two arguments are recognized in this source snapshot; no built-in classifier handler. |
| `tool.call(name)` | `[POST-BASELINE]` `[REGISTRY-ONLY]` | One argument is recognized in this source snapshot; no built-in tool handler. |
| `tool.call(name, input)` | `[POST-BASELINE]` `[REGISTRY-ONLY]` | Two arguments are recognized in this source snapshot; no built-in tool handler. |
| `tool.run(name)` | `[POST-BASELINE]` `[REGISTRY-ONLY]` | One argument is recognized in this source snapshot; no built-in tool handler. |
| `tool.run(name, input)` | `[POST-BASELINE]` `[REGISTRY-ONLY]` | Two arguments are recognized in this source snapshot; no built-in tool handler. |
| MCP client/tool execution | `[UNSUPPORTED]` | No working built-in standalone Kaze MCP integration was established. |

### 4.6 Aliases and telemetry

| Name | Tag | Resolution |
| --- | --- | --- |
| `lidar`, `camera`, `thermostat`, `probe` | `[IMPLEMENTED]` | Resource aliases resolve to the `sensor` family. |
| `fan`, `mister`, `pump`, `relay`, `valve`, `motor`, `light` | `[IMPLEMENTED]` | Resource aliases resolve to the `device` family. |
| Other unknown resource names | `[PARTIAL]` | Family resolution falls back to `device`; this does not prove a physical device exists. |
| `tools` | `[POST-BASELINE]` `[REGISTRY-ONLY]` | Resolves to the `tool` family. |
| `agent`, `model`, `llm` | `[POST-BASELINE]` `[REGISTRY-ONLY]` | Resolve to the `ai` family. |
| Bare `battery` | `[PARTIAL]` | Recognized telemetry name; actual reading depends on the backend. |
| Bare `altitude` | `[PARTIAL]` | Recognized telemetry name; actual reading depends on the backend. |
| Bare `heading` | `[PARTIAL]` | Recognized telemetry name; actual reading depends on the backend. |
| Bare `speed` | `[PARTIAL]` | Recognized telemetry name; actual reading depends on the backend. |
| Bare `position` | `[PARTIAL]` | Recognized by the parser; the built-in simulator's telemetry lookup falls back to `0`. |
| Bare `temperature` | `[PARTIAL]` | Recognized telemetry name; the standalone sensor backend is unavailable. |
| Bare `humidity` | `[PARTIAL]` | Recognized telemetry name; the standalone sensor backend is unavailable. |
| Bare `distance` | `[PARTIAL]` | Recognized telemetry name; the standalone sensor backend is unavailable. |
| Bare `armed` | `[PARTIAL]` | Recognized telemetry name; returned representation depends on the backend. |
| Bare `latitude` | `[PARTIAL]` | Recognized telemetry name; actual reading depends on the backend. |
| Bare `longitude` | `[PARTIAL]` | Recognized telemetry name; actual reading depends on the backend. |
| Bare `satellites` | `[PARTIAL]` | Recognized by the parser; the built-in simulator's telemetry lookup falls back to `0`. |
| Bare-telemetry resolution | `[PARTIAL]` | Each source file must identify exactly one hardware resource resolving to `drone`, `rover`, or `sensor`; merged imports do not fix ambiguity in that file. |

## 5. Errors and recovery

### 5.1 Individual error codes

`Catchable` means recoverable by a Kaze `try` block **when emitted by a
capability call**. Front-end failures occur before that block runs. CLI option
parsing can also produce unstructured usage text without a Kaze error code.

| Code | Tag | Typical cause | Catchable? |
| --- | --- | --- | --- |
| `E_KAZE_PARSE` | `[IMPLEMENTED]` | Lexer or grammar failure, indentation, malformed literal, or parser nesting limit. | No |
| `E_KAZE_CHECK` | `[IMPLEMENTED]` | Semantic check failure, undefined name, capability declaration, arity, or filter termination. | No |
| `E_KAZE_COMPILE` | `[IMPLEMENTED]` | Compiler failure, including excessive item slots or list literal size. | No |
| `E_KAZE_IMPORT` | `[IMPLEMENTED]` | Import cycle, collision, or invalid import resolution condition. | No |
| `E_KAZE_ENTRY` | `[IMPLEMENTED]` | Missing explicitly selected item or ambiguous default routine. | No |
| `E_KAZE_IO` | `[IMPLEMENTED]` | Source or imported-file I/O failure. | No |
| `E_KAZE_FMT` | `[IMPLEMENTED]` | Formatter failure or refusal to preserve source token meaning. | No |
| `E_KAZE_BADCODE` | `[IMPLEMENTED]` | Invalid, incompatible, truncated, or unverifiable bytecode. | No |
| `E_KAZE_TRAP` | `[IMPLEMENTED]` | An executed bytecode trap. | No |
| `E_KAZE_TYPE` | `[IMPLEMENTED]` | Invalid value type for an operation, field, index, or checked hardware argument. | No |
| `E_KAZE_RANGE` | `[IMPLEMENTED]` | Invalid geographic place or an out-of-range checked physical argument. | No |
| `E_KAZE_BOUNDS` | `[IMPLEMENTED]` | Invalid or out-of-range list, text, or point index. | No |
| `E_KAZE_DIV0` | `[IMPLEMENTED]` | Division or remainder by zero. | No |
| `E_KAZE_MATH` | `[IMPLEMENTED]` | Invalid real math, such as square root of a negative value. | No |
| `E_KAZE_OVERFLOW` | `[IMPLEMENTED]` | Arithmetic or numeric operation produces a non-finite result. | No |
| `E_KAZE_STACK` | `[IMPLEMENTED]` | Operand-stack or active recovery-handler limit exceeded. | No |
| `E_KAZE_MEMORY` | `[PARTIAL]` | Arena or collection allocation fails; argument conversion has separate silent-`nothing` cases. | No |
| `E_TIMEOUT` | `[IMPLEMENTED]` `[VERIFIED]` | Fuel exhaustion or an armed host deadline at a loop poll. | No |
| `E_CAP_UNDECLARED` | `[IMPLEMENTED]` | Required family is absent from `needs:` at the runtime gate. | No |
| `E_CAP_MISSING` | `[IMPLEMENTED]` `[POST-BASELINE]` | Any required family grant or configured backend is absent; unavailable built-in operations also return this code. | No |
| `E_CAP_DENIED` | `[IMPLEMENTED]` `[POST-BASELINE]` | A policy is missing, malformed, unsupported, or denies the call; live drone actuation is disabled pending safety checks. | No |
| `E_SAFETY_VIOLATION` | `[IMPLEMENTED]` | Safety error forwarded from an applicable host path. | No |
| `E_KAZE_LINK` | `[IMPLEMENTED]` | Link connection, device configuration, or live I/O failure. | Yes, from a capability call |
| `E_KAZE_LINK_TIMEOUT` | `[IMPLEMENTED]` | Live link did not respond within its applicable timeout. | Yes, from a capability call |
| `E_COMMAND_REJECTED` | `[IMPLEMENTED]` | A vehicle/backend rejected a command. | Yes, from a capability call |
| `E_ARMING_DENIED` | `[IMPLEMENTED]` | The vehicle did not reach the armed state or denied arming. | Yes, from a capability call |
| `E_KAZE_HOST` | `[IMPLEMENTED]` | Explicitly classified operational host failure. | Yes, from a capability call |
| `E_KAZE_HOST_UNCLASSIFIED` | `[IMPLEMENTED]` `[POST-BASELINE]` | An unknown or untyped host failure is not silently treated as a recoverable operational error. | No |

### 5.2 Error behavior

| Item | Tag | Behavior |
| --- | --- | --- |
| `try` recovery set | `[IMPLEMENTED]` | Exactly `E_KAZE_LINK`, `E_KAZE_LINK_TIMEOUT`, `E_COMMAND_REJECTED`, `E_ARMING_DENIED`, and `E_KAZE_HOST` are catchable from capability calls. |
| Unknown host error codes | `[IMPLEMENTED]` `[POST-BASELINE]` `[VERIFIED]` | Unknown or untyped host failures become `E_KAZE_HOST_UNCLASSIFIED`, which is not catchable. |
| Stack unwind | `[IMPLEMENTED]` | A caught error unwinds the operand stack and restores the verdict that existed at `try` entry; local changes are not rolled back. |
| Error location | `[PARTIAL]` | Parser errors have line/column; other failures may report an item line or line `0` where finer source location is unavailable. |
| JSON run success | `[IMPLEMENTED]` | `--json` produces a run report containing entry, value, actions, logs, instruction/fuel counts, ticks, optional verdict, and `simulated_families`. |
| JSON run failure | `[PARTIAL]` | `kaze run --json` still prints failures as text to stderr instead of a corresponding JSON diagnostic object. |
| CLI exits | `[IMPLEMENTED]` | Successful `run` exits `0`; execution/front-end errors exit `1`; usage/flag errors exit `2`. |
| Required parameter diagnostics | `[TARGET]` `[PARTIAL]` | Repository policy requires structured guidance, but standalone Kaze can bind missing input to `nothing` and later raise `E_KAZE_TYPE`. |

## 6. Execution limits and bytecode

| Limit or property | Tag | Current behavior |
| --- | --- | --- |
| Instruction fuel | `[IMPLEMENTED]` `[VERIFIED]` | Default `200,000` units shared by entry and hooks in one run; `--fuel` changes it. A requested `0` becomes `1`. |
| String arena | `[IMPLEMENTED]` | Default `256 KiB` of string bytes; runtime allocation can raise `E_KAZE_MEMORY`. |
| Collection heap | `[PARTIAL]` | Default `65,536` stored values; this counts collection values, not all allocation bytes or empty collection objects. |
| Operand stack | `[IMPLEMENTED]` | Maximum `256` values. |
| Active `try` handlers | `[IMPLEMENTED]` | Maximum `32` active recovery handlers per item. |
| Parser expression nesting | `[IMPLEMENTED]` | Maximum `64` guarded expression levels. |
| Parser block nesting | `[IMPLEMENTED]` | Maximum `64` block levels. |
| Item local slots | `[IMPLEMENTED]` | Up to `255` compiled slots per item, including parameters and compiler-generated temporaries. |
| List literal elements | `[IMPLEMENTED]` | Up to `255` elements in one source literal. |
| Capability-call arguments | `[IMPLEMENTED]` | Bytecode encodes at most `255`; each registered operation usually has a much smaller arity in §4. |
| Link silence window | `[IMPLEMENTED]` | Between `1` and `4,294,967,295` milliseconds; larger/shorter values are refused. |
| Ticks | `[IMPLEMENTED]` | Default `0`; `--ticks` accepts an unsigned 32-bit value. |
| Tick interval | `[IMPLEMENTED]` | Default `100` ms; `--tick-ms` accepts an unsigned 64-bit value. |
| Tick cleanup | `[IMPLEMENTED]` | String arena and collection heap are cleared after each supervisor tick; VM values do not persist across tick boundaries. |
| Blocking host call | `[PARTIAL]` | Fuel counts VM instructions, not time spent inside a blocking host call. |
| Host deadline | `[PARTIAL]` | An embedding host can arm the process-global deadline; loop-head polling checks it. The CLI does not expose a general deadline flag. |
| `.kazebc` format | `[IMPLEMENTED]` `[VERIFIED]` | Standalone version `4`; a compiled module contains flattened items, not runtime imports. |
| Bytecode verifier | `[IMPLEMENTED]` | Deserialization and execution validate structure, opcodes, indices, jumps, capability arity, and stack flow before execution. |
| Deterministic artifact | `[IMPLEMENTED]` `[VERIFIED]` | Repeated compilation of the same source/layout produced identical bytes locally; recorded external absolute paths can affect artifact origins. |
| Sofuu embedded artifact | `[UNSUPPORTED]` | Embedded source format `2` (and legacy format `1`) and standalone format `4` are not directly interchangeable. |
| Security compatibility | `[POST-BASELINE]` | The new gate and host defaults change which previously accepted runs succeed; bytecode format remains `4`. A release/version compatibility decision remains open. |

## 7. Configuration and command line

### 7.1 Commands

| Command | Tag | Behavior |
| --- | --- | --- |
| `kaze run <file.kaze>` | `[IMPLEMENTED]` | Parse, resolve imports, check, compile, verify, and execute source. |
| `kaze run <file.kazebc>` | `[IMPLEMENTED]` `[VERIFIED]` | Deserialize, verify, and execute standalone format-4 bytecode. |
| `kaze check <file.kaze>` | `[IMPLEMENTED]` | Prepare/check source and imports without executing the program. |
| `kaze compile <file.kaze>` | `[IMPLEMENTED]` | Emit `.kazebc`; `--out` chooses the path. |
| `kaze fmt <file.kaze>` | `[IMPLEMENTED]` | Print formatted source; `--check` and `--write` change formatter behavior. |
| `kaze bench <file>` | `[IMPLEMENTED]` | Repeatedly run a prepared source or module and report timing; run only with policy-compliant workloads. |
| `kaze selftest` | `[IMPLEMENTED]` `[POST-BASELINE]` | Run the scorecard using pure computation, bytecode validation, and gate refusals; it no longer implicitly runs a simulated mission. |
| `kaze repl` | `[IMPLEMENTED]` | Interactive buffer and commands, subject to the same backend limitations. |
| `kaze version` | `[IMPLEMENTED]` `[VERIFIED]` | Print `kaze 1.0`. |
| `kaze --version` | `[IMPLEMENTED]` `[VERIFIED]` | Alternative version command. |
| `kaze --help` | `[IMPLEMENTED]` | Print CLI help. |
| `kaze-lsp` | `[IMPLEMENTED]` | Separate language-server binary over stdio; see [TOOLING.md](TOOLING.md). |

### 7.2 CLI options

| Option | Tag | Default / meaning |
| --- | --- | --- |
| `--entry NAME` | `[IMPLEMENTED]` | Entry name; default uses root `main`, then sole root routine. |
| `--arg NAME=VALUE` | `[IMPLEMENTED]` `[VERIFIED]` | Repeatable argument. Parse value as JSON when possible; otherwise use text. |
| `--arg 0=VALUE` | `[IMPLEMENTED]` | Numeric key binds by zero-based parameter position if no named binding takes precedence. |
| `--cap FAMILY` | `[IMPLEMENTED]` `[POST-BASELINE]` | Repeatable family grant. It does not implicitly select a simulator or provide a missing backend. |
| `--cap FAMILY=BACKEND` | `[PARTIAL]` | Grant and choose a backend; the standalone live host handles `drone` and `device`, with specific mapping requirements. |
| `--cap drone=sim` | `[SIMULATED]` `[POST-BASELINE]` | Explicitly select the labelled drone simulator for testing. |
| `--cap rover=sim` | `[SIMULATED]` `[POST-BASELINE]` | Explicitly select the labelled rover simulator for testing. |
| Drone UDP backend | `[LIVE-PATH]` `[UNVERIFIED]` | `--cap drone=udp:HOST:PORT` binds a local UDP endpoint for MAVLink. |
| Drone UART backend | `[LIVE-PATH]` `[UNVERIFIED]` | `--cap drone=uart:/dev/device@baud`; omitting `@baud` selects `57600`. Requires the `uart` build feature. |
| `--pin NAME=LINE` | `[LIVE-PATH]` `[UNVERIFIED]` | Map a name to an unsigned 16-bit Linux GPIO line. |
| `--pwm NAME=CHIP:CHANNEL` | `[LIVE-PATH]` `[UNVERIFIED]` | Map a name to unsigned 16-bit PWM chip/channel numbers. |
| `--fuel N` | `[IMPLEMENTED]` | Unsigned instruction budget; default `200000`, with a minimum effective value of `1`. |
| `--ticks N` | `[IMPLEMENTED]` | Unsigned tick count; default `0`. |
| `--tick-ms N` | `[IMPLEMENTED]` | Unsigned tick interval in milliseconds; default `100`. |
| `--pace` | `[IMPLEMENTED]` | Sleep between ticks; disabled by default. |
| `--sim K=V[,K=V]` | `[SIMULATED]` | Repeatable simulator-state overrides; requires an explicit `drone=sim` or `rover=sim` testing backend. |
| `--no-policy` | `[IMPLEMENTED]` `[POST-BASELINE]` | Disables loading the policy file; capability calls still fail closed with `E_CAP_DENIED`. Pure computation can run. |
| `--json` | `[IMPLEMENTED]` | JSON success report for `run`, timings for `bench`, and scorecard output where supported. |
| `--plain` | `[IMPLEMENTED]` | Plain-text run report. |
| `--out PATH`, `-o PATH` | `[IMPLEMENTED]` | Output path for `compile`. |
| `--disasm` | `[IMPLEMENTED]` | Print compiled module disassembly instead of writing the artifact. |
| `--check` | `[IMPLEMENTED]` | For `fmt`, report whether input is already formatted. |
| `--write`, `-w` | `[IMPLEMENTED]` | For `fmt`, write formatted source to the file. |
| `--iterations N` | `[IMPLEMENTED]` | For `bench`, number of iterations; default `20`, minimum effective value `1`. |
| `--help`, `-h`, `help` | `[IMPLEMENTED]` | Help flag recognized by the shared flag parser; command-specific help behavior varies. |
| `--config PATH` | `[UNSUPPORTED]` | The standalone `kaze` CLI has no agent configuration-file option. Its flag parser treats unrecognized tokens as positionals rather than rejecting every unknown flag. |
| Positional routine values | `[UNSUPPORTED]` | The CLI uses the first positional as the source/artifact path; routine values use `--arg`. |

### 7.3 Argument, policy, and environment configuration

| Setting | Tag | Current behavior |
| --- | --- | --- |
| Named JSON argument | `[IMPLEMENTED]` | `--arg amount=4`, `--arg enabled=true`, `--arg 'items=[1,2]'`, and `--arg 'item={"x":1}'` become the corresponding values. |
| Text argument | `[IMPLEMENTED]` | If `VALUE` is not valid JSON, its raw text becomes a string. Quote shell arguments as needed. |
| Duplicate argument key | `[IMPLEMENTED]` | The later `--arg` replaces the earlier value for the same key. |
| Named vs numeric key | `[IMPLEMENTED]` | For a parameter, its named key is checked before its zero-based numeric key. |
| Missing argument | `[PARTIAL]` `[VERIFIED]` | Becomes `nothing`; later use can fail with `E_KAZE_TYPE`. No early structured missing-input guidance. |
| Argument allocation failure | `[PARTIAL]` | Conversion can substitute `nothing` when allocating an argument fails. |
| `policy.json` | `[IMPLEMENTED]` `[POST-BASELINE]` | The CLI reads this file from the current working directory. A valid file is required for capability calls; pure computation does not require one. |
| Policy deny entry | `[IMPLEMENTED]` `[POST-BASELINE]` | `deny` must be an array of known `family.operation`, `family.*`, or `*` patterns; `drone.link_lost` is also recognized. |
| Malformed policy | `[POST-BASELINE]` `[IMPLEMENTED]` | The gate rejects missing/empty objects, malformed JSON, unknown fields, non-text entries, and unknown deny patterns; CLI parsing also rejects duplicate keys. |
| `KAZE_GPIO_ROOT` | `[LIVE-PATH]` | Override the Linux GPIO sysfs root used by the hardware bus. |
| `KAZE_GPIO_CHIP` | `[LIVE-PATH]` | Override the GPIO character-device path used by applicable hardware code. |
| `KAZE_PWM_ROOT` | `[LIVE-PATH]` | Override the Linux PWM sysfs root used by the hardware bus. |
| UART feature | `[IMPLEMENTED]` | Serial transport requires building with the `uart` Cargo feature. |
| GPIO character-device feature | `[IMPLEMENTED]` | Character-device GPIO is behind the `chardev` Cargo feature. |
| `AGENT_<PARAM>` routine binding | `[UNSUPPORTED]` `[TARGET]` | Required by repository policy for standalone agent binaries; not implemented by this standalone `kaze` CLI. |
| `SOFUU_*` routine binding | `[UNSUPPORTED]` `[TARGET]` | Part of the repository's standalone-agent configuration target, not standalone `kaze` CLI parameter resolution. |
| `./agent.config.json` | `[UNSUPPORTED]` `[TARGET]` | Part of the standalone-agent target; not loaded for Kaze CLI routine binding. |
| Signature defaults | `[UNSUPPORTED]` `[TARGET]` | The repository lists signatures at the bottom of the intended precedence hierarchy; current standalone signatures have no defaults. |
| Intended agent precedence | `[TARGET]` | CLI flags/positionals > `AGENT_<PARAM>` / `SOFUU_*` environment > config file > routine signature. A Sofuu-compiled agent is a separate implementation and must be verified separately. |

### 7.4 Simulator state options

All entries in this table are `[SIMULATED]` and are not real telemetry. Their
use is constrained by `RULES.md` §1.9. These are documented to describe existing
source behavior, not as a recommendation for non-robotics execution.

| `--sim` key | Tag | Default / interpretation |
| --- | --- | --- |
| `battery` | `[SIMULATED]` | `100`. |
| `armed` | `[SIMULATED]` | `false`; `true`, `1`, or `yes` are treated as true. |
| `on_ground` | `[SIMULATED]` | `true`; same boolean parsing as `armed`. |
| `altitude` | `[SIMULATED]` | `0` meters. |
| `lat` | `[SIMULATED]` | `0`. |
| `lon` | `[SIMULATED]` | `0`. |
| `heading` | `[SIMULATED]` | `0`. |
| `speed` | `[SIMULATED]` | `0`. |
| `temperature` | `[SIMULATED]` | `22`. |
| `humidity` | `[SIMULATED]` | `45`. |
| `distance` | `[SIMULATED]` | `2`. |
| `front_distance` | `[SIMULATED]` | Alias that sets simulated distance. |
| `link_lost_after` | `[SIMULATED]` | No dropout by default; accepts a nonnegative millisecond time. |
| `link-lost-after` | `[SIMULATED]` | Alias for `link_lost_after`. |
| `link_lost` | `[SIMULATED]` | Alias for `link_lost_after`. |

### 7.5 REPL commands

| Command | Tag | Behavior |
| --- | --- | --- |
| `.help` | `[IMPLEMENTED]` | Show REPL command help. |
| `.needs family, ...` | `[IMPLEMENTED]` | Add declarations used when the buffer has no `needs:` header. |
| `.cap family` | `[IMPLEMENTED]` `[POST-BASELINE]` | Grant the family; this does not select a simulator. The REPL reads `policy.json` for capability calls. |
| `.cap drone=sim` / `.cap rover=sim` | `[SIMULATED]` `[POST-BASELINE]` | Explicitly grant and select a labelled robotics simulator for testing. |
| `.run [routine]` | `[PARTIAL]` | Run buffered source with optional entry; default run options do not bind routine arguments. |
| `.clear` | `[IMPLEMENTED]` | Clear the buffer, declarations, and grants. |
| `.list` | `[IMPLEMENTED]` | Print declarations and buffered source. |
| `.exit`, `.quit` | `[IMPLEMENTED]` | Leave the REPL. |

## 8. Evidence and maintenance

| Evidence | Tag | Scope |
| --- | --- | --- |
| Offline build | `[VERIFIED]` | The current source compiled locally into a temporary target directory without downloading dependencies or model weights. |
| Pure language probes | `[VERIFIED]` | Version, named arguments, loops, arithmetic, points, mixed-unit behavior, literal modulo ambiguity, missing inputs, fuel, imports, and format-4 artifacts. |
| Capability safety tests | `[VERIFIED]` `[POST-BASELINE]` | Local logging, policy/grant refusals, unavailable backends, link-loss gating, physical-argument refusals, lazy link setup, and disabled live drone actuation were checked; drone and rover simulation were explicitly selected and labelled. |
| Compatibility and language tests | `[VERIFIED]` `[POST-BASELINE]` | Selected format-4 compatibility, module, REPL, benchmark, standard-library, and language tests passed with explicit policy and grants; robotic simulation was explicitly selected and labelled. |
| Capability examples | `[VERIFIED]` | Filter and drone recovery examples, all 40 registry operations in expression position, and the dotted-log statement limitation were checked by the front end only; their capabilities were not executed. |
| Real external operations | `[UNVERIFIED]` | No AI, MCP, external tool, memory backend, or physical hardware was exercised for this inventory. |
| Full release gates | `[UNVERIFIED]` `[PARTIAL]` | The full test, contract, and preflight suites include non-robotics stand-ins that must be migrated before they can run under `RULES.md` §1.9. Distribution artifacts have existing verification failures. |
| Published build | `[UNVERIFIED]` | This source snapshot is not proof of the contents of any published binary. |

Primary implementation sources: [lexer](../src/lexer.rs),
[parser](../src/parser.rs), [AST](../src/ast.rs), [checker](../src/checker.rs),
[compiler](../src/compiler.rs), [values](../src/value.rs), [VM](../src/vm.rs),
[capability registry](../src/caps.rs), [hardware gate](../src/hardware.rs),
[host dispatch](../src/host.rs), [bytecode](../src/isa.rs),
[run API](../src/lib.rs), and [CLI](../src/main.rs). The existing
[specification](SPEC.md), [module reference](MODULES.md),
[compatibility notes](COMPATIBILITY.md), and [tooling reference](TOOLING.md)
provide additional context; where they differ, the gaps are tagged above.
