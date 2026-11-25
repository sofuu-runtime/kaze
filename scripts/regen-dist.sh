#!/usr/bin/env bash
# regen-dist.sh -- regenerate dist/kaze.c with no Rust toolchain.
#
# Phase 10: the standalone tree has no Rust. dist/kaze.c is produced by the self-hosted
# compiler itself: build a compiler from the C already in dist/, ask it to emit its own
# source, and require the result to be identical. When the two differ the tree is being
# advanced (a compiler edit), so the new C is adopted and the next run must then be a
# fixed point. scripts/kaze-bootstrap-verify.sh proves the fixed point independently.
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
CC=${CC:-cc}
OUT="$ROOT/dist/kaze.c"
BOOT="$ROOT/bin/kaze-bootstrap"

[ -f "$OUT" ] || { echo "regen-dist: $OUT is missing; it is the bootstrap" >&2; exit 1; }
mkdir -p "$ROOT/bin"
# A unique candidate path lets two compiler workstreams regenerate at once;
# with a fixed .new name the slower one adopts a file the faster one moved.
NEW=$(mktemp "$OUT.XXXXXX")
trap 'rm -f "$NEW" "$NEW.bin" "$NEW.err"' EXIT

if ! "$CC" -O2 -w -I "$ROOT/dist" "$OUT" "$ROOT/dist/kaze_rt.c" -lm -o "$BOOT"; then
    echo "regen-dist: the C in dist/ does not build" >&2
    exit 1
fi
if ! "$BOOT" emit-c compiler/kaze_self.kaze > "$NEW" 2>/dev/null; then
    echo "regen-dist: the bootstrap compiler could not emit its own source" >&2
    rm -f "$NEW"
    exit 1
fi
if [ ! -s "$NEW" ]; then
    echo "regen-dist: the emitted C is empty" >&2
    rm -f "$NEW"
    exit 1
fi
# Never install a bootstrap that does not compile: a poisoned dist/kaze.c has to be
# recovered with git, and that happened twice. The candidate is linked first.
if ! "$CC" -x c -O1 -w -I "$ROOT/dist" "$NEW" "$ROOT/dist/kaze_rt.c" -lm -o "$NEW.bin" 2>"$NEW.err"; then
    echo "regen-dist: the emitted C does not build; dist/kaze.c left untouched" >&2
    head -5 "$NEW.err" >&2
    rm -f "$NEW" "$NEW.err"
    exit 1
fi
rm -f "$NEW.bin" "$NEW.err"

if cmp -s "$OUT" "$NEW"; then
    echo "regen-dist: fixed point - dist/kaze.c already reproduces itself"
    rm -f "$NEW"
else
    mv "$NEW" "$OUT"
    echo "regen-dist: dist/kaze.c regenerated from the self-hosted compiler"
fi
bytes=$(wc -c < "$OUT" | tr -d ' ')
sha=$( (shasum -a 256 "$OUT" 2>/dev/null || sha256sum "$OUT") | awk '{print $1}')
echo "regen-dist: dist/kaze.c $bytes bytes, sha256 $sha"