# Kaze Modules — `use "path.kaze"` (v0.5)

`use` compiles another Kaze file into the same program. It is a **compile-time
include**, not a runtime import: by the time bytecode exists, the module has no
imports left and needs no source files beside it.

## 1. Syntax

`use` is a header line, alongside `needs:`, before the first item:

```kaze
use "lib/sensors.kaze"
use "lib/logging.kaze"

needs: drone, log

action main():
    arm drone
    takeoff to 10 meters
```

The path is a string literal ending in `.kaze`. It may be relative
(`lib/sensors.kaze`, `../shared/grid.kaze`) or absolute. It is resolved:

* relative to the **importing file** for `kaze run|check|compile <file>`;
* relative to the **current directory** for source text (`kaze run -`, and the
  Rust `prepare`/`run_source` API). For a source tree, use `prepare_file`,
  `run_file` or `compile_file`; the source-text APIs have no file to resolve
  against. Their root origin is the file name of the supplied `name`, so
  `compile_source(src, "examples/x.kaze")` and `compile_file("examples/x.kaze")`
  agree on the artifact bytes when there are no imports.

A `use` after the first item is a parse error with a hint to move it up. A
second `needs:` line is a parse error too.

## 2. Merge order

Imports are inlined depth-first, in use order, before the importing file's own
items:

```
root:  use a, use b, action main
a:     use shared, action a
b:     use shared, action b
shared:when ...
```

The flat program is: `shared`'s items, `a`'s items, `b`'s items, `root`'s items.
Hooks (`when`/`every`) therefore run in dependency-first order on every tick.

`needs:` lines are merged and deduplicated in the same order. A capability that
only a library declares is available to the root (and vice versa) for explicit
calls (`sensor.temperature()`, `drone.telemetry("battery")`), which is why the
root above may call `log` without declaring it itself.

**Bare telemetry is the exception: it is resolved per file at parse time.**
A bare `battery`, `altitude`, `temperature` or `bearing to` needs exactly one
hardware family in the file that writes it, because the family must be known
before the merge. If a library declares `needs: drone`, the root cannot write a
bare `battery` with only `needs: log`; write `drone.telemetry("battery")` or
declare `needs: drone` in the root too.

## 3. Inclusion, cycles and collisions

* **Once.** A file reached through two import paths (a diamond) is inlined
  once. The second path is skipped. Identity is the canonical path, so a
  symlink and its target count as one file; reading and nested-import
  resolution use the path as written, so a symlinked module's own `use` lines
  resolve next to the symlink, not the target.
* **Cycle.** If a file is already on the active import chain, the run stops
  with `E_KAZE_IMPORT: import cycle: a.kaze -> b.kaze -> a.kaze`. A file that
  imports itself is the same error. Canonical paths are compared, so a symlink
  or a `..` spelling cannot hide a cycle.
* **Missing file.** `E_KAZE_IO` names the use site:
  `root.kaze:2: cannot resolve 'lib/nope.kaze': ...`.
* **Non-module.** Only `.kaze` files are importable; `use "data.txt"` is a
  parse error.
* **Duplicate item.** Two files cannot define the same `action`/`check`/
  `filter` name: `E_KAZE_IMPORT: 'main' is defined in both lib.kaze and
  root.kaze`. Rename one. There are no namespaces in v0.5.
* **Hooks.** Synthesized hook names are renumbered across the whole module, so
  two libraries may both contain a `when` block without colliding.

## 4. Diagnostics

Every inlined item carries its file origin. A check, compile or run-time error
names the real file and line:

```
lib/sensors.kaze:0: 'missing_name' is not defined in 'report_temperature'
lib/sensors.kaze: 900 meters is outside the permitted range ... (line 3)
```

`kaze compile --disasm` prints the origin beside each item:

```
root: mission.kaze
...
action report_temperature (0 slots) [lib/sensors.kaze]
every every-1 (0 slots) [lib/sensors.kaze]
action main (0 slots) [mission.kaze]
```

Origins are **relative to the root module's directory**, so the artifact's
bytes do not depend on where the checkout lives. The root item's origin is its
file name; a dependency's origin is its canonical path relative to the root,
which also normalizes `..` and absolute spellings *under* the root. An import
outside the root is recorded as written (`../shared/x.kaze`); an absolute
import outside the root keeps its absolute path and is therefore inherently
machine-specific.

(Line 0 appears where the checker has only the item, not a statement: a known
limitation shared with single-file scripts.)

## 5. The compiled artifact

`kaze compile examples/modular/mission.kaze -o mission.kazebc` emits **one**
module containing every inlined item. The artifact:

* runs with `kaze run mission.kazebc` with no source files present;
* has no `use` table and performs no file I/O at run time;
* carries each item's origin, so runtime errors still name the library;
* is module format **4**; artifacts from v0.4 (format 3) are refused and must be
  recompiled.

## 6. Local checks

```bash
cargo test                                  # 267 default-feature tests
cargo run -- check examples/modular/mission.kaze
cargo run -- compile examples/modular/mission.kaze -o /tmp/mission.kazebc --disasm
cargo run -- run examples/modular/mission.kaze \
  --cap drone --cap sensor --cap log \
  --ticks 2 --tick-ms 100 --sim temperature=35 --plain
```

Expected: the root `main` logs, arms and takes off; the library's `every 100ms`
hook reports a hot sensor on each tick; the disassembly names both files; and
running the `.kazebc` after moving the sources away produces the same trace.

## 7. Deliberate limits (v0.5)

* No namespaces: item names are global across the module.
* No conditional imports, no import aliases, no selective imports.
* Imports are compile-time only; there is no dynamic load.
* There is still no user-routine call syntax. A library's `action` is reachable
  by `--entry` (or by a `when`/`every` hook), not from another routine, and it
  is **never the implicit entry**: the default (`main`, or the only routine) is
  resolved from the root file's items only.
* An imported file must contain at least one item; a header-only file is
  rejected as empty.
