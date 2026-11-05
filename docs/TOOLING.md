# Kaze Developer Tooling (v0.6)

Three tools: the formatter (`kaze fmt`), the language server (`kaze-lsp`), and
the VS Code / Cursor extension. All three are zero-dependency and built from the
same front end the runtime uses.

## 1. `kaze fmt`

```bash
kaze fmt mission.kaze              # print the canonical form to stdout
kaze fmt --check mission.kaze      # exit 1 if it is not formatted
kaze fmt --write mission.kaze      # rewrite it in place
kaze fmt -                         # stdin -> stdout
```

The formatter is **conservative**:

- It parses first. A syntax error is reported with `file:line:col`; a script
  with semantic errors (undefined names) can still be formatted.
- It never merges or splits physical code lines, and it preserves every
  comment (whole-line and trailing).
- It normalizes indentation to four spaces per block level; continuation lines
  inside `(`/`[` get one extra level.
- It normalizes token spacing: `a=1+2` → `a = 1 + 2`, `f (a , b)` → `f(a, b)`,
  `p . x` → `p.x`, `20 %` → `20%`, `1m/s` → `1 m/s`, unary minus hugs its
  operand (`x = -5`).
- It preserves blank lines (trailing ones are trimmed) so physical line
  numbers do not shift. They are observable: `err.line` in a recovery block is
  the routine's declaration line, so collapsing a run would change behaviour.
- It trims trailing whitespace and normalizes CRLF line endings (a raw CR
  inside a string or comment is left alone).
- It refuses to write if the token stream of the result differs from the input.
  A formatter must never change what a script means.

The formatted output is idempotent: `fmt(fmt(x)) == fmt(x)`. Every example in
`examples/` is checked for idempotence and re-parsing in `tests/fmt.rs`.

Known limits: it does not reflow long lines or split `let x = 1 let y = 2`
(the parser accepts the latter, the formatter leaves it alone); numeric
spellings are canonicalized (`1.20` → `1.2`), while units are preserved as
written (`500ms` → `500 ms`, not converted to seconds).

## 2. `kaze-lsp`

`cargo build --release --bin kaze-lsp`, then run it over stdio. It is a
JSON-RPC 2.0 server with `Content-Length` framing and no network listener.

**Capabilities**

- `textDocumentSync`: open/close, full-document changes, save.
- `textDocument/publishDiagnostics`: parser errors with exact line/column,
  checker and import errors from `prepare_in`.
- `textDocument/formatting`: a single full-document edit from `kaze fmt`.
- `textDocument/hover`: capability signatures/arity, family operation lists,
  telemetry, keywords, and item declarations.
- `textDocument/completion`: keywords, capability families, telemetry names,
  item names; after `drone.` it lists that family's operations; after `use "`
  it lists `.kaze` files near the document.
- `textDocument/documentSymbol`: `action`/`check`/`filter` (Function) and
  `when`/`every` (Event).
- `shutdown`/`exit`.

Tests: `tests/lsp.rs` drives `lsp::Server` directly and spawns the real binary,
asserting framed responses over stdin/stdout.

**Honest limits:** character positions are not UTF-16-normalized; imported-file
diagnostics are attached to the open document with a file-name prefix; the
server is single-threaded and re-checks the whole document on every change.

## 3. VS Code / Cursor extension

`editors/vscode/` contains:

- `package.json` — language, grammar, snippets, commands, settings.
- `language-configuration.json` — comments (`#`), brackets, indentation.
- `syntaxes/kaze.tmLanguage.json` — TextMate grammar.
- `snippets/kaze.json` — action/check/filter/when/every/try/use/needs.
- `lsp.js` — Content-Length framing (plain Node, unit-tested).
- `extension.js` — a minimal JSON-RPC client: diagnostics, formatting, hover,
  completion, symbols, restart command. No npm dependencies.
- `README.md` — build/install instructions for VS Code and Cursor.

Checks:

```bash
node --check editors/vscode/extension.js
node --check editors/vscode/lsp.js
node editors/vscode/test/lsp.test.js
python3 -m json.tool editors/vscode/package.json >/dev/null
```

The extension launches `kaze.lsp.path` (or `kaze-lsp` from `PATH`). If the
server is missing it shows a warning naming both fixes instead of failing
silently.
