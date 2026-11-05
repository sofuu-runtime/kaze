# Zero-Panic Audit (v0.7)

## 1. Policy

A **reachable** panic is a bug. The VM must return a typed `KazeError`; a
malformed artifact or a hostile source must be refused, never abort the host.
Programmer invariants on the pinned protocol tables and the formatter's
indentation stack are the only remaining panic-shaped calls, and each one is
named below.

The audit is enforced by evidence, not by a grep alone:

- `tests/fuzz.rs` feeds 2,500 mutated `.kazebc` artifacts through
  `deserialize` → verify → run, and 2,500 random source strings through
  parse → check → compile → verify → run → format. A panic fails the test.
- `tests/verifier.rs` proves the structural and stack bounds before execution.
- `kaze selftest` proves the safety refusals still fire.

## 2. Reachable sites converted

| Site | Was | Now |
| :--- | :--- | :--- |
| `host.rs` live drone dispatch | `Option::take().expect("checked above")` | `if let Some(link) = …` |
| `host.rs` live GPIO dispatch | same | `if let Some(bus) = …` |
| `link.rs` UART read | `pop_front().expect("length counted")` | `if let Some(byte)` |
| `link.rs` ACK correlation | `pending.take().expect("checked above")` | `if let Some(…)` |
| `link.rs` ACK timeout | same | `if let Some(…)` |
| `link.rs` STATUSTEXT slice | `payload[1..51.min(len)]` | `payload.get(1..…).unwrap_or(&[])` |
| `compiler.rs` And/Or in arithmetic codegen | `unreachable!()` | typed `CompileError` |
| `vm.rs` arithmetic dispatch | `unreachable!()` | `E_KAZE_BADCODE` |
| `vm.rs` comparison dispatch | `unreachable!()` | `E_KAZE_BADCODE` |
| `parser.rs` recursive descent | unbounded recursion on hostile nesting | 64-level expression/block guards (`E_KAZE_PARSE`) |

## 3. Remaining `expect`s and why they cannot fire

- `mavlink.rs` `msg_heartbeat`, `msg_command_long`, `msg_set_position_target_*`
  call `Frame::new(id, …).expect("… in the table")`. The ids are pinned in
  `MESSAGES`; `crc_extra_table_is_pinned` and the round-trip tests fail if a
  table entry disappears. These are build-time invariants, not input.
- `fmt.rs` lines 85/96/99 call `levels.last().unwrap()`. `levels` is created
  as `vec![0]`, the pop loop stops at length 1, and nothing else removes an
  entry, so it is never empty. The formatter tests exercise comment-only lines,
  dedents and nested brackets, which are the only paths that reach these calls.
- `mavlink.rs` test-only helpers and assertions.
- `pwm.rs`, `gpio_chardev.rs` test fixtures.

`Frame` field accessors are bounds-safe (`u8_at(999) == 0`, etc.), and
`statustext` no longer slices blindly.

## 4. Audit command

```bash
scripts/panic-audit.sh
```

The script stops each file at its `#[cfg(test)]` marker (test modules are at the
bottom of every file here) and skips comment lines. On the v1.0 tree it prints
exactly the six lines named in §3: three `levels.last().unwrap()` calls in
`fmt.rs` and three `Frame::new(...).expect(...)` calls in `mavlink.rs`. A new
line is a review item: either it is unreachable by construction and gets a
comment here, or it becomes a typed error with a test.

## 5. The Rust `panic = "abort"` caveat

This standalone crate does not set `panic = "abort"`; tests use unwinding so a
panic is observed as a test failure. The **Sofuu product** does set
`panic = "abort"` in its release profile. That protects an FFI boundary from
unwinding but means a panic is process death — which is exactly why the
reachable sites above were removed rather than documented as acceptable.
