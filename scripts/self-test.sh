#!/usr/bin/env bash
# Rust-free conformance harness.
#
# Phase 10 wants the Rust implementation retired. Before that is arguable, the
# self-hosted compiler needs a harness that proves its behaviour with no cargo and no
# Rust binary anywhere: this script builds (or is handed) the standalone compiler from
# dist/kaze.c and checks it against expectations derived from docs/SPEC.md.
#
#   ./scripts/self-test.sh                 # build from dist/kaze.c, run every case
#   KAZE_BIN=/path/to/kaze ./scripts/self-test.sh
#
# Exit status is non-zero if any case fails.
set -u
cd "$(dirname "$0")/.." || exit 1
ROOT=$(pwd)

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

if [ -n "${KAZE_BIN:-}" ]; then
  bin=$KAZE_BIN
else
  bin="$tmp/kaze"
  echo "self-test: building the compiler from dist/kaze.c (no Rust)"
  if ! cc -O1 -I dist dist/kaze.c dist/kaze_rt.c -lm -o "$bin" 2>"$tmp/build.err"; then
    echo "self-test: FAIL - dist/kaze.c does not build" >&2
    sed 's/^/    /' "$tmp/build.err" >&2
    exit 1
  fi
fi

cases=0; failed=0

warned=0
known_gap() {
  # A case the self-hosted front end does not refuse yet, but the reference engine does.
  # It must stay visible: WARN, counted separately, never silent.
  name=$1; src=$2; reason=$3
  path="$tmp/$name.kaze"
  printf '%s\n' "$src" > "$path"
  out=$("$bin" check "$path" 2>&1)
  warned=$((warned+1))
  printf 'WARN  %-42s %s (accepted: %s)\n' "$name" "$reason" "$(printf '%s' "$out" | head -1)"
}

ok() {
  cases=$((cases+1))
  printf 'PASS  %-42s %s\n' "$1" "${2:-}"
}
no() {
  cases=$((cases+1)); failed=$((failed+1))
  printf 'FAIL  %-42s %s\n' "$1" "$2"
}

# expect_exit NAME PROGRAM EXPECTED_STATUS [EXPECTED_TEXT]
# NOTE: `check` prints diagnostics but exits 0 (the CLI has no exit-code plumbing);
# these cases therefore assert the diagnostic text, which is what it guarantees.
expect_report() {
  name=$1; src=$2; text=$3
  path="$tmp/$name.kaze"
  printf '%s\n' "$src" > "$path"
  out=$("$bin" check "$path" 2>&1)
  if printf '%s' "$out" | grep -q "$text"; then
    ok "$name" "reported $(printf '%s' "$out" | head -1)"
  else
    no "$name" "expected '$text', got: $(printf '%s' "$out" | head -1)"
  fi
}

# 1. a valid program is accepted (prints 'check: ok')
expect_report valid_program 'needs: log
action main() -> num:
    log "hello"
    return 1 + 2' "ok"

# 2. the diagnostic families the compiler owns. Each must print a diagnostic whose
#    text names the problem; the exit status is not asserted (see the note above).
# closed in round 5: the lexer now yields an unreadable token, so it is refused
expect_report unclosed_string 'action main() -> text:
    return "never closed' "E_KAZE_PARSE"
# closed in round 6: the checker has unit dimensions for literals
expect_report unit_mismatch 'action main() -> num:
    return 1 meter + 2 seconds' "E_KAZE_UNIT_MISMATCH"
expect_report valid_units 'action main() -> num:
    let d = 1500 meters
    return d' "ok"
expect_report unknown_name 'action main() -> num:
    return missing_thing' "missing_thing"

expect_report missing_fallback 'action main(parts: num) -> num:
    return 10 / parts' "FALLBACK"
expect_report undeclared_capability 'action main():
    arm drone' "CAP"

# 3. fmt is idempotent and emit-c is deterministic
prog='needs: log

action main() -> num:
    let xs = [1, 2, 3]
    let total = 0
    for each x in xs:
        set total to total + x
    log "sum done"
    return total'
printf '%s\n' "$prog" > "$tmp/fmt.kaze"
"$bin" fmt "$tmp/fmt.kaze" > "$tmp/fmt1.kaze" 2>/dev/null
"$bin" fmt "$tmp/fmt1.kaze" > "$tmp/fmt2.kaze" 2>/dev/null
cases=$((cases+1))
if cmp -s "$tmp/fmt1.kaze" "$tmp/fmt2.kaze" && [ -s "$tmp/fmt1.kaze" ]; then
  ok fmt_idempotent "fmt(fmt(x)) == fmt(x)"
else
  failed=$((failed+1)); no fmt_idempotent "fmt is not idempotent or produced nothing"
fi

"$bin" emit-c "$tmp/fmt.kaze" > "$tmp/e1.c" 2>/dev/null
"$bin" emit-c "$tmp/fmt.kaze" > "$tmp/e2.c" 2>/dev/null
cases=$((cases+1))
if cmp -s "$tmp/e1.c" "$tmp/e2.c" && [ -s "$tmp/e1.c" ]; then
  ok emit_c_deterministic "same input, identical bytes"
else
  failed=$((failed+1)); no emit_c_deterministic "emit-c is not deterministic"
fi

# 4. a golden emit-c shape, so codegen regressions are visible
printf 'action main() -> num:\n    return 7\n' > "$tmp/golden.kaze"
"$bin" emit-c "$tmp/golden.kaze" > "$tmp/golden.c" 2>/dev/null
cases=$((cases+1))
missing=""
for needle in 'int main(' 'kaze_fn_' 'return' 'kz_num'; do
  grep -q "$needle" "$tmp/golden.c" || missing="$missing $needle"
done
if [ -z "$missing" ] && [ -s "$tmp/golden.c" ]; then
  ok emit_c_golden_shape "main + kaze_fn_ + kz_num present"
else
  failed=$((failed+1)); no emit_c_golden_shape "missing:$missing"
fi

# 5. the emitted C compiles and runs
if cc -std=c99 -O1 -w -I dist "$tmp/golden.c" dist/kaze_rt.c -lm -o "$tmp/golden" 2>/dev/null; then
  value=$("$tmp/golden" 2>/dev/null | tail -1)
  cases=$((cases+1))
  if [ "$value" = "7" ]; then ok emitted_c_runs "printed $value"; else failed=$((failed+1)); no emitted_c_runs "printed '$value', wanted 7"; fi
else
  cases=$((cases+1)); failed=$((failed+1)); no emitted_c_builds "the emitted C does not compile"
fi

echo
echo "self-test: $cases cases, $failed failures, $warned known gaps (see WARN lines)"
[ "$failed" -eq 0 ]