#!/usr/bin/env bash
# phase7-bench.sh -- reproducible Phase 7 evidence for dense matmul.
#
# The candidate subset is a pure, independent output partition: every element
# of C = A*B accumulates over k in the same order in the serial and parallel
# runs, so the results must be bit-for-bit identical. This script measures that
# property and the speedup on this machine; it does not claim that Kaze's
# automatic parallelism or GPU execution exists.
#
#   bash scripts/phase7-bench.sh
#
# No model weights and no network are used.
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
CC=${CC:-cc}
BIN="$ROOT/bin/kaze-parallel-bench"

if ! "$CC" -std=c99 -O2 -pthread -w "$ROOT/dist/kaze_parallel_bench.c" -o "$BIN" 2>"$ROOT/bin/kaze-parallel-bench.err"; then
    echo "phase7-bench: the benchmark did not build" >&2
    head -5 "$ROOT/bin/kaze-parallel-bench.err" >&2
    exit 1
fi
rm -f "$ROOT/bin/kaze-parallel-bench.err"

cores=$(sysctl -n hw.ncpu 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)
printf 'host: %s %s, %s logical cores\n' "$(uname -s)" "$(uname -m)" "$cores"
printf 'compiler: %s\n' "$("$CC" --version 2>/dev/null | head -1)"
printf '\n%-6s %-8s %12s %12s %10s %6s\n' size threads serial-ms parallel-ms speedup exact

fail=0
for size in 128 192 256; do
    for threads in 1 2 4 8; do
        [ "$threads" -le "$cores" ] || continue
        out=$("$BIN" "$size" "$threads") || fail=$((fail + 1))
        printf '%-6s %-8s %12s %12s %10s %6s\n' "$size" "$threads" \
            "$(printf '%s\n' "$out" | awk '$1 == "serial-ms" { print $2 }')" \
            "$(printf '%s\n' "$out" | awk '$1 == "parallel-ms" { print $2 }')" \
            "$(printf '%s\n' "$out" | awk '$1 == "speedup" { print $2 }')" \
            "$(printf '%s\n' "$out" | awk '$1 == "exact" { print $2 }')"
    done
done

if [ "$fail" -ne 0 ]; then
    echo "phase7-bench: $fail run(s) disagreed with the serial result" >&2
    exit 1
fi
cat <<'EOF'

decision: dense matmul is a valid candidate for an automatic-parallelism
subset because row partitioning preserves the accumulation order and the
results are exact. The prototype is a C kernel, not a compiler pass; Kaze
programs still execute sequentially. Phase 13 fibers are cooperative and
single-threaded, and the Phase 12 Metal path is host-backed model inference,
not general Kaze compute. GPU support for arbitrary Kaze code remains open.
EOF
