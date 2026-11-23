# Division 02: Lexer & Dual Syntax (Human / Machine) (K02)

> **Maturity:** Core  
> **Parent Blueprint:** [kaze-map/README.md](README.md)  
> **Primary Locations:** `kaze/src/lexer.rs`, `kaze/src/parser.rs`, `kaze/src/machine.rs`, `kaze/compiler/lexer.kaze`, `kaze/compiler/parser.kaze`, `crates/sofuu-core/src/domains/kaze/lexer.rs`, `crates/sofuu-core/src/domains/kaze/parser.rs`, `crates/sofuu-core/src/domains/kaze/machine.rs`

---

## 1. Mission & Scope

Division 02 owns the lexical analysis, parsing, and dual-syntax architecture of Kaze. Kaze uniquely provides two primary source representations of identical computational power:
1. **Human Syntax**: Natural, highly readable sentence-like syntax using significant indentation, designed for clear mental models, rapid engineering, and self-documenting intent.
2. **Machine Syntax**: Unambiguous, parenthesis-delimited tagged prefix-tree S-expressions `(kaze (format 1) ...)`, optimized for automated code generation, AST transformations, and autonomous AI synthesis.

Division 02 guarantees that both syntax formats compile to the identical Abstract Syntax Tree (AST), execute under the identical resource bounds and capability models, and can be losslessly converted in either direction without human intervention.

---

## 2. Code Manifest & Owned Files

| Path | Purpose |
|---|---|
| `kaze/src/lexer.rs` | Indentation-aware lexical tokenizer, off-side rule indentation/dedentation generator, token spans |
| `kaze/src/parser.rs` | Recursive-descent AST parser for human syntax, operator precedence climbing, capability clause parsing |
| `kaze/src/machine.rs` | Parser and serializer for tagged prefix-tree machine format `(kaze (format 1) ...)`, round-trip verifier |
| `kaze/compiler/lexer.kaze` | Self-hosted tokenizer written in pure Kaze for Stage 1/2 bootstrap |
| `kaze/compiler/parser.kaze` | Self-hosted recursive-descent parser written in pure Kaze |
| `crates/sofuu-core/src/domains/kaze/lexer.rs` | Embedded tokenizer mirror in Sofuu core |
| `crates/sofuu-core/src/domains/kaze/parser.rs` | Embedded parser mirror in Sofuu core |
| `crates/sofuu-core/src/domains/kaze/machine.rs` | Embedded machine syntax serializer mirror |

---

## 3. Architectural Specifications & Invariants

1. **Transparent Auto-Detection**:
   - The parser inspects the source prefix: if it begins with `(kaze (format 1)`, it parses via the machine S-expression parser (`machine.rs`). Otherwise, it initializes the human tokenizer (`lexer.rs`).
   - CLI tools (`kaze check`, `kaze run`, `kaze emit-c`) require no special flags to distinguish between human and machine files.
2. **Lossless Bidirectional Transpilation**:
   - Transpilation satisfies the semantic idempotency invariant:
     $$\text{AST}(H) \equiv \text{AST}(\text{to\_human}(\text{to\_machine}(H)))$$
   - Comment preservation and source location anchors are maintained across conversions.
3. **Significant Indentation & Block Scoping**:
   - The human lexer maintains an internal indentation stack. Consistent 2-space or 4-space indentations emit virtual `Indent` and `Dedent` tokens. Mixed tabs and spaces are strictly rejected at parse time.
4. **Exact Span Tracking & Caret Diagnostics**:
   - Every token and AST node records an absolute byte range `(start..end)`, line number, and column index.
   - Syntax errors render multi-line visual caret pointers with surrounding context lines and masked credentials.

---

## 4. Implementation Details & Dual-Syntax Comparison

### Equivalent Human vs Machine Syntax Representation

```kaze
# Human-oriented natural syntax
routine calculate_velocity(distance: meter, duration: second) -> velocity:
  requires distance >= 0
  requires duration > 0
  
  let speed = distance / duration otherwise 0
  return speed
```

```lisp
;; Machine-oriented tagged prefix-tree syntax
(kaze (format 1)
  (routine calculate_velocity
    (params
      (param distance (type meter))
      (param duration (type second)))
    (returns velocity)
    (requires (gte distance 0))
    (requires (gt duration 0))
    (body
      (let speed (div distance duration (otherwise 0)))
      (return speed))))
```

---

## 5. Verification & Testing

```bash
# Verify machine syntax parsing and bidirectional parity
cargo test --manifest-path kaze/Cargo.toml --lib machine

# Verify human parser and indentation handling
cargo test --manifest-path kaze/Cargo.toml --lib parser

# Run round-trip transpilation test on example routines
cargo run --manifest-path kaze/Cargo.toml -- to-machine examples/simple.kaze > /tmp/machine.kaze
cargo run --manifest-path kaze/Cargo.toml -- to-human /tmp/machine.kaze > /tmp/human.kaze
cargo run --manifest-path kaze/Cargo.toml -- check /tmp/human.kaze
```

---

## 6. Active & Completed Tasks

- [x] Implement tagged prefix-tree S-expression parser and serializer (`kaze/src/machine.rs`).
- [x] Implement CLI commands `kaze to-machine` and `kaze to-human`.
- [x] Transparent parser auto-detection between human and machine formats.
- [x] Port lexer to pure Kaze (`compiler/lexer.kaze`).
- [x] Port parser to pure Kaze (`compiler/parser.kaze`).
- [ ] Add comment preservation during bidirectional transpilation.
- [ ] Add incremental re-tokenization support for sub-millisecond IDE editing.

## Roadmap phase 1-6 update (2026-11-22)

- Human syntax gained: inline `otherwise <fallback>` (a new loosest expression level), `while <cond> bounded <n>`, `guard <expr>`, `requires`/`ensures`/`recursion bounded`/routine-level `needs:` declarations at the top of a routine body, typed parameters with defaults and a result type (`-> type`), `routine` as a synonym for `action`, and `record` declarations with instantiation.
- The unit lexicon grew from five spellings to 22 across seven dimensions, including composite `m/s`, `km/h` and `m/s2`.
- Machine syntax carries all of it: `(otherwise a b)`, `(while cond (bounded n) (body ...))`, `(guard x)`, and canonical unit spellings in `(number "1" "km")`.
- The formatter keeps the new forms idempotent; `kaze/tests/fmt.rs` and the dual-syntax tests in `kaze/tests/compat.rs` cover the round trip.
- Verification: `cargo test --manifest-path kaze/Cargo.toml --test fmt --test compat --test modules`.
