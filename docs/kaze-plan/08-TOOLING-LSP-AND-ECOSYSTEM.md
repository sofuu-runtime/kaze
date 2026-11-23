# 08. Tooling, Dual Syntax, LSP & Developer Ecosystem

> **Status:** Architecture Specification  
> **Core Objective:** Provide a world-class developer experience with instant IDE feedback, deterministic formatting, bidirectional dual syntax, and zero-dependency module resolution.

---

## 1. Bidirectional Dual Syntax

Kaze natively supports two isomorphic source representations:

```mermaid
flowchart LR
    Human["Human Sentence Syntax<br/>(action, check, otherwise:, units)"] <-->|"Lossless Conversion<br/>(kaze to-machine / kaze to-human)"| Machine["Machine S-Expression Syntax<br/>((kaze (action ...)))"]
```

### 1. Human Syntax (Readability & Review):
Designed for human programmers and code reviews:
```kaze
needs: drone, log

action patrol(points):
    log "Starting perimeter patrol"
    arm drone
    takeoff to 15 meters
    for each pt in points:
        fly to pt at 15 meters
    return to launch
```

### 2. Machine Syntax (LLMs & Tree Manipulations):
Designed for AST-manipulation tools, formal solvers, and token-efficient AI generation:
```lisp
(kaze
  (format 1)
  (needs "drone" "log")
  (action "patrol" (params "points")
    (body
      (eval (call "log" "info" (string "Starting perimeter patrol")))
      (eval (call "drone" "arm"))
      (eval (call "drone" "takeoff" (num 15 "meters")))
      (for_each "pt" (name "points")
        (eval (call "drone" "fly" (name "pt") (num 15 "meters"))))
      (eval (call "drone" "rtl")))))
```

Both formats compile to the **exact same deterministic bytecode** and the **exact same native C99 code**.

---

## 2. Language Server Protocol (LSP)

The standalone Kaze toolchain includes a built-in Language Server speaking content-length framed JSON-RPC:
* **Real-Time Diagnostics:** Reports syntax errors, unit mismatches, and contract violations inline as you type in VS Code, Neovim, or JetBrains.
* **Hover Documentation:** Hovering over any capability verb (e.g. `takeoff to`, `pulse fan`) displays its required permissions, parameter units, and physical effects.
* **Intellisense & Completion:** Typing `drone.` lists all permitted hardware verbs; typing `use "./` autocompletes `.kaze` modules.
* **Formatting Provider:** Formats documents on save according to canonical Kaze style rules.

---

## 3. Canonical Formatter (`kaze fmt`)

Kaze eliminates formatting debates with a built-in, idempotent code formatter:
* 4-space indentation for blocks.
* Aligns multi-line lists cleanly.
* Canonical spacing around binary operators (`a + b`, not `a+b`).
* Preserves comments and associates them with their syntactic items.
* Available via CLI:
  ```bash
  kaze fmt script.kaze --write     # Formats file in-place
  kaze fmt script.kaze --check     # Exits with 1 if file is not formatted
  ```

---

## 4. Zero-Dependency Module Resolution (`use "..."`)

Kaze avoids the complexity and security risks of bloated package managers (`npm`, `pip`, `crates.io`).
* **Flat File Imports:** Dependencies are inlined depth-first in topological order:
  ```kaze
  use "./math/vector.kaze"
  use "./safety/guardrail.kaze"
  ```
* **Diamond Deduplication:** If file A and file B both import `lib.kaze`, `lib.kaze` is included exactly once.
* **Self-Contained Output:** Compiling a module flattens all imported routines into a single self-contained artifact. Zero external dynamic linking required.

---

## 5. Implementation Milestones

- [ ] **DX.DualSyntax:** Expand dual-syntax emitter to support contract syntax in S-expressions (`(between <min> <max>)`).
- [ ] **DX.LSPHover:** Add hover documentation for physical units and contract ranges in `lsp.rs`.
- [ ] **DX.Fmt:** Update formatter to format routine range contracts cleanly.
- [ ] **DX.TestRunner:** Add built-in `kaze test <dir>` command for executing unit assertions in `.kaze` files.
