# Kaze Flight-Readiness Certification (v1.0)

> **Historical evidence, not current certification.** The table below records a
> prior revision. The present working tree has changed capability behavior,
> and full release gates and physical hardware have not been verified. The live
> drone path also lacks the required 1.5-second heartbeat and per-field
> telemetry freshness veto, so live drone actuation is disabled. See
> [the current reference](KAZE-1.0-REFERENCE.md).

This is the **evidence bundle**, not a certificate from an aviation authority.
Every row names the command or test that proves the property and the status of
that evidence on this machine (2026-11-05). Rows marked **HARDWARE** require a
flight controller or a Raspberry Pi 5 and are not claimed until someone runs
the named script and attaches the receipt.

## 1. Scorecard

| # | Property | Evidence | Status |
| :-- | :--- | :--- | :--- |
| 1 | Language parses, checks, compiles; hostile nesting is bounded | `cargo test` (parser/checker/compiler suites), `tests/limits.rs` | PASS |
| 2 | Bytecode structure and stack safety | `tests/verifier.rs`, `isa::verify` on load and run | PASS |
| 3 | Malformed `.kazebc` never panics | `tests/fuzz.rs` (2,500 byte mutations) | PASS |
| 4 | Random source never panics (parser → formatter → VM) | `tests/fuzz.rs` (2,500 source inputs); `docs/ZERO-PANIC.md` | PASS |
| 5 | Formatter preserves meaning | `tests/fmt.rs`, token-stream guard in `fmt.rs` | PASS |
| 6 | Language server transport and features | `tests/lsp.rs` (direct + real binary over stdio) | PASS |
| 7 | Modules resolve, dedup, refuse cycles | `tests/modules.rs` | PASS |
| 8 | Standard library composes | `tests/stdlib.rs` | PASS |
| 9 | H-1 undeclared capability refused | `selftest` runs a hand-built module with empty `needs` and asserts `E_CAP_UNDECLARED` | PASS |
| 10 | H-2 ungranted backend refused | `selftest` check; `tests/live.rs` | PASS |
| 11 | H-3 policy denial refused | `tests/kaze_tests.rs::a_policy_denial_is_never_catchable` | PASS |
| 12 | H-4 range/type refused, never clamped | `tests/kaze_tests.rs`, live gate tests | PASS |
| 13 | Fuel bounds a runaway loop | `tests/kaze_tests.rs`, `selftest` | PASS |
| 14 | Safety/budget errors are not catchable | `tests/kaze_tests.rs` (range, grant, policy, fuel) | PASS |
| 15 | Link-loss failsafe and pacing | `tests/kaze_tests.rs`, `tests/live.rs` silence test | PASS |
| 16 | Packet loss does not desync the link | `tests/live.rs` lossy autopilot; `mavlink.rs` drop test | PASS |
| 17 | Eight-waypoint mission deterministic | `tests/bench.rs`, `kaze bench` | PASS |
| 18 | Artifact bytes frozen | `tests/compat.rs` + `tests/fixtures/kaze-v1.0.kazebc.hex` | PASS |
| 19 | ISA opcode numbers frozen | `tests/verifier.rs::opcode_numbers_are_locked` | PASS |
| 20 | Collection heap is bounded | `tests/limits.rs::the_collection_heap_is_bounded` | PASS |
| 21 | Scorecard itself | `kaze selftest` (11/11) | PASS |
| 22 | SITL eight-waypoint mission | `scripts/sitl/eight_waypoints.sh` | HARDWARE — not executed here |
| 23 | Raspberry Pi 5 soak | `scripts/pi/soak.sh` | HARDWARE — not executed here |

## 2. Reproduce the software rows

```bash
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo run --bin kaze -- selftest
scripts/fmt-check.sh
node editors/vscode/test/lsp.test.js
scripts/certify.sh    # refuses a dirty tree; use --allow-dirty only for a dev receipt
```

The hardware rows are run on the machine that has the hardware:

```bash
ARDUPILOT_SITL=$HOME/ardupilot/build/SITL/bin/arducopter \
  scripts/sitl/eight_waypoints.sh

KAZE_CAP=drone KAZE_DEVICE=uart:/dev/ttyAMA0@57600 \
  KAZE_SOAK_ITERATIONS=100 scripts/pi/soak.sh
```

## 3. What "certified baseline" means here

A mission may call its baseline certified when:

1. every PASS row above is green on the exact revision being flown;
2. the SITL row has a trace showing exit 0 and eight waypoint commands;
3. the Pi 5 soak has a log covering the intended duration, with zero exits and
   the memory/temperature figures reviewed;
4. the operator has read the known limits in `docs/SPEC.md` §8 and
   `docs/BENCHMARKS.md` §6. (The SITL and Pi 5 rows are 22–23.)

It does **not** mean the autopilot, the airframe, the radio link or the
operator's procedures are certified. Kaze is the deterministic scripting layer;
it refuses what it cannot prove, and the pilot remains responsible for the
vehicle.

## 4. Safety invariants in one line each

- A physical range is refused, never clamped (`E_KAZE_RANGE`).
- A capability the host did not grant is refused (`E_CAP_MISSING`).
- A fuel-exhausted loop stops (`E_TIMEOUT`), and `try` cannot catch it.
- A malformed artifact is refused before its first instruction.
- A link-loss recovery may be attempted repeatedly; it never kills the
  supervisor.
