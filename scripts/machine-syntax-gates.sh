#!/usr/bin/env bash
# machine-syntax-gates.sh -- acceptance gates for plan 16.
#
#   bash scripts/machine-syntax-gates.sh
#   KAZE_GATE_NO_PROOF=1 bash scripts/machine-syntax-gates.sh
#   KAZE_GATE_BIN=/path/to/kaze bash scripts/machine-syntax-gates.sh
#
# Gates
#   1. `to-machine` emits `(kaze 2 ...)`; `--format=1` still round-trips
#   2. `(kaze 2 ...)` uses <= 50% of format-1 BPE tokens on the corpus
#   3. no `(unsupported*)` nodes remain in compiler/*.kaze, lib/*.kaze, or
#      tests/stdlib/*.kaze (plan 16.2)
#   4. HUMAN -> MACHINE(2) -> HUMAN keeps check diagnostics and emit-c output
#   5. every lib/ + tests/stdlib/ + tests/native/cases/ file validates against
#      dist/grammars/kaze_machine.gbnf
#   6. typed holes: normal check fails, `check --holes --json` emits contexts
#   7. `kaze patch` replaces one item, preserves the rest, is atomic, and can
#      cross-convert a machine-syntax target
#   8. a single-hole repair driven by the JSON context keeps the rest intact
#   9. `kaze ast-hash` ignores comments and tracks semantic edits
#  10. `.kaze-cache` reuses objects after cosmetic edits
#  11. Phase 16 proof certificates reuse Z3 verdicts by ast-hash
#  12. constrained generation with a local GGUF produces only grammar-valid
#      machine syntax (skipped when no local weights are present)
set -u
cd "$(dirname "$0")/.." || exit 2

ROOT=$(pwd)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/kaze-machine-gate.XXXXXX")
trap 'rm -rf "$TMP"' EXIT INT TERM

BIN=${KAZE_GATE_BIN:-$ROOT/bin/kaze}
TOKENIZER=${KAZE_TOKENIZER:-$HOME/llama.cpp/build/bin/llama-tokenize}
GBNF_VALIDATOR=${KAZE_GBNF_VALIDATOR:-$HOME/llama.cpp/build/bin/test-gbnf-validator}
GBNF="$ROOT/dist/grammars/kaze_machine.gbnf"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); printf '%-54s PASS\n' "$1"; }
fail() { FAIL=$((FAIL + 1)); printf '%-54s FAIL\n' "$1"; shift; printf '%s\n' "$@" | sed 's/^/    /' | head -14; }
note() { printf '     %s\n' "$*"; }

if [ ! -x "$BIN" ]; then
    echo "machine-syntax-gates: no binary at $BIN (run scripts/build-cli.sh)" >&2
    exit 2
fi

# ---------------------------------------------------------------- corpus
CORPUS=()
# Plan 16.2 names compiler/*.kaze as well as lib/ and tests/stdlib/. The
# self-hosted compiler is the largest single file here, so this is also the
# end-to-end scalability check for the reader and the emitter.
for f in tests/native/cases/*.kaze lib/*.kaze tests/stdlib/*.kaze \
         tests/fixtures/syntax/*.kaze compiler/*.kaze; do
    [ -f "$f" ] && CORPUS+=("$f")
done

normalize_check() {
    "$BIN" check "$1" 2>&1 | sed -E 's/^.*(E_[A-Z_]+) at line [0-9]+:/\1:/; s|^[^ ]*:[0-9]+: |X: |'
}

normalize_c() {
    sed -E 's/^#line [0-9]+$/#line N/; s/KzM_[0-9a-f]{8}/KzM_HASH/g; s/kz_m_[0-9a-f]{8}/kz_m_HASH/g; s/_kz_loop_l[0-9]+/_kz_loop_lN/g; s/_kz_[A-Za-z0-9_]*_l[0-9]+/_kz_LOOPN/g; s/kz_hook_(when|every)_[0-9]+/kz_hook_\1_N/g; s/at line [0-9]+/at line N/g'
}

# ---------------------------------------------------------------- gate 1
if "$BIN" to-machine tests/native/cases/arith.kaze >"$TMP/default.m" 2>"$TMP/e" &&
   [ "$(head -1 "$TMP/default.m")" = "(kaze 2" ]; then
    pass "default to-machine emits (kaze 2 ...)"
else
    fail "default to-machine emits (kaze 2 ...)" "$(head -3 "$TMP/e")" "$(head -2 "$TMP/default.m")"
fi

if "$BIN" to-machine --format=1 tests/native/cases/arith.kaze >"$TMP/f1.m" 2>"$TMP/e" &&
   grep -q '(format 1)' "$TMP/f1.m" &&
   "$BIN" to-human "$TMP/f1.m" >"$TMP/f1.kaze" 2>"$TMP/e" &&
   "$BIN" check "$TMP/f1.kaze" >"$TMP/o" 2>&1; then
    pass "format 1 remains readable"
else
    fail "format 1 remains readable" "$(head -4 "$TMP/e")" "$(head -4 "$TMP/o" 2>/dev/null)"
fi

# ---------------------------------------------------------------- gate 2
: >"$TMP/corpus1"
: >"$TMP/corpus2"
# Plan 16.1 sets the token budget on the core corpus (tests/native/cases and
# lib); the stdlib is included in every other gate below.
for f in tests/native/cases/*.kaze lib/*.kaze; do
    [ -f "$f" ] || continue
    "$BIN" to-machine --format=1 "$f" >>"$TMP/corpus1" 2>/dev/null
    "$BIN" to-machine "$f" >>"$TMP/corpus2" 2>/dev/null
done

if [ ! -x "$TOKENIZER" ]; then
    fail "token reduction <= 0.50" "no tokenizer at $TOKENIZER (KAZE_TOKENIZER may override)"
else
    token_models=()
    for m in "$HOME/Downloads/Llama-3.2-1B-Instruct-Q8_0.gguf" \
             "$HOME/Downloads/Qwen3-8B-Q4_K_M.gguf"; do
        [ -f "$m" ] && token_models+=("$m")
    done
    if [ "${#token_models[@]}" -eq 0 ]; then
        fail "token reduction <= 0.50" "no local GGUF weights in ~/Downloads (never download per AGENTS.md)"
    else
        tok_bad=0
        for m in "${token_models[@]}"; do
            a=$("$TOKENIZER" -m "$m" -f "$TMP/corpus1" --no-bos 2>/dev/null | grep -c ' -> ')
            b=$("$TOKENIZER" -m "$m" -f "$TMP/corpus2" --no-bos 2>/dev/null | grep -c ' -> ')
            ratio=$(python3 -c "print('%.4f' % ($b / $a))" 2>/dev/null || echo 9.9999)
            note "$(basename "$m"): format1=$a format2=$b ratio=$ratio"
            python3 -c "import sys; sys.exit(0 if $b / $a <= 0.50 else 1)" 2>/dev/null || tok_bad=$((tok_bad + 1))
        done
        if [ "$tok_bad" -eq 0 ]; then
            pass "token reduction <= 0.50"
        else
            fail "token reduction <= 0.50" "one or more tokenizers exceeded the budget"
        fi
    fi
fi

# ---------------------------------------------------------------- gate 3
# Plan 16.2 names the compiler's own sources, lib/, and tests/stdlib/ exactly.
: >"$TMP/plan16_2"
plan_emit_bad=0
for f in compiler/*.kaze lib/*.kaze tests/stdlib/*.kaze; do
    [ -f "$f" ] || continue
    if ! "$BIN" to-machine "$f" >>"$TMP/plan16_2" 2>/dev/null; then
        note "plan 16 source failed to emit: $f"
        plan_emit_bad=$((plan_emit_bad + 1))
    fi
done
if [ "$plan_emit_bad" -ne 0 ]; then
    fail "no (unsupported*) nodes" "$plan_emit_bad plan-16 source(s) failed to emit"
elif grep -qE '\(unsupported' "$TMP/plan16_2"; then
    fail "no (unsupported*) nodes" "$(grep -o '([^ ()]*unsupported[^ ()]*' "$TMP/plan16_2" | sort -u | head -10)"
else
    pass "no (unsupported*) nodes"
fi

# ---------------------------------------------------------------- gate 4
rt_bad=0
rt_total=0
for f in "${CORPUS[@]}"; do
    rt_total=$((rt_total + 1))
    # The round-trip file has to sit next to the original so relative `use`
    # paths keep resolving to the same modules.
    rt_human=$(mktemp "${f}.rt.XXXXXX") || { rt_bad=$((rt_bad + 1)); continue; }
    if ! "$BIN" to-machine "$f" >"$TMP/rt.m" 2>"$TMP/rt.e"; then
        note "emit failed: $f"; rm -f "$rt_human"; rt_bad=$((rt_bad + 1)); continue
    fi
    if ! "$BIN" to-human "$TMP/rt.m" >"$rt_human" 2>"$TMP/rt.e"; then
        note "to-human failed: $f"; rm -f "$rt_human"; rt_bad=$((rt_bad + 1)); continue
    fi
    normalize_check "$f" >"$TMP/a.check"
    normalize_check "$rt_human" >"$TMP/b.check"
    if ! cmp -s "$TMP/a.check" "$TMP/b.check"; then
        note "check diagnostics differ: $f"
        diff "$TMP/a.check" "$TMP/b.check" | head -6 | sed 's/^/      /'
        rm -f "$rt_human"
        rt_bad=$((rt_bad + 1))
        continue
    fi
    if ! "$BIN" emit-c "$f" 2>"$TMP/rt.e" | normalize_c >"$TMP/a.c" ||
       ! "$BIN" emit-c "$rt_human" 2>"$TMP/rt.e" | normalize_c >"$TMP/b.c" ||
       ! cmp -s "$TMP/a.c" "$TMP/b.c"; then
        note "emit-c differs: $f"
        diff "$TMP/a.c" "$TMP/b.c" | head -6 | sed 's/^/      /'
        rt_bad=$((rt_bad + 1))
    fi
    rm -f "$rt_human"
done
if [ "$rt_bad" -eq 0 ]; then
    pass "round-trip diagnostics and emit-c ($rt_total files)"
else
    fail "round-trip diagnostics and emit-c" "$rt_bad of $rt_total files differ"
fi

# ---------------------------------------------------------------- gate 5
if [ ! -x "$GBNF_VALIDATOR" ]; then
    fail "kaze_machine.gbnf validates corpus" "no validator at $GBNF_VALIDATOR (KAZE_GBNF_VALIDATOR may override)"
elif [ ! -f "$GBNF" ]; then
    fail "kaze_machine.gbnf validates corpus" "missing $GBNF"
else
    g_bad=0
    g_total=0
    for f in lib/*.kaze tests/stdlib/*.kaze tests/native/cases/*.kaze tests/fixtures/syntax/*.kaze; do
        [ -f "$f" ] || continue
        g_total=$((g_total + 1))
        if ! "$BIN" to-machine "$f" >"$TMP/g.m" 2>/dev/null ||
           ! "$GBNF_VALIDATOR" "$GBNF" "$TMP/g.m" >"$TMP/g.out" 2>&1; then
            note "grammar rejected: $f"
            head -3 "$TMP/g.out" | sed 's/^/      /'
            g_bad=$((g_bad + 1))
        fi
    done
    if [ "$g_bad" -eq 0 ]; then
        pass "kaze_machine.gbnf validates corpus ($g_total files)"
    else
        fail "kaze_machine.gbnf validates corpus" "$g_bad of $g_total files rejected"
    fi
fi

# ---------------------------------------------------------------- gate 6
cat >"$TMP/holes.kaze" <<'EOF'
action need(x: num) -> num:
    require x >= 0
    let y = ?
    return todo(num, "the square of x")

action choose(flag: bool) -> num:
    if flag:
        return ?
    return 0
EOF
"$BIN" check "$TMP/holes.kaze" >"$TMP/holes.out" 2>&1
holes_rc=$?
"$BIN" check --holes --json "$TMP/holes.kaze" >"$TMP/holes.json" 2>"$TMP/holes.err"
holes_json_rc=$?
if [ "$holes_rc" -ne 0 ] && grep -q 'E_KAZE_HOLE' "$TMP/holes.out" &&
   [ "$holes_json_rc" -ne 0 ] && python3 - "$TMP/holes.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
hs = d.get("holes") or []
assert len(hs) >= 3, hs
for h in hs:
    for k in ("hole", "routine", "line", "expected_type", "capabilities", "locals", "proven_facts"):
        assert k in h, (k, h)
assert {h["routine"] for h in hs} == {"need", "choose"}, hs
assert any(h["expected_type"] == "num" for h in hs), hs
PY
then
    pass "typed holes and --holes --json"
else
    fail "typed holes and --holes --json" "check_rc=$holes_rc json_rc=$holes_json_rc" "$(head -3 "$TMP/holes.err")"
fi

# ---------------------------------------------------------------- gate 7
mkdir -p "$TMP/patch"
cat >"$TMP/patch/target.kaze" <<'EOF'
use "./dep.kaze" as dep

# KEEP: file header
action alpha(x: num) -> num:
    # KEEP: alpha body comment
    return x + 1

# KEEP: beta comment
action beta(y: num) -> num:
    return y * 2

record Point:
    x: num
    y: num
EOF
cat >"$TMP/patch/dep.kaze" <<'EOF'
pub action noop() -> num:
    return 0
EOF
cat >"$TMP/patch/beta.kaze" <<'EOF'
action beta(y: num) -> num:
    let doubled = y * 4
    return doubled
EOF
cat >"$TMP/patch/good.kaze" <<'EOF'
use "./dep.kaze" as dep

action main() -> num:
    return beta(3)
EOF
mkdir -p "$TMP/patch/dir"
cp "$TMP/patch/target.kaze" "$TMP/patch/before.kaze"
if "$BIN" patch "$TMP/patch/target.kaze" "$TMP/patch/beta.kaze" >"$TMP/patch/out" 2>&1 &&
   "$BIN" check "$TMP/patch/target.kaze" >/dev/null 2>&1 &&
   grep -q 'KEEP: file header' "$TMP/patch/target.kaze" &&
   grep -q 'use "./dep.kaze" as dep' "$TMP/patch/target.kaze" &&
   grep -q 'KEEP: alpha body comment' "$TMP/patch/target.kaze" &&
   grep -q 'KEEP: beta comment' "$TMP/patch/target.kaze" &&
   grep -q 'return y \* 2' "$TMP/patch/before.kaze" &&
   ! grep -q 'return y \* 2' "$TMP/patch/target.kaze" &&
   grep -q 'let doubled' "$TMP/patch/target.kaze"; then
    pass "patch replaces one item and preserves the rest"
else
    fail "patch replaces one item and preserves the rest" "$(head -6 "$TMP/patch/out")"
fi

cat >"$TMP/patch/bad.kaze" <<'EOF'
action beta(y: num) -> num:
    return missing_name
EOF
cp "$TMP/patch/target.kaze" "$TMP/patch/atomic.before"
"$BIN" patch "$TMP/patch/target.kaze" "$TMP/patch/bad.kaze" >"$TMP/patch/bad.out" 2>&1
bad_rc=$?
if [ "$bad_rc" -ne 0 ] && cmp -s "$TMP/patch/target.kaze" "$TMP/patch/atomic.before"; then
    pass "patch is atomic on a failed check"
else
    fail "patch is atomic on a failed check" "rc=$bad_rc" "$(head -4 "$TMP/patch/bad.out")"
fi

"$BIN" to-machine "$TMP/patch/target.kaze" >"$TMP/patch/target.m"
if "$BIN" patch "$TMP/patch/target.m" "$TMP/patch/good.kaze" >"$TMP/patch/m.out" 2>&1 &&
   "$BIN" check "$TMP/patch/target.m" >/dev/null 2>&1 &&
   grep -q '(action main' "$TMP/patch/target.m"; then
    pass "patch cross-converts a machine target"
else
    fail "patch cross-converts a machine target" "$(head -5 "$TMP/patch/m.out")"
fi

# ---------------------------------------------------------------- gate 7b
mkdir -p "$TMP/repair"
cat >"$TMP/repair/target.kaze" <<'EOF'
# KEEP: repair header
action keep(x: num) -> num:
    return x + 1

action solve(x: num) -> num:
    return todo(num, "x squared")

# KEEP: repair tail
EOF
cat >"$TMP/repair/fix.kaze" <<'EOF'
action solve(x: num) -> num:
    return x * x
EOF
"$BIN" check --holes --json "$TMP/repair/target.kaze" >"$TMP/repair/holes.json" 2>/dev/null
repair_json_rc=$?
cp "$TMP/repair/target.kaze" "$TMP/repair/before.kaze"
if [ "$repair_json_rc" -ne 0 ] &&
   grep -q '"routine":"solve"' "$TMP/repair/holes.json" &&
   "$BIN" patch "$TMP/repair/target.kaze" "$TMP/repair/fix.kaze" >"$TMP/repair/patch.out" 2>&1 &&
   "$BIN" check "$TMP/repair/target.kaze" >/dev/null 2>&1 &&
   grep -q 'KEEP: repair header' "$TMP/repair/target.kaze" &&
   grep -q 'KEEP: repair tail' "$TMP/repair/target.kaze" &&
   grep -q 'return x + 1' "$TMP/repair/target.kaze" &&
   grep -q 'return (x \* x)' "$TMP/repair/target.kaze"; then
    pass "hole context drives a single-routine repair"
else
    fail "hole context drives a single-routine repair" "$(head -4 "$TMP/repair/patch.out")"
fi

# ---------------------------------------------------------------- gate 8
cp "$TMP/holes.kaze" "$TMP/hash.kaze"
"$BIN" ast-hash "$TMP/hash.kaze" >"$TMP/h1" 2>/dev/null
printf '\n# cosmetic\n\n' >>"$TMP/hash.kaze"
"$BIN" ast-hash "$TMP/hash.kaze" >"$TMP/h2" 2>/dev/null
sed -i '' 's/the square of x/the cube of x/' "$TMP/hash.kaze" 2>/dev/null || \
    sed -i 's/the square of x/the cube of x/' "$TMP/hash.kaze"
"$BIN" ast-hash "$TMP/hash.kaze" >"$TMP/h3" 2>/dev/null
if grep -q '^module [0-9a-f]\{8\}$' "$TMP/h1" &&
   grep -q '^action need [0-9a-f]\{8\}$' "$TMP/h1" &&
   cmp -s "$TMP/h1" "$TMP/h2" &&
   ! cmp -s "$TMP/h2" "$TMP/h3"; then
    pass "ast-hash ignores comments and tracks semantic edits"
else
    fail "ast-hash ignores comments and tracks semantic edits" "$(paste "$TMP/h1" "$TMP/h2" "$TMP/h3" | head -4)"
fi

# ---------------------------------------------------------------- gate 9
mkdir -p "$TMP/cache"
cat >"$TMP/cache/lib.kaze" <<'EOF'
pub action value(x: num) -> num:
    return x * 2
EOF
cat >"$TMP/cache/main.kaze" <<'EOF'
use "./lib.kaze" as lib

action main() -> num:
    return lib.value(21)
EOF
(
    cd "$TMP/cache" || exit 1
    "$BIN" build main.kaze -o prog >"$TMP/cache/build1" 2>&1 || exit 1
    printf '\n# cosmetic\n' >>lib.kaze
    "$BIN" build main.kaze -o prog >"$TMP/cache/build2" 2>&1 || exit 1
    sed -i '' 's/return x \* 2/return x * 3/' lib.kaze 2>/dev/null || \
        sed -i 's/return x \* 2/return x * 3/' lib.kaze
    "$BIN" build main.kaze -o prog >"$TMP/cache/build3" 2>&1 || exit 1
    ./prog >"$TMP/cache/run" 2>&1
)
if [ "$(cat "$TMP/cache/run" 2>/dev/null)" = "63" ] &&
   grep -q '0 compiled, 2 cached' "$TMP/cache/build2" &&
   grep -q '1 compiled, 1 cached' "$TMP/cache/build3"; then
    pass "semantic cache reuses objects after cosmetic edits"
else
    fail "semantic cache reuses objects after cosmetic edits" \
        "$(tail -2 "$TMP/cache/build1")" "$(tail -2 "$TMP/cache/build2")" "$(tail -2 "$TMP/cache/build3")"
fi

# ---------------------------------------------------------------- gate 10
if [ "${KAZE_GATE_NO_PROOF:-0}" = "1" ]; then
    note "proof-cache gate skipped (KAZE_GATE_NO_PROOF=1)"
elif [ ! -x /opt/homebrew/bin/z3 ] && ! command -v z3 >/dev/null 2>&1; then
    note "proof-cache gate skipped (z3 not installed)"
else
    mkdir -p "$TMP/proof"
    cp scripts/phase16/proof_carrying.kaze "$TMP/proof/proof.kaze"
    bash scripts/phase16/build-kaze-verify.sh "$TMP/proof/kaze-verify" >"$TMP/proof/build" 2>&1
    build_rc=$?
    if [ "$build_rc" -ne 0 ]; then
        fail "proof cache reuses Z3 verdicts" "$(tail -4 "$TMP/proof/build")"
    else
        python3 scripts/phase16/verify_build.py "$BIN" "$TMP/proof/kaze-verify" \
            "$TMP/proof/proof.kaze" "$TMP/proof/out" >"$TMP/proof/first" 2>&1
        first_rc=$?
        printf '\n# cosmetic edit\n' >>"$TMP/proof/proof.kaze"
        python3 scripts/phase16/verify_build.py "$BIN" "$TMP/proof/kaze-verify" \
            "$TMP/proof/proof.kaze" "$TMP/proof/out" >"$TMP/proof/second" 2>&1
        second_rc=$?
        if [ "$first_rc" -eq 0 ] && [ "$second_rc" -eq 0 ] &&
           grep -q '"proof_cache":"miss"' "$TMP/proof/first" &&
           grep -q '"proof_cache":"hit"' "$TMP/proof/second" &&
           grep -q '"output_equal":true' "$TMP/proof/second"; then
            pass "proof cache reuses Z3 verdicts"
        else
            fail "proof cache reuses Z3 verdicts" \
                "$(tail -1 "$TMP/proof/first")" "$(tail -1 "$TMP/proof/second")"
        fi
    fi
fi

# ---------------------------------------------------------------- gate 11
LLAMA_CLI=${KAZE_LLAMA_CLI:-$HOME/llama.cpp/build/bin/llama-cli}
GEN_MODEL=""
for m in "$HOME/Downloads/SmolLM2-360M-Instruct-Q8_0.gguf" \
         "$HOME/Downloads/Llama-3.2-1B-Instruct-Q8_0.gguf"; do
    [ -f "$m" ] && GEN_MODEL="$m" && break
done
if [ -z "$GEN_MODEL" ] || [ ! -x "$LLAMA_CLI" ]; then
    note "constrained-generation gate skipped (no local GGUF/llama-cli)"
elif perl -e 'alarm 240; exec @ARGV' python3 scripts/machine_constrained_gen.py \
        --bin "$BIN" --model "$GEN_MODEL" --llama-cli "$LLAMA_CLI" \
        --grammar "$GBNF" --samples 3 >"$TMP/gen.log" 2>&1; then
    pass "constrained generation parses"
else
    fail "constrained generation parses" "$(tail -4 "$TMP/gen.log")"
fi

echo
echo "machine-syntax-gates: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
