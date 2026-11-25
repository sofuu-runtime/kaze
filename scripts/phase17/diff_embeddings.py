#!/usr/bin/env python3
"""diff_embeddings.py -- differential check of lib/embeddings.kaze against NumPy.

The module's own suite pins a few hand-computed values and checks top-k against
an in-Kaze reference ranking. This script adds a reference nobody here wrote:
CPython with NumPy computes every dot product, cosine similarity and Euclidean
distance, ranks a generated candidate set itself, and the two answers are
compared.

Inputs are generated on a 1/64 grid, so every coordinate, product and sum is an
exact double; NumPy and the Kaze module compute the same dot product bit for bit
and the comparison is a real equality check rather than a loose one. Vectors are
deliberately unequal in length in several cases, which pins the documented
zero-padding convention, and the candidate sets include exact score ties, which
pin the (score descending, index ascending) order.

    python3 scripts/phase17/diff_embeddings.py
    KAZE_ASAN=1 python3 scripts/phase17/diff_embeddings.py

The Kaze side is a generated harness compiled with `bin/kaze emit-c` and
cc -std=c99 -O2 -I dist (plus -fsanitize=address,undefined under KAZE_ASAN=1)
and run natively. Exits 0 only when every value matches; otherwise 1. Build or
run failures exit 2. Generated files stay in a tempfile directory.
"""

from __future__ import annotations

import os
import pathlib
import random
import subprocess
import sys
import tempfile

import numpy as np

ROOT = pathlib.Path(__file__).resolve().parents[2]
# KAZE_BIN lets a build tree point the script at a specific compiler binary; the
# default is the one bin/ ships.
KAZE = pathlib.Path(os.environ.get("KAZE_BIN") or (ROOT / "bin" / "kaze"))
CC = os.environ.get("CC", "cc")
ASAN = os.environ.get("KAZE_ASAN") == "1"
SEED = 20261125


def num_literal(value: float) -> str:
    """Shortest round-trip decimal; Kaze number literals are digits and dots only."""
    text = repr(float(value))
    if "e" in text or "E" in text:
        raise SystemExit("diff embeddings: internal: %r does not fit the Kaze lexer" % value)
    return text


def kaze_vec(values) -> str:
    return "[" + ", ".join(num_literal(v) for v in values) + "]"


def padded(a, b):
    """Both vectors as float64 arrays zero-padded to the longer one."""
    width = max(len(a), len(b))
    pa = np.zeros(width)
    pb = np.zeros(width)
    pa[:len(a)] = np.asarray(a, dtype=float)
    pb[:len(b)] = np.asarray(b, dtype=float)
    return pa, pb


def cosine_ref(a, b) -> float:
    """Cosine similarity with the module's zero-padding and zero-norm conventions."""
    pa, pb = padded(a, b)
    dot = float(np.dot(pa, pb))
    na = float(np.sqrt(np.sum(pa * pa)))
    nb = float(np.sqrt(np.sum(pb * pb)))
    if na <= 0.0 or nb <= 0.0:
        return 0.0
    score = dot / (na * nb)
    if score > 1.0:
        return 1.0
    if score < -1.0:
        return -1.0
    return score


def pair_cases(rng: random.Random):
    """(a, b) pairs: fixed edges first, then generated unequal/equal dimensions."""
    pairs = [
        ([], []),
        ([1.0], []),
        ([], [2.0, 0.0 - 1.0]),
        ([0.0, 0.0], [1.0, 2.0]),
        ([1.0, 2.0, 3.0], [1.0, 2.0, 3.0]),
        ([1.0, 2.0], [1.0, 2.0, 0.0]),
        ([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]),
        ([0.5, 0.25], [0.0 - 2.0, 4.0]),
        ([1.0, 0.0], [0.0, 1.0]),
        ([1.0, 0.0], [0.0 - 1.0, 0.0]),
        ([3.0], [3.0, 3.0, 3.0, 3.0]),
        ([0.0 - 3.0, 1.5], [3.0, 0.0 - 1.5]),
    ]
    while len(pairs) < 72:
        dim_a = rng.randint(1, 12)
        dim_b = rng.randint(1, 12)
        a = [rng.randint(-192, 192) / 64.0 for _ in range(dim_a)]
        b = [rng.randint(-192, 192) / 64.0 for _ in range(dim_b)]
        pairs.append((a, b))
    return pairs


def topk_cases(rng: random.Random):
    """(query, candidates, k) scenarios: fixed edges first, then generated."""
    scenarios = [
        ([], [], 3),
        ([1.0, 0.0], [], 2),
        ([1.0, 0.0], [[1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [0.5, 0.0], [2.0, 0.0]], 3),
        ([1.0, 0.0], [[1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [0.5, 0.0], [2.0, 0.0]], 0),
        ([1.0, 0.0], [[1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [0.5, 0.0], [2.0, 0.0]], 0 - 1),
        ([0.0, 0.0], [[1.0, 0.0], [0.0, 1.0], [3.0, 4.0]], 2),
        ([1.0, 0.0], [[1.0, 0.0], [2.0, 0.0], [3.0, 0.0]], 2),
        ([1.0, 0.0], [[1.0, 0.0], [2.0, 0.0], [3.0, 0.0]], 99),
        ([1.0, 2.0, 3.0], [[1.0, 2.0, 3.0]], 99),
        ([1.0, 0.0], [[1.0, 0.0], [1.0, 0.0], [0.0, 1.0]], 2),
    ]
    while len(scenarios) < 34:
        dim = rng.randint(2, 10)
        count = rng.randint(1, 25)
        candidates = [[rng.randint(-96, 96) / 32.0 for _ in range(dim)] for _ in range(count)]
        query = [rng.randint(-96, 96) / 32.0 for _ in range(dim)]
        k = rng.choice([0, 1, 2, 3, 5, 8, 21, 50])
        # Ranking is compared index for index, so a pair of scores closer than a
        # nanounit but not exactly equal is ambiguous. The 1/32 grid makes exact
        # ties common and near-ties absent; this guard keeps that a checked
        # property instead of an assumption.
        scores = sorted(cosine_ref(np.array(query, dtype=float), np.array(c, dtype=float))
                        for c in candidates)
        if any(0.0 < b - a < 1e-9 for a, b in zip(scores, scores[1:])):
            continue
        scenarios.append((query, candidates, k))
    return scenarios


def harness_source(pairs, scenarios) -> str:
    lib = (ROOT / "lib" / "embeddings.kaze").as_posix()
    out = ["needs: log", "", 'use "%s"' % lib, "", "action main() -> num:"]
    for j, (a, b) in enumerate(pairs):
        out.append(f"    let pa{j} = {kaze_vec(a)}")
        out.append(f"    let pb{j} = {kaze_vec(b)}")
        out.append(f'    log.info("PAIR {j} " + vec_dot(pa{j}, pb{j})'
                   f' + " " + vec_cosine_similarity(pa{j}, pb{j})'
                   f' + " " + vec_euclidean_distance(pa{j}, pb{j}))')
    for j, (query, candidates, k) in enumerate(scenarios):
        cand_literal = "[" + ", ".join(kaze_vec(c) for c in candidates) + "]"
        out.append(f"    let tq{j} = {kaze_vec(query)}")
        out.append(f"    let tc{j} = {cand_literal}")
        out.append(f"    let tm{j} = vec_top_k(tq{j}, tc{j}, {max(0, k)})")
        out.append(f'    log.info("COUNT {j} " + (length of tm{j}))')
        out.append(f"    for each m{j} in tm{j}:")
        out.append(f'        log.info("MATCH {j} " + m{j}.index + " " + m{j}.score)')
    out.append("    return 0")
    return "\n".join(out) + "\n"


def parse_trace(text: str):
    pairs, counts, matches = [], {}, {}
    for line in text.splitlines():
        if not line.startswith("[info] "):
            continue
        parts = line[len("[info] "):].split()
        if not parts:
            continue
        if parts[0] == "PAIR":
            pairs.append((int(parts[1]), float(parts[2]), float(parts[3]), float(parts[4])))
        elif parts[0] == "COUNT":
            counts[int(parts[1])] = int(parts[2])
        elif parts[0] == "MATCH":
            matches.setdefault(int(parts[1]), []).append((int(parts[2]), float(parts[3])))
    return pairs, counts, matches


def build_and_run(pairs, scenarios, tmp: pathlib.Path) -> str:
    src = tmp / "harness.kaze"
    src.write_text(harness_source(pairs, scenarios), encoding="utf-8")
    bundler = ROOT / "scripts" / "bundle-imports.py"
    flat = tmp / "flat.kaze"
    bundled = subprocess.run([sys.executable, str(bundler), str(src)], capture_output=True, text=True)
    if bundled.returncode != 0:
        raise SystemExit("diff embeddings: bundling failed\n" + bundled.stderr[-2000:])
    flat.write_text(bundled.stdout, encoding="utf-8")

    checked = subprocess.run([str(KAZE), "check", str(flat)], capture_output=True, text=True)
    if "E_KAZE" in checked.stdout + checked.stderr:
        raise SystemExit("diff embeddings: the harness does not check\n" + (checked.stdout + checked.stderr)[-2000:])

    emitted = subprocess.run([str(KAZE), "emit-c", str(flat)], capture_output=True, text=True)
    if emitted.returncode != 0:
        raise SystemExit("diff embeddings: emit-c failed\n" + emitted.stderr[-2000:])
    cfile = tmp / "harness.c"
    cfile.write_text(emitted.stdout, encoding="utf-8")

    binary = tmp / "harness"
    cc_cmd = [CC, "-std=c99", "-O2", "-I", str(ROOT / "dist")]
    if ASAN:
        cc_cmd += ["-g", "-fsanitize=address,undefined", "-fno-omit-frame-pointer"]
    cc_cmd += [str(cfile), str(ROOT / "dist" / "kaze_rt.c"), "-lm", "-o", str(binary)]
    compiled = subprocess.run(cc_cmd, capture_output=True, text=True)
    if compiled.returncode != 0:
        raise SystemExit("diff embeddings: cc failed\n" + compiled.stderr[-3000:])

    env = dict(os.environ)
    if ASAN:
        env["ASAN_OPTIONS"] = "detect_leaks=0" if sys.platform == "darwin" else "detect_leaks=1"
        # Any UBSan diagnostic is a failure in this mode: the harness runs only the
        # module's arithmetic and top-k selection.
        env["UBSAN_OPTIONS"] = "halt_on_error=1:print_stacktrace=1"
    run = subprocess.run([str(binary), "--ticks", "8"], capture_output=True, text=True, env=env, timeout=300)
    if run.returncode != 0:
        raise SystemExit("diff embeddings: the harness exited %d\n%s" % (run.returncode, run.stdout[-2000:] + run.stderr[-2000:]))
    if "E_KAZE_UNSUPPORTED" in run.stderr or "runtime error" in run.stderr:
        raise SystemExit("diff embeddings: the harness reported a runtime failure\n" + run.stderr[-2000:])
    return run.stdout


def close(got: float, want: float) -> bool:
    return abs(got - want) <= 1e-9 * max(1.0, abs(want))


def main() -> int:
    rng = random.Random(SEED)
    pairs = pair_cases(rng)
    scenarios = topk_cases(rng)

    with tempfile.TemporaryDirectory(prefix="kaze-diff-embeddings-") as dirname:
        tmp = pathlib.Path(dirname)
        stdout = build_and_run(pairs, scenarios, tmp)

    got_pairs, got_counts, got_matches = parse_trace(stdout)
    mismatches = 0
    cases = 0
    if len(got_pairs) != len(pairs):
        print("FAIL  expected %d pair lines, got %d" % (len(pairs), len(got_pairs)))
        mismatches += 1
    for at, (a, b) in enumerate(pairs):
        if at >= len(got_pairs):
            break
        tag, dot, cos, dist = got_pairs[at]
        if tag != at:
            mismatches += 1
            cases += 1
            print("FAIL  pair %d: out of order (%d)" % (at, tag))
            continue
        pa, pb = padded(a, b)
        dot_ref = float(np.dot(pa, pb))
        cos_ref = cosine_ref(a, b)
        dist_ref = float(np.sqrt(np.sum((pa - pb) ** 2)))
        for name, want, have in (("dot", dot_ref, dot), ("cosine", cos_ref, cos), ("distance", dist_ref, dist)):
            cases += 1
            if not close(have, want):
                mismatches += 1
                if mismatches <= 10:
                    print("FAIL  pair %d %s: NumPy %r, Kaze %r" % (at, name, want, have))

    for at, (query, candidates, k) in enumerate(scenarios):
        q = np.array(query, dtype=float)
        scores = []
        for c in candidates:
            scores.append(cosine_ref(q, np.array(c, dtype=float)))
        order = sorted(range(len(candidates)), key=lambda i: (-scores[i], i))
        want = order[:max(0, min(k, len(candidates)))]
        cases += 1
        if got_counts.get(at, 0) != len(want):
            mismatches += 1
            if mismatches <= 10:
                print("FAIL  top-k %d: expected %d matches, got %r" % (at, len(want), got_counts.get(at)))
        have = got_matches.get(at, [])
        if len(have) != len(want):
            mismatches += 1
            if mismatches <= 10:
                print("FAIL  top-k %d: expected %d match lines, got %d" % (at, len(want), len(have)))
        for pos, index in enumerate(want):
            cases += 2
            if pos >= len(have):
                mismatches += 2
                continue
            have_index, have_score = have[pos]
            if have_index != index:
                mismatches += 1
                if mismatches <= 10:
                    print("FAIL  top-k %d position %d: NumPy index %d, Kaze %d" % (at, pos, index, have_index))
            if not close(have_score, scores[index]):
                mismatches += 1
                if mismatches <= 10:
                    print("FAIL  top-k %d position %d score: NumPy %r, Kaze %r" % (at, pos, scores[index], have_score))

    print("diff embeddings: %d cases, %d mismatches" % (cases, mismatches))
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
