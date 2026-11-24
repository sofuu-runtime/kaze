#!/usr/bin/env python3
"""phase16_check.py — Phase 16 certification driver.

For every module named on the command line:
  * run kaze-verify (json + proof + smt2) and time it
  * certify 100% of routines (no unproven non-fallback obligation, every loop
    bounded)
  * run the Z3 bridge on the emitted SMT-LIB2, require every required
    obligation unsat and no claimed proof contradicted
  * record the proof and SMT files in the output directory
Then report timing: the roadmap gate is under 50 ms of solver check per module.
"""
import glob
import json
import os
import subprocess
import sys
import time

def run(cmd):
    t0 = time.time()
    p = subprocess.run(cmd, capture_output=True, text=True)
    return p, (time.time() - t0) * 1000.0

def main():
    if len(sys.argv) < 4:
        raise SystemExit("usage: phase16_check.py <kaze-verify> <z3_bridge.py> <outdir> [modules...]")
    verifier, bridge, outdir = sys.argv[1], sys.argv[2], sys.argv[3]
    modules = sys.argv[4:]
    os.makedirs(outdir, exist_ok=True)
    if not modules:
        root = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
        # The committed standard library is the reproducible target. Modules that
        # concurrent Phase 14/15 work adds (drivers, BSPs) are validated when they
        # land but must not make this gate's result depend on an in-flight edit.
        p = subprocess.run(["git", "-C", root, "ls-files", "lib/*.kaze"],
                           capture_output=True, text=True)
        if p.returncode == 0 and p.stdout.strip():
            modules = [os.path.join(root, m) for m in p.stdout.split()]
        else:
            modules = sorted(glob.glob(os.path.join(root, "lib", "*.kaze")))
    total_routines = 0
    total_certified = 0
    total_obls = 0
    total_proven = 0
    fallbacks = 0
    fallbacks_proven = 0
    total_horn = 0
    verifier_max = 0.0
    z3_max = 0.0
    failures = []
    for path in modules:
        name = os.path.basename(path)
        p, vms = run([verifier, "--arg", "source=" + path, "--arg", "mode=json"])
        if p.returncode != 0 or not p.stdout.strip():
            failures.append("%s: verifier failed (%s)" % (name, p.stderr.strip()[:200]))
            continue
        try:
            d = json.loads(p.stdout)
        except Exception as exc:
            failures.append("%s: bad json (%s)" % (name, exc))
            continue
        verifier_max = max(verifier_max, vms)
        total_routines += d["routines"]
        total_certified += d["certified"]
        total_obls += d["obligations"]
        total_proven += d["proven"]
        fallbacks += d["fallbacks"]
        fallbacks_proven += d["fallbacks_proven"]
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
        horn_clauses = 0
        if ph.returncode == 0 and ph.stdout.startswith("; kaze-horn 1"):
            open(os.path.join(outdir, name + ".horn"), "w").write(ph.stdout)
            for raw in ph.stdout.splitlines():
                if raw.startswith("; clauses "):
                    horn_clauses = int(raw.split()[2])
        if d["obligations"] > 0 and horn_clauses == 0:
            failures.append("%s: Horn certificate has no clauses" % name)
        if horn_clauses < d["obligations"]:
            failures.append("%s: Horn clauses %d < obligations %d" % (name, horn_clauses, d["obligations"]))
        total_horn += horn_clauses
        ps, _ = run([verifier, "--arg", "source=" + path, "--arg", "mode=smt2"])
        smt_path = os.path.join(outdir, name + ".smt2")
        if ps.returncode != 0 or not ps.stdout.strip():
            failures.append("%s: smt2 emission failed" % name)
        else:
            open(smt_path, "w").write(ps.stdout)
            pb, _ = run([sys.executable, bridge, smt_path])
            if pb.returncode == 77:
                failures.append("%s: z3 unavailable" % name)
            elif pb.returncode != 0:
                failures.append("%s: z3 bridge failed (%s)" % (name, pb.stdout.strip().splitlines()[-1][:200]))
            else:
                for raw in pb.stdout.splitlines():
                    if raw.startswith("z3_bridge: ") and raw.strip().endswith("}"):
                        try:
                            s = json.loads(raw[len("z3_bridge: "):])
                            z3_max = max(z3_max, s["z3_ms"])
                        except Exception:
                            pass
                print("%-22s routines=%3d obls=%4d proven=%4d fallbacks_proven=%4d verifier=%6.2fms"
                      % (name, d["routines"], d["obligations"], d["proven"],
                         d["fallbacks_proven"], vms))
    print()
    print("phase16: routines=%d certified=%d obligations=%d proven=%d "
          "fallbacks=%d redundant=%d horn_clauses=%d" % (total_routines, total_certified,
          total_obls, total_proven, fallbacks, fallbacks_proven, total_horn))
    print("phase16: verifier max %.2f ms/module, z3 max %.2f ms/module (gate: solver < 50 ms)"
          % (verifier_max, z3_max))
    if z3_max > 50.0:
        failures.append("z3 max %.2f ms exceeds 50 ms/module" % z3_max)
    if total_certified != total_routines:
        failures.append("certified %d/%d routines" % (total_certified, total_routines))
    if failures:
        print("phase16: FAIL")
        for f in failures[:15]:
            print("    " + f)
        return 1
    print("phase16: PASS (%d/%d modules certified, every required proof unsat)"
          % (len(modules), len(modules)))
    return 0

if __name__ == "__main__":
    sys.exit(main())
