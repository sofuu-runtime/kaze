#!/usr/bin/env bash
# Eight-waypoint mission against ArduCopter SITL.
#
# This is the SITL half of the v0.8 mission benchmark. It starts a local
# ArduCopter, waits for it to settle, runs the mission over UDP, and checks that
# eight position targets left the script. If ArduPilot is not installed the
# script reports a skip (exit 0): a missing simulator is not a Red test, but it
# is not evidence either.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

SITL="${ARDUPILOT_SITL:-$HOME/ardupilot/build/SITL/bin/arducopter}"
PORT="${KAZE_SITL_PORT:-14550}"
WAIT="${KAZE_SITL_WAIT:-20}"
LOG="${KAZE_SITL_LOG:-/tmp/kaze-sitl.log}"
TRACE="${KAZE_SITL_TRACE:-/tmp/kaze-eight-waypoints.trace}"

if [ ! -x "$SITL" ]; then
    echo "sitl: no ArduCopter SITL at $SITL" >&2
    echo "sitl: set ARDUPILOT_SITL=/path/to/arducopter to run this benchmark" >&2
    echo "sitl: SKIPPED (hardware-dependent)" >&2
    exit 0
fi

echo "sitl: starting $SITL on udp:127.0.0.1:$PORT"
"$SITL" --model quad -I 0 \
    --home 47.6000,8.5000,500,0 \
    --serial0 "udpclient:127.0.0.1:$PORT" >"$LOG" 2>&1 &
SITL_PID=$!
trap 'kill "$SITL_PID" 2>/dev/null || true' EXIT

echo "sitl: waiting ${WAIT}s for the vehicle to initialise"
sleep "$WAIT"

set +e
cargo run --release --bin kaze -- run examples/missions/eight_waypoints.kaze \
    --cap "drone=udp:127.0.0.1:$PORT" --plain | tee "$TRACE"
STATUS=${PIPESTATUS[0]}
set -e

GOTOS=$(grep -c 'drone.goto' "$TRACE" || true)
echo "sitl: exit $STATUS, $GOTOS waypoint commands"

if [ "$STATUS" -ne 0 ]; then
    echo "sitl: FAIL — the mission did not finish; see $TRACE and $LOG" >&2
    exit 1
fi
if [ "$GOTOS" -ne 8 ]; then
    echo "sitl: FAIL — expected 8 waypoint commands, saw $GOTOS" >&2
    exit 1
fi
echo "sitl: PASS — eight-waypoint mission completed against SITL"
