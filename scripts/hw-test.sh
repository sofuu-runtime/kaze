#!/usr/bin/env bash
# hw-test.sh -- run the Phase 14 hardware-driver suites.
#
# These suites are not standard-library suites: they talk to peripheral registers, so the
# binary is linked with the virtual board (dist/kaze_board_sim.c) that models a timer, an
# SPI IMU, an I2C barometer, a CAN loopback, a DMA channel and a UART. The same driver code
# also builds for Cortex-M (scripts/phase14-gates.sh does that); the difference is the
# board behind kaze_board_mmio, not the Kaze source.
#
#   bash scripts/hw-test.sh                 # every suite
#   bash scripts/hw-test.sh imu             # one
#   ASAN=1 bash scripts/hw-test.sh          # plus AddressSanitizer/UBSan on each
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
KAZE="${KAZE_BIN:-$ROOT/bin/kaze}"
HOST="${KAZE_HOST:-$ROOT/bin/kaze-host}"
CC=${CC:-cc}
ASAN=${ASAN:-0}
ONLY=${1:-}

if [ ! -x "$KAZE" ]; then echo "hw-test: no compiler at $KAZE" >&2; exit 2; fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
printf '{"deny":[]}\n' > "$tmp/policy.json"

pass=0; fail=0; checks=0
for src in tests/hw/*_test.kaze; do
    [ -f "$src" ] || continue
    name=$(basename "$src" _test.kaze)
    if [ -n "$ONLY" ] && [ "$name" != "$ONLY" ]; then continue; fi
    flat="$tmp/$name.kaze"
    if ! python3 scripts/bundle-imports.py "$src" > "$flat" 2>"$tmp/$name.bundle.err"; then
        echo "FAIL  $name (bundle)"; head -3 "$tmp/$name.bundle.err" | sed 's/^/    /'; fail=$((fail + 1)); continue
    fi
    "$KAZE" check "$flat" > "$tmp/$name.check" 2>&1
    if grep -qE 'E_KAZE' "$tmp/$name.check"; then
        echo "FAIL  $name (check)"; grep -E 'E_KAZE' "$tmp/$name.check" | head -3 | sed 's/^/    /'; fail=$((fail + 1)); continue
    fi
    if ! "$KAZE" emit-c "$flat" > "$tmp/$name.c" 2>"$tmp/$name.emit.err"; then
        echo "FAIL  $name (emit)"; head -3 "$tmp/$name.emit.err" | sed 's/^/    /'; fail=$((fail + 1)); continue
    fi
    if ! "$CC" -std=c99 -g -O1 -w -I dist "$tmp/$name.c" dist/kaze_rt.c dist/kaze_board_sim.c \
            -lm -o "$tmp/$name.bin" 2>"$tmp/$name.cc"; then
        echo "FAIL  $name (cc)"; head -5 "$tmp/$name.cc" | sed 's/^/    /'; fail=$((fail + 1)); continue
    fi
    out=$( "$tmp/$name.bin" --ticks 8 2>&1 ); rc=$?
    n=$(printf '%s' "$out" | sed -n 's/.*OK [a-z0-9_]*: \([0-9]*\) checks.*/\1/p' | head -1)
    if [ "$rc" -eq 0 ] && ! printf '%s' "$out" | grep -q 'FAIL'; then
        printf 'PASS  %-22s %s checks\n' "$name" "${n:-?}"
        pass=$((pass + 1)); checks=$((checks + ${n:-0}))
    else
        printf 'FAIL  %-22s exit %d\n' "$name" "$rc"
        printf '%s\n' "$out" | grep 'FAIL' | head -6 | sed 's/^/    /'
        fail=$((fail + 1))
    fi
    if [ "$ASAN" = "1" ]; then
        if "$CC" -std=c99 -g -fsanitize=address,undefined -fno-omit-frame-pointer -w \
                -I dist "$tmp/$name.c" dist/kaze_rt.c dist/kaze_board_sim.c -lm \
                -o "$tmp/$name.asan" 2>"$tmp/$name.asan.cc"; then
            leakopt="detect_leaks=1"
            case $(uname -s) in Darwin) leakopt="detect_leaks=0" ;; esac
            if ( cd "$tmp" && ASAN_OPTIONS="$leakopt" ./"$name.asan" --ticks 8 > "$tmp/$name.asan.out" 2>&1 ); then
                printf '      %-22s asan: clean\n' "$name"
            else
                printf 'FAIL  %-22s asan reported a problem\n' "$name"
                grep -E 'ERROR|SUMMARY|runtime error' "$tmp/$name.asan.out" | head -3 | sed 's/^/    /'
                fail=$((fail + 1))
            fi
        else
            echo "      $name asan build skipped"; head -3 "$tmp/$name.asan.cc" | sed 's/^/    /'
        fi
    fi
done

echo
echo "hw-test: $pass passed, $fail failed, $checks assertions (ASan=$ASAN)"
[ "$fail" -eq 0 ]
