#!/usr/bin/env bash
# stdlib-one.sh -- run a single standard-library test by name.
#
#   bash scripts/stdlib-one.sh vector
#   ASAN=1 bash scripts/stdlib-one.sh vector
#
# Same pipeline as stdlib-test.sh (bundle, check, run, optional ASan/UBSan), but for
# one test, so a failing module can be iterated on without waiting for the rest.
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
KAZE="${KAZE_BIN:-$ROOT/bin/kaze}"
HOST="${KAZE_HOST:-$ROOT/bin/kaze-host}"
CC=${CC:-cc}
ASAN=${ASAN:-0}
name="${1:-}"

if [ -z "$name" ]; then echo "usage: stdlib-one.sh <name>" >&2; exit 2; fi
src="tests/stdlib/${name}_test.kaze"
if [ ! -f "$src" ]; then echo "stdlib-one: no $src" >&2; exit 2; fi
if [ ! -x "$KAZE" ]; then echo "stdlib-one: no compiler at $KAZE" >&2; exit 2; fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
printf '{"deny":[]}\n' > "$tmp/policy.json"

abs_src="$ROOT/$src"
"$KAZE" check "$abs_src" > "$tmp/check.out" 2>&1
if grep -qE 'E_KAZE' "$tmp/check.out"; then
    echo "FAIL $name (check)"; grep -E 'E_KAZE' "$tmp/check.out" | head -20 | sed 's/^/    /'; exit 1
fi
echo "check: ok"

out=$( cd "$tmp" && "$HOST" run "$abs_src" --ticks 8 2>&1 )
rc=$?
printf '%s\n' "$out" | sed 's/^\[info\] //' | grep -E '^(OK|FAIL)' | head -20 | sed 's/^/    /'
if [ "$rc" -ne 0 ]; then echo "FAIL $name (run exit $rc)"; printf '%s\n' "$out" | tail -20 | sed 's/^/    /'; exit 1; fi
if printf '%s\n' "$out" | grep -q 'FAIL'; then echo "FAIL $name (assertion)"; exit 1; fi

if [ "$ASAN" = "1" ]; then
    if "$KAZE" emit-c "$abs_src" > "$tmp/$name.c" 2>/dev/null && \
       "$CC" -std=c99 -g -fsanitize=address,undefined -fno-omit-frame-pointer \
            -I dist "$tmp/$name.c" dist/kaze_rt.c -lm -o "$tmp/$name.asan" 2>"$tmp/asan.cc"; then
        leakopt="detect_leaks=1"
        case $(uname -s) in Darwin) leakopt="detect_leaks=0" ;; esac
        if ( cd "$tmp" && ASAN_OPTIONS="$leakopt" ./"$name.asan" --ticks 8 > "$tmp/asan.out" 2>&1 ); then
            echo "asan: clean"
        else
            echo "FAIL $name (asan)"; grep -E 'ERROR|SUMMARY|runtime error' "$tmp/asan.out" | head -5 | sed 's/^/    /'; exit 1
        fi
    else
        echo "asan build failed"; head -5 "$tmp/asan.cc" | sed 's/^/    /'; exit 1
    fi
fi
echo "PASS $name"
