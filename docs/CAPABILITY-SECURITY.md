# Capability security, alert sinks, audit trail and effector isolation (Phase 5)

Status: **implemented in the standalone `kaze/` prototype, verified by the test
names below on this machine (macOS, no autopilot or GPIO hardware).** Nothing
here claims physical-hardware or SITL verification. Where a claim depends on
hardware the row says so.

Phase 5 of the roadmap covers five things. This page records what the code does,
which test proves it, and what is still unverified.

```bash
cargo test --manifest-path kaze/Cargo.toml --test capability_safety --test live --test effector_safety
cargo test --manifest-path kaze/Cargo.toml          # whole crate
scripts/kaze-drift-check.sh --strict                # from the repo root
```

## 1. Zero-ambient authority

Every capability call crosses the same gate, in this order ([hardware.rs](../src/hardware.rs)):

| Stage | Refusal | What it checks |
| :-- | :-- | :-- |
| H-1 | `E_CAP_UNDECLARED` | The family is in the module's `needs:`. |
| H-2 | `E_CAP_MISSING` | The host explicitly granted the family (`--cap`). |
| H-3 | `E_CAP_DENIED` | A policy is present, well formed, and does not deny the pair. |
| H-4 | `E_KAZE_TYPE` / `E_KAZE_RANGE` | Physical arguments are checked, never clamped. |
| H-5 | `E_CAP_DENIED` | The audit sink (when one was asked for) accepted the decision. |

After the gate, [host.rs](../src/host.rs) routes by family and refuses anything
it cannot really serve:

* `drone` needs a live device or an explicitly selected `drone=sim`;
* `device` needs named `--pin` / `--pwm` mappings;
* `alert` needs a sink (`--cap alert=...`);
* `rover` needs an explicit `rover=sim`;
* `log` and `policy` are the only families with a real in-process
  implementation (`IN_PROCESS_FAMILIES`);
* every other family — `sensor`, `memory`, `ai`, `tool` — is refused with the
  family and the operation named in the message. None of them can reach the
  simulator.

**Tests.** `effector_safety.rs`:

* `every_registered_operation_crosses_every_gate_stage` walks `caps::OPS` and
  proves each `(family, op)` is refused at H-1, H-2 (missing grant), H-3
  (missing policy) and H-3 (denying policy), and authorised when all three pass.
* `every_registered_operation_acts_or_fails_closed_with_no_backend` builds a
  host with no backends and calls every registered operation: `log` records a
  real log line, `policy` returns a real detector answer, and every other family
  returns `E_CAP_MISSING`/`E_CAP_DENIED`. A traced action for such a call may
  carry an `error:` result, never a value. A new registry entry without an
  argument recipe fails the sweep (`RECIPE_KEYS == caps::OPS`).
* `a_configured_gpio_and_pwm_backend_is_really_driven` proves the configured
  path is real: the fake sysfs line goes high, a read returns the file's value,
  and 50% writes a 10 000 000 ns duty cycle.
* `the_explicitly_selected_drone_simulator_changes_real_state` proves the
  drone simulator backend is deterministic state (arm then read `armed`, take off
  then read the altitude), not canned output.

## 2. Real alert sinks

`--cap alert=stdout`, `--cap alert=stderr`, or `--cap alert=file:<path>`.
The sink is opened when the host is built, so a bad path fails before the script
runs. Each `send alert` writes exactly one line:

```
2026-11-22T08:12:33.123Z alert <redacted message>
```

* **Ceiling.** The message is capped at `caps::MAX_ALERT_BYTES` (1024). A longer
  message is truncated on a character boundary and ends with `…[truncated]`;
  it is never dropped and never written whole.
* **Redaction.** `caps::redact_secrets` removes the value of `authorization`,
  `bearer`, `password`, `passwd`, `pwd`, `secret`, `token`, `api_key`,
  `api-key`, `apikey`, `access_key`, `client_secret` and `private_key`, and
  any token starting with `sk-`, `ghp_`, `xoxb-`, `AKIA`, `AIza`, `glpat-`,
  `npm_` or `shpat_`. The key name stays; the value becomes `***`. Redaction
  runs *before* truncation, so the tail of a credential cannot survive the cut.
* **The trace is redacted too.** The run report prints call arguments, so
  `alert.send` records its arguments redacted; otherwise a stdout sink and the
  report would leak what the sink just removed.
* **A missing or unwritable sink is a refusal**, not silent success:
  `E_CAP_MISSING: no alert sink is configured; add --cap alert=stdout, ...`.

**Tests.** `effector_safety.rs`:
`an_alert_sink_writes_a_timestamped_line_to_stdout_stderr_and_a_file`,
`an_alert_redacts_credentials_and_stays_under_the_byte_ceiling`,
`an_alert_without_a_sink_is_refused_and_names_the_fix`; unit tests in
[caps.rs](../src/caps.rs) and [host.rs](../src/host.rs) cover the redactor and the
sink itself.

## 3. Structured audit trail

Off by default. Set `KAZE_AUDIT=file:<path>` (a bare path also works) and every
gate decision appends one line. It is configured by the environment because the
CLI argument table (`main.rs`) is outside this change's ownership; a `--audit
<path>` flag would be a one-line addition there.

```
2026-11-22T08:12:33.123Z capability log.info decision=allow reason=H-1..H-3 passed
2026-11-22T08:12:33.456Z capability log.info decision=deny reason=E_CAP_DENIED/H-3/policy
```

* No argument values are ever written, so a credential a script passes to a
  capability cannot reach the trail; the reason string is also redacted and
  control characters are stripped.
* **A trail that was asked for and cannot be written refuses the call**
  (`E_CAP_DENIED`, rule `H-5/audit`): security telemetry that silently
  disappears is worse than no telemetry.
* One line per **gate decision**. A call refused later — by H-4 argument
  validation or by the freshness watchdog — therefore also carries a second,
  later `deny` line, so reading the trail bottom-up always ends on the
  effective decision.

**Tests.** `effector_safety.rs`:
`the_audit_trail_is_off_by_default_and_records_one_line_per_decision`,
`an_audit_line_carries_no_credential_and_a_broken_sink_fails_closed`.

## 4. Monotonic capability attenuation

**Enforcement point: the language front end.** A file's `needs:` line and the
checker's routine-level rules decide what may be called
([checker.rs](../src/checker.rs), [strict.rs](../src/strict.rs)). A routine that
uses a family its file does not declare does not compile.

**Host-side lock.** This crate has no sub-run, nested-host or async execution
path: `run_source`, `run_file` and `run_module` all build exactly one host and
one VM, and the module's `needs` plus the operator's grants are fixed for that
run. A child run cannot therefore be started with a wider grant set than its
parent here. The runtime still refuses independently of the front end, so an
artifact that bypasses the checker (hand-built, edited, or deserialised) cannot
exceed what it declares and what the host grants.

**Tests.** `effector_safety.rs`:
`a_routine_that_needs_drone_cannot_run_with_only_a_log_grant` (`needs: drone`
plus a `log` grant is `E_CAP_MISSING`),
`both_execution_paths_refuse_a_grant_the_script_does_not_hold` (source and
bytecode paths), `a_module_whose_declaration_was_stripped_is_refused_at_runtime`
(`E_CAP_UNDECLARED` even though the host grants `drone`),
`the_language_checker_rejects_an_undeclared_family_before_any_run`.

## 5. Physical effector safety watchdog

The ceiling is one constant, `link::SAFETY_FRESHNESS_MS = 1500`, used by the
link and the host. A live drone link now timestamps the HEARTBEAT and every
telemetry field as it arrives, and **every live actuation command** is checked
before anything is written to the wire:

| Command | Telemetry it depends on, beyond a fresh HEARTBEAT |
| :-- | :-- |
| `arm`, `land`, `rtl`, `hold`, `stop` | none |
| `takeoff` | `latitude`, `longitude`, `altitude` (the GUIDED position estimate) |
| `goto` with 1 number | `longitude`, `altitude` (filled from telemetry) |
| `goto` with 2 numbers | `longitude` |
| `goto(lat, lon, alt)`, or a point with its own altitude | none |

A missing field counts as stale. The refusal is `E_SAFETY_VIOLATION`, which the
VM does not treat as catchable: `try ... otherwise` cannot recover from a stale
link and retry with the same stale world model. `drone.telemetry(field)` also
only returns values inside the window; an older reading is not reported as
current.

Live drone actuation is **enabled**: the previous blanket
`E_CAP_DENIED` is gone, and a fresh link sends the command. The link also keeps
the first datagram the vehicle sends (it used to be discarded), which is usually
the HEARTBEAT the gate needs.

**Tests.** `live.rs`, against the fake-autopilot harness on a real UDP socket:
`a_script_arms_a_vehicle_over_a_real_socket` (fresh link → COMPONENT_ARM_DISARM
is received),
`a_vehicle_that_never_sends_a_heartbeat_is_refused_uncatchably` (no heartbeat →
`E_SAFETY_VIOLATION`, nothing sent, `try` cannot catch it),
`a_field_the_vehicle_never_reported_refuses_a_partial_target` (heartbeats but no
position → the partial target is refused and no target frame is sent),
`a_silent_link_past_the_freshness_limit_refuses_actuation` (a real 1.75 s pause
on a scripted link → refusal, no arm command sent),
`a_lossy_position_stream_still_completes_a_mission` (packet loss does not block
a mission while the data stays inside the window). Unit tests in
[host.rs](../src/host.rs) pin the dependency table and both halves of the veto;
[link.rs](../src/link.rs) tests the per-field ages.

**Changed behaviour (deliberate).** `capability_safety.rs`'s
`live_drone_actuation_is_disabled_before_opening_the_link` encoded the old
contract (refuse everything before opening the link). It is now
`live_drone_actuation_is_refused_when_nothing_is_streaming`: a live command
against a vehicle that never speaks is still refused, but for the real reason
(`E_KAZE_LINK: nothing is streaming`).

## 6. Automatic cleanup on abnormal termination

[gpio.rs](../src/gpio.rs) `cleanup` is the Rust counterpart of
`kaze/dist/kaze_rt.{h,c}`'s `kaze_install_cleanup_signal_handlers` /
`kaze_register_deassert` / `kaze_run_cleanups`:

* Raising a GPIO line (sysfs) registers its pre-opened `value` descriptor;
  enabling a PWM channel registers its `enable` descriptor; a character-device
  line registers a `dup` of its descriptor. Registration happens **before** the
  output goes high, and a line that cannot be covered is not driven.
* On `SIGINT`/`SIGTERM` the handler writes `0` to each registered descriptor
  (`write(2)`, async-signal-safe) or clears the line with the GPIO UAPI ioctl,
  then restores the default action and re-raises, so the process still ends from
  the signal.
* A normal exit de-asserts too: `GpioBus::drop` lowers every line it raised
  before unexporting, and `PwmBus::drop` disables every channel it enabled
  (including a channel that already existed on disk and was never exported).
* The table holds 16 descriptors (the C runtime's `KAZE_MAX_DEASSERTS`).

**Tests.** `effector_safety.rs`: `sigint_de_asserts_a_gpio_line` and
`sigterm_de_asserts_gpio_and_pwm_outputs` spawn the real `kaze` binary against a
fake sysfs root, wait for the line/channel to be on, send the signal, require the
process to die **from** it, and then read `0` back from the file. Unit tests in
`gpio.rs`/`pwm.rs` exercise the exact handler code
(`the_cleanup_table_de_asserts_a_registered_descriptor`,
`driving_a_pin_registers_its_cleanup_before_it_goes_high`,
`dropping_the_bus_leaves_a_raised_line_low`,
`enabling_a_channel_registers_its_cleanup`,
`dropping_the_bus_switches_an_enabled_channel_off`).

## 7. What is not verified here

* **No real autopilot, SITL or vehicle was run.** Every drone claim above is
  against a fake autopilot speaking MAVLink 2 over UDP, or a scripted transport.
  The C-ABI freshness watchdog in `dist/kaze_rt` is a separate implementation.
* **No real GPIO/PWM hardware was driven.** The sysfs and character-device paths
  are exercised against fake files. The character-device cleanup
  (`register_line` + `gpio_chardev::deassert`) compiles and is unit-tested at
  the registry level but has never cleared a line on a real `/dev/gpiochipN`.
* **SIGSEGV is not covered.** The C substrate also catches `SIGSEGV`; the Rust
  handler deliberately covers only `SIGINT` and `SIGTERM`, which are the
  abnormal terminations the roadmap names. A segfault still bypasses cleanup.
* **Alert redaction is heuristic.** It removes the shapes listed above from
  `alert.send` messages only. `log` values still appear verbatim in the run
  report, as documented by the language reference.
* **The audit trail's watchdog line.** The safety veto calls
  `hardware::record_safety_veto`, so a veto is audited as
  `decision=deny reason=E_SAFETY_VIOLATION…`; the end-to-end audit assertion for
  that line is not in the suite (the gate allow/deny lines are), because the
  audit sink is process-global and the freshness tests drive the link directly.
* **Documents outside this change that now describe the old contract.**
  `src/main.rs:36-37` help still says "Live drone actuation is currently
  disabled"; `docs/KAZE-1.0-REFERENCE.md:314-315,321,329,451` still tag live
  freshness `[PARTIAL] [TARGET]`, live actuation `[UNSUPPORTED]`, and say the
  host refuses actuation; `docs/CERTIFICATION.md:6-7` and
  `docs/ROADMAP.md:16-17,22` say the freshness veto is missing. Those files were
  owned by other work and were not edited here. The replacement facts are §5
  above; the durable evidence is the test suite.
