#!/usr/bin/env bash
# Check a Kaze file the way the language means it: with its imports expanded.
#
# The pure-Kaze checker reads one file and has no loader, so it must waive calls it
# cannot resolve in any file that has a `use` line -- which is how a missing action
# passed a single-file `check` while the concatenated program refused it. This
# wrapper bundles the transitive imports first, so the check is closed-world.
#
#   ./scripts/kaze-check-bundled.sh <file.kaze> [compiler-binary]
#
# Default binary: stage 1 built from compiler/kaze_self.kaze (the pure-Kaze compiler).
set -u
cd "$(dirname "$0")/.." || exit 1
ROOT=$(pwd)
file=${1:?usage: kaze-check-bundled.sh <file.kaze> [compiler]}
bin=${2:-}

if [ -z "$bin" ]; then
  bin="$ROOT/bin/kaze"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

python3 "$ROOT/scripts/bundle-imports.py" "$file" > "$tmp/bundle.kaze" || {
  echo "kaze-check-bundled: refusing $file: an import could not be resolved" >&2
  exit 1
}

"$bin" check "$tmp/bundle.kaze"
status=$?
if [ $status -eq 0 ]; then
  echo "kaze-check-bundled: $file and its imports are valid"
fi
exit $status