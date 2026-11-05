#!/usr/bin/env bash
# Fail if any example, module or library is not canonically formatted.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BIN="${KAZE_BIN:-}"
if [ -z "$BIN" ]; then
    cargo build --quiet --bin kaze
    BIN="target/debug/kaze"
fi

status=0
while IFS= read -r -d '' file; do
    if ! "$BIN" fmt --check "$file"; then
        status=1
    fi
done < <(find examples lib -name '*.kaze' -print0)

if [ "$status" -eq 0 ]; then
    echo "fmt-check: all .kaze sources are formatted"
fi
exit "$status"
