# Kaze 1.0.0 — release preparation

## Surface

- Language `1.0`, module format `4` (frozen).
- Binaries: `kaze` (CLI: run/check/compile/fmt/bench/selftest/repl/version) and
  `kaze-lsp` (stdio language server).
- Editor extension: `editors/vscode/` (VS Code and Cursor, dependency-free).
- Standard library: `lib/` (safety, guardrail, survey).
- Specification: `docs/SPEC.md`; compatibility: `docs/COMPATIBILITY.md`;
  stability: `docs/STABILITY.md`; certification: `docs/CERTIFICATION.md`.

## Gates (all must be green)

```bash
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo run --bin kaze -- selftest
scripts/fmt-check.sh
node --check editors/vscode/extension.js
node editors/vscode/test/lsp.test.js
scripts/certify.sh
```

`scripts/certify.sh` prints a receipt with the host, revision, content
fingerprint, artifact SHA-256s, gate results and the two hardware-dependent
skips. It refuses a dirty tree; `--allow-dirty` produces a development receipt
that is **not** release evidence. Attach a clean-tree receipt to the release.

## Artifacts

1. `cargo build --release --bin kaze --bin kaze-lsp`.
2. `cargo run --bin kaze -- bench examples/missions/eight_waypoints.kaze
   --iterations 20 --cap drone --cap log --json` (record the timings).
3. `editors/vscode`: `npx @vscode/vsce package` → `kaze-1.0.0.vsix`.
4. `cargo package` / the release tarball as the owner prefers.

## Owner-only steps

- Tag `v1.0.0` and publish the release (the owner does this, not an agent).
- Publish the npm package if the CLI is distributed there.
- Refresh the download page / landing artifacts.
- Run the SITL and Pi 5 drills on the machines that have that hardware and
  attach their receipts (they are **not** claimed here).

## Known limits to publish, not bury

- The shipped Sofuu product is a separate v0.1 Kaze surface; a v1.0 `.kazebc`
  does not run there. The drift gate reports the divergence.
- No user-routine calls, no namespaces, no conditional imports.
- Bare telemetry is resolved per file.
- Built-in inference is experimental and not part of this language release.
- SITL and Pi 5 soak evidence is outstanding on this machine; see
  `docs/CERTIFICATION.md` §1 rows 22–23.

## Version consistency checklist

- [ ] `Cargo.toml` `version = "1.0.0"`, `src/lib.rs` `VERSION = "1.0"`.
- [ ] `isa::FORMAT_VERSION == 4`, `tests/fixtures/kaze-v1.0.kazebc.hex` matches.
- [ ] README top note, roadmap rows v0.6–v1.0, embedding example `1.0.0`.
- [ ] `editors/vscode/package.json` version and README VSIX name `1.0.0`.
- [ ] Test counts in the checklists and `docs/MODULES.md`.
- [ ] `cargo run --bin kaze -- version` prints `kaze 1.0`.
