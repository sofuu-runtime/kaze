#!/usr/bin/env bash
# phase14-archives.sh -- cross-compile the runtime for the two embedded targets.
#
# The compiler cannot run on a Cortex-M (it reads files and prints C), but the runtime a
# compiled program uses can, and that is what an embedded archive is for. This is the same
# build the Makefile performs, kept here so the Phase 14 gate does not depend on make.
set -u
cd "$(dirname "$0")/.." || exit 2
mkdir -p bin/baremetal
inc=$(zig cc -print-file-name=include 2>/dev/null)
# thumb is built for the flight-controller class part (Cortex-M7: STM32H7), so the archive
# may use instructions an M3 lacks; the QEMU gate images are the ones pinned to cortex_m3.
for t in thumb-freestanding-eabi riscv32-freestanding-eabi; do
  cpu=""
  case "$t" in thumb*) cpu="-mcpu=cortex_m7" ;; *) cpu="-mcpu=generic_rv32" ;; esac
  zig cc -target "$t" $cpu -Os -DKAZE_NO_HEAP -DKAZE_NO_SIGNALS -ffreestanding \
    -nostdinc -isystem dist/baremetal-include -isystem "$inc" -I dist \
    -w -c dist/kaze_rt.c -o "bin/baremetal/kaze_rt-$t.o" || exit 1
  zig cc -target "$t" -Os -ffreestanding -nostdinc \
    -isystem dist/baremetal-include -isystem "$inc" -I dist \
    -w -c dist/kaze_baremetal.c -o "bin/baremetal/kaze_baremetal-$t.o" || exit 1
  # The board model is built for the target, so it takes the metal branch: SysTick is real
  # and output goes to the UART.
  zig cc -target "$t" -Os -ffreestanding -nostdinc \
    -isystem dist/baremetal-include -isystem "$inc" -I dist \
    -DKAZE_SIM_METAL -DKAZE_USE_DOUBLE -w -c dist/kaze_board_sim.c \
    -o "bin/baremetal/kaze_board_sim-$t.o" || exit 1
  rm -f "bin/baremetal/libkaze-$t.a"
  zig ar rcs "bin/baremetal/libkaze-$t.a" \
    "bin/baremetal/kaze_rt-$t.o" "bin/baremetal/kaze_baremetal-$t.o" \
    "bin/baremetal/kaze_board_sim-$t.o" || exit 1
  sz=$(stat -f%z "bin/baremetal/libkaze-$t.a" 2>/dev/null || stat -c%s "bin/baremetal/libkaze-$t.a")
  printf 'phase14-archives: %-28s %6d bytes\n' "$t" "$sz"
done
echo "phase14-archives: 2 targets built"
