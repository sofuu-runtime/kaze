#!/usr/bin/env bash
# module-gates.sh -- the acceptance gates for the native multi-file module system.
#
# Every gate drives the compiler's own loader through `check` or through the
# host's incremental `build`; nothing here glues source text together. The
# compiler is built from dist/kaze.c so the gates test the shipped artifact,
# not a developer's stale bin/kaze.
#
#   bash scripts/module-gates.sh
#   KAZE_MODULE_COMPILER=/path/to/kaze bash scripts/module-gates.sh
#   KAZE_HOST=/path/to/kaze-host bash scripts/module-gates.sh
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
CC=${CC:-cc}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/kaze-module-gates.XXXXXX")
# TMPDIR often ends in "/"; the loader normalizes duplicate slashes, so keep the
# fixture root in the same spelling or path assertions miss.
tmp=${tmp//\/\//\/}
trap 'rm -rf "$tmp"' EXIT

FAILED=0
COUNT=0
pass() {
    COUNT=$((COUNT + 1))
    printf 'PASS  %-38s %s\n' "$1" "$2"
}
fail() {
    COUNT=$((COUNT + 1))
    FAILED=$((FAILED + 1))
    printf 'FAIL  %-38s %s\n' "$1" "$2"
    if [ -n "${3:-}" ]; then printf '%s\n' "$3" | head -8 | sed 's/^/    /'; fi
}

# ── tools ──────────────────────────────────────────────────────────────
compiler=${KAZE_MODULE_COMPILER:-}
if [ -z "$compiler" ]; then
    compiler="$tmp/kaze"
    if ! "$CC" -O2 -w -I "$ROOT/dist" "$ROOT/dist/kaze.c" "$ROOT/dist/kaze_rt.c" -lm -o "$compiler" 2>"$tmp/compiler.cc"; then
        echo "module-gates: cannot build the compiler from dist/kaze.c" >&2
        head -10 "$tmp/compiler.cc" >&2
        exit 1
    fi
fi

host=${KAZE_HOST:-}
if [ -z "$host" ]; then
    host="$tmp/kaze-host"
    if ! "$CC" -O2 -w "$ROOT/dist/kaze-host.c" -o "$host" 2>"$tmp/host.cc"; then
        echo "module-gates: cannot build dist/kaze-host.c" >&2
        head -10 "$tmp/host.cc" >&2
        exit 1
    fi
fi

check() {
    "$compiler" check "$1" 2>&1
    return $?
}

now_ns() {
    python3 -c 'import time; print(time.monotonic_ns())'
}

# ── M1: collision immunity ─────────────────────────────────────────────
g1="$tmp/g1"
mkdir -p "$g1"
cat > "$g1/a.kaze" <<'EOF'
pub action run() -> num:
    return helper()

action helper() -> num:
    return 10
EOF
cat > "$g1/b.kaze" <<'EOF'
pub action run() -> num:
    return helper()

action helper() -> num:
    return 20
EOF
cat > "$g1/main.kaze" <<'EOF'
use "./a.kaze" as a
use "./b.kaze" as b

action main():
    return a.run() + b.run()
EOF
if build_out=$(KAZE_BIN="$compiler" KAZE_RT_DIR="$ROOT/dist" "$host" build "$g1/main.kaze" -o "$g1/prog" 2>&1) && \
   [ "$("$g1/prog")" = "30" ]; then
    qbe=0
    if qbe_out=$("$compiler" emit-qbe "$g1/main.kaze" 2>&1) && \
       [ "$(printf '%s' "$qbe_out" | grep -o 'kz_m_[0-9a-f]*__helper' | sort -u | wc -l | tr -d ' ')" = "2" ]; then
        qbe=1
    fi
    if [ "$qbe" -eq 1 ]; then
        pass 'M1 collision immunity' 'kaze build + QBE: two private helpers, both run()=own helper'
    else
        fail 'M1 collision immunity' 'C99 build works but QBE symbols are not per-module' "$qbe_out"
    fi
else
    fail 'M1 collision immunity' 'build failed or wrong result' "$build_out"
fi

# ── M2: visibility enforcement ─────────────────────────────────────────
g2="$tmp/g2"
mkdir -p "$g2"
cat > "$g2/lib.kaze" <<'EOF'
pub action visible() -> num:
    return 1

action hidden() -> num:
    return 2
EOF
cat > "$g2/main.kaze" <<'EOF'
use { hidden } from "./lib.kaze"

action main():
    return hidden()
EOF
out=$(check "$g2/main.kaze"); st=$?
if [ "$st" -eq 1 ] && printf '%s' "$out" | grep -q 'E_KAZE_PRIVATE_SYMBOL' && \
   printf '%s' "$out" | grep -q 'main.kaze:1:'; then
    pass 'M2 private-symbol rejection' 'exit 1, E_KAZE_PRIVATE_SYMBOL at line 1'
else
    fail 'M2 private-symbol rejection' "status=$st" "$out"
fi

# ── M3: cyclic import rejection ────────────────────────────────────────
g3="$tmp/g3"
mkdir -p "$g3"
cat > "$g3/a.kaze" <<'EOF'
use "./b.kaze"

pub action a() -> num:
    return 1

action main():
    return a()
EOF
cat > "$g3/b.kaze" <<'EOF'
use "./a.kaze"

pub action b() -> num:
    return 2
EOF
out=$(check "$g3/a.kaze"); st=$?
if [ "$st" -eq 1 ] && printf '%s' "$out" | grep -q 'E_KAZE_CYCLIC_IMPORT' && \
   printf '%s' "$out" | grep -q "$g3/a.kaze" && printf '%s' "$out" | grep -q "$g3/b.kaze"; then
    pass 'M3 cycle rejection' 'exit 1, full a -> b -> a path'
else
    fail 'M3 cycle rejection' "status=$st" "$out"
fi

# ── M4: cross-module capability attenuation ────────────────────────────
g4="$tmp/g4"
mkdir -p "$g4"
cat > "$g4/drone.kaze" <<'EOF'
needs: drone

pub action fly() -> num:
    drone.takeoff(1)
    return 1
EOF
cat > "$g4/main.kaze" <<'EOF'
use "./drone.kaze" as drone

action main():
    return drone.fly()
EOF
cat > "$g4/ok.kaze" <<'EOF'
needs: drone
use "./drone.kaze" as drone

action main():
    return drone.fly()
EOF
out=$(check "$g4/main.kaze"); st=$?
okout=$(check "$g4/ok.kaze"); okst=$?
if [ "$st" -eq 1 ] && [ "$okst" -eq 0 ] && printf '%s' "$out" | grep -q 'E_KAZE_CAP_DENIED'; then
    pass 'M4 capability attenuation' 'pure caller refused, declared caller accepted'
else
    fail 'M4 capability attenuation' "deny=$st accept=$okst" "$out
$okout"
fi

# ── M5: cross-module units and tensor shapes ──────────────────────────
g5="$tmp/g5"
mkdir -p "$g5"
cat > "$g5/lib.kaze" <<'EOF'
pub action move(d: meters) -> num:
    return d

pub action shape(x: tensor<f32,[3,2]>) -> num:
    return (length of x)

pub action project(x: tensor<f32,[2,3]>, w: tensor<f32,[3,2]>) -> tensor<f32,[2,2]>:
    return matmul(x, w)
EOF
cat > "$g5/ok.kaze" <<'EOF'
use { project } from "./lib.kaze"

action main() -> number:
    let a = tensor([1, 2, 3, 4, 5, 6], [2, 3])
    let b = tensor([1, 2, 3, 4, 5, 6], [3, 2])
    return tensor_get(project(a, b), 0)
EOF
cat > "$g5/main.kaze" <<'EOF'
use { move, shape } from "./lib.kaze"

action main():
    let a = move(3 seconds)
    let b = shape(zeros([2, 3]))
    return a + b
EOF
out=$(check "$g5/main.kaze"); st=$?
tens=0
if tens_out=$(KAZE_BIN="$compiler" KAZE_RT_DIR="$ROOT/dist" "$host" build "$g5/ok.kaze" -o "$g5/ok" 2>&1) && \
   [ "$("$g5/ok")" = "22" ]; then
    tens=1
fi
if [ "$st" -eq 1 ] && printf '%s' "$out" | grep -q 'E_KAZE_UNIT_MISMATCH' && \
   printf '%s' "$out" | grep -q 'E_KAZE_SHAPE_MISMATCH' && [ "$tens" -eq 1 ]; then
    pass 'M5 cross-module units/shapes' 'mismatches rejected; valid tensor module builds and runs'
else
    fail 'M5 cross-module units/shapes' "status=$st tensor_build=$tens" "$out
$tens_out"
fi

# ── M6: incremental per-module cache ──────────────────────────────────
g6="$tmp/g6"
mkdir -p "$g6"
for i in $(seq 0 19); do
    cat > "$g6/m$i.kaze" <<EOF
pub action value_$i() -> num:
    return helper_$i()

action helper_$i() -> num:
    return $i * 2
EOF
done
{
    echo 'use "./m0.kaze" as m0'
    for i in $(seq 1 19); do echo "use \"./m$i.kaze\" as m$i"; done
    echo ''
    echo 'action main() -> num:'
    printf '    return '
    for i in $(seq 0 19); do
        [ "$i" -gt 0 ] && printf ' + '
        printf 'm%s.value_%s()' "$i" "$i"
    done
    echo ''
} > "$g6/main.kaze"

start=$(now_ns)
if ! build_out=$(KAZE_BIN="$compiler" KAZE_RT_DIR="$ROOT/dist" "$host" build "$g6/main.kaze" -o "$g6/prog" 2>&1); then
    fail 'M6 incremental cache' 'clean build failed' "$build_out"
else
    clean_ns=$(( $(now_ns) - start ))
    cp "$g6/prog" "$g6/prog.clean"
    # A private body edit with an unchanged public interface: exactly one object
    # may be rebuilt, and the linked binary must be byte-identical. Three
    # independent probes take the fastest warm rebuild so one noisy scheduling
    # hiccup cannot decide a speed gate on a loaded machine.
    warm_ns=0
    one=1
    same=1
    for probe in 0 1 2; do
        printf '\n# incremental-cache probe %s: private comment only\n' "$probe" >> "$g6/m$probe.kaze"
        start=$(now_ns)
        build_out=$(KAZE_BIN="$compiler" KAZE_RT_DIR="$ROOT/dist" "$host" build "$g6/main.kaze" -o "$g6/prog" 2>&1)
        took=$(( $(now_ns) - start ))
        if [ "$warm_ns" -eq 0 ] || [ "$took" -lt "$warm_ns" ]; then
            warm_ns=$took
        fi
        printf '%s' "$build_out" | grep -q '1 compiled' || one=0
        cmp -s "$g6/prog" "$g6/prog.clean" || same=0
    done
    ratio=$(python3 -c "print('%.2f' % ($warm_ns / max($clean_ns, 1)))")
    fast=$(python3 -c "print(1 if $warm_ns * 5 <= $clean_ns else 0)")
    if [ "$one" -eq 1 ] && [ "$same" -eq 1 ] && [ "$fast" -eq 1 ]; then
        pass 'M6 incremental cache' "1 object rebuilt, identical binary, ${ratio}x clean"
    else
        fail 'M6 incremental cache' "one=$one identical=$same ratio=$ratio" "$build_out"
    fi
fi

echo
if [ "$FAILED" -eq 0 ]; then
    echo "module-gates: $COUNT of $COUNT passed"
    exit 0
fi
echo "module-gates: $FAILED of $COUNT failed"
exit 1
