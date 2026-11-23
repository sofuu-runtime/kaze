# Retired with the Rust engine (Phase 10)

These scripts tested the Rust reference implementation, which Phase 10 removes:

- `diff-check.sh`, `diff-fuzz.sh` compared the Rust **interpreter** against the native
  binaries. With no interpreter there is nothing to diff; the same claim is now covered by
  gate 3 of `scripts/kaze-bootstrap-verify.sh`, which compares native binaries against
  frozen reference streams in `scripts/selftest/run-golden/`.

They are kept rather than deleted so the intent is on record, and they still work if a
reference interpreter is put back on PATH via `KAZE_BIN`.
