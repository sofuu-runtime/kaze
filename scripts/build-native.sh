#!/usr/bin/env bash
# build-native.sh -- build the Stage A native host and its vendored QBE.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
CC=${CC:-cc}

bash dist/qbe/build.sh
$CC -O2 -std=c99 -w -c dist/kaze_native_rt.c -o dist/kaze_native_rt.o
$CC -O2 -std=c99 -w -I dist/qbe dist/kaze-native.c dist/qbe/libqbe.a -lm -o bin/kaze-native
echo "build-native: bin/kaze-native ($(wc -c < bin/kaze-native | tr -d ' ') bytes)"
