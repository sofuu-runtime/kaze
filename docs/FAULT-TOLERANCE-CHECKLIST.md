# Field Checklist — Kaze v0.4 Fault Tolerance & Link-Loss Failsafes

Everything deterministic is covered by `cargo test` (167 unit, integration and
live tests with default features). This file covers the part only a flight
controller and real elapsed time can answer.

## 1. Local checks (no hardware)

```bash
cargo test
cargo run --release -- run examples/fault_tolerance.kaze \
  --cap drone --cap log --ticks 35 --tick-ms 100 --sim link_lost_after=0 --plain
```

Expected:

- `takeoff to 15 meters` fails with `the drone is not armed`; the recovery block
  logs `E_KAZE_HOST`, the message and the line, then arms and takes off.
- `link lost for 3 seconds` stays quiet through virtual t=2900 ms and fires at
  t=3000, 3100, 3200, 3300 and 3400 ms — five times.
- `--sim link_lost_after=500` with `--ticks 40` moves the first firing to
  t=3500 ms; still five firings.
- `try` never swallows a safety or budget stop: `takeoff to 900 meters` inside a
  `try` still exits with `E_KAZE_RANGE`, and an exhausted loop still exits with
  `E_TIMEOUT`.

## 2. SITL: a real link-loss failsafe (ArduCopter over UDP)

```bash
# terminal 1 — ArduCopter SITL
~/ardupilot/build/SITL/bin/arducopter --model quad -I 0 \
  --home 47.6000,8.5000,500,0 --serial0 udpclient:127.0.0.1:14550

# terminal 2 — a supervisor that spends real time between polls
cat > /tmp/link_loss.kaze <<'KAZE'
needs: drone, log

when link lost for 3 seconds:
    log warning "link lost; returning to launch"
    try:
        return to launch
    otherwise with err:
        log warning err.code
KAZE

cargo run --release -- run /tmp/link_loss.kaze \
  --cap drone=udp:127.0.0.1:14550 \
  --ticks 120 --tick-ms 100 --pace --plain
```

Wait for the link to warm up (the trace starts once a frame arrives), then kill
SITL or pull the network path.

Expected:

- The hook is silent while frames arrive.
- About three seconds after the last frame, the trace shows the warning, then
  repeated `drone.rtl -> error: E_KAZE_LINK: ...` lines. The RTL failure is
  caught by the `try` in the hook, so the run keeps supervising instead of
  stopping.
- The first RTL after the dropout can block while the command path times out;
  that delay belongs to the link, not to the VM.

**`--pace` is required for a live run.** `--tick-ms` is the period of the
supervisor loop; without `--pace` the loop consumes all its ticks in
microseconds and three real seconds never pass. Simulated ticks are virtual
time, so the local checks above do not need it.

## 3. What must stay fatal

`try ... otherwise` recovers exactly five codes:

- `E_KAZE_LINK`
- `E_KAZE_LINK_TIMEOUT`
- `E_COMMAND_REJECTED`
- `E_ARMING_DENIED`
- `E_KAZE_HOST`

Everything else still stops the run. In particular, verify on hardware that a
try wrapping these still exits non-zero:

- a physical range breach (`takeoff to 900 meters`) → `E_KAZE_RANGE`;
- a capability the host did not grant → `E_CAP_MISSING`;
- a policy denial → `E_CAP_DENIED`;
- a fuel-exhausted loop → `E_TIMEOUT`;
- a type or maths error → `E_KAZE_TYPE` / `E_KAZE_MATH`.

A machine that caught one of those and kept flying would be acting on a value
it was told to refuse.

## What to report back

- the exact command, exit code, and the first error line if any;
- the timestamp of the last received frame and of the first failsafe warning;
- whether the RTL attempt failed with `E_KAZE_LINK` or `E_KAZE_LINK_TIMEOUT`;
- the autopilot firmware and mode (ArduCopter GUIDED is assumed).
