#!/usr/bin/env bash
# phase15-syntax-gates.sh -- acceptance gate for the human-syntax upgrade.
#
# Plan 15 has four phases; this script checks the claims that are behavioural
# rather than covered by the ordinary suites:
#
#   15.1  elif/else, bare and compound assignment, for-in and len() run the same
#         way through the C99 and native backends
#   15.2  declaration casing is diagnosed at the declaration, guard exits and
#         unit normalization work
#   15.3  kaze fmt --migrate is canonical, fmt -w is idempotent, emit-c is
#         semantically unchanged by formatting, co-located tests run under
#         `kaze test` and stay inert in a normal build, and the compiler still
#         reaches its bootstrap fixed point
#   15.4  the canonical corpus parses under dist/grammars/kaze.gbnf
#
# Run it through scripts/gates.sh, or directly for a faster loop:
#
#   bash scripts/phase15-syntax-gates.sh
#   KAZE_BIN=/path/to/kaze bash scripts/phase15-syntax-gates.sh
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
BIN=${KAZE_BIN:-$ROOT/bin/kaze}
CC=${CC:-cc}
WORK=$(mktemp -d "${TMPDIR:-/tmp}/kaze-phase15.XXXXXX") || exit 2
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0

ok() {
    PASS=$((PASS + 1))
    printf '  ok   %s\n' "$*"
}

bad() {
    FAIL=$((FAIL + 1))
    printf '  FAIL %s\n' "$*"
}

# Line numbers and content hashes move when the formatter reflows a file.  The
# gate wants the emitted program to be the same program, not the same bytes.
norm_c() {
    sed -E \
        -e 's/#line [0-9]+/#line N/g' \
        -e 's/_l[0-9]+/_lN/g' \
        -e 's/kz_m_[0-9a-f]+/kz_m_HASH/g' \
        -e 's/KzM_[0-9a-f]+/KzM_HASH/g' \
        -e 's/kz_hook_when_[0-9]+/kz_hook_when_N/g' \
        -e 's/line [0-9]+/line N/g'
}

[ -x "$BIN" ] || { echo "phase15-syntax-gates: missing compiler $BIN" >&2; exit 2; }

# ---------------------------------------------------------------- 15.1 syntax
echo "== 15.1 canonical statements on C99 and native"
for spec in 'phase15_basics 19' 'phase15_guard 8' 'phase15_guard_continue 4'; do
    set -- $spec
    name=$1
    want=$2
    src="tests/fixtures/syntax/$name.kaze"
    "$BIN" run "$src" >"$WORK/$name.c.out" 2>"$WORK/$name.c.err"
    crc=$?
    "$BIN" run --backend=native "$src" >"$WORK/$name.n.out" 2>"$WORK/$name.n.err"
    nrc=$?
    if [ "$crc" -eq 0 ] && [ "$nrc" -eq 0 ] &&
       [ "$(cat "$WORK/$name.c.out")" = "$want" ] &&
       cmp -s "$WORK/$name.c.out" "$WORK/$name.n.out" &&
       cmp -s "$WORK/$name.c.err" "$WORK/$name.n.err"; then
        ok "$name prints $want on both backends"
    else
        bad "$name: c99 rc=$crc native rc=$nrc (want $want)"
        head -4 "$WORK/$name.c.err" | sed 's/^/       /'
        head -4 "$WORK/$name.n.err" | sed 's/^/       /'
    fi
done

# --------------------------------------------------------------- 15.2 casing
echo "== 15.2 declaration casing"
cat >"$WORK/action_case.kaze" <<'EOF'
action Compute() -> num:
    return 1

action main() -> num:
    return Compute()
EOF
out=$("$BIN" check "$WORK/action_case.kaze" 2>&1)
if printf '%s\n' "$out" | grep -q "E_KAZE_CASING at line 1: action 'Compute'" &&
   ! printf '%s\n' "$out" | grep -q 'E_KAZE_UNKNOWN_NAME'; then
    ok "action Compute is diagnosed at the declaration"
else
    bad "action Compute did not produce the declaration-line diagnostic"
    printf '%s\n' "$out" | sed 's/^/       /'
fi

cat >"$WORK/record_case.kaze" <<'EOF'
record bar:
    value: num

action main() -> num:
    return 1
EOF
out=$("$BIN" check "$WORK/record_case.kaze" 2>&1)
if printf '%s\n' "$out" | grep -q "E_KAZE_CASING at line 1: record 'bar'"; then
    ok "record bar is diagnosed at the declaration"
else
    bad "record bar did not produce the declaration-line diagnostic"
    printf '%s\n' "$out" | sed 's/^/       /'
fi

cat >"$WORK/let_case.kaze" <<'EOF'
action main() -> num:
    let Total = 1
    return 0
EOF
out=$("$BIN" check "$WORK/let_case.kaze" 2>&1)
if printf '%s\n' "$out" | grep -q "E_KAZE_CASING at line 2: binding 'Total'"; then
    ok "let Total is diagnosed at the binding"
else
    bad "let Total did not produce the binding-line diagnostic"
    printf '%s\n' "$out" | sed 's/^/       /'
fi

mkdir -p "$WORK/alias/lib"
cat >"$WORK/alias/lib/helper.kaze" <<'EOF'
action helper() -> num:
    return 1
EOF
cat >"$WORK/alias/main.kaze" <<'EOF'
use "./lib/helper.kaze" as Helper

action main() -> num:
    return 0
EOF
out=$("$BIN" check "$WORK/alias/main.kaze" 2>&1)
if printf '%s\n' "$out" | grep -q "E_KAZE_CASING.*module alias 'Helper'"; then
    ok "module alias Helper is diagnosed at the use line"
else
    bad "module alias Helper did not produce the use-line diagnostic"
    printf '%s\n' "$out" | sed 's/^/       /'
fi

echo "== 15.2 unit normalization"
cat >"$WORK/units.kaze" <<'EOF'
action main() -> num:
    let d = 100 Meters
    let t = 10 Seconds
    let pressure = 100 hPa
    let temperature = 20 Celsius
    let elapsed = 5 ms
    return (d / t) otherwise 0
EOF
"$BIN" fmt "$WORK/units.kaze" >"$WORK/units.canon" 2>"$WORK/units.fmt.err"
fmt_rc=$?
if [ "$fmt_rc" -eq 0 ] &&
   grep -q '100 meters' "$WORK/units.canon" &&
   grep -q '10 seconds' "$WORK/units.canon" &&
   grep -q '100 hPa' "$WORK/units.canon" &&
   grep -q '20 celsius' "$WORK/units.canon" &&
   grep -q '5 ms' "$WORK/units.canon"; then
    ok "spelled-out units canonicalize; SI abbreviations stay exact"
else
    bad "unit spellings were not canonicalized"
    sed 's/^/       /' "$WORK/units.canon"
    head -3 "$WORK/units.fmt.err" | sed 's/^/       /'
fi
"$BIN" run "$WORK/units.kaze" >"$WORK/units.out" 2>"$WORK/units.run.err"
if [ $? -eq 0 ] && [ "$(cat "$WORK/units.out")" = '10' ]; then
    ok "unit-annotated arithmetic runs"
else
    bad "unit-annotated arithmetic did not print 10"
    head -4 "$WORK/units.run.err" | sed 's/^/       /'
fi

# ------------------------------------------------------------- 15.3 migrate
echo "== 15.3 fmt --migrate"
cat >"$WORK/legacy.kaze" <<'EOF'
needs: log

routine total(xs: list) -> number:
    requires length of xs > 0
    let out = 0
    for each x in xs:
        set out to out + x
    if out > 10:
        set out to out - 1
    otherwise:
        if out > 5:
            set out to 5
        otherwise:
            set out to 0
    return out

routine main() -> number:
    let xs = [1, 2, 3]
    return total(xs)
EOF
if "$BIN" fmt --check "$WORK/legacy.kaze" >/dev/null 2>&1; then
    bad "fmt --check accepted the legacy spelling"
else
    ok "fmt --check rejects the legacy spelling"
fi
"$BIN" fmt --migrate "$WORK/legacy.kaze" >"$WORK/migrate.out" 2>"$WORK/migrate.err"
migrate_rc=$?
if [ "$migrate_rc" -eq 0 ] &&
   grep -q '^needs log$' "$WORK/legacy.kaze" &&
   grep -q '^action total(xs: list) -> num:$' "$WORK/legacy.kaze" &&
   grep -q '^    require len(xs) > 0$' "$WORK/legacy.kaze" &&
   grep -q '^        out += x$' "$WORK/legacy.kaze" &&
   grep -q '^    elif out > 5:$' "$WORK/legacy.kaze" &&
   grep -q '^    else:$' "$WORK/legacy.kaze" &&
   grep -q '^action main() -> num:$' "$WORK/legacy.kaze"; then
    ok "legacy source migrates to canonical syntax"
else
    bad "fmt --migrate did not produce canonical syntax"
    sed 's/^/       /' "$WORK/legacy.kaze"
    head -3 "$WORK/migrate.err" | sed 's/^/       /'
fi
if "$BIN" fmt --check "$WORK/legacy.kaze" >/dev/null 2>&1 &&
   [ "$("$BIN" run "$WORK/legacy.kaze" 2>"$WORK/legacy.run.err")" = '5' ]; then
    ok "migrated source is canonical and still runs"
else
    bad "migrated source is not canonical or changed behaviour"
    head -4 "$WORK/legacy.run.err" | sed 's/^/       /'
fi
cp "$WORK/legacy.kaze" "$WORK/legacy.once"
"$BIN" fmt -w "$WORK/legacy.kaze" >/dev/null 2>&1
if cmp -s "$WORK/legacy.once" "$WORK/legacy.kaze"; then
    ok "migration is idempotent"
else
    bad "second fmt -w changed the migrated file"
fi

# ------------------------------------------------------------ 15.3 formatter
echo "== 15.3 fmt -w over lib"
mkdir -p "$WORK/lib"
cp lib/*.kaze "$WORK/lib/" || exit 2
"$BIN" fmt -w "$WORK/lib" >"$WORK/lib.fmt.log" 2>&1
if [ $? -ne 0 ]; then
    bad "fmt -w failed over a lib copy"
    head -5 "$WORK/lib.fmt.log" | sed 's/^/       /'
else
    cp -R "$WORK/lib" "$WORK/lib.once"
    "$BIN" fmt -w "$WORK/lib" >"$WORK/lib.fmt2.log" 2>&1
    if diff -r "$WORK/lib.once" "$WORK/lib" >"$WORK/lib.diff" 2>&1; then
        ok "fmt -w is byte-idempotent over $(find "$WORK/lib" -name '*.kaze' | wc -l | tr -d ' ') lib sources"
    else
        bad "fmt -w is not idempotent over lib"
        head -10 "$WORK/lib.diff" | sed 's/^/       /'
    fi
    if "$BIN" fmt --check "$WORK/lib" >/dev/null 2>&1; then
        ok "canonical lib copy passes fmt --check"
    else
        bad "canonical lib copy fails fmt --check"
    fi
fi

echo "== 15.3 emit-c parity across formatting"
same=0
differ=0
for src in lib/*.kaze; do
    base=$(basename "$src")
    "$BIN" emit-c "$src" >"$WORK/before.raw" 2>"$WORK/before.err"
    arc=$?
    "$BIN" emit-c "$WORK/lib/$base" >"$WORK/after.raw" 2>"$WORK/after.err"
    brc=$?
    if [ "$arc" -ne 0 ] || [ "$brc" -ne 0 ]; then
        bad "emit-c failed for $base (before rc=$arc, after rc=$brc)"
        head -3 "$WORK/before.err" "$WORK/after.err" | sed 's/^/       /'
        continue
    fi
    norm_c <"$WORK/before.raw" >"$WORK/before.c"
    norm_c <"$WORK/after.raw" >"$WORK/after.c"
    if cmp -s "$WORK/before.c" "$WORK/after.c"; then
        same=$((same + 1))
    else
        differ=$((differ + 1))
        bad "emit-c differs for $base"
        diff -u "$WORK/before.c" "$WORK/after.c" | head -8 | sed 's/^/       /'
    fi
done
if [ "$differ" -eq 0 ] && [ "$same" -gt 0 ]; then
    ok "$same lib modules emit the same C before and after formatting"
fi

# ------------------------------------------------------- 15.3 co-located tests
echo "== 15.3 co-located tests"
if out=$("$BIN" test text 2>&1) &&
   printf '%s\n' "$out" | grep -q 'PASS  text' &&
   ! printf '%s\n' "$out" | grep -q 'FAIL'; then
    ok "kaze test runs the co-located text tests"
else
    bad "kaze test text did not pass"
    printf '%s\n' "$out" | tail -6 | sed 's/^/       /'
fi

mkdir -p "$WORK/srcproj/src"
cat >"$WORK/srcproj/src/co_located.kaze" <<'EOF'
test "runs from src":
    require 1 == 1

action main() -> num:
    return 0
EOF
if out=$(cd "$WORK/srcproj" && "$BIN" test co_located 2>&1) &&
   printf '%s\n' "$out" | grep -q 'PASS  co_located'; then
    ok "kaze test discovers co-located tests in src/"
else
    bad "kaze test did not discover a src/ co-located test"
    printf '%s\n' "$out" | tail -6 | sed 's/^/       /'
fi

"$BIN" emit-c lib/text.kaze >"$WORK/text-normal.c" 2>"$WORK/text-normal.err"
TESTC=${KAZE_TESTC_BIN:-$ROOT/bin/kaze-bootstrap}
if [ -x "$TESTC" ] &&
   "$TESTC" test-c lib/text.kaze >"$WORK/text-test.c" 2>"$WORK/text-test.err" &&
   grep -q 'Co-located Test Driver' "$WORK/text-test.c" &&
   grep -q 'text_length counts a text' "$WORK/text-test.c" &&
   ! grep -q 'Co-located Test Driver' "$WORK/text-normal.c"; then
    ok "co-located tests stay inert in a normal build"
else
    bad "normal emit-c and test-c did not separate test blocks"
fi

mkdir -p "$WORK/proj/lib"
cat >"$WORK/proj/lib/failing.kaze" <<'EOF'
action main() -> num:
    return 0

test "fails on purpose":
    require 1 == 0
EOF
if out=$(cd "$WORK/proj" && "$BIN" test failing 2>&1); then
    bad "a failing co-located test did not fail kaze test"
    printf '%s\n' "$out" | tail -4 | sed 's/^/       /'
else
    ok "a failing co-located test fails kaze test"
fi

# --------------------------------------------- 15.3 self-sync and fixed point
echo "== 15.3 self-sync and bootstrap fixed point"
if out=$(bash scripts/self-sync.sh --check 2>&1); then
    ok "$out"
else
    bad "compiler/kaze_self.kaze is stale; run scripts/self-sync.sh"
    printf '%s\n' "$out" | sed 's/^/       /'
fi

if "$CC" -std=c99 -O2 -w -I dist dist/kaze.c dist/kaze_rt.c -lm \
        -o "$WORK/stage1" 2>"$WORK/stage1.cc.log" &&
   "$WORK/stage1" emit-c compiler/kaze_self.kaze >"$WORK/stage2.c" 2>"$WORK/stage2.err" &&
   [ -s "$WORK/stage2.c" ] &&
   "$CC" -std=c99 -O2 -w -I dist "$WORK/stage2.c" dist/kaze_rt.c -lm \
        -o "$WORK/stage2" 2>"$WORK/stage2.cc.log" &&
   "$WORK/stage2" emit-c compiler/kaze_self.kaze >"$WORK/stage3.c" 2>"$WORK/stage3.err" &&
   cmp -s "$WORK/stage2.c" "$WORK/stage3.c"; then
    ok "bootstrap fixed point: stage 2 emits stage 3 byte for byte"
else
    bad "bootstrap fixed point did not hold"
    head -4 "$WORK/stage2.err" "$WORK/stage3.err" 2>/dev/null | sed 's/^/       /'
    cmp "$WORK/stage2.c" "$WORK/stage3.c" 2>&1 | sed 's/^/       /'
fi

# ----------------------------------------------------------- 15.4 grammar
echo "== 15.4 grammar parity"
if out=$(bash scripts/grammar-check.sh 2>&1); then
    ok "$(printf '%s\n' "$out" | tail -1)"
else
    bad "canonical corpus does not parse under kaze.gbnf"
    printf '%s\n' "$out" | tail -8 | sed 's/^/       /'
fi

echo
if [ "$FAIL" -eq 0 ]; then
    echo "phase15-syntax-gates: $PASS checks passed"
    exit 0
fi
echo "phase15-syntax-gates: $FAIL of $((PASS + FAIL)) checks failed"
exit 1
