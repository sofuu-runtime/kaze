#!/usr/bin/env bash
# stdlib-test.sh -- run every standard-library test.
#
# A test is a Kaze program under tests/stdlib/ named *_test.kaze. It imports the library
# with `use`, so it is flattened first (the compiler reads one file and has no loader),
# then checked and run. A test passes when it exits 0 and prints no FAIL line; it prints
# `OK <name>: <n> checks` when every assertion held.
#
# ASan=1 additionally rebuilds the emitted C with -fsanitize=address and runs it again,
# which is how the Phase 11 leak gate is measured.
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
KAZE="${KAZE_BIN:-$ROOT/bin/kaze}"
HOST="${KAZE_HOST:-$ROOT/bin/kaze-host}"
CC=${CC:-cc}
ASAN=${ASAN:-0}

if [ ! -x "$KAZE" ]; then echo "stdlib-test: no compiler at $KAZE" >&2; exit 2; fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
printf '{"deny":[]}\n' > "$tmp/policy.json"

pass=0; fail=0; checks=0
for src in tests/stdlib/*_test.kaze; do
    [ -f "$src" ] || continue
    name=$(basename "$src" _test.kaze)
    flat="$tmp/$name.kaze"
    if ! python3 scripts/bundle-imports.py "$src" > "$flat" 2>"$tmp/$name.bundle.err"; then
        echo "FAIL  $name (bundle)"; sed 's/^/    /' "$tmp/$name.bundle.err" | head -3; fail=$((fail + 1)); continue
    fi
    # 'kaze check' exits 0 even with diagnostics, so the output is the verdict here.
    "$KAZE" check "$flat" > "$tmp/$name.check" 2>&1
    if grep -qE 'E_KAZE' "$tmp/$name.check"; then
        echo "FAIL  $name (check)"; grep -E 'E_KAZE' "$tmp/$name.check" | head -3 | sed 's/^/    /'; fail=$((fail + 1)); continue
    fi
    out=$( cd "$tmp" && "$HOST" run "$flat" --ticks 4 2>&1 )
    rc=$?
    # Suite names carry digits (sha256, ed25519), so the character class has to as well.
    n=$(printf '%s' "$out" | sed -n 's/.*OK [a-z0-9_]*: \([0-9]*\) checks.*/\1/p' | head -1)
    if [ "$rc" -eq 0 ] && ! printf '%s' "$out" | grep -q 'FAIL'; then
        printf 'PASS  %-22s %s checks\n' "$name" "${n:-?}"
        pass=$((pass + 1)); checks=$((checks + ${n:-0}))
    else
        printf 'FAIL  %-22s exit %d\n' "$name" "$rc"
        printf '%s' "$out" | grep 'FAIL' | head -4 | sed 's/^/    /'
        fail=$((fail + 1))
    fi
    if [ "$ASAN" = "1" ]; then
        if "$KAZE" emit-c "$flat" > "$tmp/$name.c" 2>/dev/null && \
           "$CC" -std=c99 -g -fsanitize=address,undefined -fno-omit-frame-pointer \
                -I dist "$tmp/$name.c" dist/kaze_rt.c -lm -o "$tmp/$name.asan" 2>"$tmp/$name.asan.cc"; then
            # LeakSanitizer does not exist on Darwin; there ASan still checks overflows and
            # UBSan still checks undefined behaviour, and the arena has its own accounting.
            leakopt="detect_leaks=1"
            case $(uname -s) in Darwin) leakopt="detect_leaks=0" ;; esac
            if ( cd "$tmp" && ASAN_OPTIONS="$leakopt" ./"$name.asan" --ticks 4 > "$tmp/$name.asan.out" 2>&1 ); then
                printf '      %-22s asan: clean\n' "$name"
            else
                printf 'FAIL  %-22s asan reported a problem\n' "$name"
                grep -E 'ERROR|SUMMARY' "$tmp/$name.asan.out" | head -3 | sed 's/^/    /'
                fail=$((fail + 1))
            fi
        else
            echo "      $name asan build skipped"; head -3 "$tmp/$name.asan.cc" | sed 's/^/    /'
        fi
    fi
done

echo
echo "stdlib-test: $pass passed, $fail failed, $checks assertions (ASan=$ASAN)"
[ "$fail" -eq 0 ]