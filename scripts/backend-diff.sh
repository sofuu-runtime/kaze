#!/usr/bin/env bash
# backend-diff.sh -- prove --backend=native and --backend=c99 agree.
#
# For every fixture under tests/native/cases this script builds the program
# twice: once through the QBE native host, once through the authoritative C99
# emitter and a C compiler.  stdout, stderr and the exit status must match
# byte-for-byte, including the E_KAZE_* trap text.  It also runs the ABI layout
# assertions and measures the 1k-LOC cold path.
#
#   bash scripts/backend-diff.sh                 # cases + ABI + latency
#   bash scripts/backend-diff.sh --full          # also 72 strict + 22 stdlib programs
#   bash scripts/backend-diff.sh --no-latency    # skip the timing probe
#   bash scripts/backend-diff.sh --only div      # substring filter
#   bash scripts/backend-diff.sh --list          # case names
#
# Environment: KAZE_BIN, KAZE_NATIVE_BIN, KAZE_NATIVE_TARGET, CC.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
KAZE=${KAZE_BIN:-$ROOT/bin/kaze}
NATIVE=${KAZE_NATIVE_BIN:-$ROOT/bin/kaze-native}
CC=${CC:-cc}
ONLY=''
LATENCY=1
FULL=0
want_list=0
want_only=0
for arg in "$@"; do
    case "$arg" in
        --list) want_list=1 ;;
        --full) FULL=1 ;;
        --no-latency) LATENCY=0 ;;
        --only) want_only=1 ;;
        --only=*) ONLY=${arg#--only=} ;;
    esac
done
if [ "$want_only" -eq 1 ]; then
    while [ "$#" -gt 0 ]; do
        if [ "$1" = '--only' ] && [ "$#" -ge 2 ]; then ONLY=$2; fi
        shift
    done
fi
if [ "$want_list" -eq 1 ]; then
    for f in "$ROOT"/tests/native/cases/*.kaze; do basename "$f" .kaze; done
    exit 0
fi

die() { echo "backend-diff: $*" >&2; exit 2; }
[ -x "$KAZE" ] || die "missing compiler $KAZE (build it first)"
if [ ! -x "$NATIVE" ]; then
    bash "$ROOT/scripts/build-native.sh" >/dev/null || die "could not build kaze-native"
fi

# The oracle below is `emit-c` plus a direct C compile: it deliberately skips
# the checker so the fixtures can exercise runtime semantics (IEEE division,
# undeclared log) that the checker rejects.  The native host must use the same
# post-check program, so tell it not to re-run the checker.
export KAZE_NATIVE_NO_CHECK=1

TMP=$(mktemp -d "${TMPDIR:-/tmp}/kaze-backend-diff.XXXXXX") || die "mktemp"
trap 'rm -rf "$TMP"' EXIT

cases=0; passed=0; failed=0; unsupported=0
for src in "$ROOT"/tests/native/cases/*.kaze; do
    name=$(basename "$src" .kaze)
    case "$name" in
        *"$ONLY"*) ;;
        *) continue ;;
    esac
    cases=$((cases + 1))
    nout="$TMP/$name.n.out"; nerr="$TMP/$name.n.err"
    cout="$TMP/$name.c.out"; cerr="$TMP/$name.c.err"
    # One optional line of program arguments, whitespace separated.  This is
    # how parameterized main cases carry `--arg d=4`; values with spaces are
    # not supported by this harness.
    case_args=''
    if [ -f "${src%.kaze}.args" ]; then
        case_args=$(tr '\n' ' ' < "${src%.kaze}.args")
    fi
    set -f
    "$NATIVE" run "$src" $case_args >"$nout" 2>"$nerr"
    nrc=$?
    if grep -q 'E_KAZE_BACKEND_UNSUPPORTED' "$nerr" 2>/dev/null; then
        unsupported=$((unsupported + 1))
        printf 'UNSUPPORTED %-22s %s\n' "$name" "$(head -1 "$nerr")"
        continue
    fi
    if ! "$KAZE" emit-c "$src" >"$TMP/$name.c" 2>"$TMP/$name.emit.err"; then
        failed=$((failed + 1))
        printf 'FAIL        %-22s emit-c failed\n' "$name"
        continue
    fi
    if ! "$CC" -std=c99 -O1 -w -I "$ROOT/dist" "$TMP/$name.c" \
            "$ROOT/dist/kaze_rt.c" -lm -o "$TMP/$name.c99" 2>"$TMP/$name.cc.err"; then
        failed=$((failed + 1))
        printf 'FAIL        %-22s C99 build failed\n' "$name"
        continue
    fi
    "$TMP/$name.c99" $case_args >"$cout" 2>"$cerr"
    crc=$?
    set +f
    if [ "$nrc" -eq "$crc" ] && cmp -s "$nout" "$cout" && cmp -s "$nerr" "$cerr"; then
        passed=$((passed + 1))
        printf 'PASS        %-22s rc=%d\n' "$name" "$nrc"
    else
        failed=$((failed + 1))
        printf 'FAIL        %-22s native rc=%d c99 rc=%d\n' "$name" "$nrc" "$crc"
        diff -u "$cout" "$nout" | sed 's/^/    /' | head -12
        diff -u "$cerr" "$nerr" | sed 's/^/    /' | head -12
    fi
done

full_cases=0; full_passed=0; full_failed=0
if [ "$FULL" -eq 1 ] && [ "$ONLY" = '' ]; then
    # Whole programs, run through the unified front end exactly as a user
    # would. The fixture loop above deliberately disables the checker for
    # runtime-semantics cases; these suites must include it, so drop it here.
    unset KAZE_NATIVE_NO_CHECK
    ftmp="$TMP/full"
    mkdir -p "$ftmp"
    for src in "$ROOT"/tests/fixtures/strict/*.kaze; do
        [ -f "$src" ] || continue
        name=$(basename "$src" .kaze)
        full_cases=$((full_cases + 1))
        "$KAZE" run --backend=native "$src" >"$ftmp/$name.n.out" 2>"$ftmp/$name.n.err"; nrc=$?
        "$KAZE" run "$src" >"$ftmp/$name.c.out" 2>"$ftmp/$name.c.err"; crc=$?
        if [ "$nrc" -eq "$crc" ] && cmp -s "$ftmp/$name.n.out" "$ftmp/$name.c.out" && cmp -s "$ftmp/$name.n.err" "$ftmp/$name.c.err"; then
            full_passed=$((full_passed + 1))
        else
            full_failed=$((full_failed + 1))
            printf 'FULL FAIL   %-22s native rc=%d c99 rc=%d\n' "$name" "$nrc" "$crc"
            diff -u "$ftmp/$name.c.out" "$ftmp/$name.n.out" | sed 's/^/    /' | head -8
            diff -u "$ftmp/$name.c.err" "$ftmp/$name.n.err" | sed 's/^/    /' | head -8
        fi
    done
    for src in "$ROOT"/tests/stdlib/*_test.kaze; do
        [ -f "$src" ] || continue
        name=$(basename "$src" _test.kaze)
        full_cases=$((full_cases + 1))
        "$NATIVE" run "$src" --ticks 4 >"$ftmp/$name.n.out" 2>"$ftmp/$name.n.err"; nrc=$?
        "$KAZE" run "$src" --ticks 4 >"$ftmp/$name.c.out" 2>"$ftmp/$name.c.err"; crc=$?
        if [ "$nrc" -eq "$crc" ] && cmp -s "$ftmp/$name.n.out" "$ftmp/$name.c.out" && cmp -s "$ftmp/$name.n.err" "$ftmp/$name.c.err"; then
            full_passed=$((full_passed + 1))
        else
            full_failed=$((full_failed + 1))
            printf 'FULL FAIL   %-22s native rc=%d c99 rc=%d\n' "$name" "$nrc" "$crc"
            diff -u "$ftmp/$name.c.out" "$ftmp/$name.n.out" | sed 's/^/    /' | head -8
            diff -u "$ftmp/$name.c.err" "$ftmp/$name.n.err" | sed 's/^/    /' | head -8
        fi
    done
    # Universal-parallelism semantic programs. The native backend runs the
    # ordered loop sequentially; stdout and exit status must still match.
    for name in test_map test_filter test_pfor; do
        src="$ROOT/tests/parallel/$name.kaze"
        [ -f "$src" ] || continue
        full_cases=$((full_cases + 1))
        "$NATIVE" run "$src" >"$ftmp/$name.n.out" 2>"$ftmp/$name.n.err"; nrc=$?
        "$KAZE" run "$src" >"$ftmp/$name.c.out" 2>"$ftmp/$name.c.err"; crc=$?
        if [ "$nrc" -eq "$crc" ] && cmp -s "$ftmp/$name.n.out" "$ftmp/$name.c.out" && cmp -s "$ftmp/$name.n.err" "$ftmp/$name.c.err"; then
            full_passed=$((full_passed + 1))
        else
            full_failed=$((full_failed + 1))
            printf 'FULL FAIL   %-22s native rc=%d c99 rc=%d\n' "$name" "$nrc" "$crc"
            diff -u "$ftmp/$name.c.out" "$ftmp/$name.n.out" | sed 's/^/    /' | head -8
        fi
    done
    printf 'FULL        %d of %d strict+stdlib+parallel programs match\n' "$full_passed" "$full_cases"
fi

abi=1
if [ "$ONLY" = '' ]; then
    abi=0
    if abi_out=$(bash "$ROOT/tests/native/abi-layout.sh" 2>&1); then
        abi=1
        printf 'ABI         %s\n' "$abi_out"
    else
        printf 'ABI         FAIL\n'
        printf '%s\n' "$abi_out" | sed 's/^/    /'
    fi
fi

latency_line='latency skipped'
latency_ok=1
if [ "$LATENCY" -eq 1 ] && [ "$ONLY" = '' ]; then
    gen="$TMP/latency-1k.kaze"
    {
        i=0
        while [ "$i" -lt 330 ]; do
            printf 'action helper%s(x: num) -> num:\n    return x + %s\n\n' "$i" "$((i % 7))"
            i=$((i + 1))
        done
        printf 'action main() -> num:\n    return helper329(1)\n'
    } >"$gen"
    if command -v python3 >/dev/null 2>&1; then
        measured=$(python3 - "$NATIVE" "$gen" "$TMP" <<'PY'
import subprocess, sys, time
native, gen, tmp = sys.argv[1:4]
def best(cmd):
    out = 1e9
    for _ in range(5):
        t = time.perf_counter()
        p = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        if p.returncode != 0:
            return 0.0
        out = min(out, (time.perf_counter() - t) * 1000.0)
    return out
front = best([native, "asm", gen, "-o", tmp + "/lat.s"])
full = best([native, "build", gen, "-o", tmp + "/lat.bin"])
print(f"{front:.1f} {full:.1f}")
PY
)
        front=$(printf '%s' "$measured" | cut -d' ' -f1)
        full=$(printf '%s' "$measured" | cut -d' ' -f2)
        front=${front:-0}; full=${full:-0}
        # The front end + vendored QBE is the part the backend owns and the
        # plan's <50 ms budget. The system as/ld pass is reported separately.
        if awk -v f="$front" 'BEGIN { exit !(f > 0 && f < 50) }'; then
            latency_ok=1
        else
            latency_ok=0
        fi
        latency_line="front-end+QBE ${front} ms, full as+ld ${full} ms"
    else
        latency_line='python3 missing; timing not measured'
    fi
fi

abi_label='ABI assertions'
if [ "$ONLY" != '' ]; then abi_label='ABI skipped'; fi
full_label=''
if [ "$FULL" -eq 1 ]; then full_label="; full $full_passed/$full_cases"; fi
if [ "$failed" -eq 0 ] && [ "$full_failed" -eq 0 ] && [ "$abi" -eq 1 ] && [ "$latency_ok" -eq 1 ] && [ "$unsupported" -eq 0 ]; then
    echo "backend-diff: OK $passed of $cases cases$full_label; $abi_label; $latency_line"
    exit 0
fi
if [ "$cases" -eq 0 ]; then
    echo "backend-diff: no cases matched" >&2
    exit 1
fi
echo "backend-diff: FAIL $passed of $cases cases$full_label, $unsupported unsupported, $full_failed full failures; $abi_label; $latency_line"
exit 1
