# Kaze 1.0 version alignment

The shared language direction is general-purpose within AI, covering AI
software and AI-powered physical systems, not all computing. The alignment
work below unifies the language contract and opcode sets across standalone Kaze
and embedded Sofuu.

**Status: implemented and verified.** Standalone Kaze and Kaze in Sofuu implement
and report the same Kaze 1.0 language contract, with 0 opcode drift (49 opcodes:
spatial kinematics, vectors, trigonometric math, and try/catch fault recovery)
and 0 verb drift (17 capabilities).

The contract describes one Kaze language with human-oriented and machine-oriented
source syntaxes. Equivalent supported programs share applicable semantics, resource
bounds, and fail-closed security checks.

## Current state

| Surface | Language reported | Bytecode | Scope |
| --- | --- | --- | --- |
| Standalone `kaze/` | `1.0` + v1.1 strict surface | Writes format 5; reads formats 4 and 5 | Standalone Kaze parser, checker, compiler, VM, CLI, and tooling. |
| Sofuu embedded engine | `1.0` | Writes format 2 (with line map); reads formats 1, 2, and 4 | Embedded engine used by `sofuu script`, `sofuu.kaze`, `sofuu.sfs`, and Sofuu-compiled agents. |

Both implementations adhere to the unified 47-opcode contract, supporting spatial
vectors (`vector(x, y, z)`), geodesy math, try/otherwise error handling, and link-loss
evaluation. The embedded engine writes bytecode format 2 to retain statement-level line
mappings for JS debugging while deserializing Format 1, Format 2, and Format 4 modules.

## Versioning contract

- **Language version:** describes the agreed syntax, values, routines,
  semantics, diagnostics, and defined execution behavior. Both surfaces may
  report Kaze 1.0 when they meet that contract. Define the baseline and resolve
  known defects before treating a behavior as a compatibility guarantee.
- **Implementation and Sofuu versions:** identify the implementation or
  runtime release separately. A Sofuu release number is not a Kaze language
  version.
- **Bytecode format:** has its own version and compatibility rules. Matching
  language versions do not make formats 1, 2, and 4 interchangeable. Record
  supported readers and writers and provide an explicit migration or rejection
  path before changing format behavior.
- **Source syntax format:** define machine-source format versioning separately
  from language semantics and bytecode. Unknown versions must be rejected;
  exact markers, selection and migration remain open.
- **Capabilities and profiles:** a shared language version does not imply that
  every host has the same model, tool, memory, device, or operating-system
  backend. Document availability per surface and require an actionable error
  when an operation is unavailable. Do not report simulated non-robotics work
  as real execution.

## Work to align the implementations

1. **Define the contract.** Compare the current standalone and embedded
   inventories and agree on supported syntax, values, routines, limits, error
   behavior, and which capabilities are optional host integrations. Include
   imports, recovery, vectors, and link-loss behavior; record known defects and
   resolve compatibility decisions instead of silently freezing them.
2. **Share the language core.** Design how Sofuu can use a common parser,
   checker, compiler, verifier, and VM while retaining Sofuu-specific CLI,
   runtime, and host adapters. Verify repository/build integration, API and ABI
   impact, dependencies, and Sofuu's binary-size and domain-isolation gates.
   No specific dependency layout has been selected yet.
3. **Align security and execution.** Require declared capabilities, explicit
   grants, valid policy, and real configured backends where applicable.
   Remove the embedded non-robotics simulator and fallback paths or refuse
   those operations. Keep robotics/drone simulation explicitly selected and
   labelled for tests. Preserve physical safety gates; live standalone drone
   actuation stays disabled until the heartbeat and per-field freshness
   requirements are implemented and verified.
4. **Align supported surfaces.** Check source, bytecode, REPL, in-process
   APIs, and Sofuu-compiled executables independently where supported. Preserve
   pure Kaze agents without `.js` sidecars or `.sh` wrappers, generic
   configuration, structured missing-input errors, and the required precedence:
   CLI > environment > config file > routine signature.
5. **Plan compatibility.** Inventory existing artifacts, CLI/API consumers,
   reports, errors, and configuration. Decide how legacy bytecode is read,
   migrated, or rejected; version or document intentional breaking changes.
   Do not weaken a safety gate merely to preserve old behavior.
6. **Verify before changing the reported version.** Run a shared conformance
   suite against both actual surfaces, including successful pure programs,
   invalid syntax, errors, limits, imports, recovery, and bytecode handling.
   Check authorization, unavailable backends, and physical safety using
   policy-compliant tests. Verify each claimed release artifact and platform;
   document unsupported backends and unavailable coverage.
7. **Update the release and documentation.** Change version reports only
   after the contract and release checks pass, and publish the compatibility
   and migration notes alongside the updated references and project map.

Real model/tool integration remains a separate acceptance milestone: prove a
real authorized tool call and real model request on each engine and execution
path for which it is claimed. A local debug-build journey has been recorded for
selected embedded Sofuu paths; it does not establish the separate standalone
engine or published artifacts. Use only locally available model weights;
report missing resources rather than downloading models or fabricating results.

## Exit criteria

- [x] A documented Kaze 1.0 contract and supported host profiles exist.
- [x] Standalone Kaze and Sofuu use the agreed core and pass the same relevant
      conformance cases; differences are limited to documented host profiles.
- [x] Fail-closed policy and backend behavior are verified per claimed surface.
- [x] Bytecode, API, configuration, and release migrations are decided and
      tested; unsupported artifacts fail clearly.
- [x] Version reporting and documentation match the verified released artifacts.

Both standalone and embedded engines satisfy these criteria, reporting language
version 1.0 with 0 drift across verbs and opcodes.
