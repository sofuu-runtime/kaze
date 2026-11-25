#!/usr/bin/env bash
# Build the vendored QBE (MIT) into libqbe.a for in-process use.
set -eu
cd "$(dirname "$0")"
CC=${CC:-cc}
CFLAGS=${CFLAGS:--O2 -std=c99 -fPIC -w}
rm -rf obj
mkdir -p obj/amd64 obj/arm64 obj/rv64
for src in *.c amd64/*.c arm64/*.c rv64/*.c; do
    [ "$src" = "main.c" ] && continue
    obj="obj/${src%.c}.o"
    $CC $CFLAGS -I. -c "$src" -o "$obj"
done
ar rcs libqbe.a obj/*.o obj/amd64/*.o obj/arm64/*.o obj/rv64/*.o
echo "qbe: libqbe.a ($(wc -c < libqbe.a | tr -d ' ') bytes, QBE 1.2)"
