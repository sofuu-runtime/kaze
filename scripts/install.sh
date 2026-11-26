#!/usr/bin/env bash
# install.sh -- install the unified Kaze toolchain.
#
#   bash scripts/install.sh                 # ~/.local (no sudo)
#   PREFIX=/usr/local bash scripts/install.sh
#   DESTDIR=/tmp/pkg PREFIX=/usr/local bash scripts/install.sh
#   bash scripts/install.sh --uninstall
#
# The installed tree is self-contained:
#
#   ${PREFIX}/bin/kaze
#   ${PREFIX}/bin/kaze-native
#   ${PREFIX}/lib/kaze/libkaze_{rt,tensor,sitl}.a
#   ${PREFIX}/lib/kaze/kaze_native_rt.o
#   ${PREFIX}/share/kaze/std/*.kaze
#   ${PREFIX}/share/kaze/grammars/*
#
# The binary carries its runtime sources and headers, so it still builds and
# runs when only this tree is present.  The archives are an optimisation: a
# source checkout without them falls back to the embedded translation units.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
PREFIX=${PREFIX:-$HOME/.local}
DESTDIR=${DESTDIR:-}
DO_BUILD=1
DO_UNINSTALL=0

usage() {
    sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
}

for arg in "$@"; do
    case "$arg" in
        --prefix=*) PREFIX=${arg#--prefix=} ;;
        --destdir=*) DESTDIR=${arg#--destdir=} ;;
        --no-build) DO_BUILD=0 ;;
        --uninstall|uninstall) DO_UNINSTALL=1 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "install: unknown argument $arg" >&2; usage >&2; exit 2 ;;
    esac
done

# Keep PREFIX absolute; a relative prefix would be interpreted inside DESTDIR
# differently by the installed binary.
case "$PREFIX" in
    /*) ;;
    *) PREFIX="$PWD/$PREFIX" ;;
esac
DEST="$DESTDIR$PREFIX"

say() { printf 'install: %s\n' "$*"; }

if [ "$DO_UNINSTALL" -eq 1 ]; then
    say "removing $DEST/bin/kaze and kaze-native"
    rm -f "$DEST/bin/kaze" "$DEST/bin/kaze-native"
    say "removing $DEST/lib/kaze"
    rm -rf "$DEST/lib/kaze"
    say "removing $DEST/share/kaze"
    rm -rf "$DEST/share/kaze"
    # Leave the prefix itself in place: it may contain unrelated tools.
    rmdir "$DEST/bin" "$DEST/lib" "$DEST/share" 2>/dev/null || true
    exit 0
fi

if [ "$DO_BUILD" -eq 1 ]; then
    say "building the unified binary and runtime archives in $ROOT"
    bash "$ROOT/scripts/build-cli.sh"
    say "building the QBE native host"
    if ! bash "$ROOT/scripts/build-native.sh" >/dev/null 2>&1; then
        say "warning: the native host did not build; --backend=native needs KAZE_NATIVE_BIN"
    fi
fi

SOURCE_BIN=${KAZE_INSTALL_BIN:-$ROOT/bin/kaze}
if [ ! -x "$SOURCE_BIN" ]; then
    echo "install: $SOURCE_BIN is missing; run scripts/build-cli.sh" >&2
    exit 2
fi

say "installing to $DEST"
install -d "$DEST/bin" "$DEST/lib/kaze" "$DEST/share/kaze/std" "$DEST/share/kaze/grammars"
install -m 0755 "$SOURCE_BIN" "$DEST/bin/kaze"
if [ -x "$ROOT/bin/kaze-native" ]; then
    install -m 0755 "$ROOT/bin/kaze-native" "$DEST/bin/kaze-native"
fi
if [ -f "$ROOT/dist/kaze_native_rt.o" ]; then
    install -m 0644 "$ROOT/dist/kaze_native_rt.o" "$DEST/lib/kaze/kaze_native_rt.o"
fi

for archive in libkaze_rt.a libkaze_tensor.a libkaze_sitl.a libkaze_blas.a; do
    [ -f "$ROOT/dist/lib/$archive" ] || continue
    install -m 0644 "$ROOT/dist/lib/$archive" "$DEST/lib/kaze/$archive"
done

# The standard library is source; keep every module, not just the ones a
# particular test imports.
if [ -d "$ROOT/lib" ]; then
    for module in "$ROOT"/lib/*.kaze; do
        [ -f "$module" ] || continue
        install -m 0644 "$module" "$DEST/share/kaze/std/$(basename "$module")"
    done
fi

# Editor/grammar assets are optional in a source checkout, but a complete
# installation should have them.
if [ -f "$ROOT/editors/vscode/syntaxes/kaze.tmLanguage.json" ]; then
    install -m 0644 "$ROOT/editors/vscode/syntaxes/kaze.tmLanguage.json" \
        "$DEST/share/kaze/grammars/kaze.tmLanguage.json"
fi
if [ -f "$ROOT/dist/grammars/kaze.gbnf" ]; then
    install -m 0644 "$ROOT/dist/grammars/kaze.gbnf" \
        "$DEST/share/kaze/grammars/kaze.gbnf"
fi
if [ -f "$ROOT/dist/grammars/kaze_machine.gbnf" ]; then
    install -m 0644 "$ROOT/dist/grammars/kaze_machine.gbnf" \
        "$DEST/share/kaze/grammars/kaze_machine.gbnf"
fi

say "installed $(wc -c < "$DEST/bin/kaze" | tr -d ' ') bytes to $DEST/bin/kaze"
if [ -z "$DESTDIR" ]; then
    say "run '$DEST/bin/kaze version' to verify the install"
else
    say "staged under $DESTDIR; run with the matching PREFIX from the target root"
fi
