#!/usr/bin/env bash
# Compile-time ABI layout checks for the native backend's runtime interop.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
CC=${CC:-cc}
TMP=$(mktemp -d "${TMPDIR:-/tmp}/kaze-abi.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
SRC="$ROOT/tests/native/abi_layout.c"
CFLAGS="-std=c99 -O2 -Wall -Wextra -Werror -pedantic -I$ROOT/dist"
"$CC" $CFLAGS "$SRC" -o "$TMP/abi_host"
"$TMP/abi_host"
host_arch=$("$TMP/abi_host" >/dev/null 2>&1; uname -m)
x86=skipped
case $(uname -s) in
  Darwin)
    if "$CC" $CFLAGS -arch x86_64 -c "$SRC" -o "$TMP/abi_x86_64.o" 2>"$TMP/x86.err"; then
      x86=compile-clean
    else
      x86="failed"
      cat "$TMP/x86.err" >&2
    fi
    ;;
esac
echo "abi-layout: host($host_arch) assertions passed; x86_64 $x86"
[ "$x86" != failed ]
