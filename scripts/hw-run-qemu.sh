#!/usr/bin/env bash
# hw-run-qemu.sh -- build one Kaze program for Cortex-M and run it on emulated silicon.
#
# The host suites (scripts/hw-test.sh) prove behaviour; this proves the same source also
# runs as a bare-metal image, with the emulator's SysTick as the clock. With -icount each
# instruction costs one count, so a measured cycle count here is the program's instruction
# count - which is what makes the Phase 14 timing gates measurable at all on a machine with
# no flight controller attached.
#
#   bash scripts/hw-run-qemu.sh tests/hw/board_test.kaze [-- <program args>]
#
# Prints the program's own output. Exits 0 when the image finished through semihosting.
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
KAZE="${KAZE_BIN:-$ROOT/bin/kaze}"
SRC=${1:-}
WATCHDOG=${QEMU_WATCHDOG:-60}
shift || true
if [ -z "$SRC" ]; then echo "usage: hw-run-qemu.sh <file.kaze>" >&2; exit 2; fi
[ -f "$SRC" ] || { echo "hw-run-qemu: no $SRC" >&2; exit 2; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
inc=$(zig cc -print-file-name=include 2>/dev/null)

python3 scripts/bundle-imports.py "$SRC" > "$tmp/prog.kaze" || { echo "hw-run-qemu: bundle failed" >&2; exit 2; }
"$KAZE" check "$tmp/prog.kaze" > "$tmp/check.out" 2>&1
if grep -qE 'E_KAZE' "$tmp/check.out"; then grep -E 'E_KAZE' "$tmp/check.out" >&2; exit 2; fi
"$KAZE" emit-c "$tmp/prog.kaze" > "$tmp/prog.c" || { echo "hw-run-qemu: emit failed" >&2; exit 2; }

# -mcpu=cortex_m3 is not a tuning hint here, it is correctness: emitted double arithmetic
# calls into compiler-rt, and the default profile uses UMAAL, which a Cortex-M3 does not
# have. Without this flag every division in a Kaze program is an Undefined Instruction on
# the emulated core - and the fault vector is outside flash, so the run looks like a random
# lockup rather than a floating-point problem.
build_one() {
  local src="$1" out="$2"
  zig cc -target thumb-freestanding-eabi -mcpu=cortex_m3 -Os -ffreestanding -nostdinc \
    -isystem dist/baremetal-include -isystem "$inc" -I dist \
    -DKAZE_NO_HEAP -DKAZE_NO_SIGNALS -DKAZE_SIM_METAL -DKAZE_USE_DOUBLE -w -c "$src" -o "$out"
}

build_one dist/kaze_rt.c "$tmp/kaze_rt.o" || exit 2
build_one dist/kaze_baremetal.c "$tmp/kaze_baremetal.o" || exit 2
build_one dist/kaze_board_sim.c "$tmp/kaze_board_sim.o" || exit 2
build_one "$tmp/prog.c" "$tmp/prog.o" || exit 2
zig cc -target thumb-freestanding-eabi -mcpu=cortex_m3 -c dist/baremetal_start_arm.s -o "$tmp/start.o" || exit 2

zig cc -target thumb-freestanding-eabi -mcpu=cortex_m3 -nostartfiles -Wl,-T,dist/baremetal_arm.ld \
  -Wl,--entry=_start -Wl,--gc-sections \
  "$tmp/start.o" "$tmp/prog.o" "$tmp/kaze_rt.o" "$tmp/kaze_baremetal.o" "$tmp/kaze_board_sim.o" \
  -o "$tmp/prog.elf" || { echo "hw-run-qemu: link failed" >&2; exit 2; }

size=$(stat -f%z "$tmp/prog.elf" 2>/dev/null || stat -c%s "$tmp/prog.elf")
echo "hw-run-qemu: $SRC -> $(basename "$SRC" .kaze).elf ($size bytes)"

# No 'timeout' on this host: start QEMU, wait, and kill it only if it is still alive.
qemu-system-arm -M lm3s6965evb -nographic -semihosting -icount shift=0 \
  -kernel "$tmp/prog.elf" > "$tmp/uart.out" 2>&1 &
qpid=$!
waited=0
while kill -0 $qpid 2>/dev/null && [ $waited -lt "$WATCHDOG" ]; do
  sleep 1
  waited=$((waited + 1))
done
if kill -0 $qpid 2>/dev/null; then
  kill $qpid 2>/dev/null
  echo "hw-run-qemu: watchdog after ${WATCHDOG}s (the image did not exit)" >&2
  cat "$tmp/uart.out" >&2
  exit 3
fi
wait $qpid 2>/dev/null
cat "$tmp/uart.out"
