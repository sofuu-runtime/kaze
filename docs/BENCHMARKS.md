# Mission Benchmarks (v0.8)

Three pieces: a deterministic eight-waypoint mission, `kaze bench`, and the
hardware-dependent SITL and Raspberry Pi 5 drills. The simulation results are
reproducible on any machine; the hardware results require hardware and are
marked as **not executed** here rather than guessed.

## 1. The mission

`examples/missions/eight_waypoints.kaze` arms, climbs to 15 m, visits eight
points in a grid, and returns to launch:

```bash
kaze run examples/missions/eight_waypoints.kaze --cap drone --cap log --plain
```

The trace has 12 actions: one log, `arm`, `takeoff`, eight `goto`/position
targets, and `rtl`. The simulator is deterministic, so the trace is identical
on every run.

## 2. `kaze bench`

```bash
kaze bench examples/missions/eight_waypoints.kaze --iterations 20 \
  --cap drone --cap log
# or machine-readable:
kaze bench examples/missions/eight_waypoints.kaze --iterations 20 \
  --cap drone --cap log --json
```

`kaze bench` prepares the script (or loads the `.kazebc`) **once**, runs it N
times against a fresh simulator state, and reports:

- actions (and `gotos`, the `drone.goto` subset), instructions, fuel and ticks
  of the last run;
- min / median / p95 / mean wall-clock time. Median is the middle sample, or
  the mean of the two middle samples for an even iteration count; p95 is the
  nearest-rank sample (`ceil(0.95·N)`-th);
- `deterministic: yes|no` and `trace_fnv`, an FNV-1a fingerprint of the
  canonical trace. Every run's trace is compared to the first, and a soak can
  compare the fingerprint across processes with a string compare. The timer
  covers verification and execution (the module clone happens outside it). A
  non-deterministic result exits 1 because it is not benchmark evidence; a live
  backend that legitimately varies must not be used for a certification
  receipt.

### Measured here (simulation)

Run on 2026-11-05 on the development host (macOS arm64, **debug** build via
`cargo run`). Timing is host-specific; the action and instruction counts are
not.

| Run (20 iterations) | Result |
| :--- | :--- |
| actions | 12 (log, arm, takeoff, eight gotos, RTL) |
| instructions / fuel / ticks | 205 / 205 / 0 |
| wall clock | min 0.270 ms · median 0.297 ms · p95 0.416 ms · mean 0.318 ms |
| determinism | yes (identical canonical traces) |

A release build is faster; the point of the table is that the trace and the
work count are stable, and that the benchmark itself is deterministic. The
golden corpus (`examples/golden/eight_waypoints.txt`) pins the exact actions,
and `tests/bench.rs` pins source and `.kazebc` to 12 actions with
`deterministic: true`.

## 3. SITL (ArduCopter over UDP) — not executed here

`scripts/sitl/eight_waypoints.sh` starts a local ArduCopter SITL, waits for it
to settle, runs the mission with `--cap drone=udp:127.0.0.1:14550`, and checks
that eight position targets left the script.

```bash
ARDUPILOT_SITL=$HOME/ardupilot/build/SITL/bin/arducopter \
  scripts/sitl/eight_waypoints.sh
```

**Status: NOT EXECUTED** — ArduPilot SITL is not installed in this environment.
The script says so and exits 0 (a skip, not a pass). Run it on a machine with
SITL and attach the trace; a pass means exit 0 and exactly eight `drone.goto`
lines.

## 4. Raspberry Pi 5 soak — not executed here

`scripts/pi/soak.sh` runs the mission repeatedly against a real backend and
records timing/RSS (with GNU `time`) plus the SoC temperature (with
`vcgencmd`):

```bash
KAZE_CAP=drone KAZE_DEVICE=uart:/dev/ttyAMA0@57600 \
  KAZE_SOAK_ITERATIONS=100 scripts/pi/soak.sh
```

The default mission needs the `drone` family. A script that needs other
effector families also needs their grants; the runner passes the one `--cap`
named by `KAZE_CAP` plus `KAZE_PINS`.

**Status: NOT EXECUTED** — no Raspberry Pi 5 or flight controller is attached
to this machine. With `KAZE_DEVICE` unset the script prints the setup and exits
0 (skip). A pass requires every iteration to exit 0 and the eight-waypoint count
to hold across the run.

## 5. Packet loss resilience

Two automated checks run here:

- `tests/live.rs::a_lossy_position_stream_still_completes_a_mission` starts a
  stand-in autopilot that **drops every second position frame**, leaving
  heartbeats and ACKs reliable, and runs arm → takeoff → position target →
  telemetry. The mission completes; the lossy telemetry does not desynchronise
  the link.
- `mavlink.rs::dropped_frames_do_not_desynchronise_the_stream` feeds the codec
  frames 1 and 3 with frame 2 missing and asserts both are decoded in order
  with no discarded bytes.

This is loss at the frame level with a reliable command path. A real radio can
also lose ACKs and heartbeats; the SITL/Pi drills are the place to measure that,
and they are not claimed here.

## 6. Honest limits

- Simulation timing is not flight timing: no radio, no scheduler jitter, no
  thermal effects.
- The SITL and Pi 5 rows are **not evidence** until someone runs them and
  attaches the trace/log named by each script.
- "Flight-certified" in v1.0 means the checklist and receipts, not a regulatory
  certification.
