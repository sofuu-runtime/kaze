#!/usr/bin/env bash
# kaze-tensor-build.sh -- compile a Kaze program that uses the Phase 12 tensor
# builtins.
#
#   kaze-tensor-build.sh <kaze-compiler> <program.kaze> <out-binary>
#
# The emitted C includes kaze_tensor.h, so the tensor/weight/inference runtime
# objects are linked alongside kaze_rt.c. Programs that do not use tensors keep
# the plain two-file build; this script is the tensor-enabled variant.
set -euo pipefail
CC=cc
COMPILER=$1
SRC=$2
OUT=$3
HERE=$(cd "$(dirname "$0")" && pwd)
DIST="$HERE/../../dist"
if [ -n "$(printenv KAZE_DIST)" ]; then DIST=$(printenv KAZE_DIST); fi
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
"$COMPILER" emit-c "$SRC" > "$TMP/program.c"
$CC -std=c99 -O2 -Wall -Wextra -Werror -pedantic -I "$DIST" \
    "$TMP/program.c" "$DIST/kaze_rt.c" "$DIST/kaze_tensor.c" \
    "$DIST/kaze_quant.c" "$DIST/kaze_gguf.c" "$DIST/kaze_llama.c" \
    "$DIST/kaze_tokenizer.c" "$DIST/kaze_autograd.c" -lm -o "$OUT"
echo "kaze-tensor-build: wrote $OUT"
