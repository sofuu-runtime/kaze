#!/usr/bin/env bash
# build-kaze-verify.sh -- build the Phase 16 verifier with the self-hosted compiler.
#
#   build-kaze-verify.sh [output]
#
# The verifier reuses the compiler's token/AST/lexer/parser modules (so it sees
# exactly the language the compiler sees) and adds the verification module and a
# small CLI. Nothing in compiler/*.kaze is modified; the verifier is a separate
# binary built the same way the compiler's own monolith is built.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
OUT=$1
if [ -z "$OUT" ]; then OUT="$ROOT/bin/kaze-verify"; fi
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
PARTS="$ROOT/compiler/token.kaze $ROOT/compiler/ast.kaze $ROOT/compiler/lexer.kaze $ROOT/compiler/parser.kaze $ROOT/scripts/phase16/verify.kaze $ROOT/scripts/phase16/verify_main.kaze"
: > "$TMP/verify_all.kaze"
for p in $PARTS; do
    grep -v '^use "' "$p" >> "$TMP/verify_all.kaze"
    echo "" >> "$TMP/verify_all.kaze"
done
"$ROOT/bin/kaze" emit-c "$TMP/verify_all.kaze" > "$TMP/verify.c"
cc -std=c99 -O2 -w -I "$ROOT/dist" "$TMP/verify.c" "$ROOT/dist/kaze_rt.c" -lm -o "$OUT"
echo "build-kaze-verify: wrote $OUT"
