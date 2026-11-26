#!/usr/bin/env python3
"""diff_spatial.py -- differential test of lib/spatial.kaze against CPython math.

The suite in tests/stdlib/spatial_test.kaze states closed-form expectations. This
script adds a second opinion from an implementation nobody here wrote: it builds a
Kaze harness out of generated inputs, runs it through bin/kaze emit-c and the host
C compiler, and compares every produced number against a CPython reference
implementation of the same published formulae (Hamilton quaternions, ZYX Euler
angles, spherical interpolation).

Inputs are generated once per case and each case exercises one operation group,
cycling through vectors, quaternion algebra, Euler/interpolation, and transforms.
That keeps the generated translation unit small enough to compile quickly while
every operation is still compared on the same number of generated inputs.

    python3 scripts/phase17/diff_spatial.py
    KAZE_ASAN=1 python3 scripts/phase17/diff_spatial.py

Exits 0 only when every case matches within 1e-9 relative tolerance. Generated
sources and binaries live in a temporary directory and are removed afterwards.
"""
import math
import os
import random
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
KAZE = os.path.join(ROOT, "bin", "kaze")
DIST = os.path.join(ROOT, "dist")
RT = os.path.join(DIST, "kaze_rt.c")
SEED = 20261125
TOL = 1e-9
CASES = 100
GROUPS = 4


# ---- literals ------------------------------------------------------------------

def num_lit(x):
    """A Kaze number literal. Kaze has no exponent notation, so avoid repr 'e'."""
    x = float(x)
    s = repr(x)
    if "e" in s or "E" in s or "inf" in s or "nan" in s:
        s = format(x, ".17f")
        if "." in s:
            s = s.rstrip("0")
            if s.endswith("."):
                s += "0"
    return s


def vec_lit(v):
    return "Vec3(x: %s, y: %s, z: %s)" % tuple(num_lit(c) for c in v)


def quat_lit(q):
    return "Quat(w: %s, x: %s, y: %s, z: %s)" % tuple(num_lit(c) for c in q)


def euler_lit(e):
    return "Euler(roll: %s, pitch: %s, yaw: %s)" % tuple(num_lit(c) for c in e)


# ---- CPython reference ---------------------------------------------------------

def vadd(a, b):
    return [a[0] + b[0], a[1] + b[1], a[2] + b[2]]


def vsub(a, b):
    return [a[0] - b[0], a[1] - b[1], a[2] - b[2]]


def vscale(a, s):
    return [a[0] * s, a[1] * s, a[2] * s]


def vdot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def vcross(a, b):
    return [a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0]]


def vnorm(a):
    return math.sqrt(vdot(a, a))


def vnormalize(a):
    n = vnorm(a)
    if not n > 0:
        return [0.0, 0.0, 0.0]
    inv = 1.0 / n
    return vscale(a, inv)


def qdot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]


def qnormalize(q):
    n = math.sqrt(qdot(q, q))
    if not n > 0:
        return [1.0, 0.0, 0.0, 0.0]
    inv = 1.0 / n
    return [q[0] * inv, q[1] * inv, q[2] * inv, q[3] * inv]


def qmultiply(a, b):
    w1, x1, y1, z1 = a
    w2, x2, y2, z2 = b
    return [w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2,
            w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
            w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2,
            w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2]


def qconjugate(q):
    return [q[0], -q[1], -q[2], -q[3]]


def qfrom_axis_angle(axis, angle):
    u = vnormalize(axis)
    if u[0] == 0 and u[1] == 0 and u[2] == 0:
        return [1.0, 0.0, 0.0, 0.0]
    half = angle * 0.5
    s = math.sin(half)
    return [math.cos(half), u[0] * s, u[1] * s, u[2] * s]


def qfrom_euler(e):
    rx = qfrom_axis_angle([1, 0, 0], e[0])
    ry = qfrom_axis_angle([0, 1, 0], e[1])
    rz = qfrom_axis_angle([0, 0, 1], e[2])
    return qmultiply(rz, qmultiply(ry, rx))


def qto_euler(q):
    w, x, y, z = qnormalize(q)
    sin_roll = 2 * (w * x + y * z)
    cos_roll = 1 - 2 * (x * x + y * y)
    sin_pitch = 2 * (w * y - z * x)
    if sin_pitch > 1:
        sin_pitch = 1.0
    if sin_pitch < -1:
        sin_pitch = -1.0
    cos_pitch = math.sqrt(1 - sin_pitch * sin_pitch)
    sin_yaw = 2 * (w * z + x * y)
    cos_yaw = 1 - 2 * (y * y + z * z)
    return [math.atan2(sin_roll, cos_roll),
            math.atan2(sin_pitch, cos_pitch),
            math.atan2(sin_yaw, cos_yaw)]


def qrotate(q, v):
    # Independent formulation: q * (0, v) * conj(q), with q normalised first.
    u = qnormalize(q)
    rotated = qmultiply(qmultiply(u, [0.0, v[0], v[1], v[2]]), qconjugate(u))
    return [rotated[1], rotated[2], rotated[3]]


def qslerp(q1, q2, t):
    a = qnormalize(q1)
    b = qnormalize(q2)
    u = t
    if u < 0:
        u = 0.0
    if u > 1:
        u = 1.0
    d = qdot(a, b)
    c = b
    if d < 0:
        c = [-b[0], -b[1], -b[2], -b[3]]
        d = -d
    if d > 1:
        d = 1.0
    if d > 0.9995:
        mix = [a[i] * (1 - u) + c[i] * u for i in range(4)]
        return qnormalize(mix)
    theta = math.atan2(math.sqrt(1 - d * d), d)
    s = math.sin(theta)
    if not s > 0:
        return a
    inv_s = 1.0 / s
    wa = math.sin((1 - u) * theta) * inv_s
    wb = math.sin(u * theta) * inv_s
    return qnormalize([a[i] * wa + c[i] * wb for i in range(4)])


def transform_apply(translation, rotation, v):
    return vadd(qrotate(rotation, v), translation)


def transform_compose(ta_t, ta_q, tb_t, tb_q):
    translation = vadd(ta_t, qrotate(ta_q, tb_t))
    rotation = qmultiply(ta_q, tb_q)
    return translation, rotation


def expected_values(case):
    group = case["group"]
    a = case["a"]
    b = case["b"]
    qa = case["qa"]
    qb = case["qb"]
    e = case["e"]
    v = case["v"]
    out = []
    if group == 0:
        out += vadd(a, b)
        out += vsub(a, b)
        out += vscale(a, case["s"])
        out.append(vdot(a, b))
        out += vcross(a, b)
        out.append(vdot(a, a))
        out.append(vnorm(a))
        out += vnormalize(a)
        out.append(vnorm(vsub(a, b)))
    elif group == 1:
        out += qfrom_euler(e)
        out += qnormalize(qa)
        out += qmultiply(qa, qb)
        out += qconjugate(qa)
        out.append(qdot(qa, qb))
    elif group == 2:
        qe = qfrom_euler(e)
        out += qe
        out += qto_euler(qe)
        out += qslerp(qa, qb, case["t"])
        out += qfrom_axis_angle(case["axis"], case["angle"])
    elif group == 3:
        qe = qfrom_euler(e)
        out += qrotate(qa, v)
        out += transform_apply(case["ta_t"], qe, v)
        ct, cq = transform_compose(case["ta_t"], qe, case["tb_t"], case["tb_q"])
        out += transform_apply(ct, cq, v)
    else:
        raise AssertionError("unknown group")
    return out


# ---- generated inputs ----------------------------------------------------------

def rand_vec(rng):
    return [rng.uniform(-10.0, 10.0) for _ in range(3)]


def rand_quat(rng):
    q = [rng.uniform(-1.0, 1.0) for _ in range(4)]
    while qdot(q, q) < 1e-6:
        q = [rng.uniform(-1.0, 1.0) for _ in range(4)]
    return q


def rand_euler(rng):
    return [rng.uniform(-math.pi, math.pi),
            rng.uniform(-math.pi / 2, math.pi / 2),
            rng.uniform(-math.pi, math.pi)]


def make_case(rng, group, **overrides):
    case = {
        "group": group,
        "a": rand_vec(rng),
        "b": rand_vec(rng),
        "s": rng.uniform(-3.0, 3.0),
        "axis": rand_vec(rng),
        "angle": rng.uniform(-math.pi, math.pi),
        "qa": rand_quat(rng),
        "qb": rand_quat(rng),
        "e": rand_euler(rng),
        "v": rand_vec(rng),
        "t": rng.uniform(0.0, 1.0),
        "ta_t": rand_vec(rng),
        "tb_t": rand_vec(rng),
        "tb_q": rand_quat(rng),
    }
    case.update(overrides)
    return case


def generated_cases():
    rng = random.Random(SEED)
    cases = [
        # Degenerate directions: the module promises defined answers, not traps.
        make_case(rng, 0, a=[0.0, 0.0, 0.0], b=[0.0, 0.0, 0.0], s=0.0),
        make_case(rng, 0, a=[3.0, 4.0, 0.0], b=[1.0, 0.0, 0.0], s=0.0),
        make_case(rng, 0, a=[1e-6, -2e-6, 3e-6], b=[1e6, -2e6, 3e6]),
        # The same rotation by q and -q: slerp must take the short arc.
        make_case(rng, 2, qa=[math.sqrt(0.5), 0.0, 0.0, math.sqrt(0.5)],
                  qb=[-math.sqrt(0.5), 0.0, 0.0, -math.sqrt(0.5)], t=0.5),
        make_case(rng, 2, t=0.0),
        make_case(rng, 2, t=1.0),
        # Gimbal lock: pitch pinned at +/-90 degrees, where the asin domain clamps.
        make_case(rng, 1, e=[0.0, math.pi / 2, 0.0]),
        make_case(rng, 2, e=[0.0, math.pi / 2, 0.0]),
        make_case(rng, 2, e=[0.0, -math.pi / 2, 0.0]),
        # Axis-aligned quarter turns.
        make_case(rng, 3, e=[0.0, 0.0, math.pi / 2]),
        make_case(rng, 3, e=[math.pi / 2, 0.0, 0.0]),
        make_case(rng, 1, qa=[0.0, 0.0, 0.0, 0.0], qb=[0.0, 0.0, 0.0, 0.0]),
        make_case(rng, 2, axis=[0.0, 0.0, 0.0]),
    ]
    while len(cases) < CASES:
        cases.append(make_case(rng, len(cases) % GROUPS))
    return cases


# ---- harness -------------------------------------------------------------------

SPATIAL_LIB = os.path.join(ROOT, "lib", "spatial.kaze")


def build_program(cases):
    lines = [
        "needs: log",
        "",
        'use "%s"' % SPATIAL_LIB,
        "",
        "action sf3(v: Vec3):",
        '    return "" + v.x + " " + v.y + " " + v.z',
        "",
        "action sf4(q: Quat):",
        '    return "" + q.w + " " + q.x + " " + q.y + " " + q.z',
        "",
        "action sfe(e: Euler):",
        '    return "" + e.roll + " " + e.pitch + " " + e.yaw',
        "",
        "action sf1(x):",
        '    return "" + x',
        "",
        "action main() -> num:",
    ]
    for i, case in enumerate(cases):
        group = case["group"]
        lines.append("    let a%d = %s" % (i, vec_lit(case["a"])))
        lines.append("    let b%d = %s" % (i, vec_lit(case["b"])))
        if group == 0:
            parts = [
                "sf3(vec3_add(a%d, b%d))" % (i, i),
                "sf3(vec3_sub(a%d, b%d))" % (i, i),
                "sf3(vec3_scale(a%d, %s))" % (i, num_lit(case["s"])),
                "sf1(vec3_dot(a%d, b%d))" % (i, i),
                "sf3(vec3_cross(a%d, b%d))" % (i, i),
                "sf1(vec3_length_sq(a%d))" % i,
                "sf1(vec3_norm(a%d))" % i,
                "sf3(vec3_normalize(a%d))" % i,
                "sf1(vec3_distance(a%d, b%d))" % (i, i),
            ]
        elif group == 1:
            lines.append("    let eu%d = %s" % (i, euler_lit(case["e"])))
            lines.append("    let qa%d = %s" % (i, quat_lit(case["qa"])))
            lines.append("    let qb%d = %s" % (i, quat_lit(case["qb"])))
            lines.append("    let qe%d = quat_from_euler(eu%d)" % (i, i))
            parts = [
                "sf4(qe%d)" % i,
                "sf4(quat_normalize(qa%d))" % i,
                "sf4(quat_multiply(qa%d, qb%d))" % (i, i),
                "sf4(quat_conjugate(qa%d))" % i,
                "sf1(quat_dot(qa%d, qb%d))" % (i, i),
            ]
        elif group == 2:
            lines.append("    let eu%d = %s" % (i, euler_lit(case["e"])))
            lines.append("    let ax%d = %s" % (i, vec_lit(case["axis"])))
            lines.append("    let qa%d = %s" % (i, quat_lit(case["qa"])))
            lines.append("    let qb%d = %s" % (i, quat_lit(case["qb"])))
            lines.append("    let qe%d = quat_from_euler(eu%d)" % (i, i))
            lines.append("    let ee%d = quat_to_euler(qe%d)" % (i, i))
            parts = [
                "sf4(qe%d)" % i,
                "sfe(ee%d)" % i,
                "sf4(quat_slerp(qa%d, qb%d, %s))" % (i, i, num_lit(case["t"])),
                "sf4(quat_from_axis_angle(ax%d, %s))" % (i, num_lit(case["angle"])),
            ]
        else:
            lines.append("    let eu%d = %s" % (i, euler_lit(case["e"])))
            lines.append("    let qa%d = %s" % (i, quat_lit(case["qa"])))
            lines.append("    let v%d = %s" % (i, vec_lit(case["v"])))
            lines.append("    let qe%d = quat_from_euler(eu%d)" % (i, i))
            lines.append("    let ta%d = Transform3D(translation: %s, rotation: qe%d)"
                         % (i, vec_lit(case["ta_t"]), i))
            lines.append("    let tb%d = Transform3D(translation: %s, rotation: %s)"
                         % (i, vec_lit(case["tb_t"]), quat_lit(case["tb_q"])))
            parts = [
                "sf3(quat_rotate_vec3(qa%d, v%d))" % (i, i),
                "sf3(transform3d_apply(ta%d, v%d))" % (i, i),
                "sf3(transform3d_apply(transform3d_compose(ta%d, tb%d), v%d))" % (i, i, i),
            ]
        expr = ' + " " + '.join(parts)
        lines.append('    log.info("S %d " + %s)' % (i, expr))
    lines.append("    return 0")
    return "\n".join(lines) + "\n"


def run_harness(program):
    env = os.environ.copy()
    asan = os.environ.get("KAZE_ASAN") == "1"
    if asan and sys.platform == "darwin":
        env["ASAN_OPTIONS"] = "detect_leaks=0"
    with tempfile.TemporaryDirectory() as tmp:
        raw = os.path.join(tmp, "harness.kaze")
        cfile = os.path.join(tmp, "harness.c")
        exe = os.path.join(tmp, "harness")
        with open(raw, "w", encoding="utf-8") as fh:
            fh.write(program)
        emit = subprocess.run([KAZE, "emit-c", raw], capture_output=True, text=True)
        if emit.returncode != 0 or not emit.stdout:
            print("diff_spatial: emit-c failed\n" + emit.stderr[:2000], file=sys.stderr)
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
            print("diff_spatial: C build failed\n" + build.stderr[:2000], file=sys.stderr)
            return None
        run = subprocess.run([exe, "--ticks", "8"], capture_output=True, text=True, env=env)
        if run.returncode != 0:
            print("diff_spatial: harness exited %d\n%s" % (run.returncode,
                                                           (run.stdout + run.stderr)[-2000:]),
                  file=sys.stderr)
            return None
        return run.stdout


def parse_output(stdout):
    rows = {}
    for line in stdout.splitlines():
        if not line.startswith("[info] S "):
            continue
        parts = line[len("[info] S "):].split()
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
        print("diff_spatial: expected %d result lines, got %d" % (len(cases), len(rows)),
              file=sys.stderr)
        return 2
    mismatches = 0
    reported = 0
    for i, case in enumerate(cases):
        want = expected_values(case)
        got = rows.get(i)
        if got is None or len(got) != len(want):
            mismatches += 1
            if reported < 10:
                print("FAIL case %d (group %d): expected %d values, got %r"
                      % (i, case["group"], len(want), got))
                reported += 1
            continue
        for k, (g, w) in enumerate(zip(got, want)):
            if abs(g - w) > TOL * max(1.0, abs(w)):
                mismatches += 1
                if reported < 10:
                    print("FAIL case %d (group %d) value %d: kaze %.17g python %.17g"
                          % (i, case["group"], k, g, w))
                    reported += 1
                break
    print()
    print("diff spatial: %d cases, %d mismatches" % (len(cases), mismatches))
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
