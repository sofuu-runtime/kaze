#!/usr/bin/env bash
# The v1.0 certification receipt. Software gates run here; the SITL and Pi 5
# drills are reported as hardware-dependent and are not claimed.
set -euo pipefail

ALLOW_DIRTY=0
case "${1:-}" in
    --allow-dirty) ALLOW_DIRTY=1 ;;
    "") ;;
    *)
        echo "usage: scripts/certify.sh [--allow-dirty]" >&2
        exit 2
        ;;
esac

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

REVISION="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
DIRTY="$(git status --porcelain 2>/dev/null | wc -l | tr -d ' ')"
# The revision alone does not identify a dirty tree: hash the tracked diff and
# every untracked file so the receipt names the exact code that was tested.
TREE_FP="$(
    {
        git diff HEAD --binary 2>/dev/null || true
        git ls-files --others --exclude-standard -z 2>/dev/null |
            while IFS= read -r -d '' file; do
                printf 'untracked %s\n' "$file"
                shasum -a 256 "$file" 2>/dev/null || true
            done
    } | shasum -a 256 | cut -d' ' -f1
)"
HOST="$(uname -srm)"
echo "kaze certify — $HOST, revision $REVISION, tree $TREE_FP, $DIRTY uncommitted path(s), $(date -u +%Y-%m-%dT%H:%M:%SZ)"
if [ "$DIRTY" -gt 0 ] && [ "$ALLOW_DIRTY" -ne 1 ]; then
    echo "certify: FAIL — the working tree is dirty; commit the release revision first."
    echo "certify: use --allow-dirty only for a development receipt (it is not release evidence)."
    exit 1
fi
echo

echo "== tests (all features) =="
cargo test --all-features

echo "== clippy =="
cargo clippy --all-targets --all-features -- -D warnings

echo "== build =="
cargo build --release --bin kaze --bin kaze-lsp
KAZE_SHA="$(shasum -a 256 target/release/kaze | cut -d' ' -f1)"
LSP_SHA="$(shasum -a 256 target/release/kaze-lsp | cut -d' ' -f1)"

echo "== selftest =="
target/release/kaze selftest

echo "== format =="
scripts/fmt-check.sh

echo "== editor extension =="
node --check editors/vscode/extension.js
node --check editors/vscode/lsp.js
node editors/vscode/test/lsp.test.js
for file in editors/vscode/package.json \
            editors/vscode/language-configuration.json \
            editors/vscode/syntaxes/kaze.tmLanguage.json \
            editors/vscode/snippets/kaze.json; do
    python3 -m json.tool "$file" >/dev/null
done

echo "== drift (prototype vs product) =="
if [ -f ../scripts/kaze-drift-check.sh ]; then
    (cd .. && bash scripts/kaze-drift-check.sh)
else
    echo "drift: parent script not present (standalone checkout) — skipped"
fi

echo
if [ "$DIRTY" -gt 0 ]; then
    echo "certify: WARNING — development receipt: $DIRTY uncommitted path(s), tree $TREE_FP."
    echo "certify: this is not release evidence until the tree is committed."
fi
echo "certify: PASS (software gates; kaze sha256 $KAZE_SHA; kaze-lsp sha256 $LSP_SHA)"
echo "certify: SITL and Pi 5 soak remain HARDWARE-DEPENDENT:"
echo "  scripts/sitl/eight_waypoints.sh"
echo "  KAZE_DEVICE=... scripts/pi/soak.sh"
