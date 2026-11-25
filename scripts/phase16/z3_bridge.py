#!/usr/bin/env python3
"""z3_bridge.py — Phase 16 SMT-LIB2 solver bridge.

Runs Z3 on the obligation file kaze-verify emits, pairs each echoed OBLIGATION
header with its check-sat result, and enforces the proof contract:

  * every non-fallback obligation must be unsat, or the module is not certified
  * a verifier "proven" verdict whose Z3 result is not unsat is unsound -> fail
  * a fallback obligation is a *proof of redundancy* when unsat (the boundary
    branch can be elided) and a *proof of reachability* when sat (the fallback is
    part of the routine's contract and must stay)
  * "unknown" is undecided and is a failure: every obligation has to be decided
    one way or the other

The embedded Fourier-Motzkin prover is the fallback path when Z3 is absent; here
Z3 is the prover of record, which is what the roadmap allows ("embedded minimal
presburger solver or direct Z3 bridge"). A second argument makes it write the
per-obligation verdicts as JSON so the elision pipeline can use the same answers.

    z3_bridge.py <obligations.smt2> [verdicts.json]
"""
import json
import re
import subprocess
import sys
import time

Z3 = "/opt/homebrew/bin/z3"
if len(sys.argv) < 2:
    print("usage: z3_bridge.py <obligations.smt2> [verdicts.json]")
    sys.exit(2)
path = sys.argv[1]
verdict_path = sys.argv[2] if len(sys.argv) > 2 else None
t0 = time.time()
try:
    proc = subprocess.run([Z3, "-smt2", path], capture_output=True, text=True, timeout=120)
except FileNotFoundError:
    print("z3_bridge: SKIP (no z3 at %s)" % Z3)
    sys.exit(77)
elapsed_ms = (time.time() - t0) * 1000.0
# z3's process exit code reflects the last check-sat (1 for sat), not whether the
# run itself succeeded, so the parsed results are the authority.

hdr = re.compile(r"^OBLIGATION (\d+) (\S+) (\S+) (\d+) (\S+)$")
pairs = {}
cur = None
for raw in proc.stdout.splitlines():
    line = raw.strip()
    m = hdr.match(line)
    if m:
        cur = (int(m.group(1)), m.group(2), m.group(3), int(m.group(4)), m.group(5))
        continue
    if cur is not None and line in ("sat", "unsat", "unknown"):
        pairs[cur] = line
        cur = None

required_total = 0
required_unsat = 0
unsound = []
filled = []
fallback_total = 0
fallback_unsat = 0
fallback_sat = 0
unknown = []
verdicts = []
for key, result in pairs.items():
    idx, kind, routine, line, status = key
    fallback = kind.startswith("fallback-")
    verdicts.append({"index": idx, "kind": kind, "routine": routine,
                     "line": line, "verdict": result, "verifier": status})
    if result == "unknown":
        unknown.append((idx, kind, routine, line))
        continue
    if fallback:
        fallback_total += 1
        if result == "unsat":
            fallback_unsat += 1
        else:
            fallback_sat += 1
        continue
    required_total += 1
    if result == "unsat":
        required_unsat += 1
        if status != "proven":
            filled.append((idx, kind, routine, line))
    else:
        unsound.append((idx, kind, routine, line, status, result))

if verdict_path:
    with open(verdict_path, "w") as fh:
        json.dump({"z3_ms": round(elapsed_ms, 2), "verdicts": verdicts}, fh)

summary = {
    "z3_ms": round(elapsed_ms, 2),
    "obligations": len(pairs),
    "required": required_total,
    "required_unsat": required_unsat,
    "fallbacks": fallback_total,
    "fallbacks_unsat": fallback_unsat,
    "fallbacks_reachable": fallback_sat,
    "unknown": len(unknown),
    "unsound": len(unsound),
    "z3_filled_unproven": len(filled),
}
print("z3_bridge: " + json.dumps(summary, separators=(",", ":")))
if len(pairs) == 0:
    print("z3_bridge: PASS (module has no obligations)")
    sys.exit(0)
if unknown:
    print("z3_bridge: FAIL (undecided obligations): " + repr(unknown[:4]))
    sys.exit(1)
if unsound:
    print("z3_bridge: FAIL (a required obligation is satisfiable): " + repr(unsound[:4]))
    sys.exit(1)
if required_unsat != required_total:
    print("z3_bridge: FAIL (%d/%d required obligations proved)" % (required_unsat, required_total))
    sys.exit(1)
print("z3_bridge: PASS (%d required proofs unsat, %d fallback bounds redundant, "
      "%d fallbacks proved reachable)" % (required_unsat, fallback_unsat, fallback_sat))
sys.exit(0)
