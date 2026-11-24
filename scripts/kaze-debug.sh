#!/usr/bin/env bash
# kaze-debug.sh -- run a Kaze program under lldb with routine breakpoints.
#
# The compiler emits plain C and does not yet emit #line directives, so a
# breakpoint names a Kaze routine, not a Kaze source line. Once stopped you are
# in the generated C: `frame variable` shows the lowered locals, `bt` shows the
# call stack, and `step`/`next` walk the generated statements. That is a
# source-adjacent debugger, and the limits are stated rather than hidden.
#
#   scripts/kaze-debug.sh mission.kaze --list
#   scripts/kaze-debug.sh mission.kaze --break takeoff --batch
#   scripts/kaze-debug.sh mission.kaze --break main --print x --batch
#   scripts/kaze-debug.sh mission.kaze --trace          # log every call, then run
#   scripts/kaze-debug.sh mission.kaze                 # interactive lldb
#
# A routine breakpoint is `--break <name>`; `main` maps to the generated
# `kaze_fn__kz_main`. Arguments after `--` are passed to the program.
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
KAZE="${KAZE_BIN:-$ROOT/bin/kaze}"
CC=${CC:-cc}
LLDB=${LLDB:-lldb}

if [ $# -lt 1 ]; then
    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
src=$1; shift
if [ ! -f "$src" ]; then echo "kaze-debug: no such file: $src" >&2; exit 2; fi

list_routines() {
    # "line routine" -- the declaration form is not repeated for every caller.
    grep -nE '^[[:space:]]*(action|check|filter|fiber)[[:space:]]+[A-Za-z_][A-Za-z0-9_]*' "$src" |
        sed -E 's/^([0-9]+):[[:space:]]*(action|check|filter|fiber)[[:space:]]+([A-Za-z_][A-Za-z0-9_]*).*/\1  \3/'
}

breaks=()
prints=()
batch=0
trace=0
prog_args=()
while [ $# -gt 0 ]; do
    case $1 in
        --list)
            echo "routines in $src:"
            list_routines
            exit 0
            ;;
        --break|-b)
            shift
            [ $# -gt 0 ] || { echo "kaze-debug: --break needs a routine" >&2; exit 2; }
            breaks+=("$1")
            ;;
        --print|-p)
            shift
            [ $# -gt 0 ] || { echo "kaze-debug: --print needs a variable" >&2; exit 2; }
            prints+=("$1")
            ;;
        --batch|-B) batch=1 ;;
        --trace|-t) trace=1 ;;
        --) shift; while [ $# -gt 0 ]; do prog_args+=("$1"); shift; done; break ;;
        *) echo "kaze-debug: unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

if ! command -v "$LLDB" >/dev/null 2>&1; then echo "kaze-debug: no lldb on PATH" >&2; exit 2; fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cfile="$tmp/program.c"
bin="$tmp/program"
cmd="$tmp/lldb.cmd"

abs_src=$(cd "$(dirname "$src")" && pwd)/$(basename "$src")

if ! "$KAZE" emit-c "$src" > "$cfile" 2>"$tmp/emit.err"; then
    echo "kaze-debug: emit-c failed" >&2
    head -5 "$tmp/emit.err" >&2
    exit 1
fi
sed -i '' -e "s|^/\* --- Routines --- \*/|#line 1 \"$abs_src\"\n/* --- Routines --- */|" "$cfile"
if ! "$CC" -std=c99 -g -O0 -w -I "$ROOT/dist" "$cfile" "$ROOT/dist/kaze_rt.c" -lm -o "$bin" 2>"$tmp/cc.err"; then
    echo "kaze-debug: the emitted C did not build" >&2
    head -10 "$tmp/cc.err" >&2
    exit 1
fi

: > "$cmd"
if [ "$batch" -eq 1 ]; then
    echo 'settings set target.process.stop-on-sharedlibrary-events false' >> "$cmd"
fi
if [ "$trace" -eq 1 ]; then
    # Instrument every Kaze routine with a one-line stack print, then continue.
    # One breakpoint per routine is explicit: it names the symbols the compiler
    # actually emitted, and it does not depend on lldb's regex support.
    n=0
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        n=$((n + 1))
        echo "breakpoint set --name $name" >> "$cmd"
        echo "breakpoint command add $n" >> "$cmd"
        echo "bt 1" >> "$cmd"
        echo "continue" >> "$cmd"
        echo "DONE" >> "$cmd"
    done < <(nm "$bin" 2>/dev/null | awk '$3 ~ /kaze_fn_/ { print $3 }' | sed 's/^_//' | sort -u)
    if [ "$n" -eq 0 ]; then
        # nm is allowed to be absent; fall back to the routine list in the source.
        while IFS= read -r name; do
            [ -n "$name" ] || continue
            n=$((n + 1))
            echo "breakpoint set --name kaze_fn_$name" >> "$cmd"
            echo "breakpoint command add $n -o \"bt 1\" -o \"continue\"" >> "$cmd"
        done < <(list_routines | awk '{ print $2 }')
    fi
else
    if [ ${#breaks[@]} -eq 0 ]; then
        echo 'breakpoint set --name kaze_fn__kz_main' >> "$cmd"
    else
        for b in ${breaks[@]+"${breaks[@]}"}; do
            if [[ "$b" =~ ^[0-9]+$ ]]; then
                echo "breakpoint set --file \"$abs_src\" --line $b" >> "$cmd"
            elif [[ "$b" =~ :[0-9]+$ ]]; then
                bfile=$(echo "$b" | cut -d: -f1)
                bline=$(echo "$b" | cut -d: -f2)
                echo "breakpoint set --file \"$bfile\" --line $bline" >> "$cmd"
            else
                if [ "$b" = "main" ]; then b="_kz_main"; fi
                echo "breakpoint set --name kaze_fn_$b" >> "$cmd"
            fi
        done
    fi
fi
if [ ${#prog_args[@]} -gt 0 ]; then
    printf 'settings set target.run-args' >> "$cmd"
    printf ' %q' ${prog_args[@]+"${prog_args[@]}"} >> "$cmd"
    echo >> "$cmd"
fi
echo 'run' >> "$cmd"
if [ "$batch" -eq 1 ]; then
    if [ "$trace" -eq 0 ]; then echo 'bt' >> "$cmd"; fi
    for p in ${prints[@]+"${prints[@]}"}; do
        # shellcheck disable=SC2016
        printf 'frame variable %s\n' "$p" >> "$cmd"
    done
    echo 'quit' >> "$cmd"
fi

# lldb's -s runs the command file and then exits, so interactive mode is a
# plain `lldb -- prog`; the file is only needed for breakpoints, and it is
# loaded with -s in both modes. In interactive mode lldb stays open after run.
if [ "$batch" -eq 1 ]; then
    "$LLDB" -b -s "$cmd" -- "$bin"
else
    echo "kaze-debug: generated C at $cfile, binary at $bin"
    echo "kaze-debug: lldb commands in $cmd (breakpoints above); :quit or Ctrl-D leaves."
    "$LLDB" -s "$cmd" -- "$bin"
fi
