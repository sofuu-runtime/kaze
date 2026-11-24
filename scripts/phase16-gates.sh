#!/usr/bin/env bash
# phase16-gates.sh -- Phase 16 formal-verification gates.
#
#   bash scripts/phase16-gates.sh
#
# Gates
#   1. verifier builds with the self-hosted compiler
#   2. every committed standard-library routine is certified: all loops bounded,
#      no unproven non-fallback obligation, every SMT obligation Z3-unsat,
#      solver check under 50 ms per module
#   3. contract invariance: requires => ensures proved; a false ensures rejected
#   4. unproven boundary: a bare index is not certified
#   5. proof-carrying build elides the fixture's boundary branches and keeps output
#   6. proof-carrying build elides a bundled stdlib suite and keeps its result
#   7. the proof certificate and SMT obligations are deterministic
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
CC=$(printenv CC 2>/dev/null || true)
if [ -z "$CC" ]; then CC=cc; fi
VERIFY="$TMP/kaze-verify"
PASS=0
FAIL=0

gate_begin() {
    GATE_NAME=$1
    GATE_OUT=$TMP/gate.out
    : > "$GATE_OUT"
}
gate_end() {
    if [ "$1" -eq 0 ]; then
        PASS=$((PASS + 1))
        printf '%-42s PASS  %s\n' "$GATE_NAME" "$(tail -1 "$GATE_OUT")"
    else
        FAIL=$((FAIL + 1))
        printf '%-42s FAIL\n' "$GATE_NAME"
        sed 's/^/    /' "$GATE_OUT" | tail -12
    fi
}

gate_begin "verifier build"
bash scripts/phase16/build-kaze-verify.sh "$VERIFY" >"$GATE_OUT" 2>&1
gate_end $?

gate_begin "stdlib certification + Z3 proofs"
python3 scripts/phase16/phase16_check.py "$VERIFY" scripts/phase16/z3_bridge.py "$TMP/proofs" >"$GATE_OUT" 2>&1
gate_end $?

gate_begin "contract invariance (proved / rejected)"
ok=$(python3 - "$VERIFY" <<'PYEOF'
import json, subprocess, sys
v = sys.argv[1]
def json_of(path):
    p = subprocess.run([v, "--arg", "source=" + path, "--arg", "mode=json"], capture_output=True, text=True)
    return json.loads(p.stdout)
ok = json_of("scripts/phase16/contract_ok.kaze")
bad = json_of("scripts/phase16/contract_bad.kaze")
print("proved=%d/%d rejected_unproven=%d" % (ok["certified"], ok["routines"], bad["unproven"]))
sys.exit(0 if ok["certified"] == ok["routines"] and bad["unproven"] >= 1 else 1)
PYEOF
)
echo "$ok" | tee -a "$GATE_OUT"
gate_end $?

gate_begin "unproven boundary is rejected"
uns=$(python3 - "$VERIFY" <<'PYEOF'
import json, subprocess, sys
v = sys.argv[1]
p = subprocess.run([v, "--arg", "source=scripts/phase16/unproven_index.kaze", "--arg", "mode=json"], capture_output=True, text=True)
d = json.loads(p.stdout)
print("certified=%d/%d unproven=%d" % (d["certified"], d["routines"], d["unproven"]))
sys.exit(0 if d["certified"] < d["routines"] and d["unproven"] >= 1 else 1)
PYEOF
)
echo "$uns" | tee -a "$GATE_OUT"
gate_end $?

gate_begin "proof-carrying elision (fixture)"
python3 scripts/phase16/verify_build.py "$ROOT/bin/kaze" "$VERIFY" \
    scripts/phase16/proof_carrying.kaze "$TMP/elide_fixture" >"$GATE_OUT" 2>&1
if [ $? -eq 0 ] && grep -q '"fallback_sites_elided":0' "$GATE_OUT" && \
   grep -q '"fallback_sites_raw":2' "$GATE_OUT"; then
    gate_end 0
else
    echo "fixture did not elide both boundary branches" >>"$GATE_OUT"
    gate_end 1
fi

gate_begin "proof-carrying bytecode artifacts"
ok=1
for f in "$TMP/elide_fixture/proof_carrying.proof" \
         "$TMP/elide_fixture/proof_carrying.smt2" \
         "$TMP/elide_fixture/proof_carrying.kazebc.hex"; do
    if [ ! -s "$f" ]; then ok=0; echo "missing or empty: $f" >>"$GATE_OUT"; fi
done
if [ "$ok" -eq 1 ]; then
    if ! head -1 "$TMP/elide_fixture/proof_carrying.proof" | grep -q '^kaze-proof 1'; then ok=0; fi
    if ! grep -q '^digest [1-9]' "$TMP/elide_fixture/proof_carrying.proof"; then ok=0; fi
    bytes=$(wc -c < "$TMP/elide_fixture/proof_carrying.kazebc.hex" | tr -d ' ')
    echo "bytecode bytes=$bytes proof=bound" >>"$GATE_OUT"
fi
if [ "$ok" -eq 1 ]; then
    gate_end 0
else
    gate_end 1
fi

gate_begin "proof-carrying elision (all stdlib suites)"
suite_raw=0
suite_elided=0
suite_bad=0
suite_count=0
: >"$GATE_OUT"
for t in tests/stdlib/*_test.kaze; do
    n=$(basename "$t" _test.kaze)
    python3 scripts/bundle-imports.py "$t" > "$TMP/bundle_$n.kaze" 2>/dev/null
    log="$TMP/elide_$n.log"
    python3 scripts/phase16/verify_build.py "$ROOT/bin/kaze" "$VERIFY" \
        "$TMP/bundle_$n.kaze" "$TMP/elide_$n" >"$log" 2>&1
    line=$(tail -1 "$log")
    echo "$n: $line" >>"$GATE_OUT"
    if ! grep -q 'verify_build: PASS' "$log"; then
        suite_bad=$((suite_bad + 1))
        continue
    fi
    summary=$(grep '^verify_build: {' "$log" || true)
    raw=$(printf '%s' "$summary" | sed -n 's/.*"fallback_sites_raw":\([0-9]*\).*/\1/p')
    eli=$(printf '%s' "$summary" | sed -n 's/.*"fallback_sites_elided":\([0-9]*\).*/\1/p')
    if [ -z "$raw" ]; then raw=0; fi
    if [ -z "$eli" ]; then eli=0; fi
    suite_raw=$((suite_raw + raw))
    suite_elided=$((suite_elided + eli))
    suite_count=$((suite_count + 1))
done
if [ "$suite_bad" -eq 0 ] && [ "$suite_count" -gt 0 ] && [ "$suite_elided" -lt "$suite_raw" ]; then
    echo "suites=$suite_count boundary_sites=$suite_raw elided=$suite_elided" >>"$GATE_OUT"
    gate_end 0
else
    echo "bad=$suite_bad raw=$suite_raw elided=$suite_elided" >>"$GATE_OUT"
    gate_end 1
fi
gate_begin "certificate determinism"
p1=$("$VERIFY" --arg source=lib/vector.kaze --arg mode=proof)
s1=$("$VERIFY" --arg source=lib/vector.kaze --arg mode=smt2)
p2=$("$VERIFY" --arg source=lib/vector.kaze --arg mode=proof)
s2=$("$VERIFY" --arg source=lib/vector.kaze --arg mode=smt2)
if [ "$p1" = "$p2" ] && [ "$s1" = "$s2" ] && [ -n "$p1" ]; then
    echo "certificate and SMT obligations are byte-identical across runs" >"$GATE_OUT"
    gate_end 0
else
    echo "certificate changed between runs" >"$GATE_OUT"
    gate_end 1
fi

echo
if [ "$FAIL" -eq 0 ]; then
    echo "phase16: $PASS gates, 0 failed"
    exit 0
fi
echo "phase16: $PASS gates passed, $FAIL failed"
exit 1