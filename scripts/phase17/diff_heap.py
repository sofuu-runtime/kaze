#!/usr/bin/env python3
"""diff_heap.py -- differential check of lib/heap.kaze against CPython's heapq.

The module's own suite checks the heap against an in-Kaze model. This script adds
an implementation nobody here wrote: a harness compiled with `bin/kaze emit-c`
drives a min-heap and a max-heap through a fixed-seed LCG, and CPython's heapq
replays the identical LCG and operation mix.

The reference entry is (priority, seq) for the min-heap and (-priority, seq) for
the max-heap. That is exactly the order lib/heap.kaze documents: lower (min) or
higher (max) priority first, equal priorities in insertion order. So the two
implementations must agree entry for entry, not merely as multisets.

    python3 scripts/phase17/diff_heap.py
    KAZE_ASAN=1 python3 scripts/phase17/diff_heap.py

Exits 0 only when every popped and peeked entry matches; any mismatch exits 1.
Build/run failures exit 2. Generated sources and binaries live in a tempfile dir.
"""

from __future__ import annotations

import heapq
import os
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
# KAZE_BIN lets a build tree point the script at a specific compiler binary; the
# default is the one bin/ ships.
KAZE = pathlib.Path(os.environ.get("KAZE_BIN") or (ROOT / "bin" / "kaze"))
CC = os.environ.get("CC", "cc")
ASAN = os.environ.get("KAZE_ASAN") == "1"

OPS = 1500
PRIORITIES = [-2.5, 0.0, 0.25, 1.5, 3.0, 3.75, 1024.0]


def lcg(state: int) -> int:
    return (state * 1664525 + 1013904223) & 0xFFFFFFFF


def priority_of(seed: int) -> float:
    return PRIORITIES[(seed >> 16) % 7]


def num_literal(value: float) -> str:
    """Shortest round-trip decimal; Kaze number literals are digits and dots only."""
    text = repr(float(value))
    if "e" in text or "E" in text:
        raise SystemExit("diff heap: internal: %r does not fit the Kaze lexer" % value)
    return text


def harness_source() -> str:
    values = ", ".join(num_literal(p) for p in PRIORITIES)
    lib = (ROOT / "lib" / "heap.kaze").as_posix()
    return f"""needs: log

use "{lib}"

action lcg_next(state) -> num:
    let s = (state * 1664525 + 1013904223) & 4294967295
    return s

action priority_of(seed) -> number:
    let pick = (seed >> 16) % 7
    let values = [{values}]
    return values[pick] otherwise 0

action run_mode(is_min: bool) -> num:
    let h = heap_create_min()
    if is_min == false:
        set h to heap_create_max()
    let seed = 424242
    let seq = 0
    let i = 0
    while i < {OPS} bounded 100000:
        set seed to lcg_next(seed)
        let op = seed & 3
        if op < 2:
            set seed to lcg_next(seed)
            set h to heap_push(h, priority_of(seed), seq)
            set seq to seq + 1
        otherwise:
            let read = heap_pop(h)
            if read.ok:
                log.info("POP " + read.priority + " " + read.item)
                set h to read.heap
            otherwise:
                log.info("EMPTY")
        if i % 50 == 49 and heap_is_empty(h) == false:
            let e = heap_peek(h)
            log.info("PEEK " + e.priority + " " + e.item)
        set i to i + 1
    let live = heap_size(h)
    let d = 0
    while d < live bounded 100000:
        let read = heap_pop(h)
        set h to read.heap
        log.info("POP " + read.priority + " " + read.item)
        set d to d + 1
    log.info("END " + heap_size(h))
    return 0

action main() -> num:
    let min_run = run_mode(true)
    let max_run = run_mode(false)
    return min_run + max_run
"""


def reference(is_min: bool) -> list:
    """Replay the harness LCG with heapq and return the expected trace."""
    out: list = []
    entries: list = []
    seed = 424242
    seq = 0
    for i in range(OPS):
        seed = lcg(seed)
        op = seed & 3
        if op < 2:
            seed = lcg(seed)
            priority = priority_of(seed)
            heapq.heappush(entries, (priority if is_min else -priority, seq))
            seq += 1
        else:
            if entries:
                stored, item = heapq.heappop(entries)
                out.append(("POP", stored if is_min else -stored, item))
            else:
                out.append(("EMPTY",))
        if i % 50 == 49 and entries:
            stored, item = entries[0]
            out.append(("PEEK", stored if is_min else -stored, item))
    while entries:
        stored, item = heapq.heappop(entries)
        out.append(("POP", stored if is_min else -stored, item))
    out.append(("END", 0))
    return out


def parse_trace(text: str) -> list:
    out: list = []
    for line in text.splitlines():
        if not line.startswith("[info] "):
            continue
        parts = line[len("[info] "):].split()
        if not parts:
            continue
        if parts[0] == "POP":
            out.append(("POP", float(parts[1]), int(parts[2])))
        elif parts[0] == "PEEK":
            out.append(("PEEK", float(parts[1]), int(parts[2])))
        elif parts[0] == "EMPTY":
            out.append(("EMPTY",))
        elif parts[0] == "END":
            out.append(("END", int(parts[1])))
    return out


def build_and_run(tmp: pathlib.Path) -> str:
    src = tmp / "harness.kaze"
    src.write_text(harness_source(), encoding="utf-8")
    bundler = ROOT / "scripts" / "bundle-imports.py"
    flat = tmp / "flat.kaze"
    bundled = subprocess.run([sys.executable, str(bundler), str(src)], capture_output=True, text=True)
    if bundled.returncode != 0:
        raise SystemExit("diff heap: bundling failed\n" + bundled.stderr[-2000:])
    flat.write_text(bundled.stdout, encoding="utf-8")

    checked = subprocess.run([str(KAZE), "check", str(flat)], capture_output=True, text=True)
    if "E_KAZE" in checked.stdout + checked.stderr:
        raise SystemExit("diff heap: the harness does not check\n" + (checked.stdout + checked.stderr)[-2000:])

    emitted = subprocess.run([str(KAZE), "emit-c", str(flat)], capture_output=True, text=True)
    if emitted.returncode != 0:
        raise SystemExit("diff heap: emit-c failed\n" + emitted.stderr[-2000:])
    cfile = tmp / "harness.c"
    cfile.write_text(emitted.stdout, encoding="utf-8")

    binary = tmp / "harness"
    cc_cmd = [CC, "-std=c99", "-O2", "-I", str(ROOT / "dist")]
    if ASAN:
        cc_cmd += ["-g", "-fsanitize=address,undefined", "-fno-omit-frame-pointer"]
    cc_cmd += [str(cfile), str(ROOT / "dist" / "kaze_rt.c"), "-lm", "-o", str(binary)]
    compiled = subprocess.run(cc_cmd, capture_output=True, text=True)
    if compiled.returncode != 0:
        raise SystemExit("diff heap: cc failed\n" + compiled.stderr[-3000:])

    env = dict(os.environ)
    if ASAN:
        env["ASAN_OPTIONS"] = "detect_leaks=0" if sys.platform == "darwin" else "detect_leaks=1"
        # Any UBSan diagnostic is a failure in this mode: the harness is small and
        # the heap operations are the only code it runs.
        env["UBSAN_OPTIONS"] = "halt_on_error=1:print_stacktrace=1"
    run = subprocess.run([str(binary), "--ticks", "8"], capture_output=True, text=True, env=env, timeout=300)
    if run.returncode != 0:
        raise SystemExit("diff heap: the harness exited %d\n%s" % (run.returncode, run.stdout[-2000:] + run.stderr[-2000:]))
    if "E_KAZE_UNSUPPORTED" in run.stderr or "runtime error" in run.stderr:
        raise SystemExit("diff heap: the harness reported a runtime failure\n" + run.stderr[-2000:])
    return run.stdout


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="kaze-diff-heap-") as dirname:
        tmp = pathlib.Path(dirname)
        stdout = build_and_run(tmp)

    got = parse_trace(stdout)
    expected = reference(True) + reference(False)

    mismatches = 0
    for at in range(max(len(got), len(expected))):
        want = expected[at] if at < len(expected) else ("MISSING",)
        have = got[at] if at < len(got) else ("MISSING",)
        if want != have:
            mismatches += 1
            if mismatches <= 10:
                print("FAIL  entry %d: CPython %r, Kaze %r" % (at, want, have))
    cases = len(expected)
    if not cases:
        raise SystemExit("diff heap: internal: empty reference trace")
    print("diff heap: %d cases, %d mismatches" % (cases, mismatches))
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
