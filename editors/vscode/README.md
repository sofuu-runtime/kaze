# Kaze for VS Code / Cursor

Syntax highlighting, diagnostics, formatting, hover, completion and document
symbols for `.kaze` files, powered by `kaze-lsp`.

The extension is **dependency-free**: it does not use `vscode-languageclient`
and needs no `npm install`. It speaks the language server's JSON-RPC directly.

## 1. Build the language server

```bash
cd kaze
cargo build --release --bin kaze-lsp
```

Put it on `PATH`, or point the extension at it with the `kaze.lsp.path` setting:

```json
{ "kaze.lsp.path": "/absolute/path/to/kaze/target/release/kaze-lsp" }
```

## 2. Install the extension

**Development host**

```bash
code --extensionDevelopmentPath=/absolute/path/to/kaze/editors/vscode
```

or open `editors/vscode` in VS Code and press `F5`.

**A normal install**

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
| Syntax highlighting | TextMate grammar (`syntaxes/kaze.tmLanguage.json`) |
| Diagnostics | Parser, checker and import errors on open/change/save |
| Format on demand | `Format Document` / `Format Selection` → `kaze fmt` via the server |
| Hover | Capability signatures, telemetry, keywords, item declarations |
| Completion | Keywords, families, telemetry, item names, `drone.` operations, `use "…"` modules |
| Document symbols | `action`/`check`/`filter` and `when`/`every` hooks |
| Snippets | action, check, filter, when, every, try, use, needs |

## 4. Settings

- `kaze.lsp.path` — path to `kaze-lsp` (empty = from `PATH`).
- `kaze.lsp.trace` — log JSON-RPC traffic to the **Kaze** output channel.

Command **Kaze: Restart Language Server** reloads the server after a rebuild.

## 5. Tests

The framing helper is plain Node and has no VS Code dependency:

```bash
node --check extension.js
node --check lsp.js
node test/lsp.test.js
```

The extension itself is exercised by the language server's Rust tests
(`tests/lsp.rs`), including the real `kaze-lsp` binary over stdio.

## 6. Honest limits

- Position mapping uses character counts (ASCII-safe; a line with non-ASCII text
  before the cursor can shift by a few columns).
- Diagnostics for errors inside an imported module are attached to the open
  document and prefixed with the imported file name.
- Sync is full-document: every change resends the buffer.
- The server is single-threaded; it is fast on scripts, not designed for huge
  workspaces.
