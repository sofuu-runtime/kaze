#!/usr/bin/env bash
# phase14-gates.sh -- the Phase 14 acceptance gates.
#
#   bash scripts/phase14-gates.sh
#   bash scripts/phase14-gates.sh --list
#
# Four kinds of evidence, because the phase asks for four different things:
#
#   1. The drivers behave. Every hardware suite runs on the host against the virtual board
#      (dist/kaze_board_sim.c), and again under AddressSanitizer/UBSan.
#   2. The same source runs as a bare-metal image. Each gate program is cross-compiled for
#      Cortex-M and executed on an emulated MCU (QEMU, -icount), which is the only clock
#      this machine has.
#   3. The timing gates. PWM jitter and the SPI/IMU deadline are measured there, where each
#      instruction is one cycle, and the numbers are printed.
#   4. The static path length, counted from the disassembly of the built image, as a second
#      opinion on what the dynamic measurement reports.
#
# What this is NOT: a measurement on real silicon. There is no flight controller attached,
# QEMU's timer model has no crystal, and the jitter figure is therefore the software's
# contribution - which is the part a language and a driver can control. The report says so,
# and the roadmap records the gate as met for software jitter and unmet for oscillator
# jitter.
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
FAILED=0
COUNT=0

if [ "${1:-}" = '--list' ]; then
  sed -n 's/^run \(.[^ ]*\).*/\1/p' "$0" | tr -d "'"
  exit 0
fi

run() {
  name=$1
  shift
  COUNT=$((COUNT + 1))
  printf '%-34s' "$name"
  if out=$("$@" 2>&1); then
    summary=$(printf '%s' "$out" | grep -E 'jitter|deadline|checks|clean|images|archives|instructions|PASS|passed' | tail -1 | cut -c1-64)
    echo "PASS  $summary"
  else
    FAILED=$((FAILED + 1))
    echo 'FAIL'
    printf '%s\n' "$out" | tail -8 | sed 's/^/    /'
  fi
}

# ---- 1. host suites ---------------------------------------------------------------
run 'driver suites (host)' bash scripts/hw-test.sh
run 'driver suites (asan)' env ASAN=1 bash scripts/hw-test.sh

# ---- 2. bare-metal builds and the board suite on an emulated MCU --------------------
run 'bare-metal archives' bash scripts/phase14-archives.sh
run 'cortex-m board suite' bash scripts/phase14-metal.sh tests/hw/board_test.kaze

# ---- 3. the timing gates, measured on the emulated core ----------------------------
run 'pwm jitter < 0.2us' bash scripts/phase14-timing.sh tests/hw/pwm_jitter_test.kaze
run 'spi imu deadline 125us' bash scripts/phase14-timing.sh tests/hw/imu_deadline_test.kaze

# ---- 4. the static path length ----------------------------------------------------
run 'actuator path (static)' bash scripts/phase14-static.sh

echo
if [ "$FAILED" -eq 0 ]; then
  echo "phase14-gates: $COUNT of $COUNT passed (no board attached; see kaze/docs/PHASE-14-REPORT.md)"
  exit 0
fi
echo "phase14-gates: $FAILED of $COUNT failed"
exit 1
