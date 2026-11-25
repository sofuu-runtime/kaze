#!/usr/bin/env bash
# build-cli.sh -- build the unified `kaze` executable.
#
#   bash scripts/build-cli.sh
#   CC=clang bash scripts/build-cli.sh
#
# The result is bin/kaze: one process that owns the command surface, the
# multi-file loader and the C linker, with the pure-Kaze compiler and the
# Phase 16 verifier linked in as library entries.  It needs no sibling
# compiler binary at run time and carries its runtime sources in
# dist/kaze_embedded_rt.h.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

CC=${CC:-cc}
AR=${AR:-ar}
CFLAGS=${CFLAGS:--O2}
BOOT="$ROOT/bin/kaze-bootstrap"
OUT="$ROOT/bin/kaze"
CLI="$ROOT/dist/kaze_cli.c"
EMBED="$ROOT/dist/kaze_embedded_rt.h"
VERIFY_C="$ROOT/bin/kaze_verify_gen.c"
COMPILER_O="$ROOT/bin/kaze_compiler_entry.o"
VERIFY_O="$ROOT/bin/kaze_verify_entry.o"
CLI_O="$ROOT/bin/kaze_cli.o"

mkdir -p "$ROOT/bin" "$ROOT/dist/lib"

say() { printf 'build-cli: %s\n' "$*"; }

# 1. A bootstrap compiler from the checked-in C.  It emits the verifier's C
#    and is the same compiler the bootstrap gates use.
if [ ! -x "$BOOT" ] || [ "$ROOT/dist/kaze.c" -nt "$BOOT" ] || [ "$ROOT/dist/kaze_rt.h" -nt "$BOOT" ]; then
    say "building bin/kaze-bootstrap from dist/kaze.c"
    "$CC" -O2 -w -I "$ROOT/dist" "$ROOT/dist/kaze.c" "$ROOT/dist/kaze_rt.c" -lm -o "$BOOT"
fi

# 2. The runtime sources the unified binary carries.  Python is the natural
#    generator, but it is not required to *build* an already-generated tree:
#    keep the checked-in header when no interpreter is installed.
if command -v python3 >/dev/null 2>&1; then
    say "generating dist/kaze_embedded_rt.h"
    python3 "$ROOT/scripts/gen-embedded-rt.py" >/dev/null
elif [ -s "$EMBED" ]; then
    say "python3 not found; using the existing dist/kaze_embedded_rt.h"
else
    echo "build-cli: python3 is needed to generate $EMBED" >&2
    exit 2
fi

# 3. The verifier is a second pure-Kaze program.  Its `main` is renamed to
#    kaze_verify_main so it can live beside the compiler's entry point.
need_verify=0
if [ ! -s "$VERIFY_C" ] || [ "$ROOT/dist/kaze.c" -nt "$VERIFY_C" ]; then
    need_verify=1
else
    for p in compiler/token.kaze compiler/ast.kaze compiler/lexer.kaze compiler/parser.kaze \
             scripts/phase16/verify.kaze scripts/phase16/verify_main.kaze; do
        [ "$p" -nt "$VERIFY_C" ] && need_verify=1
    done
fi
if [ "$need_verify" -eq 1 ]; then
    say "emitting the embedded verifier"
    tmp=$(mktemp -d "${TMPDIR:-/tmp}/kaze-verify-gen.XXXXXX")
    trap 'rm -rf "$tmp"' EXIT
    : > "$tmp/verify_all.kaze"
    for p in compiler/token.kaze compiler/ast.kaze compiler/lexer.kaze compiler/parser.kaze \
             scripts/phase16/verify.kaze scripts/phase16/verify_main.kaze; do
        grep -v '^use "' "$p" >> "$tmp/verify_all.kaze"
        printf '\n' >> "$tmp/verify_all.kaze"
    done
    "$BOOT" emit-c "$tmp/verify_all.kaze" > "$tmp/verify.c"
    mv "$tmp/verify.c" "$VERIFY_C"
    rm -rf "$tmp"
    trap - EXIT
fi

# 4. Compile the two generated entries and the driver.
say "compiling compiler entry"
"$CC" -std=c99 "$CFLAGS" -w -I "$ROOT/dist" -Dmain=kaze_compiler_main -c \
    "$ROOT/dist/kaze.c" -o "$COMPILER_O"

verify_ok=1
say "compiling verifier entry"
if ! "$CC" -std=c99 "$CFLAGS" -w -I "$ROOT/dist" -Dmain=kaze_verify_main -c \
        "$VERIFY_C" -o "$VERIFY_O" 2>"$ROOT/bin/kaze_verify.err"; then
    verify_ok=0
    say "warning: the verifier did not compile; kaze verify will look for a sibling"
    head -5 "$ROOT/bin/kaze_verify.err" >&2 || true
    rm -f "$VERIFY_O"
fi
rm -f "$ROOT/bin/kaze_verify.err"

say "compiling the unified driver"
"$CC" -std=c99 "$CFLAGS" -Wall -Wextra -Wno-unused-parameter -I "$ROOT/dist" \
    -DKAZE_HAVE_VERIFIER="$verify_ok" -c "$CLI" -o "$CLI_O"

# A unique candidate keeps two concurrent workstreams from adopting the same
# half-written binary; the final rename is atomic.
tmp_bin=$(mktemp "$ROOT/bin/kaze.XXXXXX")
trap 'rm -f "$tmp_bin"' EXIT
say "linking bin/kaze"
"$CC" -std=c99 "$CFLAGS" -w -I "$ROOT/dist" \
    "$CLI_O" "$COMPILER_O" \
    $( [ "$verify_ok" -eq 1 ] && printf '%s' "$VERIFY_O" ) \
    "$ROOT/dist/kaze_rt.c" -lm -o "$tmp_bin"
chmod 755 "$tmp_bin"
mv "$tmp_bin" "$OUT"
trap - EXIT

# 5. Pre-built archives used by `kaze build` / `kaze run` when installed.
#    A source checkout still works without them: the CLI falls back to the
#    embedded translation units.
say "building runtime archives in dist/lib"
obj="$ROOT/bin/obj"
rm -rf "$obj"
mkdir -p "$obj"

"$CC" -std=c99 "$CFLAGS" -w -I "$ROOT/dist" -c "$ROOT/dist/kaze_rt.c" -o "$obj/kaze_rt.o"
"$AR" rcs "$ROOT/dist/lib/libkaze_rt.a" "$obj/kaze_rt.o"

tensor_objs=()
for f in kaze_tensor kaze_quant kaze_gguf kaze_llama kaze_tokenizer kaze_autograd; do
    "$CC" -std=c99 "$CFLAGS" -w -pthread -I "$ROOT/dist" -c "$ROOT/dist/$f.c" -o "$obj/$f.o"
    tensor_objs+=("$obj/$f.o")
done
"$AR" rcs "$ROOT/dist/lib/libkaze_tensor.a" "${tensor_objs[@]}"

"$CC" -std=c99 "$CFLAGS" -w -I "$ROOT/dist" -c "$ROOT/dist/kaze_mavlink.c" -o "$obj/kaze_mavlink.o"
# kaze_sitl_client.c is included by the emitted program itself (see
# -DKAZE_SITL_LINK), so only the MAVLink codec belongs in the archive.
"$AR" rcs "$ROOT/dist/lib/libkaze_sitl.a" "$obj/kaze_mavlink.o"

"$CC" -std=c99 "$CFLAGS" -w -I "$ROOT/dist" -c "$ROOT/dist/kaze_blas.c" -o "$obj/kaze_blas.o" 2>/dev/null || true
if [ -f "$obj/kaze_blas.o" ]; then
    "$AR" rcs "$ROOT/dist/lib/libkaze_blas.a" "$obj/kaze_blas.o"
fi

rm -rf "$obj"

say "wrote $OUT ($(wc -c < "$OUT" | tr -d ' ') bytes)"
