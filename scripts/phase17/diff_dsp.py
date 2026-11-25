#!/usr/bin/env python3
"""diff_dsp.py -- differential test of lib/dsp.kaze against a NumPy Kalman reference.

The suite in tests/stdlib/dsp_test.kaze pins first-step and convergence values. This
script runs generated measurement sequences through a Kaze harness and compares
every intermediate state and covariance against a NumPy reference implementation of
the same linear-filter equations: constant-velocity predict, position-measurement
update, the generic linear measurement row, and the one-pole low-pass filter.

    python3 scripts/phase17/diff_dsp.py
    KAZE_ASAN=1 python3 scripts/phase17/diff_dsp.py

Exits 0 only when every case matches within 1e-9 relative tolerance. Generated
sources and binaries live in a temporary directory and are removed afterwards.
"""
import math
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
TOL = 1e-9


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


def list_lit(xs):
    return "[" + ", ".join(num_lit(x) for x in xs) + "]"


def rows_lit(rows):
    return "[" + ", ".join("[" + ", ".join(num_lit(v) for v in row) + "]" for row in rows) + "]"


# ---- reference filter ----------------------------------------------------------

def get(m, r, c):
    if r < 0 or c < 0:
        return 0.0
    if r >= len(m):
        return 0.0
    row = m[r]
    if c >= len(row):
        return 0.0
    return float(row[c])


def get_list(xs, i):
    if i < 0 or i >= len(xs):
        return 0.0
    return float(xs[i])


def lowpass(prev, current, alpha):
    a = alpha
    if a < 0:
        a = 0.0
    if a > 1:
        a = 1.0
    return prev + a * (current - prev)


def kalman1d(kf, z):
    x, p, q, r = kf
    p_pred = p + q
    s = p_pred + r
    if not s > 0:
        return (x, p, q, r)
    k = p_pred / s
    if k < 0:
        k = 0.0
    if k > 1:
        k = 1.0
    x = x + k * (z - x)
    p = (1 - k) * p_pred
    if p < 0:
        p = 0.0
    return (x, p, q, r)


def multi_predict(state, cov, q, dt):
    # Independent formulation: F P F^T + Q with F the constant-velocity matrix.
    f = np.array([[1.0, dt], [0.0, 1.0]])
    p = np.array([[get(cov, r, c) for c in range(2)] for r in range(2)])
    qm = np.array([[get(q, r, c) for c in range(2)] for r in range(2)])
    new_cov = f @ p @ f.T + qm
    new_state = f @ np.array([get_list(state, 0), get_list(state, 1)])
    return new_state.tolist(), new_cov.tolist()


def linear_update(state, cov, noise, h, z):
    # Independent formulation with NumPy: S = h P h^T + r, K = P h^T / S,
    # x <- x + K (z - h x), P <- P - K (h P).
    n = len(state)
    p = np.array([[get(cov, i, j) for j in range(n)] for i in range(n)])
    hv = np.array([get_list(h, i) for i in range(n)])
    s = float(hv @ p @ hv) + noise
    if not s > 0:
        return list(state), p.tolist()
    x = np.array(state, dtype=float)
    k = (p @ hv) / s
    new_state = x + k * (z - float(hv @ x))
    new_cov = p - np.outer(k, hv @ p)
    return new_state.tolist(), new_cov.tolist()


def flat(m):
    return [float(v) for row in m for v in row]


# ---- generated inputs ----------------------------------------------------------

def make_case_lowpass(rng):
    return {"kind": "lowpass",
            "prev": rng.uniform(-20, 20),
            "current": rng.uniform(-20, 20),
            "alpha": rng.choice([0.0, 0.25, 0.5, 1.0, rng.uniform(-1.0, 2.0)])}


def make_case_kalman1d(rng):
    return {"kind": "kalman1d",
            "kf": [rng.uniform(-5, 5), rng.uniform(0.1, 2.0),
                   rng.uniform(0.0001, 0.1), rng.uniform(0.01, 2.0)],
            "zs": [rng.uniform(-5, 5) for _ in range(6)]}


def make_case_multi(rng):
    a = rng.uniform(0.001, 0.1)
    c = rng.uniform(0.001, 0.1)
    b = rng.uniform(-0.01, 0.01)
    return {"kind": "multi",
            "pos": rng.uniform(-5, 5),
            "vel": rng.uniform(-5, 5),
            "q": [[a, b], [b, c]],
            "r": rng.uniform(0.01, 2.0),
            "dts": [rng.uniform(0.1, 2.0) for _ in range(5)],
            "zs": [rng.uniform(-5, 5) for _ in range(5)]}


def make_case_multi_update(rng):
    a = rng.uniform(0.1, 2.0)
    c = rng.uniform(0.1, 2.0)
    b = rng.uniform(-0.5, 0.5)
    return {"kind": "multi_update",
            "state": [rng.uniform(-5, 5), rng.uniform(-5, 5)],
            "cov": [[a, b], [b, c]],
            "r": rng.uniform(0.01, 2.0),
            "zs": [rng.uniform(-5, 5) for _ in range(4)]}


def make_case_linear(rng):
    p = [[rng.gauss(0.0, 1.0) for _ in range(3)] for _ in range(3)]
    cov = (np.array(p) @ np.array(p).T + np.eye(3) * 0.1).tolist()
    hs = []
    for k in range(4):
        if k == 3:
            hs.append([])
        elif k == 2:
            hs.append([round(rng.uniform(-1, 1), 4) for _ in range(2)])
        else:
            row = [rng.uniform(-1, 1) for _ in range(3)]
            hs.append(row)
    return {"kind": "linear",
            "state": [rng.uniform(-5, 5), rng.uniform(-5, 5), rng.uniform(-5, 5)],
            "cov": cov,
            "r": rng.uniform(0.01, 2.0),
            "hs": hs,
            "zs": [rng.uniform(-5, 5) for _ in range(4)]}


def generated_cases():
    rng = random.Random(SEED)
    cases = [
        # The guard cases: a variance that gives S <= 0 must leave the filter alone.
        {"kind": "kalman1d", "kf": [5.0, 0.0, 0.0, 0.0], "zs": [9.0, 9.0, 9.0, 9.0, 9.0, 9.0]},
        {"kind": "multi_update", "state": [1.0, 2.0], "cov": [[1.0, 0.0], [0.0, 1.0]],
         "r": -1.0, "zs": [7.0, 7.0, 7.0, 7.0]},
        {"kind": "linear", "state": [0.0, 0.0, 0.0], "cov": [[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]],
         "r": 0.0, "hs": [[1.0, 0.0, 0.0]] * 4, "zs": [1.0, 2.0, 3.0, 4.0]},
        # Alpha outside [0, 1] is clamped, not extrapolated.
        {"kind": "lowpass", "prev": 2.0, "current": 8.0, "alpha": -3.0},
        {"kind": "lowpass", "prev": 2.0, "current": 8.0, "alpha": 5.0},
        # A constant measurement with tiny model noise must converge exactly.
        {"kind": "kalman1d", "kf": [0.0, 1.0, 0.001, 0.5], "zs": [10.0, 10.0, 10.0, 10.0, 10.0, 10.0]},
    ]
    while len([c for c in cases if c["kind"] == "lowpass"]) < 24:
        cases.append(make_case_lowpass(rng))
    while len([c for c in cases if c["kind"] == "kalman1d"]) < 30:
        cases.append(make_case_kalman1d(rng))
    while len([c for c in cases if c["kind"] == "multi"]) < 24:
        cases.append(make_case_multi(rng))
    while len([c for c in cases if c["kind"] == "multi_update"]) < 10:
        cases.append(make_case_multi_update(rng))
    while len([c for c in cases if c["kind"] == "linear"]) < 14:
        cases.append(make_case_linear(rng))
    return cases


def expected(index, case):
    out = []
    if case["kind"] == "lowpass":
        out.append(lowpass(case["prev"], case["current"], case["alpha"]))
        return out
    if case["kind"] == "kalman1d":
        kf = tuple(case["kf"])
        for z in case["zs"]:
            kf = kalman1d(kf, z)
            out += [kf[0], kf[1]]
        return out
    if case["kind"] == "multi":
        state = [case["pos"], case["vel"]]
        cov = [[1.0, 0.0], [0.0, 1.0]]
        for dt, z in zip(case["dts"], case["zs"]):
            state, cov = multi_predict(state, cov, case["q"], dt)
            state, cov = linear_update(state, cov, case["r"], [1.0, 0.0], z)
            out += [state[0], state[1]] + flat(cov)
        return out
    if case["kind"] == "multi_update":
        state = list(case["state"])
        cov = [row[:] for row in case["cov"]]
        for z in case["zs"]:
            state, cov = linear_update(state, cov, case["r"], [1.0, 0.0], z)
            out += [state[0], state[1]] + flat(cov)
        return out
    if case["kind"] == "linear":
        state = list(case["state"])
        cov = [row[:] for row in case["cov"]]
        for h, z in zip(case["hs"], case["zs"]):
            state, cov = linear_update(state, cov, case["r"], h, z)
            out += state + flat(cov)
        return out
    raise AssertionError("unknown case kind " + case["kind"])


# ---- harness -------------------------------------------------------------------

DSP_LIB = os.path.join(ROOT, "lib", "dsp.kaze")


def build_program(cases):
    lines = [
        "needs: log",
        "",
        'use "%s"' % DSP_LIB,
        "",
        "action sf1(x):",
        '    return "" + x',
        "",
        "action main() -> num:",
    ]
    for i, case in enumerate(cases):
        prefix = '"D %d "' % i
        if case["kind"] == "lowpass":
            lines.append("    log.info(%s + sf1(lowpass_filter(%s, %s, %s)))"
                         % (prefix, num_lit(case["prev"]), num_lit(case["current"]),
                            num_lit(case["alpha"])))
        elif case["kind"] == "kalman1d":
            x, p, q, r = case["kf"]
            lines.append("    let kf%d = Kalman1D(x: %s, p: %s, q: %s, r: %s)"
                         % (i, num_lit(x), num_lit(p), num_lit(q), num_lit(r)))
            lines.append("    for each z%d in %s:" % (i, list_lit(case["zs"])))
            lines.append("        set kf%d to kalman1d_update(kf%d, z%d)" % (i, i, i))
            lines.append('        log.info(%s + sf1(kf%d.x) + " " + sf1(kf%d.p))' % (prefix, i, i))
        elif case["kind"] == "multi":
            lines.append("    let mk%d = kalman_multi_new(%s, %s, matrix_from_rows(%s), %s)"
                         % (i, num_lit(case["pos"]), num_lit(case["vel"]),
                            rows_lit(case["q"]), num_lit(case["r"])))
            lines.append("    let mi%d = 0" % i)
            lines.append("    for each dt%d in %s:" % (i, list_lit(case["dts"])))
            lines.append("        let mz%d = %s[mi%d] otherwise 0" % (i, list_lit(case["zs"]), i))
            lines.append("        set mk%d to kalman_multi_step(mk%d, dt%d, mz%d)" % (i, i, i, i))
            lines.append("        set mi%d to mi%d + 1" % (i, i))
            lines.append("        log.info(%s + sf1(mk%d.state_vector[0] otherwise 0) + \" \" + sf1(mk%d.state_vector[1] otherwise 0) + \" \" + sf1(matrix_get(mk%d.covariance_matrix, 0, 0)) + \" \" + sf1(matrix_get(mk%d.covariance_matrix, 0, 1)) + \" \" + sf1(matrix_get(mk%d.covariance_matrix, 1, 0)) + \" \" + sf1(matrix_get(mk%d.covariance_matrix, 1, 1)))" % (prefix, i, i, i, i, i, i))
        elif case["kind"] == "multi_update":
            lines.append("    let mu%d = KalmanMultiState(state_vector: %s, covariance_matrix: matrix_from_rows(%s), process_noise: matrix_from_rows(%s), measurement_noise: %s)"
                         % (i, list_lit(case["state"]), rows_lit(case["cov"]),
                            rows_lit([[0.01, 0.0], [0.0, 0.01]]), num_lit(case["r"])))
            lines.append("    for each uz%d in %s:" % (i, list_lit(case["zs"])))
            lines.append("        set mu%d to kalman_multi_update(mu%d, uz%d)" % (i, i, i))
            lines.append("        log.info(%s + sf1(mu%d.state_vector[0] otherwise 0) + \" \" + sf1(mu%d.state_vector[1] otherwise 0) + \" \" + sf1(matrix_get(mu%d.covariance_matrix, 0, 0)) + \" \" + sf1(matrix_get(mu%d.covariance_matrix, 0, 1)) + \" \" + sf1(matrix_get(mu%d.covariance_matrix, 1, 0)) + \" \" + sf1(matrix_get(mu%d.covariance_matrix, 1, 1)))" % (prefix, i, i, i, i, i, i))
        elif case["kind"] == "linear":
            lines.append("    let lc%d = KalmanMultiState(state_vector: %s, covariance_matrix: matrix_from_rows(%s), process_noise: matrix_from_rows(%s), measurement_noise: %s)"
                         % (i, list_lit(case["state"]), rows_lit(case["cov"]),
                            rows_lit([[0.01, 0.0, 0.0], [0.0, 0.01, 0.0], [0.0, 0.0, 0.01]]),
                            num_lit(case["r"])))
            lines.append("    let li%d = 0" % i)
            lines.append("    for each lh%d in %s:" % (i, rows_lit(case["hs"])))
            lines.append("        let lz%d = %s[li%d] otherwise 0" % (i, list_lit(case["zs"]), i))
            lines.append("        set lc%d to kalman_linear_update(lc%d, lh%d, lz%d)" % (i, i, i, i))
            lines.append("        set li%d to li%d + 1" % (i, i))
            lines.append("        log.info(%s + sf1(lc%d.state_vector[0] otherwise 0) + \" \" + sf1(lc%d.state_vector[1] otherwise 0) + \" \" + sf1(lc%d.state_vector[2] otherwise 0) + \" \" + sf1(matrix_get(lc%d.covariance_matrix, 0, 0)) + \" \" + sf1(matrix_get(lc%d.covariance_matrix, 0, 1)) + \" \" + sf1(matrix_get(lc%d.covariance_matrix, 0, 2)) + \" \" + sf1(matrix_get(lc%d.covariance_matrix, 1, 0)) + \" \" + sf1(matrix_get(lc%d.covariance_matrix, 1, 1)) + \" \" + sf1(matrix_get(lc%d.covariance_matrix, 1, 2)) + \" \" + sf1(matrix_get(lc%d.covariance_matrix, 2, 0)) + \" \" + sf1(matrix_get(lc%d.covariance_matrix, 2, 1)) + \" \" + sf1(matrix_get(lc%d.covariance_matrix, 2, 2)))" % ((prefix,) + (i,) * 12))
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
            print("diff_dsp: bundling failed\n" + bundle.stderr, file=sys.stderr)
            return None
        with open(flat, "w", encoding="utf-8") as fh:
            fh.write(bundle.stdout)
        emit = subprocess.run([KAZE, "emit-c", flat], capture_output=True, text=True)
        if emit.returncode != 0 or not emit.stdout:
            print("diff_dsp: emit-c failed\n" + emit.stderr[:2000], file=sys.stderr)
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
            print("diff_dsp: C build failed\n" + build.stderr[:2000], file=sys.stderr)
            return None
        run = subprocess.run([exe, "--ticks", "8"], capture_output=True, text=True, env=env)
        if run.returncode != 0:
            print("diff_dsp: harness exited %d\n%s" % (run.returncode,
                                                        (run.stdout + run.stderr)[-2000:]),
                  file=sys.stderr)
            return None
        return run.stdout


def parse_output(stdout):
    rows = {}
    for line in stdout.splitlines():
        if not line.startswith("[info] D "):
            continue
        parts = line[len("[info] D "):].split()
        index = int(parts[0])
        rows.setdefault(index, []).extend(float(tok) for tok in parts[1:])
    return rows


def main():
    cases = generated_cases()
    program = build_program(cases)
    stdout = run_harness(program)
    if stdout is None:
        return 2
    rows = parse_output(stdout)
    if len(rows) != len(cases):
        print("diff_dsp: expected %d result groups, got %d" % (len(cases), len(rows)),
              file=sys.stderr)
        return 2
    mismatches = 0
    reported = 0
    for i, case in enumerate(cases):
        want = expected(i, case)
        got = rows.get(i, [])
        if len(got) != len(want):
            mismatches += 1
            if reported < 10:
                print("FAIL case %d (%s): expected %d values, got %d"
                      % (i, case["kind"], len(want), len(got)))
                reported += 1
            continue
        for k, (g, w) in enumerate(zip(got, want)):
            if abs(g - w) > TOL * max(1.0, abs(w)):
                mismatches += 1
                if reported < 10:
                    print("FAIL case %d (%s) value %d: kaze %.17g numpy %.17g"
                          % (i, case["kind"], k, g, w))
                    reported += 1
                break
    print()
    print("diff dsp: %d cases, %d mismatches" % (len(cases), mismatches))
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
