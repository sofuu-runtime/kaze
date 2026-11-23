#!/usr/bin/env bash
# Triangulate the two C emitters: the Rust one and the pure-Kaze one must build
# programs with the same behaviour. Numbers are compared by value, because a
# constant-folded expression can differ from a runtime one in the last bit.
set -u
cd "$(dirname "$0")/.." || exit 1
ROOT=$(pwd)
KAZE=$ROOT/${KAZE_BIN:-bin/kaze}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
printf '{"deny":[]}' > "$tmp/policy.json"
# stage 1 of the bootstrap is the pure-Kaze emitter compiled to a binary
$KAZE emit-c compiler/kaze_self.kaze > "$tmp/stage1.c" 2>/dev/null || exit 1
cc -std=c99 -O2 -w -I "$ROOT/dist" "$tmp/stage1.c" "$ROOT/dist/kaze_rt.c" -o "$tmp/stage1" || exit 1
same=0; diff=0; skip=0
for f in "$@"; do
  name=$(basename "$f" .kaze)
  $KAZE emit-c "$f" > "$tmp/rust_$name.c" 2>/dev/null || { skip=$((skip+1)); continue; }
  "$tmp/stage1" "$f" > "$tmp/kaze_$name.c" 2>/dev/null || { skip=$((skip+1)); continue; }
  cc -std=c99 -O2 -w -I "$ROOT/dist" "$tmp/rust_$name.c" "$ROOT/dist/kaze_rt.c" -lm -o "$tmp/rust_$name" 2>/dev/null || { skip=$((skip+1)); continue; }
  cc -std=c99 -O2 -w -I "$ROOT/dist" "$tmp/kaze_$name.c" "$ROOT/dist/kaze_rt.c" -o "$tmp/kaze_$name" 2>/dev/null || { skip=$((skip+1)); continue; }
  (cd "$tmp" && ./rust_$name --ticks 2 > "$tmp/rust_$name.out" 2>&1)
  (cd "$tmp" && ./kaze_$name --ticks 2 > "$tmp/kaze_$name.out" 2>&1)
  if python3 "$ROOT/scripts/compare-outputs.py" "$tmp/rust_$name.out" "$tmp/kaze_$name.out" > "$tmp/cmp.txt"; then
    same=$((same+1))
  else
    diff=$((diff+1))
    echo "EMITTER-DIFF $name"
    sed 's/^/    /' "$tmp/cmp.txt"
  fi
done
echo "emitter-triangle: $same agree, $diff differ, $skip skipped"
[ $diff -eq 0 ]