#!/usr/bin/env bash
# phase15-gates.sh -- the SITL gate. The checks live in phase15-gates.py so the
# report parsing stays typed; this wrapper keeps the gates.sh calling convention.
set -u
cd "$(dirname "$0")/.." || exit 2
exec python3 scripts/phase15-gates.py "$@"
