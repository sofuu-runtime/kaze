#!/usr/bin/env bash
# gates.sh -- every Phase 8-10 gate in one command, with no Rust toolchain.
#
# Phase 10 removed the Rust engine, so every claim below is checked against the
# self-hosted compiler, the frozen reference data in scripts/selftest/, or the runtime
# itself. Each gate prints PASS/FAIL with its summary line; the exit status is 0 only
# when every gate passed.
#
#   bash scripts/gates.sh            # all gates
#   bash scripts/gates.sh --list     # names only
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
FAILED=0
COUNT=0

run() {
    name=$1
    shift
    COUNT=$((COUNT + 1))
    printf '%-32s' "$name"
    if out=$("$@" 2>&1); then
        summary=$(printf '%s' "$out" | grep -E 'summary|agree|match|valid|checks|current|OK|PASS|failures|formatted' | tail -1 | cut -c1-62)
        echo "PASS  $summary"
    else
        FAILED=$((FAILED + 1))
        echo 'FAIL'
        printf '%s\n' "$out" | tail -5 | sed 's/^/    /'
    fi
}

if [ "${1:-}" = '--list' ]; then
    sed -n 's/^run \(.[^ ]*\).*/\1/p' "$0" | tr -d "'"
    exit 0
fi

run 'bootstrap + fixed point' bash scripts/kaze-bootstrap-verify.sh
run 'verdict parity' bash scripts/golden-check.sh
run 'bytecode writer' env KAZE_BIN="$ROOT/bin/kaze" python3 scripts/bytecode-check.py
run 'conformance harness' bash scripts/self-test.sh
run 'interactive LSP' env KAZE_BIN="$ROOT/bin/kaze" python3 scripts/host-lsp-smoke.py "$ROOT/bin/kaze-host"
run 'hostile client' env KAZE_BIN="$ROOT/bin/kaze" python3 scripts/host-hostile.py "$ROOT/bin/kaze-host"
run 'batch LSP' python3 scripts/lsp-smoke.py
run 'standard library suites' bash scripts/stdlib-test.sh
run 'arena-only allocation' bash scripts/alloc-audit.sh
run 'json vs cpython (rfc 8259)' python3 scripts/json-conformance-diff.py
run 'formatter' bash scripts/fmt-check.sh
run 'phase 7 strict gates' bash scripts/phase7-gates.sh
run 'concurrency (phase 13)' bash scripts/phase13-gates.sh
run 'tooling' bash scripts/tooling-smoke.sh
run 'http' python3 scripts/http-smoke.py
run 'parallel' bash scripts/phase7-bench.sh
run 'dist regenerates itself' bash scripts/regen-dist.sh

echo
if [ "$FAILED" -eq 0 ]; then
    echo "gates: $COUNT of $COUNT passed (no Rust toolchain used)"
    exit 0
fi
echo "gates: $FAILED of $COUNT failed"
exit 1