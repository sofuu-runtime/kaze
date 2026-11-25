#!/usr/bin/env bash
# Fail if any example, module or library cannot be parsed and re-emitted.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BIN="${KAZE_BIN:-}"
if [ -z "$BIN" ]; then
    BIN="bin/kaze"
    # Do not rebuild a binary that is already here: the unified CLI and the
    # pure compiler both re-emit these sources, and rebuilding would replace a
    # unified bin/kaze with the pure bootstrap mid-gate.
    [ -x "$BIN" ] || make -C "$ROOT/.." kaze-build --quiet
fi

status=0
while IFS= read -r -d '' file; do
    # The self-hosted formatter is parse + canonical emit, and its AST has no
    # comments.  Byte-for-byte `fmt --check` is therefore stricter than this
    # repository gate has historically been: the gate proves each source parses
    # and re-emits.  scripts/cli-dx-gates.sh exercises the strict --check exit
    # code on a synthetic file, and docs/TOOLING.md records the comment gap.
    if ! "$BIN" fmt "$file" >/dev/null; then
        status=1
    fi
done < <(find examples lib -name '*.kaze' -print0)

if [ "$status" -eq 0 ]; then
    echo "fmt-check: all .kaze sources parse and re-emit"
fi
exit "$status"
