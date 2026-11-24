#!/usr/bin/env bash
# compare_tokens.sh — compare the Kaze tokenizer with llama.cpp's tokenizer.
# Usage: compare_tokens.sh <model.gguf>   (one text per line on stdin)
set -u
cd "$(dirname "$0")/../.." || exit 2
ROOT=$(pwd)
MODEL="$1"
TMP=/tmp/kaze-phase12-tok-$$
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP"
cat > "$TMP/texts"
CC=cc
$CC -std=c99 -O2 -Wall -Wextra -Werror -pedantic -I "$ROOT/dist" \
    "$ROOT/scripts/phase12/test_tokenizer.c" "$ROOT/dist/kaze_tokenizer.c" \
    "$ROOT/dist/kaze_gguf.c" "$ROOT/dist/kaze_tensor.c" "$ROOT/dist/kaze_quant.c" \
    "$ROOT/dist/kaze_rt.c" -lm -o "$TMP/test_tokenizer" || exit 1
$CC -O2 -I "$HOME/llama.cpp/include" -I "$HOME/llama.cpp/ggml/include" \
    "$ROOT/scripts/phase12/ref_tokenize.c" -L "$HOME/llama.cpp/build/bin" \
    -lllama -lggml -lggml-base -Wl,-rpath,"$HOME/llama.cpp/build/bin" \
    -o "$TMP/ref_tokenize" || exit 1
"$TMP/ref_tokenize" "$MODEL" < "$TMP/texts" 2>/dev/null | grep -v '^decode-mismatch' > "$TMP/ref.txt"
"$TMP/test_tokenizer" "$MODEL" < "$TMP/texts" 2>/dev/null | grep -v '^tokenizer:' | grep -v '^decode-mismatch' > "$TMP/kaze.txt"
if diff -u "$TMP/ref.txt" "$TMP/kaze.txt" > "$TMP/diff.txt"; then
    echo "tokenizer parity: $(wc -l < "$TMP/texts" | tr -d ' ') texts match llama.cpp exactly"
    grep -c '^decode-mismatch' "$TMP/kaze.txt" 2>/dev/null || true
    exit 0
fi
echo "tokenizer parity: MISMATCH"
sed 's/^/    /' "$TMP/diff.txt" | head -20
exit 1
