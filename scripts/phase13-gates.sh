#!/usr/bin/env bash
# phase13-gates.sh -- the concurrency gate: deterministic fibers, bounded tasks,
# fuel metering and value channels.
#
# Every claim is measured on this machine, with the self-hosted compiler and no
# Rust toolchain:
#
#   * pure fibers interleave in round-robin spawn order, identically on every run
#   * a task's return value arrives at the join, a channel moves values through a
#     bounded ring, and an empty channel takes its fallback instead of blocking
#   * the task ceiling and the fuel budget refuse with E_KAZE_TASK_BOUND and
#     E_KAZE_FUEL_EXHAUSTED rather than clamping
#   * a cancelled task never resumes; a task's exit cancels its children
#   * yield / fiber-call / fiber-effect / receive-without-fallback / spawn-target
#     are refused at check time with their own codes
#   * the emitted program builds warning-free under -Wall -Wextra -Werror
#     -pedantic -std=c99, spawns no OS thread and uses no atomics
#   * a task switch costs under the 50 ns the phase asks for (differential
#     measurement against the same loop without a yield)
#
#   bash scripts/phase13-gates.sh
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
KAZE="$ROOT/bin/kaze"
CORPUS="$ROOT/scripts/selftest/phase13"
WORK=$(mktemp -d) || exit 2
trap 'rm -rf "$WORK"' EXIT
PASS=0
FAIL=0

ok() { PASS=$((PASS + 1)); printf '  PASS  %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL  %s\n' "$1"; }

build() {  # build <source> <binary>
    "$KAZE" emit-c "$1" > "$WORK/build.c" 2> "$WORK/build.err" || return 1
    cc -O2 -std=c99 -pedantic -Wall -Wextra -Werror -I "$ROOT/dist" \
        "$WORK/build.c" "$ROOT/dist/kaze_rt.c" -lm -o "$2" 2> "$WORK/cc.err" || {
        sed -n '1,10p' "$WORK/cc.err" >&2
        return 1
    }
}

echo "phase 13 -- deterministic concurrency"

# 1. the valid corpus checks clean
for f in ok_order ok_join ok_chan ok_cancel ok_tasks ok_fiber_main; do
    if "$KAZE" check "$CORPUS/$f.kaze" | grep -q '^check: ok'; then
        ok "check $f.kaze"
    else
        "$KAZE" check "$CORPUS/$f.kaze" >&2
        bad "check $f.kaze"
    fi
done

# 2. the refused corpus refuses with the code it must
check_code() {  # check_code <file> <code>
    out=$("$KAZE" check "$CORPUS/$1" 2>&1)
    if printf '%s' "$out" | grep -q "$2"; then
        ok "$1 -> $2"
    else
        printf '%s\n' "$out" >&2
        bad "$1 -> $2 (got something else)"
    fi
}
check_code bad_yield.kaze E_KAZE_YIELD
check_code bad_fiber_call.kaze E_KAZE_FIBER_CALL
check_code bad_fiber_effect.kaze E_KAZE_FIBER_EFFECT
check_code bad_receive.kaze E_KAZE_MISSING_FALLBACK
check_code bad_spawn_target.kaze E_KAZE_FIBER_TARGET

# 3. behaviour: round-robin order, join values, channel, cancel count
behave() {  # behave <source> <expected> <label>
    if ! build "$CORPUS/$1.kaze" "$WORK/$1"; then bad "$3 (build)"; return; fi
    got=$("$WORK/$1" 2>&1)
    if [ "$got" = "$2" ]; then ok "$3"; else bad "$3 (got '$got', want '$2')"; fi
}
behave ok_order 121212 "fibers interleave in spawn order"
behave ok_join 304 "a join returns the task's value"
behave ok_chan 6 "a bounded ring moves values"
behave ok_cancel 1 "cancel stops a task, parent exit cancels the rest"
behave ok_tasks 2 "tasks bounded 3 with two children runs"
behave ok_fiber_main 5 "a fiber main runs as the root task"

# 4. determinism: ten runs, one answer (order and value)
if build "$CORPUS/ok_order.kaze" "$WORK/det"; then
    runs=$(for i in 1 2 3 4 5 6 7 8 9 10; do "$WORK/det"; done | sort -u | tr '\n' ' ')
    if [ "$runs" = "121212 " ]; then ok "ten runs of the ordering program agree"; else bad "determinism ($runs)"; fi
else
    bad "determinism (build)"
fi

# 5. the declared task ceiling refuses, and so does a tighter env ceiling
if build "$CORPUS/bounds_tasks.kaze" "$WORK/bound"; then
    out=$("$WORK/bound" 2>&1); code=$?
    if [ "$code" -eq 70 ] && printf '%s' "$out" | grep -q E_KAZE_TASK_BOUND; then
        ok "tasks bounded 2 refuses a third spawn"
    else
        bad "tasks bounded 2 (exit $code: $out)"
    fi
else
    bad "tasks bounded 2 (build)"
fi
if build "$CORPUS/ok_join.kaze" "$WORK/ceiling"; then
    out=$(KAZE_MAX_TASKS=1 "$WORK/ceiling" 2>&1); code=$?
    if [ "$code" -eq 70 ] && printf '%s' "$out" | grep -q E_KAZE_TASK_BOUND; then
        ok "KAZE_MAX_TASKS=1 refuses the second spawn"
    else
        bad "KAZE_MAX_TASKS=1 (exit $code: $out)"
    fi
fi

# 6. a declared fuel budget stops a task
if build "$CORPUS/bounds_fuel.kaze" "$WORK/fuel"; then
    out=$("$WORK/fuel" 2>&1); code=$?
    if [ "$code" -eq 70 ] && printf '%s' "$out" | grep -q E_KAZE_FUEL_EXHAUSTED; then
        ok "fuel 5 refuses a twenty-step task"
    else
        bad "fuel 5 (exit $code: $out)"
    fi
fi

# 7. the machine form round-trips and the formatter is idempotent
if "$KAZE" to-machine "$CORPUS/ok_order.kaze" > "$WORK/order.m" \
   && "$KAZE" to-human "$WORK/order.m" > "$WORK/order.h" \
   && "$KAZE" check "$WORK/order.h" | grep -q '^check: ok'; then
    ok "to-machine -> to-human keeps the check verdict"
else
    bad "machine round trip"
fi
if grep -q '(spawn' "$WORK/order.m" && grep -q '(yield)' "$WORK/order.m" && grep -q '(receive' "$WORK/order.m"; then
    ok "the machine form carries spawn, yield and receive"
else
    bad "the machine form lost a concurrency construct"
fi
if "$KAZE" fmt "$CORPUS/ok_chan.kaze" > "$WORK/f1" && "$KAZE" fmt "$WORK/f1" > "$WORK/f2" \
   && cmp -s "$WORK/f1" "$WORK/f2"; then
    ok "fmt is idempotent on a channel program"
else
    bad "fmt idempotency"
fi

# 8. the emitted program is single-threaded: no threads, no atomics, no locks
if build "$CORPUS/ok_order.kaze" "$WORK/threads"; then
    if grep -Eq 'pthread|thrd_|_Atomic|atomic_|mtx_' "$WORK/build.c"; then
        bad "emitted C is not single-threaded"
    else
        ok "emitted C spawns no thread and uses no atomic"
    fi
fi

# 9. the switch cost: same loop with and without a yield, three runs each
if build "$CORPUS/bench_yield.kaze" "$WORK/by" && build "$CORPUS/bench_plain.kaze" "$WORK/bp"; then
    python3 - "$WORK/by" "$WORK/bp" <<'PY'
import os, subprocess, sys, time
by, bp = sys.argv[1], sys.argv[2]
env = dict(os.environ, KAZE_FUEL="100000000")
steps = 2000000

def best(path):
    t = 1e9
    for _ in range(3):
        t0 = time.perf_counter()
        subprocess.run([path], env=env, check=True, stdout=subprocess.DEVNULL)
        t = min(t, time.perf_counter() - t0)
    return t

fiber_t, plain_t = best(by), best(bp)
per_step = (fiber_t - plain_t) / steps * 1e9
total = fiber_t / steps * 1e9
# The marginal cost is the switch itself: the same loop, same compiler, with
# and without the yield. The end-to-end figure also carries the loop body.
print(f"  switch: {per_step:.1f} ns marginal, {total:.1f} ns per step end to end")
sys.exit(0 if per_step < 50.0 and total < 50.0 else 1)
PY
    if [ $? -eq 0 ]; then ok "a task switch costs under 50 ns"; else bad "switch cost"; fi
else
    bad "switch cost (build)"
fi

# 10. the bytecode writer names the one thing it cannot encode
out=$("$KAZE" compile "$CORPUS/ok_join.kaze" 2>&1)
if printf '%s' "$out" | grep -q E_KAZE_FIBER_BYTECODE; then
    ok "compile refuses a fiber program instead of dropping the spawn"
else
    bad "compile refusal (got '$out')"
fi

echo
if [ "$FAIL" -eq 0 ]; then
    echo "phase13: $PASS of $((PASS + FAIL)) checks passed"
    exit 0
fi
echo "phase13: $FAIL of $((PASS + FAIL)) checks failed"
exit 1