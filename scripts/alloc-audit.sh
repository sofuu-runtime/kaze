#!/usr/bin/env bash
# alloc-audit.sh -- Phase 11 gate: a Kaze program allocates only from its arena.
#
# "Zero fragmentation, zero leaks" is easy to claim and hard to see. It is visible here:
# every standard-library suite is emitted to C and the emitted program body is searched for
# a raw allocator call. A body that reaches malloc/calloc/realloc/free has left the arena
# and the gate fails.
#
# The runtime prelude the emitter embeds is audited too, with one known exception:
# kz_read_stream grows a buffer for stdin, because a stream of unknown length cannot be
# sized up front. The gate requires that exception to be the only one, so adding a second
# heap path to the prelude fails here rather than going unnoticed.
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
KAZE="${KAZE_BIN:-$ROOT/bin/kaze}"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

if [ ! -x "$KAZE" ]; then echo "alloc-audit: no compiler at $KAZE" >&2; exit 2; fi

# A raw allocator call, but not the arena's own kmalloc.
RAW='(^|[^_a-zA-Z0-9])(malloc|calloc|realloc|free)[[:space:]]*\('

files=0
bad=0
for src in tests/stdlib/*_test.kaze examples/hello.kaze; do
    [ -f "$src" ] || continue
    name=$(basename "$src" .kaze)
    "$KAZE" emit-c "$src" > "$tmp/$name.c" 2>/dev/null || { echo "alloc-audit: emit failed for $src" >&2; exit 2; }
    files=$((files + 1))

    # 1. The program body: from the routines marker to the end of the file.
    awk '/^\/\* --- Routines --- \*\//{body=1} body' "$tmp/$name.c" > "$tmp/$name.body.c"
    body_hits=$(grep -nE "$RAW" "$tmp/$name.body.c" | grep -v 'kmalloc' | head -5)
    if [ -n "$body_hits" ]; then
        bad=$((bad + 1))
        echo "FAIL  $name program body allocates outside the arena"
        printf '%s\n' "$body_hits" | sed 's/^/    /'
    fi

    # 2. The prelude: any raw allocator line must sit inside kz_read_stream.
    prelude_hits=$(grep -nE "$RAW" "$tmp/$name.c" | grep -v 'kmalloc')
    if [ -n "$prelude_hits" ]; then
        outside=$(printf '%s\n' "$prelude_hits" | while IFS=: read -r line rest; do
            if awk -v target="$line" 'BEGIN{inrs=0}
                /^KZ_MAYBE_UNUSED static inline KzValue kz_read_stream/ {inrs=1}
                inrs && NR==target {print "inside"; exit}
                inrs && /^\}/ {inrs=0}' "$tmp/$name.c" | grep -q inside; then
                :
            else
                printf '%s\n' "$rest"
            fi
        done)
        if [ -n "$outside" ]; then
            bad=$((bad + 1))
            echo "FAIL  $name prelude allocates outside kz_read_stream"
            printf '%s\n' "$outside" | head -3 | sed 's/^/    /'
        fi
    fi
done

echo
echo "alloc-audit: $files programs, $bad outside the arena"
[ "$bad" -eq 0 ]
