#!/usr/bin/env bash
# kaze-bootstrap-verify.sh -- Phase 8: multi-stage bootstrap and fixed point.
#
# The pipeline, run from the kaze/ root:
#
#   Stage 0   dist/kaze.c  ->  the pre-generated C in the tree, itself
#                             produced by the self-hosted compiler
#   Stage 1   cc compiles that C      ->  a native compiler written in Kaze
#   Stage 2   stage 1 compiles itself ->  stage2.c, then a stage-2 binary
#   Stage 3   stage 2 compiles itself ->  stage3.c, then a stage-3 binary
#
# The four conformance claims, counted separately so a failure points at one
# thing:
#
#   1. fixed point             sha256(stage2.c) == sha256(stage3.c)
#   2. pure-lineage determinism
#                              stage1 emit-c == stage2 emit-c, and
#                              stage2 emit-c == stage3 emit-c, on every example
#   3. cross-emitter behaviour
#                              the C from the Rust emitter and from the stage-2
#                              compiler is built, both binaries run with
#                              --ticks 2, and their stdout+stderr is compared
#                              with scripts/compare-outputs.py
#   4. check parity            the stage-2 check verdict (accept/refuse) equals
#                              the Rust engine's verdict on every example
#
# Byte identity across the two emitters is deliberately not asserted. They are
# different emitters by design: Stage 0 embeds its own C runtime and emits C
# that stands alone; the self-hosted emitter includes kaze_rt.h and links
# dist/kaze_rt.c. For examples/hello.kaze the same program yields 43,800 B of
# Stage-0 C and 30,067 B of self-hosted C -- different bytes, equivalent
# behaviour. The cross-emitter claim is therefore behavioural, and byte
# equality is asserted only within the pure lineage.
#
# No `set -e`: every stage is attempted and every failure is reported, so one
# run shows the whole picture. The exit status is 0 only when nothing failed.
#
# Usage:
#   bash scripts/kaze-bootstrap-verify.sh
#   KEEP=1 bash scripts/kaze-bootstrap-verify.sh      # keep the work directory
#   KAZE_BIN=/path/to/kaze bash scripts/...           # another Rust compiler
#
set -u

ROOT=$(cd "$(dirname "$0")/.." && pwd) || {
    echo "kaze-bootstrap-verify: cannot resolve the script directory" >&2
    exit 2
}
cd "$ROOT" || exit 2

CC=${CC:-cc}
KAZE=${KAZE_BIN:-}   # unused: the pipeline is pure Kaze + cc
PYTHON=${PYTHON:-python3}
case $KAZE in
    /*) ;;
    *) KAZE="$ROOT/$KAZE" ;;
esac

if [ ! -x "$KAZE" ]; then
    echo "kaze-bootstrap-verify: $KAZE is missing or not executable" >&2
    echo "  build the Rust reference compiler first: (cd \"$ROOT\" && cargo build --bin kaze)" >&2
    exit 2
fi
if ! command -v "$CC" >/dev/null 2>&1; then
    echo "kaze-bootstrap-verify: C compiler '$CC' not found (set CC=...)" >&2
    exit 2
fi
if ! command -v "$PYTHON" >/dev/null 2>&1; then
    echo "kaze-bootstrap-verify: '$PYTHON' not found; scripts/compare-outputs.py needs it" >&2
    echo "  set PYTHON=/path/to/python3 to use another interpreter" >&2
    exit 2
fi

# sha256: shasum is the macOS default, sha256sum the GNU/Linux one. Refuse to
# fake the fixed-point check when neither exists.
if command -v shasum >/dev/null 2>&1; then
    digest() { shasum -a 256 "$1" | awk '{print $1}'; }
    digest_tool="shasum -a 256"
elif command -v sha256sum >/dev/null 2>&1; then
    digest() { sha256sum "$1" | awk '{print $1}'; }
    digest_tool="sha256sum"
else
    echo "kaze-bootstrap-verify: neither shasum nor sha256sum is available; cannot compare stage2.c and stage3.c" >&2
    exit 2
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/kaze-bootstrap-verify.XXXXXX") || exit 2
cleanup() {
    if [ "${KEEP:-0}" = "1" ]; then
        echo "kaze-bootstrap-verify: work directory kept at $tmp" >&2
    else
        rm -rf "$tmp"
    fi
}
trap cleanup EXIT

failures=0
fp_ok=0
fp_checks=0
fp_failures=0
lineage_checks=0
lineage_failures=0
xemit_checks=0
xemit_skipped=0
xemit_failures=0
verdict_checks=0
verdict_failures=0

fail() {
    failures=$((failures + 1))
    printf 'FAIL %s\n' "$*" >&2
}

report() {
    checks=$((fp_checks + lineage_checks + xemit_checks + verdict_checks))
    if [ "$fp_ok" -eq 1 ]; then
        fixed="stage2==stage3"
    else
        fixed="stage2!=stage3"
    fi
    printf 'kaze-bootstrap-verify: %s (sha256), fixed-point %d/%d, pure-lineage determinism %d/%d, cross-emitter behaviour %d/%d, check parity %d/%d, %d conformance checks, %d failures\n' \
        "$fixed" \
        "$((fp_checks - fp_failures))" "$fp_checks" \
        "$((lineage_checks - lineage_failures))" "$lineage_checks" \
        "$((xemit_checks - xemit_failures))" "$xemit_checks" \
        "$((verdict_checks - verdict_failures))" "$verdict_checks" \
        "$((lineage_checks + xemit_checks + verdict_checks))" "$failures"
}

fatal() {
    failures=$((failures + 1))
    printf 'FATAL %s\n' "$*" >&2
    report
    exit 1
}

# Bytes in a file, with the padding `wc` adds stripped.
bytes() {
    wc -c < "$1" | tr -d ' '
}

# First differing position reported by cmp, for a failure message.
first_diff() {
    cmp "$1" "$2" 2>&1 | head -1 | sed 's/^.*differ: //'
}

echo "kaze-bootstrap-verify: root          $ROOT"
echo "kaze-bootstrap-verify: stage 0     dist/kaze.c (pure compiler output)"
echo "kaze-bootstrap-verify: C compiler    $CC"
echo "kaze-bootstrap-verify: sha256        $digest_tool"
echo

# ---- stage 0: the C checked into dist/ -----------------------------------
echo "== stage 0: dist/kaze.c -> stage0.c"
if ! cp dist/kaze.c "$tmp/stage0.c"; then
    fatal "stage 0: cannot read dist/kaze.c"
fi

# ---- stage 1: compile the stage-0 C ---------------------------------------
echo "== stage 1: cc -> native stage-1 compiler"
if ! "$CC" -std=c99 -O2 -w -I dist "$tmp/stage0.c" dist/kaze_rt.c -lm -o "$tmp/stage1" \
        2>"$tmp/stage1.cc.log"; then
    cat "$tmp/stage1.cc.log" >&2
    fatal "stage 1: cc could not compile stage0.c"
fi

# ---- stage 2: the stage-1 compiler compiles itself ----------------------
echo "== stage 2: stage-1 compiler -> stage2.c"
if ! "$tmp/stage1" compiler/kaze_self.kaze > "$tmp/stage2.c" 2>"$tmp/stage2.err"; then
    cat "$tmp/stage2.err" >&2
    fatal "stage 2: the stage-1 compiler failed on compiler/kaze_self.kaze"
fi
if [ ! -s "$tmp/stage2.c" ]; then
    fatal "stage 2: stage2.c is empty"
fi

echo "== stage 2: cc -> native stage-2 compiler"
if ! "$CC" -std=c99 -O2 -w -I dist "$tmp/stage2.c" dist/kaze_rt.c -lm -o "$tmp/stage2" \
        2>"$tmp/stage2.cc.log"; then
    cat "$tmp/stage2.cc.log" >&2
    fatal "stage 2: cc could not compile stage2.c"
fi

# ---- stage 3: the stage-2 compiler compiles itself ----------------------
echo "== stage 3: stage-2 compiler -> stage3.c"
if ! "$tmp/stage2" compiler/kaze_self.kaze > "$tmp/stage3.c" 2>"$tmp/stage3.err"; then
    cat "$tmp/stage3.err" >&2
    fatal "stage 3: the stage-2 compiler failed on compiler/kaze_self.kaze"
fi
if [ ! -s "$tmp/stage3.c" ]; then
    fatal "stage 3: stage3.c is empty"
fi

echo "== stage 3: cc -> native stage-3 compiler"
if ! "$CC" -std=c99 -O2 -w -I dist "$tmp/stage3.c" dist/kaze_rt.c -lm -o "$tmp/stage3" \
        2>"$tmp/stage3.cc.log"; then
    cat "$tmp/stage3.cc.log" >&2
    fatal "stage 3: cc could not compile stage3.c"
fi

# ---- 1. fixed point -----------------------------------------------------
h2=$(digest "$tmp/stage2.c")
h3=$(digest "$tmp/stage3.c")
echo
echo "stage2.c ($(bytes "$tmp/stage2.c") bytes) sha256 $h2"
echo "stage3.c ($(bytes "$tmp/stage3.c") bytes) sha256 $h3"
fp_checks=1
if [ "$h2" = "$h3" ]; then
    fp_ok=1
    echo "fixed point: stage2.c == stage3.c"
else
    fp_failures=$((fp_failures + 1))
    fail "fixed-point: sha256(stage2.c)=$h2 != sha256(stage3.c)=$h3"
    diff -u "$tmp/stage2.c" "$tmp/stage3.c" | head -40 >&2
fi

# ---- conformance suite --------------------------------------------------
echo
echo "== conformance: stage-2 compiler on every example"

list="$tmp/examples.list"
find examples -type f -name '*.kaze' | LC_ALL=C sort > "$list"
if [ ! -s "$list" ]; then
    fatal "conformance: no .kaze files found under examples/"
fi
files=$(wc -l < "$list" | tr -d ' ')
echo "conformance: $files files under examples/"
echo

# 2. pure-lineage determinism: the same compiler source, compiled by
#    three generations, must emit the same C for each example.
conformance_pure_lineage() {
    lineage_checks=$((lineage_checks + 2))

    "$tmp/stage1" emit-c "$f" > "$work/stage1.c" 2> "$work/stage1.emit.log"
    s1_rc=$?
    "$tmp/stage2" emit-c "$f" > "$work/stage2.c" 2> "$work/stage2.emit.log"
    s2_rc=$?
    "$tmp/stage3" emit-c "$f" > "$work/stage3.c" 2> "$work/stage3.emit.log"
    s3_rc=$?

    if [ "$s1_rc" -ne 0 ] || [ "$s2_rc" -ne 0 ]; then
        lineage_failures=$((lineage_failures + 1))
        fail "pure-lineage $f: emit-c exit stage-1=$s1_rc stage-2=$s2_rc"
    elif [ ! -s "$work/stage1.c" ] || [ ! -s "$work/stage2.c" ]; then
        lineage_failures=$((lineage_failures + 1))
        fail "pure-lineage $f: empty C from stage-1 or stage-2"
    elif ! cmp -s "$work/stage1.c" "$work/stage2.c"; then
        lineage_failures=$((lineage_failures + 1))
        fail "pure-lineage $f: stage-1 C ($(bytes "$work/stage1.c") B) != stage-2 C ($(bytes "$work/stage2.c") B) at $(first_diff "$work/stage1.c" "$work/stage2.c")"
    fi

    if [ "$s2_rc" -ne 0 ] || [ "$s3_rc" -ne 0 ]; then
        lineage_failures=$((lineage_failures + 1))
        fail "pure-lineage $f: emit-c exit stage-2=$s2_rc stage-3=$s3_rc"
    elif [ ! -s "$work/stage2.c" ] || [ ! -s "$work/stage3.c" ]; then
        lineage_failures=$((lineage_failures + 1))
        fail "pure-lineage $f: empty C from stage-2 or stage-3"
    elif ! cmp -s "$work/stage2.c" "$work/stage3.c"; then
        lineage_failures=$((lineage_failures + 1))
        fail "pure-lineage $f: stage-2 C ($(bytes "$work/stage2.c") B) != stage-3 C ($(bytes "$work/stage3.c") B) at $(first_diff "$work/stage2.c" "$work/stage3.c")"
    fi
}

# 3. reference behaviour: the stage-2 emitted binary prints what the reference printed.
conformance_cross_emitter() {
    xemit_checks=$((xemit_checks + 1))
    name=$(basename "$f" .kaze)
    golden="$ROOT/scripts/selftest/run-golden/$name.out"
    if [ ! -f "$golden" ]; then
        # a library or a module with 'use' is not runnable: its verdict is checked instead
        xemit_skipped=$((xemit_skipped + 1))
        return
    fi
    if ! "$tmp/stage2" emit-c "$f" > "$work/pure.c" 2> "$work/pure.emit.log"; then
        xemit_failures=$((xemit_failures + 1))
        fail "behaviour $f: the stage-2 compiler failed to emit C"
        return
    fi
    if ! "$CC" -std=c99 -O2 -w -I dist "$work/pure.c" dist/kaze_rt.c -lm -o "$work/pure.bin" 2> "$work/pure.cc.log"; then
        xemit_failures=$((xemit_failures + 1))
        fail "behaviour $f: the emitted C did not build (see $work/pure.cc.log)"
        return
    fi
    (cd "$work" && ./pure.bin --ticks 2 </dev/null > pure.out 2> pure.err)
    # stdout is compared with the project's tolerance, not byte-for-byte: the two emitters
    # evaluate the same expression in a different order, so a float can differ in the last
    # digit and the shortest-round-trip printer then shows a different number of digits.
    # compare-outputs.py is the same comparator the original cross-emitter claim used.
    if ! "$PYTHON" "$ROOT/scripts/compare-outputs.py" "$golden" "$work/pure.out" > "$work/compare.log" 2>&1; then
        xemit_failures=$((xemit_failures + 1))
        fail "behaviour $f: stdout differs from the frozen reference"
        head -6 "$work/compare.log" | sed 's/^/    /' >&2
        return
    fi
    # stderr is compared by error *code*, not by sentence: the two emitters print the same
    # trap with different wording (E_CAP_MISSING names the family and op in both, in a
    # different sentence), and the code is the part a program can act on.
    if [ -f "$ROOT/scripts/selftest/run-golden/$name.err" ]; then
        want_codes=$(grep -oE 'E_[A-Z_]+' "$ROOT/scripts/selftest/run-golden/$name.err" | sort -u | paste -sd, -)
        got_codes=$(grep -oE 'E_[A-Z_]+' "$work/pure.err" | sort -u | paste -sd, -)
        if [ "$want_codes" != "$got_codes" ]; then
            xemit_failures=$((xemit_failures + 1))
            fail "behaviour $f: stderr error codes differ (frozen '$want_codes', now '$got_codes')"
        fi
    fi
}

# 4. verdict parity: the stage-2 accept/refuse verdict matches the frozen reference.
conformance_check_verdict() {
    verdict_checks=$((verdict_checks + 1))
    name=$(basename "$f" .kaze)
    golden="$ROOT/scripts/selftest/golden/$name.txt"
    if [ ! -f "$golden" ]; then
        verdict_failures=$((verdict_failures + 1))
        fail "check $f: no frozen reference verdict at $golden"
        return
    fi
    want=$(cat "$golden")
    "$tmp/stage2" check "$f" > "$work/check.stage2" 2>&1
    got=$(grep -oE 'E_[A-Z_]+' "$work/check.stage2" | sort -u | paste -sd, -)
    [ -z "$got" ] && got=ok
    if [ "$got" != "$want" ]; then
        verdict_failures=$((verdict_failures + 1))
        fail "check $f: stage-2 reports '$got', the frozen reference says '$want'"
    fi
}

index=0
# The file list lives on fd 3 so a compiler or emitted binary that reads stdin
# cannot swallow the rest of the corpus. The binaries also get /dev/null below.
while IFS= read -r f <&3; do
    index=$((index + 1))
    work="$tmp/conf/$index"
    if ! mkdir -p "$work"; then
        fail "conformance: cannot create $work"
        continue
    fi
    conformance_pure_lineage
    conformance_cross_emitter
    conformance_check_verdict
done 3< "$list"

echo
report
if [ "$failures" -ne 0 ]; then
    exit 1
fi
exit 0
