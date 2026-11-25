# Kaze 1.0 for VS Code / Cursor

Syntax highlighting, diagnostics, formatting, hover, completion and document
symbols for `.kaze` files, powered by Kaze's pure-C99 / self-hosted language
server (`kaze-host lsp`).

The extension is **dependency-free**: it does not use `vscode-languageclient`,
needs no `npm install`, and requires **zero Rust toolchain**. It speaks the
language server's JSON-RPC directly over stdio.

## 1. Build the pure-C99 language server

From the repository root (or inside `kaze/`):

```bash
# Build the self-hosted Kaze compiler (under 1 s with cc)
make kaze-build

# Build the interactive LSP stdio host driver
cc -O2 -std=c99 kaze/dist/kaze-host.c -o kaze/bin/kaze-host
```

The extension automatically detects `bin/kaze-host` (or `kaze/bin/kaze-host`) in
your open workspace folder, or uses `kaze-host` from `PATH`. To specify a
custom binary path:

```json
{ "kaze.lsp.path": "/absolute/path/to/kaze/bin/kaze-host" }
```

## 2. Install the extension

**Development host**

```bash
code --extensionDevelopmentPath=/absolute/path/to/kaze/editors/vscode
```

or open `editors/vscode` in VS Code and press `F5`.

**Normal install (VSIX)**

```bash
cd editors/vscode
npx @vscode/vsce package
code --install-extension kaze-1.0.0.vsix
```

Cursor uses the same VSIX through *Extensions → … → Install from VSIX*, or
`cursor --install-extension kaze-1.0.0.vsix`. A folder symlink under
`~/.vscode/extensions/sofuu-runtime.kaze-1.0.0` also works for development.

## 3. What you get

| Feature | How |
| :--- | :--- |
| Syntax highlighting | TextMate grammar (`syntaxes/kaze.tmLanguage.json`) including `record`, `fiber`, `parallel`, `regs`, and tensors |
| Diagnostics | Real-time compiler diagnostics (`bin/kaze check`) on open and change |
| Hover & Definition | In-document declarations (`action`, `routine`, `record`) via `kaze-host lsp` |
| Snippets | `action`, `check`, `filter`, `record`, `fiber`, `parallel`, `requires`, `when`, `every`, `try`, `use`, `needs` |

## 4. Settings

- `kaze.lsp.path` — path to `kaze-host` (or `kaze`). Empty = auto-detect in workspace (`bin/kaze-host`) or `PATH`.
- `kaze.lsp.args` — optional override arguments (defaults to `["lsp"]` for `kaze-host` / `kaze`).
- `kaze.lsp.trace` — log JSON-RPC traffic to the **Kaze** output channel.

Command **Kaze: Restart Language Server** reloads the server after a rebuild.

## 5. Tests

The framing and server-resolution tests run in plain Node with zero external packages:

```bash
node --check extension.js
node --check lsp.js
node test/lsp.test.js
```

End-to-end LSP tests against the real pure-C99 and self-hosted binaries run via:

```bash
env KAZE_BIN=bin/kaze python3 scripts/host-lsp-smoke.py bin/kaze-host
env KAZE_BIN=bin/kaze python3 scripts/host-hostile.py bin/kaze-host
python3 scripts/lsp-smoke.py
```
