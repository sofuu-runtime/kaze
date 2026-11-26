#!/usr/bin/env bash
# grammar-check.sh -- Phase 15.4: the shipped GBNF accepts the canonical corpus.
#
# The plan asks for 100% of the canonicalized lib/*.kaze and tests/stdlib/*.kaze
# sources to parse under dist/grammars/kaze.gbnf.  The repository keeps some
# sources in their pre-formatter spelling, so this gate canonicalizes a copy with
# `kaze fmt -w` first and then validates that copy.  `dist/grammars/kaze.gbnf`
# remains the constrained-decoding view of the language; the parser and checker
# stay the authority for indentation, scopes and types.
#
# The validator is a packrat reader for the GBNF notation.  llama.cpp's own
# exhaustive validator is exponential on whole source files; see
# scripts/gbnf-validate.py for why ordered choice is sound for this grammar.
#
#   bash scripts/grammar-check.sh
#   KAZE_BIN=/path/to/kaze bash scripts/grammar-check.sh
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
BIN=${KAZE_BIN:-$ROOT/bin/kaze}
GRAMMAR=${KAZE_GRAMMAR:-$ROOT/dist/grammars/kaze.gbnf}
PY=${PYTHON:-python3}

[ -x "$BIN" ] || { echo "grammar-check: missing compiler $BIN" >&2; exit 2; }
[ -f "$GRAMMAR" ] || { echo "grammar-check: missing grammar $GRAMMAR" >&2; exit 2; }
command -v "$PY" >/dev/null 2>&1 || { echo "grammar-check: $PY is not available" >&2; exit 2; }

tmp=$(mktemp -d "${TMPDIR:-/tmp}/kaze-grammar.XXXXXX") || exit 2
trap 'rm -rf "$tmp"' EXIT

mkdir -p "$tmp/lib" "$tmp/stdlib"
cp "$ROOT"/lib/*.kaze "$tmp/lib/" || exit 2
cp "$ROOT"/tests/stdlib/*.kaze "$tmp/stdlib/" || exit 2

# Canonical spelling is the language the grammar documents.
if ! "$BIN" fmt -w "$tmp/lib" "$tmp/stdlib" >"$tmp/fmt.log" 2>&1; then
    echo "grammar-check: fmt failed" >&2
    cat "$tmp/fmt.log" >&2
    exit 1
fi

# The second pass must not move a byte.  A grammar that only accepts the
# formatter's first output would hide a formatter that is still converging.
cp -R "$tmp/lib" "$tmp/lib.once" || exit 2
cp -R "$tmp/stdlib" "$tmp/stdlib.once" || exit 2
if ! "$BIN" fmt -w "$tmp/lib" "$tmp/stdlib" >"$tmp/fmt2.log" 2>&1; then
    echo "grammar-check: second fmt pass failed" >&2
    cat "$tmp/fmt2.log" >&2
    exit 1
fi
if ! diff -r "$tmp/lib.once" "$tmp/lib" >"$tmp/diff.log" 2>&1 ||
   ! diff -r "$tmp/stdlib.once" "$tmp/stdlib" >>"$tmp/diff.log" 2>&1; then
    echo "grammar-check: fmt -w is not idempotent" >&2
    head -20 "$tmp/diff.log" >&2
    exit 1
fi

lib_count=$(find "$tmp/lib" -name '*.kaze' | wc -l | tr -d ' ')
stdlib_count=$(find "$tmp/stdlib" -name '*.kaze' | wc -l | tr -d ' ')
if ! "$PY" "$ROOT/scripts/gbnf-validate.py" "$GRAMMAR" \
        "$tmp"/lib/*.kaze "$tmp"/stdlib/*.kaze; then
    echo "grammar-check: FAIL - canonical corpus does not match $GRAMMAR" >&2
    exit 1
fi

echo "grammar-check: $lib_count lib + $stdlib_count stdlib sources valid under kaze.gbnf"
