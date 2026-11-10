# Kaze Stability Guarantee (v1.0 LTS)

> **Current-source note:** Capability behavior and the public run report have
> changed from the frozen baseline. The release/compatibility decision is
> pending; the present tree has not passed all release gates or physical
> hardware verification. See [the current reference](KAZE-1.0-REFERENCE.md).

> **Scope.** This guarantee covers the standalone Kaze prototype in this
> repository. The Kaze engine inside `sofuu` is a separate, frozen v0.1 surface
> (`docs/KAZE-SPEC.md`) with its own policy.

## 1. Versioning

Kaze follows semantic versioning for the **prototype**:

- **Major (2.0):** a breaking language, ISA, module-format, CLI or public-API
  change. A v2 artifact is not required to load in v1, and vice versa.
- **Minor (1.x):** additive changes that a v1.0 reader already handles, or
  feature additions behind a new format version if reading requires it. No
  removal or renumbering.
- **Patch (1.0.x):** bug fixes, documentation, tests and benchmark updates with
  no format or surface change.

The module format is **4** and the opcode table is **locked** for the life of
v1.x (see `docs/COMPATIBILITY.md`).

## 2. What is stable

- The language surface in `docs/SPEC.md` §2–§5.
- The ISA numbers, stack effects and module layout in §6.
- The error codes in §7.
- The CLI subcommands `run`, `check`, `compile`, `fmt`, `bench`, `selftest`,
  `repl`, `version` and their documented flags.
- The public Rust items the CLI and the language server use:
  `prepare`, `prepare_file`, `prepare_in`, `compile_source`, `compile_file`,
  `run_source`, `run_file`, `run_module`, `run_prepared`, `RunOptions`,
  `RunReport`, `KazeError`, the `isa` module (types and `verify`), `fmt`, `lsp`.
- File formats: format 4 `.kazebc`, `.kaze` source, format-framed LSP over
  stdio.

## 3. Deprecation

A public item or CLI flag is marked deprecated in its documentation for at
least one minor release before removal. The release notes name the replacement.
No deprecation removes a safety guarantee.

## 4. Platforms

- **Tested here:** macOS arm64 (the development host used for this release).
  CI (`.github/workflows/ci.yml`) runs on Linux x86_64 (`ubuntu-latest`); no
  arm64 CI runner is configured. Linux arm64 has no platform-specific runtime
  requirement beyond the Linux GPIO/PWM paths, but it is **build-from-source and
  self-tested**, not CI-tested: run `cargo test --all-features` there before
  flying a Linux arm64 build.
- **Build from source:** any Rust 2021 toolchain. `cargo test --all-features`
  is the compatibility gate.
- **Live hardware** requires separate verification on the actual devices. The
  older suite uses non-robotics stand-ins and fake sysfs, which must be migrated
  before it can run under `RULES.md` §1.9. See `docs/BENCHMARKS.md` for
  historical evidence, not verification of this working tree.

## 5. Quality gates

Every release must pass:

```bash
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo run --bin kaze -- selftest
scripts/fmt-check.sh
scripts/certify.sh          # the full receipt; SITL/Pi remain hardware-dependent
node editors/vscode/test/lsp.test.js
```

The gates are fail-closed: a new swallowed error, a panic on a reachable path,
a verifier regression, a formatter that changes tokens, an artifact-byte change
or a broken ISA number is a build failure, not a review note.

## 6. Security and support

- The security posture is in `docs/CERTIFICATION.md` and the verifier/fuzz
  suites in `tests/verifier.rs` and `tests/fuzz.rs`.
- A security fix is backported to the latest v1.x minor.
- The v1.0 line is supported until a v2.0 line is published; there is no
  end-of-life date in this document that the project has not announced.

## 7. Honest boundaries

"LTS" here means the **prototype's** language, artifact and CLI surface are
stable and gated. It does not mean the built-in inference engine, the Sofuu
product, or a regulatory flight certification. The flight-readiness claim is
the scorecard and the hardware drills in `docs/CERTIFICATION.md`, not a
certificate from an aviation authority.
