#!/usr/bin/env bash
# Compare the self-hosted compiler's verdicts against frozen reference verdicts.
#
# Phase 10 wants the Rust engine retired, and the obstacle is that it is the oracle.
# scripts/capture-golden.py froze its verdicts as data; this script checks the
# self-hosted compiler against them. Divergences are counted and named, and the count
# is a ratchet: it may fall, never rise.
#
#   ./scripts/golden-check.sh                 # build from dist/kaze.c and compare
#   KAZE_BIN=... ./scripts/golden-check.sh    # use a given compiler
#   ./scripts/golden-check.sh --update-baseline
set -u
cd "$(dirname "$0")/.." || exit 1
ROOT=$(pwd)
GOLDEN="$ROOT/scripts/selftest/golden"
BASELINE_FILE="$ROOT/scripts/selftest/golden-baseline.txt"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

if [ -n "${KAZE_BIN:-}" ]; then
  bin=$KAZE_BIN
else
  bin="$tmp/kaze"
  cc -O1 -I dist dist/kaze.c dist/kaze_rt.c -lm -o "$bin" || { echo 'golden-check: cannot build dist/kaze.c'; exit 1; }
fi

baseline=0
[ -f "$BASELINE_FILE" ] && baseline=$(tr -dc '0-9' < "$BASELINE_FILE")
[ -n "$baseline" ] || baseline=0

match=0; differ=0; names=""
codes_of() {
  out=$("$bin" check "$1" 2>&1)
  c=$(printf '%s' "$out" | grep -oE 'E_[A-Z_]+' | sort -u | paste -sd, -)
  if [ -z "$c" ]; then printf 'ok'; else printf '%s' "$c"; fi
}
for g in "$GOLDEN"/*.txt; do
  name=$(basename "$g" .txt)
  want=$(cat "$g")
  path=""
  for cand in "examples/$name.kaze" "examples/missions/$name.kaze" "examples/modular/$name.kaze" "tests/fixtures/strict/$name.kaze" "tests/fixtures/parity/$name.kaze"; do
    [ -f "$cand" ] && path=$cand && break
  done
  [ -z "$path" ] && continue
  got=$(codes_of "$path")
  if [ "$got" = "$want" ]; then
    match=$((match+1))
  else
    differ=$((differ+1))
    names="$names $name(ref=$want,self=$got)"
  fi
done

total=$((match+differ))
echo "golden-check: $match of $total verdicts match the reference, $differ differ (baseline $baseline)"
if [ "$differ" -gt 0 ]; then
  for n in $names; do echo "  differs: $n"; done
fi

if [ "${1:-}" = "--update-baseline" ]; then
  printf '%d\n' "$differ" > "$BASELINE_FILE"
  echo "golden-check: baseline updated to $differ"
  exit 0
fi
if [ "$differ" -gt "$baseline" ]; then
  echo "golden-check: FAIL - divergences rose from $baseline to $differ" >&2
  exit 1
fi
if [ "$differ" -lt "$baseline" ]; then
  echo "golden-check: improved - $differ divergences (baseline $baseline); run --update-baseline to lock it in"
fi
exit 0