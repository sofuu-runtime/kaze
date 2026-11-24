#!/usr/bin/env bash
# phase14-metal.sh -- run one hardware suite as a Cortex-M image and require a verdict.
#
# The point of running the same Kaze source on the emulated core is that the host build and
# the target build are different worlds: doubles, 32-bit ints, no heap, no libc. A suite
# that only ever ran on the host has not been tested where it will live.
set -u
cd "$(dirname "$0")/.." || exit 2
SRC=${1:-}
[ -n "$SRC" ] || { echo "usage: phase14-metal.sh <file.kaze>" >&2; exit 2; }
[ -f "$SRC" ] || { echo "phase14-metal: no $SRC" >&2; exit 1; }

out=$(bash scripts/hw-run-qemu.sh "$SRC" 2>&1) || { printf '%s\n' "$out"; exit 1; }
printf '%s\n' "$out"
if printf '%s\n' "$out" | grep -q 'FAIL'; then exit 1; fi
if ! printf '%s\n' "$out" | grep -qE 'OK [a-z0-9_]+: [0-9]+ checks'; then
  echo "phase14-metal: no OK line, so there is no verdict" >&2
  exit 1
fi
echo "phase14-metal: ran on emulated Cortex-M and passed"
