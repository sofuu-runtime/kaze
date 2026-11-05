# Kaze Compatibility & ISA Lock (v1.0)

## 1. What is frozen

- **Language version:** `1.0`.
- **Module format:** `4` (`isa::FORMAT_VERSION`).
- **Opcode numbers:** 0–46, exactly as listed in `docs/SPEC.md` §6.3.
- **Artifact bytes:** a fixed source compiles to the bytes in
  `tests/fixtures/kaze-v1.0.kazebc.hex`; `tests/compat.rs` compares every byte.
- **Error codes:** the names in `docs/SPEC.md` §7 are stable.
- **Safety rules:** H-1..H-4 and the non-catchable error set are semantic
  guarantees, not implementation details.

## 2. The rules for changing the surface

1. **Never renumber or reuse an opcode.** The table is a lock file. A new
   operation takes the next number and bumps the module format.
2. **Never change the layout of format 4.** Additive data (a new table, a new
   per-item field) requires format 5; old artifacts must keep loading under the
   same major version if the change is additive, and the reader must say why it
   refuses one it cannot read.
3. **Adding is allowed inside a v1.x release only when readers can keep up.**
   A new optional field that a v1.0 reader would misread is not compatible; the
   format must move instead.
4. **A v1.x implementation must read every v1.0 artifact.** The checked-in
   hex fixture is the regression: if it stops loading and running, the change
   is a v2 change, not a patch.
5. **Remove nothing without a deprecation release.** A CLI flag or public API
   item gets at least one minor version marked deprecated before removal.

`tests/verifier.rs::opcode_numbers_are_locked` and
`tests/compat.rs::the_v1_artifact_bytes_are_frozen` fail the build when a rule
is broken.

## 3. Reader behaviour

- A v1.0 reader accepts format 4 and rejects formats 2/3 (v0.3/v0.4) and any
  unknown version before reading a single instruction, with a version message.
- `deserialize` verifies structure and stack dataflow; a malformed artifact is
  `E_KAZE_BADCODE`, never an out-of-bounds access.
- `run_module` verifies again, so a hand-built `Module` gets the same guarantee
  as a file.

## 4. The product is a separate surface

The shipped `sofuu` engine (`crates/sofuu-core/src/domains/kaze/`) is language
**v0.1**, module format **1**, specified by `docs/KAZE-SPEC.md`. It is not
covered by this document. `scripts/kaze-drift-check.sh` reports the divergence
(prototype ahead) on every run, and the README says so at the top. A v1.0
`.kazebc` does **not** run on the product, and a product v0.1 module does not
run here; both readers refuse the other's version rather than misread it.

## 5. Compatibility checklist for a release

```bash
cargo test --all-features                       # includes the byte and opcode locks
cargo run --bin kaze -- selftest                # version/format + safety scorecard
cargo run --bin kaze -- compile examples/missions/eight_waypoints.kaze -o /tmp/x.kazebc
cargo run --bin kaze -- run /tmp/x.kazebc --cap drone --cap log --plain
```

A green run means: the format still reads, the opcode table is unchanged, the
safety refusals still fire, and the benchmark mission still produces its 12
actions.
