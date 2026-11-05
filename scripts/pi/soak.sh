#!/usr/bin/env bash
# Raspberry Pi 5 / companion-computer soak test (v0.8).
#
# This script only runs where there is real hardware. It is intentionally
# parameterised and it refuses to pretend: with no KAZE_DEVICE it prints the
# setup and exits 0 (a skip, not a pass).
#
#   KAZE_CAP=drone KAZE_DEVICE=uart:/dev/ttyAMA0@57600 \
#     KAZE_SOAK_ITERATIONS=100 scripts/pi/soak.sh
#
# The default mission needs the drone family. A different script that needs
# other effector families must also be given their grants; the soak runner only
# passes the one --cap named by KAZE_CAP plus KAZE_PINS.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

CAP="${KAZE_CAP:-drone}"
DEVICE="${KAZE_DEVICE:-}"
ITERATIONS="${KAZE_SOAK_ITERATIONS:-100}"
PINS="${KAZE_PINS:-}"
SCRIPT="${KAZE_SCRIPT:-examples/missions/eight_waypoints.kaze}"
BIN="${KAZE_BIN:-kaze}"
LOG="${KAZE_SOAK_LOG:-/tmp/kaze-pi-soak.log}"
# The documented default mission has 12 actions, 8 of them `drone.goto`.
# A custom mission must state its own expectation (or leave them empty to skip
# the count check; determinism is always required).
if [ "$SCRIPT" = "examples/missions/eight_waypoints.kaze" ]; then
    EXPECT_ACTIONS="${KAZE_EXPECT_ACTIONS:-12}"
    EXPECT_GOTOS="${KAZE_EXPECT_GOTOS:-8}"
else
    EXPECT_ACTIONS="${KAZE_EXPECT_ACTIONS:-}"
    EXPECT_GOTOS="${KAZE_EXPECT_GOTOS:-}"
fi

if [ -z "$DEVICE" ]; then
    cat >&2 <<'EOF'
soak: no hardware device configured; SKIPPED (hardware-dependent)
      Set KAZE_DEVICE to the live backend, for example:
        KAZE_CAP=drone KAZE_DEVICE=uart:/dev/ttyAMA0@57600 scripts/pi/soak.sh
      Then run this on the Raspberry Pi 5 with the flight controller attached.
EOF
    exit 0
fi

if ! command -v "$BIN" >/dev/null 2>&1; then
    echo "soak: '$BIN' is not installed; set KAZE_BIN or add it to PATH" >&2
    exit 2
fi

: >"$LOG"
echo "soak: $ITERATIONS iterations of $SCRIPT against $CAP=$DEVICE $PINS" | tee -a "$LOG"
TRACE_FNV=""

for i in $(seq 1 "$ITERATIONS"); do
    if command -v /usr/bin/time >/dev/null 2>&1; then
        RESULT="$(/usr/bin/time -v "$BIN" bench "$SCRIPT" \
            --iterations 1 --cap "$CAP=$DEVICE" $PINS --json 2>>"$LOG")"
    else
        RESULT="$("$BIN" bench "$SCRIPT" \
            --iterations 1 --cap "$CAP=$DEVICE" $PINS --json)"
    fi
    printf '%s\n' "$RESULT" >>"$LOG"
    if ! grep -q '"deterministic":true' <<<"$RESULT"; then
        echo "soak: FAIL — iteration $i was not deterministic; log at $LOG" >&2
        exit 1
    fi
    FNV="$(sed -n 's/.*"trace_fnv":\([0-9]*\).*/\1/p' <<<"$RESULT")"
    if [ -z "$FNV" ]; then
        echo "soak: FAIL — iteration $i did not report a trace fingerprint; log at $LOG" >&2
        exit 1
    fi
    if [ -z "$TRACE_FNV" ]; then
        TRACE_FNV="$FNV"
    elif [ "$FNV" != "$TRACE_FNV" ]; then
        echo "soak: FAIL — iteration $i trace $FNV differs from $TRACE_FNV; log at $LOG" >&2
        exit 1
    fi
    if [ -n "$EXPECT_ACTIONS" ] && ! grep -q "\"actions\":${EXPECT_ACTIONS}," <<<"$RESULT"; then
        echo "soak: FAIL — iteration $i did not report ${EXPECT_ACTIONS} actions; log at $LOG" >&2
        exit 1
    fi
    if [ -n "$EXPECT_GOTOS" ] && ! grep -q "\"gotos\":${EXPECT_GOTOS}," <<<"$RESULT"; then
        echo "soak: FAIL — iteration $i did not report ${EXPECT_GOTOS} drone.goto actions; log at $LOG" >&2
        exit 1
    fi
    TEMP=""
    if command -v vcgencmd >/dev/null 2>&1; then
        TEMP=" $(vcgencmd measure_temp 2>/dev/null || true)"
    fi
    echo "soak: iteration $i${TEMP}" | tee -a "$LOG"
done

if [ -n "$EXPECT_ACTIONS" ]; then
    echo "soak: PASS — $ITERATIONS iterations completed ($EXPECT_ACTIONS actions, $EXPECT_GOTOS drone.goto, trace $TRACE_FNV on every run); log at $LOG"
else
    echo "soak: PASS — $ITERATIONS iterations completed (trace $TRACE_FNV, deterministic; action counts not asserted for a custom mission); log at $LOG"
fi
echo "soak: review $LOG for RSS/timing and attach it to the release receipt"
