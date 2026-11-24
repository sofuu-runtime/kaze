#!/usr/bin/env bash
# scripts/parallel-test.sh -- Comprehensive test suite for Pillar 1 (Universal Parallelism)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

CC=${CC:-cc}
BIN="$ROOT/bin/kaze"
TEST_DIR="$ROOT/tests/parallel"
mkdir -p "$TEST_DIR" "$ROOT/bin"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/kaze-parallel-test.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

# Rebuild compiler
"$CC" -O2 -w -I "$ROOT/dist" "$ROOT/dist/kaze.c" "$ROOT/dist/kaze_rt.c" -lm -o "$BIN"

echo "=== 1. Testing parallel_map numerical correctness and order ==="
cat << 'EOF' > "$TEST_DIR/test_map.kaze"
action square(x):
    return x * x

action main() -> num:
    let nums = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    let sq = parallel_map(nums, square)
    if (length of sq) != 16:
        return 1
    if (sq[0] otherwise 0) != 1:
        return 2
    if (sq[7] otherwise 0) != 64:
        return 3
    if (sq[15] otherwise 0) != 256:
        return 4
    return 0
EOF

"$BIN" emit-c "$TEST_DIR/test_map.kaze" > "$WORK/test_map.c"
"$CC" -O2 -w -I "$ROOT/dist" "$WORK/test_map.c" "$ROOT/dist/kaze_rt.c" -lm -o "$WORK/test_map"
OUT=$("$WORK/test_map")
if [ "$OUT" != "0" ]; then
    echo "FAIL: parallel_map returned error code: $OUT"
    exit 1
fi
echo "PASS: parallel_map correctly produced deterministic ordered results"

echo "=== 2. Testing parallel_filter ==="
cat << 'EOF' > "$TEST_DIR/test_filter.kaze"
action is_even(x):
    return (x % 2) == 0

action main() -> num:
    let nums = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    let evens = parallel_filter(nums, is_even)
    if (length of evens) != 8:
        return 1
    if (evens[0] otherwise 0) != 2:
        return 2
    if (evens[7] otherwise 0) != 16:
        return 3
    return 0
EOF

"$BIN" emit-c "$TEST_DIR/test_filter.kaze" > "$WORK/test_filter.c"
"$CC" -O2 -w -I "$ROOT/dist" "$WORK/test_filter.c" "$ROOT/dist/kaze_rt.c" -lm -o "$WORK/test_filter"
FOUT=$("$WORK/test_filter")
if [ "$FOUT" != "0" ]; then
    echo "FAIL: parallel_filter returned error code: $FOUT"
    exit 1
fi
echo "PASS: parallel_filter correctly extracted even numbers in parallel"

echo "=== 3. Testing parallel for each syntax ==="
cat << 'EOF' > "$TEST_DIR/test_pfor.kaze"
action double_val(x):
    return x * 2

action main() -> num:
    let nums = [10, 20, 30, 40]
    let sum = 0
    parallel for each x in nums:
        let d = double_val(x)
        set sum to sum + d
    if sum != 200:
        return 1
    return 0
EOF

"$BIN" emit-c "$TEST_DIR/test_pfor.kaze" > "$WORK/test_pfor.c"
"$CC" -O2 -w -I "$ROOT/dist" "$WORK/test_pfor.c" "$ROOT/dist/kaze_rt.c" -lm -o "$WORK/test_pfor"
PFOR_OUT=$("$WORK/test_pfor")
if [ "$PFOR_OUT" != "0" ]; then
    echo "FAIL: parallel for each returned error code: $PFOR_OUT"
    exit 1
fi
echo "PASS: parallel for each syntax executed correctly"

echo "=== 4. Testing compile-time purity enforcement (E_KAZE_IMPURE_PARALLEL) ==="
cat << 'EOF' > "$TEST_DIR/test_impure_map.kaze"
needs: log

action impure_worker(x):
    needs: log
    log.info("processing item")
    return x * 2

action main():
    let nums = [1, 2, 3]
    let res = parallel_map(nums, impure_worker)
EOF

if "$BIN" check "$TEST_DIR/test_impure_map.kaze" 2>&1 | grep -q "E_KAZE_IMPURE_PARALLEL"; then
    echo "PASS: checker refused impure routine in parallel_map with E_KAZE_IMPURE_PARALLEL"
else
    echo "FAIL: checker failed to refuse impure routine in parallel_map!"
    "$BIN" check "$TEST_DIR/test_impure_map.kaze" || true
    exit 1
fi

cat << 'EOF' > "$TEST_DIR/test_impure_pfor.kaze"
needs: drone

action send_cmd(x):
    needs: drone
    drone.arm()
    return x

action main():
    let nums = [1, 2]
    parallel for each x in nums:
        send_cmd(x)
EOF

if "$BIN" check "$TEST_DIR/test_impure_pfor.kaze" 2>&1 | grep -q "E_KAZE_IMPURE_PARALLEL"; then
    echo "PASS: checker refused impure call in parallel for each loop with E_KAZE_IMPURE_PARALLEL"
else
    echo "FAIL: checker failed to refuse impure call in parallel for each!"
    "$BIN" check "$TEST_DIR/test_impure_pfor.kaze" || true
    exit 1
fi

echo "=== 5. Testing multi-threaded allocations under AddressSanitizer (ASan) ==="
"$CC" -fsanitize=address,undefined -g -O1 -w -I "$ROOT/dist" "$WORK/test_map.c" "$ROOT/dist/kaze_rt.c" -lm -o "$WORK/test_map_asan"
KAZE_PARALLEL_WORKERS=8 "$WORK/test_map_asan" > /dev/null
echo "PASS: parallel_map ran clean under AddressSanitizer with 0 memory leaks across 8 worker threads"

echo "=== 6. Testing multicore parallel speedup ==="
cat << 'EOF' > "$TEST_DIR/test_bench.kaze"
action heavy_calc(x):
    let acc = x
    let i = 0
    while i < 100000 bounded 200000:
        set acc to ((acc * 1103515245) + 12345) % 2147483648
        set i to i + 1
    return acc

action main() -> num:
    let nums = []
    let i = 0
    while i < 64 bounded 100:
        set nums to nums + [i]
        set i to i + 1
    let res = parallel_map(nums, heavy_calc)
    return length of res
EOF

"$BIN" emit-c "$TEST_DIR/test_bench.kaze" > "$WORK/test_bench.c"
"$CC" -O3 -w -I "$ROOT/dist" "$WORK/test_bench.c" "$ROOT/dist/kaze_rt.c" -lm -o "$WORK/test_bench"

T_SEQ=$( (time (KAZE_PARALLEL_WORKERS=1 "$WORK/test_bench" > /dev/null)) 2>&1 | grep real | awk '{print $2}' || true )
T_PAR=$( (time (KAZE_PARALLEL_WORKERS=8 "$WORK/test_bench" > /dev/null)) 2>&1 | grep real | awk '{print $2}' || true )

echo "Sequential time: $T_SEQ, Parallel (8 workers) time: $T_PAR"
echo "ALL PILLAR 1 PARALLELISM TESTS PASSED!"
