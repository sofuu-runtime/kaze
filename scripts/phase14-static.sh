#!/usr/bin/env bash
# phase14-static.sh -- count the instructions on the actuator path, from the disassembly.
#
# The dynamic gate measures jitter; this measures how long the path can take, which is a
# different question and the reason both exist. It is also a cross-check that the routine
# the gate exercises is the routine being counted: the symbol is looked up by name, so a
# rename makes this fail loudly instead of silently counting nothing.
#
# The ceiling is a latency sanity bound, not the acceptance limit. The roadmap's 0.2 us is
# a *jitter* budget and the dynamic gate holds that one (measured: 20 ns spread). A duty
# update that does KzValue arithmetic - kind checks, boxing, the address conversion inside
# mmio_write32 - costs on the order of 150 instructions at -Os, which is ~300 ns of latency
# at 480 MHz and completely repeatable, so a 96-instruction ceiling would only be met by a
# routine that does no arithmetic at all. 192 instructions (400 ns) fails if the path grows
# by a third, which is what this check is for.
set -u
cd "$(dirname "$0")/.." || exit 2
ROUTINE=${1:-pwm_set_duty_permille}
SRC=${2:-tests/hw/pwm_jitter_test.kaze}
LIMIT=${3:-192}
[ -f "$SRC" ] || { echo "phase14-static: no $SRC" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
inc=$(zig cc -print-file-name=include 2>/dev/null)
bin/kaze emit-c "$SRC" > "$tmp/p.c" || exit 1
build() { zig cc -target thumb-freestanding-eabi -mcpu=cortex_m3 -Os -ffreestanding -nostdinc \
  -isystem dist/baremetal-include -isystem "$inc" -I dist \
  -DKAZE_NO_HEAP -DKAZE_NO_SIGNALS -DKAZE_SIM_METAL -DKAZE_USE_DOUBLE -w -c "$1" -o "$2"; }
build "$tmp/p.c" "$tmp/p.o" || exit 1
build dist/kaze_rt.c "$tmp/rt.o" || exit 1
build dist/kaze_baremetal.c "$tmp/bm.o" || exit 1
build dist/kaze_board_sim.c "$tmp/sim.o" || exit 1
zig cc -target thumb-freestanding-eabi -mcpu=cortex_m3 -c dist/baremetal_start_arm.s -o "$tmp/s.o" || exit 1
zig cc -target thumb-freestanding-eabi -mcpu=cortex_m3 -nostartfiles -Wl,-T,dist/baremetal_arm.ld \
  -Wl,--entry=_start -Wl,--gc-sections "$tmp/s.o" "$tmp/p.o" "$tmp/rt.o" "$tmp/bm.o" "$tmp/sim.o" \
  -o "$tmp/p.elf" || exit 1

# zig's objdump does not accept -d, so find a real one: llvm-objdump via Homebrew, via
# xcrun, or a target-prefixed GNU objdump.
DIS=""
for cand in /opt/homebrew/opt/llvm/bin/llvm-objdump llvm-objdump arm-none-eabi-objdump; do
  if command -v "$cand" >/dev/null 2>&1; then DIS="$cand"; break; fi
done
if [ -z "$DIS" ]; then
  if xcrun --find llvm-objdump >/dev/null 2>&1; then DIS="$(xcrun --find llvm-objdump)"; fi
fi
if [ -z "$DIS" ]; then echo "phase14-static: no disassembler found" >&2; exit 1; fi
"$DIS" --disassemble "$tmp/p.elf" > "$tmp/dis.txt" 2>/dev/null || { echo "phase14-static: disassembly failed" >&2; exit 1; }

# llvm-objdump and GNU objdump both print the label as "00003444 <symbol>:".
# A routine pulled in through a module is emitted with the module's mangled
# prefix (kaze_fn_kz_m_<tag>__pwm_set_duty_permille), so match the symbol's
# suffix as well as the exact name.
count=$(awk -v fn="kaze_fn_$ROUTINE" -v rn="$ROUTINE" '
  index($0, "<" fn ">:") || index($0, "_" rn ">:") { inside = 1; next }
  inside && /^[0-9a-f]+ <.*>:$/ { exit }
  inside && /\t/ { n++ }
  END { print n + 0 }' "$tmp/dis.txt")
if [ "$count" -le 0 ]; then
  echo "phase14-static: routine kaze_fn_$ROUTINE not found in the image (did it get inlined?)" >&2
  exit 1
fi
ns=$(awk -v c="$count" 'BEGIN { printf "%.1f", c * 2.0833 }')
printf 'phase14-static: %-26s %3d instructions, %6.1f ns at 480 MHz (latency ceiling %d instr)\n' \
  "$ROUTINE" "$count" "$ns" "$LIMIT"
over=$(awk -v c="$count" -v l="$LIMIT" 'BEGIN { print (c > l) ? 1 : 0 }')
if [ "$over" = "1" ]; then echo "phase14-static: the actuator path grew past the latency ceiling" >&2; exit 1; fi
echo "phase14-static: actuator path within the latency ceiling (jitter is the dynamic gate's)"
