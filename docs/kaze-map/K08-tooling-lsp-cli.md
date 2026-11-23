# Division 08: Developer Tooling, LSP & CLI Experience (K08)

> **Maturity:** Core  
> **Parent Blueprint:** [kaze-map/README.md](README.md)  
> **Primary Locations:** `kaze/src/main.rs`, `kaze/src/lsp.rs`, `kaze/src/lsp_main.rs`, `kaze/src/fmt.rs`, `kaze/editors/`, `kaze/src/lib.rs`

---

## 1. Mission & Scope

Division 08 delivers the developer toolchain, command-line interface, code formatting engine, and editor integrations for Kaze. A systems language is only as reliable and effective as the tooling supporting it. Division 08 guarantees that developers and autonomous agents interact with Kaze through deterministic, lightning-fast utilities that provide instant feedback, standardized code formatting, and rich IDE diagnostics.

The core tools of Division 08 are self-contained, requiring zero external package managers, network requests, or third-party background daemons.

---

## 2. Code Manifest & Owned Files

| Path | Purpose |
|---|---|
| `kaze/src/main.rs` | Unified CLI entrypoint, argument parsing, command dispatching (`run`, `check`, `fmt`, `emit-c`, `to-machine`, `to-human`) |
| `kaze/src/lsp.rs` | Language Server Protocol (LSP) server engine handling diagnostics, completions, and hover documentation over JSON-RPC |
| `kaze/src/lsp_main.rs` | Standalone LSP daemon executable entrypoint |
| `kaze/src/fmt.rs` | Idempotent, canonical AST auto-formatter enforcing official Kaze styling |
| `kaze/editors/` | Editor syntax highlighters and configuration files for VS Code and Neovim |
| `kaze/src/lib.rs` | Public library interface for programmatic compilation and embedding |

---

## 3. Architectural Specifications & Invariants

1. **Deterministic 12-Factor CLI Argument Resolution**:
   - Program inputs resolve according to the strict 12-factor hierarchy:
     $$\text{CLI Flags } (\text{--arg key=val}) \;>\; \text{Environment Variables } (\text{AGENT\_*}) \;>\; \text{Configuration File } (\text{--config}) \;>\; \text{Routine Defaults}$$
   - Never hardcode domain parameter names into CLI dispatchers. Missing required parameters emit structured guidance (`E_KAZE_PARAM`) with flag syntax examples.
2. **Canonical Idempotent Auto-Formatting (`kaze fmt`)**:
   - The formatter operates directly on parsed AST nodes, reconstructing canonical human syntax with standardized 2-space indentation, consistent operator spacing, and normalized capability clauses.
   - Idempotency invariant:
     $$\text{fmt}(\text{fmt}(\text{source})) \equiv \text{fmt}(\text{source})$$
3. **Sub-Millisecond Language Server Protocol (LSP)**:
   - The LSP daemon communicates via standard JSON-RPC 2.0 over `stdin`/`stdout`.
   - Diagnostics, symbol indexing, and type hovers are computed in-memory directly from the parsed AST with response times under 5 milliseconds.
4. **Zero Decorative Emojis & Clean ANSI Stripping**:
   - Output adheres strictly to professional formatting standards.
   - When `NO_COLOR=1` or `TERM=dumb` is set, all ANSI terminal escape codes are automatically stripped from compiler output and error messages.

---

## 4. Implementation Details & CLI Command Suite

### Official Kaze CLI Command Reference

| Command | Syntax | Description |
|---|---|---|
| `run` | `kaze run <file.kaze> [--arg k=v]` | Execute routine directly via bytecode VM |
| `check` | `kaze check <file.kaze>` | Run lexer, parser, and static type checker; report diagnostics |
| `compile` | `kaze compile <file.kaze> -o <out.kazebc>` | Compile source to binary format 5 bytecode module |
| `emit-c` | `kaze emit-c <file.kaze> -o <out.c>` | Generate native standalone ANSI C99 source code |
| `fmt` | `kaze fmt <file.kaze> [--check]` | Format source code to canonical standard (or check formatting) |
| `to-machine` | `kaze to-machine <file.kaze>` | Transpile human syntax to machine S-expression format |
| `to-human` | `kaze to-human <file.kaze>` | Transpile machine S-expression format to human syntax |
| `lsp` | `kaze lsp` | Launch Language Server Protocol daemon on stdio |

---

## 5. Verification & Testing

```bash
# Verify CLI command parsing and help output
cargo run --manifest-path kaze/Cargo.toml -- --help

# Verify auto-formatter idempotency
cargo test --manifest-path kaze/Cargo.toml --lib fmt

# Test LSP JSON-RPC message handling
cargo test --manifest-path kaze/Cargo.toml --lib lsp
```

---

## 6. Active & Completed Tasks

- [x] Implement CLI command dispatcher supporting 12-factor argument hierarchy (`main.rs`).
- [x] Build native Language Server Protocol server daemon (`lsp.rs`, `lsp_main.rs`).
- [x] Implement canonical auto-formatter (`fmt.rs`).
- [x] Package syntax highlighting extensions for VS Code (`editors/vscode`).
- [ ] Add semantic token highlighting to LSP server for dimensional units.
- [ ] Add interactive debugging support (`--inspect-brk`) for single-stepping VM opcodes.

## Roadmap phase 1-6 update (2026-11-22)

- `--config <path>` (default `./agent.config.json`) plus `AGENT_<PARAM>`/`SOFUU_<PARAM>` environment resolution, with the CLI argument on top and the signature default at the bottom; a typed parameter with no value is `E_KAZE_PARAM` naming all three fixes.
- CLI and library diagnostics surface the specific code (for example `E_KAZE_MISSING_FALLBACK`) instead of `E_KAZE_CHECK` for everything.
- The CLI gained `--config` help text; `contract_block` errors name the line where the contract must move.
- Verification: `cargo test --manifest-path kaze/Cargo.toml --test contracts --test lsp --test repl`.
