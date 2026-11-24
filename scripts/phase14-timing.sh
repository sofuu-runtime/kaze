#!/usr/bin/env bash
# phase14-timing.sh -- run a timing gate on the emulated core and hold it to its number.
#
# The gate program measures itself and prints one machine-readable line:
#
#   MEASURE jitter_ns=<n> periods=<n> mean_us=<n>          (PWM actuator jitter)
#   MEASURE worst_cycles=<n> mean_cycles=<n> overruns=<n> iterations=<n>   (SPI/IMU deadline)
#
# This script re-checks the verdict rather than trusting the program's own PASS: it parses
# the numbers and applies the limit the roadmap sets.
set -u
cd "$(dirname "$0")/.." || exit 2
SRC=${1:-}
[ -n "$SRC" ] || { echo "usage: phase14-timing.sh <file.kaze>" >&2; exit 2; }
[ -f "$SRC" ] || { echo "phase14-timing: no $SRC" >&2; exit 1; }

out=$(bash scripts/hw-run-qemu.sh "$SRC" 2>&1) || { printf '%s\n' "$out"; exit 1; }
printf '%s\n' "$out"
line=$(printf '%s\n' "$out" | grep 'MEASURE' | head -1)
if [ -z "$line" ]; then echo "phase14-timing: no MEASURE line" >&2; exit 1; fi
if printf '%s\n' "$out" | grep -q 'FAIL'; then exit 1; fi

num() {
  printf '%s\n' "$1" | sed -n "s/.*$2=\([0-9][0-9.]*\).*/\1/p" | head -1
}
jitter=$(num "$line" jitter_ns)
overruns=$(num "$line" overruns)
deadline=$(num "$line" worst_cycles)
if [ -n "$jitter" ]; then
  ok=$(awk -v j="$jitter" 'BEGIN { print (j < 200.0) ? 1 : 0 }')
  if [ "$ok" != "1" ]; then echo "phase14-timing: jitter ${jitter} ns is over the 200 ns limit" >&2; exit 1; fi
  echo "phase14-timing: jitter ${jitter} ns over the run (limit 200 ns)"
  exit 0
fi
if [ -n "$overruns" ]; then
  if [ "$overruns" != "0" ]; then echo "phase14-timing: ${overruns} overruns" >&2; exit 1; fi
  limit=$(awk -v c="$deadline" 'BEGIN { print (c < 60000) ? 1 : 0 }')
  if [ "$limit" != "1" ]; then echo "phase14-timing: worst cycle ${deadline} is over 60000 (125 us at 480 MHz)" >&2; exit 1; fi
  echo "phase14-timing: worst ${deadline} cycles, 0 overruns (deadline 60000 cycles)"
  exit 0
fi
echo "phase14-timing: MEASURE line has no recognised field: $line" >&2
exit 1
