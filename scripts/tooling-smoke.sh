#!/usr/bin/env bash
# tooling-smoke.sh -- prove the Phase 2 and Phase 4 tools actually run.
#
# Each check executes the real tool against examples/hello.kaze and asserts on
# observed output, not on the tool's exit status alone.
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
FAIL=0
say() { printf '%s\n' "$*"; }
check() {
    local name=$1 ok=$2
    if [ "$ok" -eq 0 ]; then say "PASS  $name"; else say "FAIL  $name"; FAIL=$((FAIL + 1)); fi
}

# --- REPL: an expression and a definition round-trip through the compiler.
out=$(printf '= 1 + 2\naction greet() -> text:\n    return "hi"\n\n= greet()\n:quit\n' | bash scripts/kaze-repl.sh 2>/dev/null)
printf '%s\n' "$out" | grep -q '^3$'
check 'repl evaluates an expression' $?
printf '%s\n' "$out" | grep -q '^hi$'
check 'repl keeps a definition for the next evaluation' $?

out=$(printf 'action ok() -> num:\n    return 1\n\n:check\n:quit\n' | bash scripts/kaze-repl.sh 2>/dev/null)
printf '%s\n' "$out" | grep -q 'check: ok'
check 'repl :check reports a clean buffer' $?

# --- Debugger: routine listing and a real lldb stop.
out=$(bash scripts/kaze-debug.sh examples/hello.kaze --list 2>&1)
printf '%s\n' "$out" | grep -q 'main'
check 'debugger lists routines' $?
out=$(bash scripts/kaze-debug.sh examples/hello.kaze --break main --batch 2>&1)
printf '%s\n' "$out" | grep -q 'stop reason = breakpoint'
check 'debugger stops at a routine breakpoint' $?
out=$(bash scripts/kaze-debug.sh examples/hello.kaze --break main --print total --batch 2>&1)
printf '%s\n' "$out" | grep -q '(KzValue) total'
check 'debugger prints a routine local' $?
out=$(bash scripts/kaze-debug.sh examples/hello.kaze --break 13 --batch 2>&1)
printf '%s\n' "$out" | grep -q 'hello.kaze:13'
check 'debugger stops at a source-line breakpoint' $?
out=$(bash scripts/kaze-debug.sh examples/hello.kaze --break examples/hello.kaze:13 --print total --batch 2>&1)
printf '%s\n' "$out" | grep -q '(KzValue) total'
check 'debugger stops at file:line breakpoint and prints local' $?

# --- Profiler: a resolved routine with a non-zero count.
out=$(bash scripts/kaze-profile.sh examples/hello.kaze --top 10 2>&1)
printf '%s\n' "$out" | grep -q 'kaze_fn__kz_main'
check 'profiler resolves a generated routine' $?
printf '%s\n' "$out" | grep -qE '^[0-9]+[[:space:]]+[0-9.]+'
check 'profiler reports a call row' $?

if [ "$FAIL" -eq 0 ]; then say 'tooling-smoke: 10 of 10 checks passed'; exit 0; fi
say "tooling-smoke: $FAIL check(s) failed"
exit 1
