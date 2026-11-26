#!/usr/bin/env bash
# kaze-repl.sh -- a small interactive Kaze session.
#
# The compiler reads one file and has no loader, so the session is a buffer that
# is flattened, checked and run on demand. Definitions persist because the buffer
# is re-compiled whole; effects do not persist because each run is a fresh
# process. That trade-off is deliberate and printed by :help.
#
#   scripts/kaze-repl.sh                 start empty
#   scripts/kaze-repl.sh mission.kaze    preload a file
#
# Input:
#   action greet() -> num:        append a definition (finish the block with a
#       log.info("hi")            blank line, or keep typing)
#       return 0
#   = 1 + 2                       evaluate an expression and print its value
#   :run                          run the buffered program
#   :check                        check the buffer without running it
#   :show                         print the buffer
#   :clear                        empty the buffer (keeps the preloaded file)
#   :load FILE                    replace the buffer with FILE
#   :save FILE                    write the buffer to FILE
#   :history                      print the lines entered in this session
#   :help                         the short reference
#   :quit                         leave
#
# Environment: KAZE_BIN, KAZE_HOST, KAZE_CC (same names the other scripts use).
set -u
cd "$(dirname "$0")/.." || exit 2
ROOT=$(pwd)
KAZE="${KAZE_BIN:-$ROOT/bin/kaze}"
HOST="${KAZE_HOST:-$ROOT/bin/kaze-host}"
CC=${CC:-cc}

if [ ! -x "$KAZE" ]; then echo "kaze-repl: no compiler at $KAZE" >&2; exit 2; fi
if [ ! -x "$HOST" ]; then echo "kaze-repl: no host at $HOST" >&2; exit 2; fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
session=""
preload=""
if [ $# -gt 0 ]; then
    if [ ! -f "$1" ]; then echo "kaze-repl: no such file: $1" >&2; exit 2; fi
    preload=$(cat "$1")
    session="$preload"
fi

history=()
pending=""

say() { printf '%s\n' "$*"; }

usage() {
    say 'Kaze REPL -- definitions accumulate; effects run in a fresh process.'
    say ''
    say '  <kaze source>   append a line (blank line closes a multi-line block)'
    say '  = <expression>  evaluate and print a value'
    say '  :run            compile and run the buffer'
    say '  :check          compile-check the buffer'
    say '  :show           print the buffer'
    say '  :clear          empty the buffer'
    say '  :load FILE      replace the buffer with FILE'
    say '  :save FILE      write the buffer to FILE'
    say '  :history        numbered input history'
    say '  :help           this text'
    say '  :quit           leave'
    say ''
    say 'Limits: no loader (use "use" and the buffer is bundled per run); a run'
    say 'does not preserve values, only source; "=" is unavailable after the'
    say 'buffer defines its own main. A run uses kaze-host with --ticks 8;'
    say '--arg, --entry and capability flags need a file.'
}

needs_block() {
    if ! printf '%s\n' "$session" | grep -q '^[[:space:]]*needs:'; then
        printf 'needs: log\n'
    fi
}

check_buffer() {
    local src="$tmp/check.kaze"
    { needs_block; printf '%s\n' "$session"; } > "$src"
    local out
    out=$("$KAZE" check "$src" 2>&1)
    if printf '%s\n' "$out" | grep -q 'E_KAZE'; then
        printf '%s\n' "$out" | sed 's/^/  /'
        return 1
    fi
    say 'check: ok'
    return 0
}

run_buffer() {
    local src="$tmp/run.kaze"
    { needs_block; printf '%s\n' "$session"; } > "$src"
    if ! printf '%s\n' "$session" | grep -qE '^[[:space:]]*(action|check|filter|fiber)[[:space:]]+main[[:space:]]*\('; then
        printf '\naction main() -> num:\n    return 0\n' >> "$src"
    fi
    local out rc
    out=$("$HOST" run "$src" --ticks 8 2>&1)
    rc=$?
    printf '%s\n' "$out" | sed 's/^\[info\] //'
    return $rc
}

eval_expr() {
    local expr=$1
    if printf '%s\n' "$session" | grep -qE '^[[:space:]]*(action|check|filter|fiber)[[:space:]]+main[[:space:]]*\('; then
        say 'eval: the buffer already defines main; use :run or :clear'
        return 1
    fi
    local src="$tmp/eval.kaze"
    {
        needs_block
        printf '%s\n' "$session"
        printf '\naction main() -> text:\n'
        printf '    return "" + (%s)\n' "$expr"
    } > "$src"
    local out rc
    out=$("$HOST" run "$src" --ticks 8 2>&1)
    rc=$?
    printf '%s\n' "$out" | sed 's/^\[info\] //'
    return $rc
}

say "Kaze REPL. :help for commands, :quit to leave." >&2
[ -n "$preload" ] && say "loaded ${#preload} bytes from $1"

while true; do
    if [ -n "$pending" ]; then
        printf '...> ' >&2
    else
        printf 'kaze> ' >&2
    fi
    if ! IFS= read -r line; then
        printf '\n'
        break
    fi

    if [ -z "$pending" ]; then
        case $line in
            :q|:quit|quit|exit)
                break
                ;;
            :help|help|'?')
                usage
                continue
                ;;
            :show)
                printf '%s\n' "$session"
                continue
                ;;
            :clear)
                session=""
                pending=""
                say 'buffer cleared'
                continue
                ;;
            :history)
                n=${#history[@]}
                i=0
                while [ "$i" -lt "$n" ]; do
                    printf '%4d  %s\n' "$((i + 1))" "${history[$i]}"
                    i=$((i + 1))
                done
                continue
                ;;
            :load\ *)
                f=${line#:load }
                f=${f%\"}; f=${f#\"}
                if [ ! -f "$f" ]; then say "no such file: $f"; continue; fi
                session=$(cat "$f")
                if [ -n "$session" ] && [ "${session: -1}" != $'\n' ]; then session+=$'\n'; fi
                preload="$session"
                say "loaded $f"
                continue
                ;;
            :save\ *)
                f=${line#:save }
                f=${f%\"}; f=${f#\"}
                { needs_block; printf '%s\n' "$session"; } > "$f"
                say "wrote $f"
                continue
                ;;
            :check)
                check_buffer
                continue
                ;;
            :run)
                run_buffer
                continue
                ;;
            =*)
                expr=${line#=}
                expr=${expr# }
                history+=("$line")
                if [ -n "$expr" ]; then eval_expr "$expr"; fi
                continue
                ;;
            :*)
                say "unknown command: $line"
                continue
                ;;
        esac
    fi

    if [ -n "$line" ]; then
        history+=("$line")
        pending+="$line"$'\n'
    else
        # A blank line closes a block and appends it; at the top level it does
        # nothing. That is the paste-friendly rule: a whole action goes in one
        # submission even though each line arrived separately.
        if [ -n "$pending" ]; then
            if [ -n "$session" ] && [ "${session: -1}" != $'\n' ]; then session+=$'\n'; fi
            session+="$pending"
            pending=""
        fi
    fi
done
