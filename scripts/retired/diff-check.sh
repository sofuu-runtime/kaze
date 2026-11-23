#!/usr/bin/env bash
# Differential check: for every program both engines can run, the bytecode VM and
# the emitted C must produce the same observable output — the returned value and
# the sequence of logged values. Values are normalised before comparison: numbers
# round to six significant digits and the VM's quotes around strings are dropped,
# because those are presentation differences, not semantic ones.
set -u
cd "$(dirname "$0")/.." || exit 1
ROOT=$(pwd)
KAZE=$ROOT/${KAZE_BIN:-bin/kaze}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
printf '{"deny":[]}' > "$tmp/policy.json"
same=0; diff=0; skip=0

# one value per line, normalised
normalise() {
  sed 's/^"//; s/"$//' | awk '{
    if ($0 ~ /^-?[0-9]+(\.[0-9]+)?([eE][-+]?[0-9]+)?$/) { printf "%.6g\n", $0; next; }
    if ($0 ~ /^\(/) {
      line = "(";
      n = split($0, parts, ",");
      for (i = 1; i <= n; i++) {
        v = parts[i];
        gsub(/[() ]/, "", v);
        if (v ~ /^-?[0-9]+(\.[0-9]+)?([eE][-+]?[0-9]+)?$/) v = sprintf("%.6g", v);
        line = line (i > 1 ? ", " : "") v;
      }
      printf "%s)\n", line;
      next;
    }
    print $0;
  }'
}

for f in "$@"; do
  name=$(basename "$f")
  abs=$(cd "$(dirname "$f")" && pwd)/$name
  grep -q '^action main' "$abs" || { skip=$((skip+1)); continue; }
  vm_raw=$(cd "$tmp" && "$KAZE" run "$abs" --plain --cap log --cap decide --cap hardware --cap alert --cap memory --cap gpio --cap i2c --cap drone=sim --cap rover=sim --ticks 2 2>/dev/null)
  printf '%s' "$vm_raw" | grep -q '^main -> ' || { skip=$((skip+1)); continue; }
  vm_value=$(printf '%s\n' "$vm_raw" | grep -m1 '^main -> ' | sed 's/^main -> //' | normalise)
  printf '%s\n' "$vm_raw" | sed -n 's/.*log\.info(\(.*\)).*/\1/p' | normalise > "$tmp/vm.log"
  if ! "$KAZE" emit-c "$abs" -o "$tmp/p.c" >/dev/null 2>&1; then
    echo "EMIT-FAIL $name"; diff=$((diff+1)); continue;
  fi
  if ! cc -std=c99 -O2 -w "$tmp/p.c" -o "$tmp/p" 2>/dev/null; then
    echo "BUILD-FAIL $name"; diff=$((diff+1)); continue;
  fi
  nat_raw=$(cd "$tmp" && ./p --ticks 2 2>/dev/null)
  nat_value=$(printf '%s\n' "$nat_raw" | grep -v '^\[info\] ' | tail -1 | normalise)
  printf '%s\n' "$nat_raw" | grep '^\[info\] ' | sed 's/^\[info\] //' | normalise > "$tmp/nat.log"
  ok=1
  if [ "$vm_value" != "nothing" ] && [ -n "$vm_value" ]; then
    [ "$vm_value" = "$nat_value" ] || ok=0
  fi
  cmp -s "$tmp/vm.log" "$tmp/nat.log" || ok=0
  if [ "$ok" = "1" ]; then
    same=$((same+1))
  else
    diff=$((diff+1))
    echo "DIFF $name (value: vm=$vm_value nat=$nat_value)"
    diff "$tmp/vm.log" "$tmp/nat.log" | head -6 | sed 's/^/    /'
  fi
done
echo "diff-check: $same agree, $diff differ, $skip skipped"
[ $diff -eq 0 ]