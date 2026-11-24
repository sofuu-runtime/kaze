#!/usr/bin/env bash
# kaze-profile.sh -- per-routine call counts and inclusive/exclusive time.
#
# The compiler needs no instrumentation flag: the emitted C is compiled with
# `-finstrument-functions` and linked with dist/kaze_profile.c. The shim writes
# addresses, counts and nanoseconds; this script resolves the addresses with
# atos. That keeps the profile honest (it is measured, not inferred) and keeps
# the compiler free of a profiling mode it cannot test on its own.
#
#   scripts/kaze-profile.sh examples/hello.kaze
#   scripts/kaze-profile.sh examples/hello.kaze --top 10
#
# Output columns: calls, inclusive ms, exclusive ms, routine.
# Limits: routine granularity, fixed 4096-function/4096-frame tables, timing
# overhead included, and no source-line attribution (the emitted C has no #line
# directives yet). Unavailable information is reported, never invented.
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
KAZE="${KAZE_BIN:-$ROOT/bin/kaze}"
CC=${CC:-cc}
ATOS=${ATOS:-atos}
top=${KAZE_PROFILE_TOP:-25}

if [ $# -lt 1 ]; then
    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
src=$1; shift
while [ $# -gt 0 ]; do
    case $1 in
        --top|-n) shift; top=${1:-25} ;;
        *) echo "kaze-profile: unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done
if [ ! -f "$src" ]; then echo "kaze-profile: no such file: $src" >&2; exit 2; fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cfile="$tmp/program.c"
prog="$tmp/program"
data="$tmp/profile.txt"

if ! "$KAZE" emit-c "$src" > "$cfile" 2>"$tmp/emit.err"; then
    echo "kaze-profile: emit-c failed" >&2
    head -5 "$tmp/emit.err" >&2
    exit 1
fi

# Compile the program with instrumentation; the runtime and the shim stay
# uninstrumented so the table is the program's routines, not libc's.
if ! "$CC" -std=c99 -O1 -g -w -finstrument-functions -I "$ROOT/dist" -c "$cfile" -o "$tmp/program.o" 2>"$tmp/cc.err"; then
    echo "kaze-profile: the emitted C did not build" >&2
    head -10 "$tmp/cc.err" >&2
    exit 1
fi
if ! "$CC" -std=c99 -O2 -w -I "$ROOT/dist" -c "$ROOT/dist/kaze_rt.c" -o "$tmp/rt.o" 2>>"$tmp/cc.err"; then
    echo "kaze-profile: the runtime did not build" >&2
    head -10 "$tmp/cc.err" >&2
    exit 1
fi
# The shim records the runtime load base, so atos resolves addresses even on a
# PIE executable. -no_pie is requested too, but the base is authoritative.
if ! "$CC" -std=c99 -O1 -g -w "$tmp/program.o" "$tmp/rt.o" "$ROOT/dist/kaze_profile.c" \
        -Wl,-no_pie -lm -o "$prog" 2>"$tmp/link.err"; then
    echo "kaze-profile: the instrumented program did not link" >&2
    head -10 "$tmp/link.err" >&2
    exit 1
fi

KAZE_PROFILE_OUT="$data" "$prog" "$@" >/dev/null 2>"$tmp/run.err"
rc=$?
if [ ! -s "$data" ]; then
    echo "kaze-profile: the program produced no profile (exit $rc)" >&2
    head -5 "$tmp/run.err" >&2
    exit 1
fi

if ! command -v "$ATOS" >/dev/null 2>&1; then
    echo "kaze-profile: no atos; raw addresses follow" >&2
    sed -n '6,$p' "$data"
    exit 0
fi
arch=$(uname -m)
base=$(awk '$1 == "base" { print $2 }' "$data")

printf '%-8s %12s %12s  %s\n' calls 'inclusive ms' 'exclusive ms' routine
sort -k2 -nr <(sed -n '6,$p' "$data") | head -n "$top" | while read -r calls incl excl addr; do
    name=$("$ATOS" -o "$prog" -arch "$arch" -l "$base" "$addr" 2>/dev/null | head -1)
    [ -n "$name" ] || name=$addr
    printf '%-8s %12.3f %12.3f  %s\n' "$calls" \
        "$(awk -v n="$incl" 'BEGIN { print n / 1000000 }')" \
        "$(awk -v n="$excl" 'BEGIN { print n / 1000000 }')" \
        "$name"
done

# The root routine is the second line of the program's own table entry; this is
# informational, not a claim that the profiler can see host waits.
dropped=$(awk '$1 == "dropped-frames" { print $2 }' "$data")
if [ "${dropped:-0}" != "0" ]; then
    echo "profile: $dropped frames were dropped (stack depth ceiling)" >&2
fi
exit $rc
