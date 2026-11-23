#!/usr/bin/env bash
# Differential fuzz: generate programs both engines can run and compare their
# output. Any disagreement is a defect in one of them.
#
#   scripts/diff-fuzz.sh [count] [first-seed]
set -u
cd "$(dirname "$0")/.." || exit 1
ROOT=$(pwd)
KAZE=$ROOT/${KAZE_BIN:-target/debug/kaze}
count=${1:-25}
first=${2:-1}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# capability programs need a policy; both engines read it from the working
# directory, so every run happens here.
printf '{"deny":[]}' > "$tmp/policy.json"
same=0; diff=0; bad=0
seed=$first
last=$((first + count - 1))
while [ $seed -le $last ]; do
  python3 scripts/diff-fuzz.py "$seed" > "$tmp/p.kaze"
  vm_err=$(cd "$tmp" && "$KAZE" run p.kaze --plain --cap log --ticks 2 2>&1 | grep -o 'E_[A-Z_]*' | head -1)
  if [ -n "$vm_err" ] && [ -z "$(cd "$tmp" && "$KAZE" run p.kaze --plain --cap log --ticks 2 2>/dev/null | grep -m1 '^main -> ')" ]; then
    # the program is refused: the binary must refuse with the same code
    if ! "$KAZE" emit-c "$tmp/p.kaze" -o "$tmp/p.c" >/dev/null 2>&1; then bad=$((bad+1)); seed=$((seed+1)); continue; fi
    if ! cc -std=c99 -O2 -w "$tmp/p.c" -o "$tmp/p" 2>/dev/null; then bad=$((bad+1)); seed=$((seed+1)); continue; fi
    nat_err=$(cd "$tmp" && ./p --ticks 2 2>&1 | grep -o 'E_[A-Z_]*' | head -1)
    if [ "$vm_err" = "$nat_err" ]; then
      same=$((same+1))
    else
      diff=$((diff+1))
      echo "DIFF refusal seed=$seed vm=$vm_err nat=$nat_err"
    fi
    seed=$((seed+1)); continue
  fi
  vm=$(cd "$tmp" && "$KAZE" run p.kaze --plain --cap log --ticks 2 2>/dev/null | grep -m1 '^main -> ' | sed 's/^main -> //')
  if [ -z "$vm" ]; then
    bad=$((bad+1))
    echo "UNUSABLE seed=$seed: $(cd "$tmp" && "$KAZE" run p.kaze --plain --cap log --ticks 2 2>&1 | head -1 | cut -c1-90)"
    seed=$((seed+1)); continue
  fi
  if ! "$KAZE" emit-c "$tmp/p.kaze" -o "$tmp/p.c" >/dev/null 2>&1; then bad=$((bad+1)); seed=$((seed+1)); continue; fi
  if ! cc -std=c99 -O2 -w "$tmp/p.c" -o "$tmp/p" 2>/dev/null; then bad=$((bad+1)); seed=$((seed+1)); continue; fi
  nat=$(cd "$tmp" && ./p --ticks 2 2>/dev/null | grep -v '^\[info\] ' | head -1)
  # the logged lines must match too, not just the returned value
  nat_logs=$(cd "$tmp" && ./p --ticks 2 2>/dev/null | grep '^\[info\] ' | sed 's/^\[info\] //')
  vm_logs=$(cd "$tmp" && "$KAZE" run p.kaze --plain --cap log --ticks 2 2>/dev/null | sed -n 's/.*log\.info(\(.*\)).*/\1/p' | sed 's/^"//; s/"$//')
  if [ "$vm_logs" != "$nat_logs" ]; then
    diff=$((diff+1))
    echo "DIFF logs seed=$seed"
    echo "  vm:  $(echo "$vm_logs" | tr '\n' '|')"
    echo "  nat: $(echo "$nat_logs" | tr '\n' '|')"
    seed=$((seed+1)); continue
  fi
  if [ "$vm" = "$nat" ]; then
    same=$((same+1))
  else
    diff=$((diff+1))
    echo "DIFF seed=$seed"
    echo "  vm:  $vm"
    echo "  nat: $nat"
  fi
  seed=$((seed+1))
done
echo "diff-fuzz: $same agree, $diff differ, $bad unusable"
[ $diff -eq 0 ]