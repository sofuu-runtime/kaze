#!/usr/bin/env python3
"""phase16_check.py — Phase 16 certification driver.

For every module: run the pure-Kaze verifier (certification, proof text, Horn
clauses, SMT-LIB2), then hand the obligations to Z3 through the bridge. The two
provers are reported separately and merged the only sound way:

  * a non-fallback obligation is proved when Z3 says unsat (the verifier's own
    "proven" must agree, or the bridge fails the module as unsound);
  * a fallback obligation is *redundant* when Z3 says unsat — the boundary branch
    can be elided — and *reachable* when Z3 says sat, which is a result too: the
    fallback is part of the routine's contract, not an assertion to remove;
  * anything Z3 leaves "unknown" is undecided, and undecided is a failure.

The latency gate is the roadmap's: the formal check for one module, verifier plus
solver, under 50 ms. The verifier is timed three times and the best run is used,
because this machine is shared and a single sample measures the other build as
much as this one; the worst sample is printed next to it.

    phase16_check.py <kaze-verify> <z3_bridge.py> <outdir> [modules...]
"""
import glob
import json
import os
import statistics
import subprocess
import sys
import time

def run(cmd):
    t0 = time.time()
    p = subprocess.run(cmd, capture_output=True, text=True)
    return p, (time.time() - t0) * 1000.0

def main():
    if len(sys.argv) < 4:
        print(__doc__)
        return 2
    verifier, bridge, outdir = sys.argv[1:4]
    modules = sys.argv[4:]
    if not modules:
        root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        modules = sorted(glob.glob(os.path.join(root, "lib", "*.kaze")))
    os.makedirs(outdir, exist_ok=True)

    tot = {"routines": 0, "certified": 0, "obligations": 0, "required": 0,
           "required_unsat": 0, "fallbacks": 0, "redundant": 0, "reachable": 0,
           "undecided": 0, "filled": 0, "horn": 0}
    check_best = 0.0
    check_worst = 0.0
    z3_worst = 0.0
    failures = []

    for path in modules:
        name = os.path.basename(path)
        # Certification + artifacts.
        p, vms = run([verifier, "--arg", "source=" + path, "--arg", "mode=json"])
        if p.returncode != 0 or not p.stdout.strip():
            failures.append("%s: verifier failed (%s)" % (name, p.stderr.strip()[:200]))
            continue
        try:
            d = json.loads(p.stdout)
        except Exception as exc:
            failures.append("%s: bad json (%s)" % (name, exc))
            continue
        # Latency: best of three, reporting the worst.
        times = [vms]
        for _ in range(2):
            _, t = run([verifier, "--arg", "source=" + path, "--arg", "mode=json"])
            times.append(t)
        v_ms = min(times)
        v_worst = max(times)
        tot["routines"] += d["routines"]
        tot["certified"] += d["certified"]
        tot["obligations"] += d["obligations"]
        tot["fallbacks"] += d["fallbacks"]
        if d["certified"] != d["routines"]:
            failures.append("%s: certified %d/%d" % (name, d["certified"], d["routines"]))
        if d["loops_bounded"] != d["loops"]:
            failures.append("%s: bounded loops %d/%d" % (name, d["loops_bounded"], d["loops"]))
        if d["unproven"] != 0:
            failures.append("%s: %d unproven non-fallback obligation(s)" % (name, d["unproven"]))
        pp, _ = run([verifier, "--arg", "source=" + path, "--arg", "mode=proof"])
        if pp.returncode != 0 or not pp.stdout.startswith("kaze-proof 1"):
            failures.append("%s: proof certificate failed" % name)
        else:
            open(os.path.join(outdir, name + ".proof"), "w").write(pp.stdout)
            if "digest 0\n" in pp.stdout or "digest nil" in pp.stdout:
                failures.append("%s: proof certificate has no source digest" % name)
        ph, _ = run([verifier, "--arg", "source=" + path, "--arg", "mode=horn"])
        horn = 0
        if ph.returncode == 0 and ph.stdout.startswith("; kaze-horn 1"):
            open(os.path.join(outdir, name + ".horn"), "w").write(ph.stdout)
            for raw in ph.stdout.splitlines():
                if raw.startswith("; clauses "):
                    horn = int(raw.split()[2])
        if d["obligations"] > 0 and horn == 0:
            failures.append("%s: Horn certificate has no clauses" % name)
        if horn < d["obligations"]:
            failures.append("%s: Horn clauses %d < obligations %d" % (name, horn, d["obligations"]))
        tot["horn"] += horn
        # Obligations -> SMT-LIB2 -> Z3.
        ps, _ = run([verifier, "--arg", "source=" + path, "--arg", "mode=smt2"])
        if ps.returncode != 0 or not ps.stdout.strip():
            failures.append("%s: smt2 emission failed" % name)
            continue
        smt = os.path.join(outdir, name + ".smt2")
        open(smt, "w").write(ps.stdout)
        verdicts = os.path.join(outdir, name + ".verdicts.json")
        pb, _ = run([sys.executable, bridge, smt, verdicts])
        if pb.returncode == 77:
            # No solver: the embedded prover is the authority instead.
            proven = d["proven"]
            redundant = d["fallbacks_proven"]
            tot["required_unsat"] += proven
            tot["redundant"] += redundant
            tot["reachable"] += d["fallbacks"] - redundant
            print("%-22s routines=%3d obls=%4d proved=%4d redundant=%4d (no z3) verifier=%6.2fms"
                  % (name, d["routines"], d["obligations"], proven, redundant, v_ms))
            continue
        if pb.returncode != 0:
            failures.append("%s: z3 bridge failed (%s)" % (name, pb.stdout.strip().splitlines()[-1][:200]))
            continue
        s = None
        for raw in pb.stdout.splitlines():
            if raw.startswith("z3_bridge: ") and raw.rstrip().endswith("}"):
                try:
                    s = json.loads(raw[len("z3_bridge: "):])
                except Exception:
                    s = None
        if s is None:
            failures.append("%s: z3 bridge produced no summary" % name)
            continue
        z3_ms = s["z3_ms"]
        z3_worst = max(z3_worst, z3_ms)
        check = v_ms + z3_ms
        check_best = max(check_best, check)
        check_worst = max(check_worst, v_worst + z3_ms)
        tot["required"] += s["required"]
        tot["required_unsat"] += s["required_unsat"]
        tot["redundant"] += s["fallbacks_unsat"]
        tot["reachable"] += s["fallbacks_reachable"]
        tot["undecided"] += s["unknown"]
        tot["filled"] += s["z3_filled_unproven"]
        if s["unknown"]:
            failures.append("%s: %d undecided obligation(s)" % (name, s["unknown"]))
        if s["unsound"]:
            failures.append("%s: %d required obligation(s) satisfiable" % (name, s["unsound"]))
        print("%-22s routines=%3d obls=%4d proved=%4d redundant=%4d reachable=%3d "
              "verifier=%6.2fms z3=%5.2fms check=%6.2fms"
              % (name, d["routines"], s["obligations"],
                 s["required_unsat"] + s["fallbacks_unsat"], s["fallbacks_unsat"],
                 s["fallbacks_reachable"], v_ms, z3_ms, check))

    decided = tot["required_unsat"] + tot["redundant"] + tot["reachable"]
    print()
    print("phase16: routines=%d certified=%d obligations=%d proved=%d "
          "(required %d/%d, fallbacks redundant %d) reachable=%d undecided=%d horn_clauses=%d"
          % (tot["routines"], tot["certified"], tot["obligations"],
             tot["required_unsat"] + tot["redundant"], tot["required_unsat"], tot["required"],
             tot["redundant"], tot["reachable"], tot["undecided"], tot["horn"]))
    print("phase16: formal check max %.2f ms/module (verifier+solver, worst sample %.2f ms; "
          "z3 max %.2f ms) -- gate: under 50 ms" % (check_best, check_worst, z3_worst))
    if tot["certified"] != tot["routines"]:
        failures.append("certified %d/%d routines" % (tot["certified"], tot["routines"]))
    if tot["required_unsat"] != tot["required"]:
        failures.append("required obligations proved %d/%d" % (tot["required_unsat"], tot["required"]))
    if tot["undecided"] != 0:
        failures.append("%d undecided obligation(s)" % tot["undecided"])
    if decided != tot["obligations"]:
        failures.append("decided %d of %d obligations" % (decided, tot["obligations"]))
    if check_best > 50.0:
        failures.append("formal check %.2f ms/module exceeds 50 ms" % check_best)
    if failures:
        for f in failures[:20]:
            print("FAIL " + f)
        return 1
    print("phase16: PASS (every obligation decided: %d proved, %d proved reachable; "
          "all required proofs unsat)" % (tot["required_unsat"] + tot["redundant"], tot["reachable"]))
    return 0

if __name__ == "__main__":
    sys.exit(main())
