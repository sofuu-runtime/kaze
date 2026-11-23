# Kaze Division Registry

> **Single source of truth:** [`kaze-map.md`](../../kaze-map.md) at the repository root.
> This directory used to carry a second full copy of the map; two copies drifted
> (one still said 47 opcodes after the instruction set had 49). The copy is gone:
> the root file is the map, and each division keeps its own detail file here.

## Divisions

| Division | Code | Name | Detail |
|---|---|---|---|
| K01 | `ISA` | Language core, bytecode ISA and instructions | [K01-language-core-isa.md](K01-language-core-isa.md) |
| K02 | `SYNTAX` | Lexer and dual syntax | [K02-lexer-dual-syntax.md](K02-lexer-dual-syntax.md) |
| K03 | `CHECK` | Type checker, units, contracts and strictness | [K03-type-checker-units-contracts.md](K03-type-checker-units-contracts.md) |
| K04 | `COMPILER` | Self-hosted compiler and C99 code generation | [K04-self-hosted-compiler-c99.md](K04-self-hosted-compiler-c99.md) |
| K05 | `VM` | Virtual machine, arena and memory | [K05-vm-runtime-arena.md](K05-vm-runtime-arena.md) |
| K06 | `CAPS` | Capability gates, policy and sandbox | [K06-capability-security-policy.md](K06-capability-security-policy.md) |
| K07 | `ROBOT` | Robotics, effectors and telemetry | [K07-robotics-hardware-telemetry.md](K07-robotics-hardware-telemetry.md) |
| K08 | `TOOLING` | Tooling, LSP and CLI | [K08-tooling-lsp-cli.md](K08-tooling-lsp-cli.md) |
| K09 | `BRIDGE` | Embedded Sofuu bridge and C-FFI | [K09-embedded-sofuu-bridge.md](K09-embedded-sofuu-bridge.md) |
| K10 | `VERIFY` | Verification, conformance and tests | [K10-verification-conformance.md](K10-verification-conformance.md) |

## Status

Phases 1-6 of the roadmap are implemented in the standalone engine;
see the status table in [`kaze-map.md`](../../kaze-map.md) section 5 and the
evidence in [`kaze/docs/PHASE-1-6-REPORT.md`](../../kaze/docs/PHASE-1-6-REPORT.md).
