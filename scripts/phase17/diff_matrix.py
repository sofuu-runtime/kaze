#!/usr/bin/env python3
"""diff_matrix.py -- differential test of lib/matrix.kaze against NumPy.

The suite in tests/stdlib/matrix_test.kaze states hand-computed expectations. This
script generates integer matrices, runs them through a Kaze harness built with
bin/kaze emit-c and the host C compiler, and compares every shape, entry,
determinant and inverse against NumPy: numpy.linalg.det for the determinant and
numpy.linalg.inv for the inverse, with the module's documented zero-padding rules
for shape mismatches.

Inputs are generated once per case and each case exercises one operation group,
cycling through construction/transpose, zeros/identity, add/scale, multiply,
element access, and determinant/inverse. That keeps the generated translation unit
small while every operation is compared on the same number of generated inputs.

    python3 scripts/phase17/diff_matrix.py
    KAZE_ASAN=1 python3 scripts/phase17/diff_matrix.py

Exits 0 only when every case matches. Generated sources and binaries live in a
temporary directory and are removed afterwards.
"""
import os
import random
import subprocess
import sys
import tempfile

import numpy as np

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
KAZE = os.path.join(ROOT, "bin", "kaze")
BUNDLE = os.path.join(ROOT, "scripts", "bundle-imports.py")
DIST = os.path.join(ROOT, "dist")
RT = os.path.join(DIST, "kaze_rt.c")
SEED = 20261125
GROUPS = 6
PER_GROUP = 20
CASES = GROUPS * PER_GROUP


def num_lit(x):
    x = float(x)
    s = repr(x)
    if "e" in s or "E" in s or "inf" in s or "nan" in s:
        s = format(x, ".17f")
        if "." in s:
            s = s.rstrip("0")
            if s.endswith("."):
                s += "0"
    return s


def rows_lit(rows):
    return "[" + ", ".join("[" + ", ".join(num_lit(v) for v in row) + "]" for row in rows) + "]"


# ---- CPython / NumPy reference -------------------------------------------------

def pad(rows):
    cols = max([len(r) for r in rows]) if rows else 0
    return [list(r) + [0.0] * (cols - len(r)) for r in rows]


def flatten(rows):
    out = []
    for row in rows:
        out.extend(row)
    return out


def dims(a):
    rows = len(a)
    return rows, (len(a[0]) if rows else 0)


def transpose(a):
    rows, cols = dims(a)
    return [[a[r][c] for r in range(rows)] for c in range(cols)]


def add_padded(a, b):
    rows, cols = dims(a)
    return [[a[r][c] + (b[r][c] if r < len(b) and c < len(b[r]) else 0.0)
             for c in range(cols)] for r in range(rows)]


def multiply_padded(a, b):
    rows, cols_a = dims(a)
    _, cols_b = dims(b)
    inner = min(cols_a, len(b))
    return [[sum(a[r][k] * (b[k][c] if c < len(b[k]) else 0.0) for k in range(inner))
             for c in range(cols_b)] for r in range(rows)]


def scale(a, s):
    return [[v * s for v in row] for row in a]


def det(a):
    rows, cols = dims(a)
    if rows != cols:
        return 0.0
    if rows == 0:
        return 1.0
    return float(np.linalg.det(np.array(a, dtype=float)))


def inverse(a):
    rows, cols = dims(a)
    if rows != cols or rows == 0:
        return None
    mat = np.array(a, dtype=float)
    if float(np.linalg.det(mat)) == 0.0:
        return None
    try:
        return np.linalg.inv(mat).tolist()
    except np.linalg.LinAlgError:
        return None


def mat_out(m):
    rows, cols = dims(m)
    return [float(rows), float(cols)] + flatten(m)


def expected_case(case):
    if case["kind"] == "ragged":
        a = pad(case["rows"])
    else:
        a = pad(case["a"])
        b = pad(case["b"])
    group = case["group"]
    rows, cols = dims(a)
    out = []
    if group == 0:
        out += mat_out(a)
        out += mat_out(transpose(a))
    elif group == 1:
        out += mat_out([[0.0] * cols for _ in range(rows)])
        out += mat_out([[1.0 if r == c else 0.0 for c in range(3)] for r in range(3)])
    elif group == 2:
        out += mat_out(add_padded(a, b))
        out += mat_out(scale(a, case["s"]))
    elif group == 3:
        out += mat_out(multiply_padded(a, b))
    elif group == 4:
        changed = [row[:] for row in a]
        if changed:
            changed[0][0] = 42.0
        out += mat_out(changed)
        last = a[rows - 1] if rows else []
        out += [float(len(last))] + last
    elif group == 5:
        out.append(det(a))
        inv = inverse(a)
        out.append(1.0 if inv is not None else 0.0)
        if inv is not None:
            out += mat_out(inv)
    else:
        raise AssertionError("unknown group")
    return out


# ---- generated inputs ----------------------------------------------------------

def rand_rows(rng, rows, cols):
    return [[rng.randint(-3, 3) for _ in range(cols)] for _ in range(rows)]


def rand_case(rng, group):
    if group == 5 and rng.random() < 0.25:
        # A singular square matrix by duplicate rows: det 0, inverse refused.
        n = rng.randint(2, 4)
        base_row = rand_rows(rng, 1, n)[0]
        a = [base_row[:], base_row[:]]
        while len(a) < n:
            a.append(rand_rows(rng, 1, n)[0])
        return {"kind": "matrix", "group": group, "a": a, "b": rand_rows(rng, n, n), "s": 1.0}
    if group == 5:
        n = rng.randint(1, 4)
        while True:
            a = rand_rows(rng, n, n)
            if det(a) != 0.0:
                break
        return {"kind": "matrix", "group": group, "a": a, "b": rand_rows(rng, n, n), "s": 1.0}
    a = rand_rows(rng, rng.randint(1, 4), rng.randint(1, 4))
    b = rand_rows(rng, rng.randint(1, 4), rng.randint(1, 4))
    return {"kind": "matrix", "group": group, "a": a, "b": b,
            "s": rng.choice([-2.5, -1.0, 0.0, 0.5, 1.0, 2.0, 3.0])}


def generated_cases():
    rng = random.Random(SEED)
    by_group = {g: [] for g in range(GROUPS)}

    # Singular matrices by construction: both engines must report det 0 / ok false.
    by_group[5].append({"kind": "matrix", "group": 5, "a": [[1, 2], [2, 4]],
                        "b": rand_rows(rng, 2, 2), "s": 2.0})
    by_group[5].append({"kind": "matrix", "group": 5, "a": [[0, 0], [0, 0]],
                        "b": rand_rows(rng, 2, 3), "s": 0.5})
    by_group[5].append({"kind": "matrix", "group": 5,
                        "a": [[1, 2, 3], [1, 2, 3], [4, 5, 6]],
                        "b": rand_rows(rng, 3, 2), "s": -1.0})
    # Rectangular matrices must be refused by matrix_inverse, and 0x0 is det 1.
    by_group[5].append({"kind": "matrix", "group": 5, "a": [[1, 2], [2, 4], [3, 6]],
                        "b": rand_rows(rng, 2, 2), "s": 1.0})
    by_group[5].append({"kind": "matrix", "group": 5, "a": [], "b": rand_rows(rng, 1, 1), "s": 1.0})

    # Ragged rows exercise the longest-row padding rule.
    by_group[0].append({"kind": "ragged", "group": 0, "rows": [[1, 2, 3], [4], [5, 6]]})
    by_group[0].append({"kind": "ragged", "group": 0, "rows": [[1], [2, 3], [4, 5, 6, 7]]})
    by_group[1].append({"kind": "ragged", "group": 1, "rows": []})
    by_group[1].append({"kind": "ragged", "group": 1, "rows": [[7]]})
    by_group[1].append({"kind": "ragged", "group": 1, "rows": [[1, 2], [3, 4]]})

    for group in range(GROUPS):
        while len(by_group[group]) < PER_GROUP:
            by_group[group].append(rand_case(rng, group))
    cases = []
    for group in range(GROUPS):
        cases.extend(by_group[group])
    return cases


# ---- harness -------------------------------------------------------------------

MATRIX_LIB = os.path.join(ROOT, "lib", "matrix.kaze")


def build_program(cases):
    lines = [
        "needs: log",
        "",
        'use "%s"' % MATRIX_LIB,
        "",
        "# Append a matrix as its shape followed by its row-major data.",
        "action mappend(xs, m: Matrix):",
        "    set xs to xs + [m.rows, m.cols]",
        "    let i = 0",
        "    while i < (length of m.data) bounded 4096:",
        "        set xs to xs + [m.data[i] otherwise 0]",
        "        set i to i + 1",
        "    return xs",
        "",
        "# Append a number list as its length followed by its entries.",
        "action lappend(xs, ys):",
        "    set xs to xs + [(length of ys)]",
        "    let i = 0",
        "    while i < (length of ys) bounded 4096:",
        "        set xs to xs + [ys[i] otherwise 0]",
        "        set i to i + 1",
        "    return xs",
        "",
        "action npush(xs, x):",
        "    set xs to xs + [x]",
        "    return xs",
        "",
        "action sdump(tag, xs):",
        "    let out = tag",
        "    for each v in xs:",
        '        set out to out + " " + v',
        "    log.info(out)",
        "",
        "action main() -> num:",
    ]
    for i, case in enumerate(cases):
        group = case["group"]
        if case["kind"] == "ragged":
            lines.append("    let a%d = matrix_from_rows(%s)" % (i, rows_lit(case["rows"])))
        else:
            lines.append("    let a%d = matrix_from_rows(%s)" % (i, rows_lit(case["a"])))
            if group in (2, 3):
                lines.append("    let b%d = matrix_from_rows(%s)" % (i, rows_lit(case["b"])))
        lines.append("    let r%d = []" % i)
        if group == 0:
            lines.append("    set r%d to mappend(r%d, a%d)" % (i, i, i))
            lines.append("    set r%d to mappend(r%d, matrix_transpose(a%d))" % (i, i, i))
        elif group == 1:
            lines.append("    set r%d to mappend(r%d, matrix_zeros(a%d.rows, a%d.cols))" % (i, i, i, i))
            lines.append("    set r%d to mappend(r%d, matrix_identity(3))" % (i, i))
        elif group == 2:
            lines.append("    set r%d to mappend(r%d, matrix_add(a%d, b%d))" % (i, i, i, i))
            lines.append("    set r%d to mappend(r%d, matrix_scale(a%d, %s))" % (i, i, i, num_lit(case["s"])))
        elif group == 3:
            lines.append("    set r%d to mappend(r%d, matrix_multiply(a%d, b%d))" % (i, i, i, i))
        elif group == 4:
            lines.append("    set r%d to mappend(r%d, matrix_set(a%d, 0, 0, 42))" % (i, i, i))
            lines.append("    set r%d to lappend(r%d, matrix_row(a%d, a%d.rows - 1))" % (i, i, i, i))
        else:
            lines.append("    set r%d to npush(r%d, matrix_det(a%d))" % (i, i, i))
            lines.append("    let iv%d = matrix_inverse(a%d)" % (i, i))
            lines.append("    if iv%d.ok:" % i)
            lines.append("        set r%d to npush(r%d, 1)" % (i, i))
            lines.append("        set r%d to mappend(r%d, iv%d.matrix)" % (i, i, i))
            lines.append("    otherwise:")
            lines.append("        set r%d to npush(r%d, 0)" % (i, i))
        lines.append('    sdump("M %d", r%d)' % (i, i))
    lines.append("    return 0")
    return "\n".join(lines) + "\n"


def run_harness(program):
    env = os.environ.copy()
    asan = os.environ.get("KAZE_ASAN") == "1"
    if asan and sys.platform == "darwin":
        env["ASAN_OPTIONS"] = "detect_leaks=0"
    with tempfile.TemporaryDirectory() as tmp:
        raw = os.path.join(tmp, "harness.kaze")
        flat = os.path.join(tmp, "harness_flat.kaze")
        cfile = os.path.join(tmp, "harness.c")
        exe = os.path.join(tmp, "harness")
        with open(raw, "w", encoding="utf-8") as fh:
            fh.write(program)
        bundle = subprocess.run([sys.executable, BUNDLE, raw],
                                capture_output=True, text=True)
        if bundle.returncode != 0:
            print("diff_matrix: bundling failed\n" + bundle.stderr, file=sys.stderr)
            return None
        with open(flat, "w", encoding="utf-8") as fh:
            fh.write(bundle.stdout)
        emit = subprocess.run([KAZE, "emit-c", flat], capture_output=True, text=True)
        if emit.returncode != 0 or not emit.stdout:
            print("diff_matrix: emit-c failed\n" + emit.stderr[:2000], file=sys.stderr)
            return None
        with open(cfile, "w", encoding="utf-8") as fh:
            fh.write(emit.stdout)
        cc = os.environ.get("CC", "cc")
        cmd = [cc, "-std=c99", "-O2", "-I", DIST]
        if asan:
            cmd += ["-g", "-fsanitize=address,undefined", "-fno-omit-frame-pointer"]
        cmd += [cfile, RT, "-lm", "-o", exe]
        build = subprocess.run(cmd, capture_output=True, text=True)
        if build.returncode != 0:
            print("diff_matrix: C build failed\n" + build.stderr[:2000], file=sys.stderr)
            return None
        run = subprocess.run([exe, "--ticks", "8"], capture_output=True, text=True, env=env)
        if run.returncode != 0:
            print("diff_matrix: harness exited %d\n%s" % (run.returncode,
                                                           (run.stdout + run.stderr)[-2000:]),
                  file=sys.stderr)
            return None
        return run.stdout


def parse_output(stdout):
    rows = {}
    for line in stdout.splitlines():
        if not line.startswith("[info] M "):
            continue
        parts = line[len("[info] M "):].split()
        index = int(parts[0])
        rows[index] = [float(tok) for tok in parts[1:]]
    return rows


def main():
    cases = generated_cases()
    program = build_program(cases)
    stdout = run_harness(program)
    if stdout is None:
        return 2
    rows = parse_output(stdout)
    if len(rows) != len(cases):
        print("diff_matrix: expected %d result lines, got %d" % (len(cases), len(rows)),
              file=sys.stderr)
        return 2
    mismatches = 0
    reported = 0
    for i, case in enumerate(cases):
        want = expected_case(case)
        got = rows.get(i, [])
        if len(got) != len(want):
            mismatches += 1
            if reported < 10:
                print("FAIL case %d (group %d): expected %d values, got %d"
                      % (i, case["group"], len(want), len(got)))
                reported += 1
            continue
        for k, (g, w) in enumerate(zip(got, want)):
            if abs(g - w) > 1e-7 * max(1.0, abs(w)):
                mismatches += 1
                if reported < 10:
                    print("FAIL case %d (group %d) value %d: kaze %.17g numpy %.17g"
                          % (i, case["group"], k, g, w))
                    reported += 1
                break
    print()
    print("diff matrix: %d cases, %d mismatches" % (len(cases), mismatches))
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
