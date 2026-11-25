#!/usr/bin/env bash
# phase17-gates.sh -- acceptance gates for Phase 17.
#
# Phase 17 is scalar unboxing, zero-copy string views, whole-program dead-code
# elimination, and the nine standard-library modules the phase added. Each gate
# below measures one of those claims against something that is not the compiler.
#
#   bash scripts/phase17-gates.sh
#   KAZE_P17_COMPILER=/path/to/kaze bash scripts/phase17-gates.sh
#   CC=clang REFCC=clang bash scripts/phase17-gates.sh
#   KAZE_P17_SKIP_FIXPOINT=1 bash scripts/phase17-gates.sh
#
# Gates
#   1. the shipped compiler (dist/kaze.c) builds and emits C
#   2. scalar N-body, 1,000,000 steps, within 1.10x of clang -O3 C99
#   3. JSON/text tokenization within 1.10x of clang -O3 C99 over a multi-MB file
#   4. both benchmarks clean under AddressSanitizer + UndefinedBehaviorSanitizer
#   5. a minimal program built with dead-code elimination is <= 50 KiB stripped
#   6. unreachable routines are absent from the emitted C and the binary still runs
#   7. scanning allocates nothing per byte: arena call count is flat in input size
#   8. the nine Phase 17 stdlib modules agree with CPython/NumPy under ASan
#   9. the compiler still bootstraps to a byte-identical fixed point
#
# The performance gates compare the Kaze-emitted C against the hand-written C99
# reference in scripts/phase17/ under the same optimiser *and the same
# floating-point semantics*: the emitter pins IEEE evaluation with
# "#pragma STDC FP_CONTRACT OFF" so a result does not depend on whether the
# compiler happened to fuse a multiply-add, and the reference is therefore built
# with -ffp-contract=off. Without that, the reference gets FMA fusion the
# language forbids and the comparison flatters it by about 8%.
#
# Measurement is CPU time (user+sys of the child) rather than wall time. This
# host is shared with other work: wall time inflates by up to 5% for the *same*
# binary depending on load, which is larger than the margin being tested. CPU
# time moves by under 1% run to run. The reported figure is the minimum over
# repeated runs, and a ratio over budget is re-measured up to three times before
# the gate fails.
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
TMPBASE=$(printenv TMPDIR 2>/dev/null || true)
if [ -z "$TMPBASE" ]; then TMPBASE=/tmp; fi
TMP=$(mktemp -d "$TMPBASE/kaze-phase17.XXXXXX") || exit 2
trap 'rm -rf "$TMP"' EXIT
CC=$(printenv CC 2>/dev/null || true); if [ -z "$CC" ]; then CC=cc; fi
REFCC=$(printenv REFCC 2>/dev/null || true); if [ -z "$REFCC" ]; then REFCC=clang; fi
COMPILER=$(printenv KAZE_P17_COMPILER 2>/dev/null || true)
PASS=0
FAIL=0
STARTED=$(date +%s)

# macOS ld drops unreachable functions with -dead_strip; GNU ld needs section GC.
DEADSTRIP=""
case $(uname -s) in
Darwin) DEADSTRIP="-Wl,-dead_strip" ;;
*) DEADSTRIP="-Wl,--gc-sections -ffunction-sections -fdata-sections" ;;
esac

gate_begin() {
    GATE_NAME=$1
    GATE_OUT=$TMP/gate.out
    : > "$GATE_OUT"
}
gate_end() {
    if [ "$1" -eq 0 ]; then
        PASS=$((PASS + 1))
        printf '%-46s PASS  %s\n' "$GATE_NAME" "$(tail -1 "$GATE_OUT")"
    else
        FAIL=$((FAIL + 1))
        printf '%-46s FAIL\n' "$GATE_NAME"
        sed 's/^/    /' "$GATE_OUT" | tail -12
    fi
}

# emit_build <source.kaze> <binary> <optimiser> [extra cc flags...]
emit_build() {
    src=$1
    out=$2
    opt=$3
    shift 3
    extras=$*
    if ! "$COMPILER" emit-c "$src" > "$out.c" 2> "$out.emit.err"; then
        echo "the compiler refused $src"
        head -3 "$out.emit.err"
        return 1
    fi
    if ! grep -q '^int main' "$out.c"; then
        echo "the emitted C for $src has no entry point"
        return 1
    fi
    if ! $CC $opt -std=c99 -w -I dist $DEADSTRIP $extras "$out.c" dist/kaze_rt.c -lm -o "$out" 2> "$out.cc.err"; then
        echo "the emitted C for $src did not build"
        head -3 "$out.cc.err"
        return 1
    fi
    return 0
}

# ── gate 1: the shipped compiler builds and emits C ─────────────────────
gate_begin "compiler artifact builds (dist/kaze.c)"
rc=0
if [ -z "$COMPILER" ]; then
    COMPILER="$TMP/kaze-boot"
    $CC -O2 -w -I dist dist/kaze.c dist/kaze_rt.c -lm -o "$COMPILER" >"$GATE_OUT" 2>&1 || rc=1
fi
if [ "$rc" -eq 0 ]; then
    printf 'action main() -> num:\n    return 2 + 3\n' > "$TMP/probe.kaze"
    if "$COMPILER" emit-c "$TMP/probe.kaze" 2>/dev/null | grep -q 'kaze_u__kz_main'; then
        echo "emits C from $(basename "$COMPILER")"
    else
        echo "the compiler emitted no entry routine"
        rc=1
    fi
fi
gate_end $rc

# ── gate 2: scalar N-body within 1.10x of clang -O3 ─────────────────────
gate_begin "scalar n-body <= 1.10x clang -O3"
rc=1
if emit_build scripts/phase17/bench_nbody.kaze "$TMP/nbody_kaze" "-O3"; then
    if $REFCC -O3 -std=c99 -ffp-contract=off scripts/phase17/bench_nbody_ref.c -lm -o "$TMP/nbody_ref" >>"$GATE_OUT" 2>&1; then
        python3 - "$TMP/nbody_ref" "$TMP/nbody_kaze" >>"$GATE_OUT" 2>&1 <<'PYEOF'
import resource, subprocess, sys
ref, kz = sys.argv[1], sys.argv[2]
BUDGET = 1.10
def value(binary):
    out = subprocess.run([binary], capture_output=True, text=True)
    if out.returncode != 0:
        print("FAIL %s exited %d" % (binary, out.returncode)); raise SystemExit(1)
    return float(out.stdout.strip().splitlines()[-1])
a, b = value(ref), value(kz)
scale = max(abs(a), abs(b), 1.0)
if abs(a - b) / scale > 1e-12:
    print("FAIL values differ: ref=%.17g kaze=%.17g" % (a, b)); raise SystemExit(1)
def cpu_ms(binary):
    t0 = resource.getrusage(resource.RUSAGE_CHILDREN)
    subprocess.run([binary], stdout=subprocess.DEVNULL, check=True)
    t1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    return ((t1.ru_utime - t0.ru_utime) + (t1.ru_stime - t0.ru_stime)) * 1000.0
best = None
for attempt in range(3):
    r = [cpu_ms(ref) for _ in range(9)]
    k = [cpu_ms(kz) for _ in range(9)]
    ratio = min(k) / min(r)
    best = ratio if best is None else min(best, ratio)
    if ratio <= BUDGET:
        break
print("ratio %.3f (budget %.2f, ref %.2f ms cpu, kaze %.2f ms cpu), value %.17g" % (best, BUDGET, min(r), min(k), b))
raise SystemExit(0 if best <= BUDGET else 1)
PYEOF
        rc=$?
    fi
fi
gate_end $rc

# ── gate 3: tokenization within 1.10x of clang -O3 ──────────────────────
gate_begin "json/text tokenize <= 1.10x clang -O3"
rc=1
python3 scripts/phase17/gen_tokenize_input.py "$TMP/tokens.json" >>"$GATE_OUT" 2>&1
if emit_build scripts/phase17/bench_tokenize.kaze "$TMP/tok_kaze" "-O3"; then
    if $REFCC -O3 -std=c99 -ffp-contract=off scripts/phase17/bench_tokenize_ref.c -o "$TMP/tok_ref" >>"$GATE_OUT" 2>&1; then
        python3 - "$TMP/tok_ref" "$TMP/tok_kaze" "$TMP/tokens.json" >>"$GATE_OUT" 2>&1 <<'PYEOF'
import resource, subprocess, sys
ref, kz, path = sys.argv[1], sys.argv[2], sys.argv[3]
BUDGET = 1.10
def value(binary):
    out = subprocess.run([binary, path], capture_output=True, text=True)
    if out.returncode != 0:
        print("FAIL %s exited %d" % (binary, out.returncode)); raise SystemExit(1)
    return out.stdout.strip().splitlines()[-1]
a, b = value(ref), value(kz)
if a != b:
    print("FAIL token counts differ: ref=%s kaze=%s" % (a, b)); raise SystemExit(1)
def cpu_ms(binary):
    t0 = resource.getrusage(resource.RUSAGE_CHILDREN)
    subprocess.run([binary, path], stdout=subprocess.DEVNULL, check=True)
    t1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    return ((t1.ru_utime - t0.ru_utime) + (t1.ru_stime - t0.ru_stime)) * 1000.0
best = None
for attempt in range(3):
    r = [cpu_ms(ref) for _ in range(7)]
    k = [cpu_ms(kz) for _ in range(7)]
    ratio = min(k) / min(r)
    best = ratio if best is None else min(best, ratio)
    if ratio <= BUDGET:
        break
print("ratio %.3f (budget %.2f, ref %.2f ms cpu, kaze %.2f ms cpu), digest %s" % (best, BUDGET, min(r), min(k), b))
raise SystemExit(0 if best <= BUDGET else 1)
PYEOF
        rc=$?
    fi
fi
gate_end $rc

# ── gate 4: sanitizer-clean benchmarks ──────────────────────────────────
gate_begin "asan + ubsan clean (both benchmarks)"
rc=0
# LeakSanitizer is not implemented on macOS: asking for it makes ASan abort
# before the program runs. Leak evidence on that platform is the arena
# accounting in gate 7, which counts every allocation the runtime makes.
ASAN_OPTS=""
case $(uname -s) in
Darwin) ASAN_NOTE="leak detection not available on this platform (gate 7 counts arena calls)" ;;
*)
    ASAN_OPTS="detect_leaks=1"
    ASAN_NOTE="detect_leaks=1"
    ;;
esac
if emit_build scripts/phase17/bench_nbody.kaze "$TMP/nbody_asan" "-O1" -fsanitize=address,undefined -fno-omit-frame-pointer; then
    ASAN_OPTIONS="$ASAN_OPTS" "$TMP/nbody_asan" >>"$GATE_OUT" 2>&1 || rc=1
else
    rc=1
fi
if emit_build scripts/phase17/bench_tokenize.kaze "$TMP/tok_asan" "-O1" -fsanitize=address,undefined -fno-omit-frame-pointer; then
    ASAN_OPTIONS="$ASAN_OPTS" "$TMP/tok_asan" "$TMP/tokens.json" >>"$GATE_OUT" 2>&1 || rc=1
else
    rc=1
fi
if grep -qE 'AddressSanitizer|LeakSanitizer|runtime error:|SUMMARY:' "$GATE_OUT"; then
    rc=1
fi
if [ "$rc" -eq 0 ]; then
    echo "no sanitizer diagnostics; $ASAN_NOTE"
fi
gate_end $rc

# ── gate 5: minimal program <= 50 KiB stripped ──────────────────────────
gate_begin "minimal binary <= 50 KiB stripped"
rc=1
if emit_build examples/hello.kaze "$TMP/hello" "-O3"; then
    cp "$TMP/hello" "$TMP/hello.stripped"
    strip -x "$TMP/hello.stripped" 2>/dev/null || strip "$TMP/hello.stripped" 2>/dev/null || true
    size=$(wc -c < "$TMP/hello.stripped" | tr -d " ")
    out=$("$TMP/hello" 2>/dev/null | tail -1)
    if [ "$out" != "10" ]; then
        echo "the stripped program printed '$out', expected 10"
    elif [ "$size" -le 51200 ]; then
        echo "$size bytes (limit 51200), prints 10"
        rc=0
    else
        echo "$size bytes exceeds 51200"
    fi
fi
gate_end $rc

# ── gate 6: unreachable routines are dropped ────────────────────────────
gate_begin "dead-code elimination drops unreachable routines"
rc=1
cat > "$TMP/dce.kaze" <<'KZEOF'
action used_one(x: num) -> num:
    return x + 1

action shadow_a(x: num) -> num:
    return x + 2

action shadow_b(x: num) -> num:
    return x + 3

action shadow_c(x: num) -> num:
    return x + 4

action shadow_d(x: num) -> num:
    return x + 5

action main() -> num:
    return used_one(41)
KZEOF
if "$COMPILER" emit-c "$TMP/dce.kaze" > "$TMP/dce.c" 2>>"$GATE_OUT" && [ -s "$TMP/dce.c" ]; then
    kept=0
    for name in shadow_a shadow_b shadow_c shadow_d; do
        if grep -q "kaze_fn_$name" "$TMP/dce.c"; then kept=$((kept + 1)); fi
    done
    if [ "$kept" -ne 0 ]; then
        echo "$kept unreachable routine(s) survived dead-code elimination"
    elif ! grep -q 'kaze_fn_used_one' "$TMP/dce.c"; then
        echo "the reachable routine was dropped as well"
    elif emit_build "$TMP/dce.kaze" "$TMP/dce" "-O2" && [ "$("$TMP/dce")" = "42" ]; then
        echo "4 unreachable routines dropped, program prints 42"
        rc=0
    else
        echo "the pruned program did not build or ran wrong"
    fi
fi
gate_end $rc

# ── gate 7: scanning does not allocate per byte ─────────────────────────
gate_begin "zero-copy scan: arena calls flat in input size"
rc=1
if emit_build scripts/phase17/bench_tokenize.kaze "$TMP/tok_stats" "-O2"; then
    python3 -c "import json; json.dump([{'id':i,'name':'sensor-%08d'%i,'v':[i,i+1,i+2]} for i in range(120000)], open('$TMP/large.json','w'), separators=(',',':'))"
    printf '{"a":1,"b":[2,3],"c":"x"}' > "$TMP/small.json"
    small_bytes=$(wc -c < "$TMP/small.json" | tr -d " ")
    large_bytes=$(wc -c < "$TMP/large.json" | tr -d " ")
    KAZE_ARENA_STATS=1 "$TMP/tok_stats" "$TMP/small.json" > "$TMP/stat_small.out" 2>&1
    KAZE_ARENA_STATS=1 "$TMP/tok_stats" "$TMP/large.json" > "$TMP/stat_large.out" 2>&1
    calls_small=$(awk '/kaze-alloc:/{for (i = 1; i <= NF; i++) if ($i == "calls") {c += $(i - 1)}} END{print c + 0}' "$TMP/stat_small.out")
    calls_large=$(awk '/kaze-alloc:/{for (i = 1; i <= NF; i++) if ($i == "calls") {c += $(i - 1)}} END{print c + 0}' "$TMP/stat_large.out")
    if [ "$calls_small" -eq 0 ] || [ "$calls_large" -eq 0 ]; then
        echo "no allocation statistics were reported"
    elif [ "$calls_small" -ne "$calls_large" ]; then
        echo "arena calls grew with the input: $calls_small for $small_bytes bytes vs $calls_large for $large_bytes"
    else
        echo "$calls_large arena call(s) for both "$small_bytes"B and "$large_bytes"B inputs"
        rc=0
    fi
fi
gate_end $rc

# ── gate 8: the nine modules agree with CPython/NumPy ───────────────────
gate_begin "nine stdlib modules vs CPython/NumPy (ASan)"
rc=0
count=0
for suite in base64 hex url time spatial matrix dsp heap embeddings; do
    script="scripts/phase17/diff_$suite.py"
    if [ ! -f "$script" ]; then
        echo "missing $script" >> "$GATE_OUT"
        rc=1
        continue
    fi
    out=$(KAZE_BIN="$COMPILER" KAZE_ASAN=1 python3 "$script" 2>&1)
    status=$?
    printf '%-10s %s\n' "$suite" "$(printf '%s' "$out" | tail -1)" >> "$GATE_OUT"
    if [ "$status" -ne 0 ] || ! printf '%s' "$out" | grep -q '0 mismatches'; then
        rc=1
        printf '%s\n' "$out" | tail -6 >> "$GATE_OUT"
    else
        count=$((count + 1))
    fi
done
if [ "$rc" -eq 0 ]; then
    echo "$count modules, 0 mismatches each"
fi
gate_end $rc

# ── gate 9: bootstrap fixed point preserved ─────────────────────────────
gate_begin "bootstrap fixed point preserved"
skip=$(printenv KAZE_P17_SKIP_FIXPOINT 2>/dev/null || true)
if [ "$skip" = "1" ]; then
    echo "skipped (KAZE_P17_SKIP_FIXPOINT=1)" >> "$GATE_OUT"
    rc=0
else
    bash scripts/kaze-bootstrap-verify.sh >>"$GATE_OUT" 2>&1
    rc=$?
fi
gate_end $rc

echo
total=$((PASS + FAIL))
if [ "$FAIL" -eq 0 ]; then
    elapsed=$(( $(date +%s) - STARTED ))
    echo "phase17-gates: $PASS of $total gates passed ("$elapsed"s)"
    exit 0
fi
echo "phase17-gates: $FAIL of $total gates failed"
exit 1
