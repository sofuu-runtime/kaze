#!/usr/bin/env bash
# List panic-shaped calls in production code.
#
# Test modules in this repository sit at the bottom of each file, so everything
# from the first `#[cfg(test)]` marker onward is treated as test-only. A line
# this script prints is a review item: prove it unreachable and name it in
# docs/ZERO-PANIC.md, or convert it to a typed error.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

find src -name '*.rs' -print0 | sort -z | while IFS= read -r -d '' file; do
    awk '
        /#\[cfg\(test\)\]/ { exit }
        {
            line = $0
            sub(/^[ \t]+/, "", line)
            if (line ~ /^\/\//) next
            if (line ~ /unwrap\(\)|expect\(|panic!|unreachable!/) {
                printf "%s:%d:%s\n", FILENAME, FNR, $0
            }
        }
    ' "$file"
done
