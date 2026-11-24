#!/usr/bin/env bash
# failure_injection.sh -- motor cutout and GCS link loss must trigger the
# failsafe return-to-launch every time, and the same mission without an injected
# failure must not.
#
#   bash scripts/sitl/failure_injection.sh [seeds...]      default: 3 5 7 11
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

SEEDS="${*:-3 5 7 11}"
MISSION="$ROOT/scripts/selftest/phase15/failsafe_leg.kaze"

python3 - "$MISSION" "$SEEDS" <<'PY'
import os, pathlib, sys
sys.path.insert(0, os.path.join(os.getcwd(), "scripts", "sitl"))
import harness

mission = pathlib.Path(sys.argv[1])
seeds = [int(s) for s in sys.argv[2].split()] if len(sys.argv) > 2 else [3, 5, 7, 11]
binary, log = harness.prepare(mission)
if log:
    print("failure injection: FAIL -- build: " + log[:400], file=sys.stderr)
    sys.exit(1)

cases = [
    ("motor cutout", "--motor-cutout-at 25", "failsafe:motor-cutout"),
    ("GCS link loss", "--link-loss-at 25 --link-loss-for 6", "failsafe:link-loss"),
]
failures = 0
total = 0

def one(name, twin_args, expect_prefix, seed, inject):
    global failures, total
    total += 1
    args = twin_args if inject else ""
    run = harness.fly(mission, twin_args=args, seed=seed, speed=30, binary=binary)
    report = run.get("report") or {}
    landing = report.get("landing_error_m")
    if inject:
        # 0.0 m is a perfect touchdown, so "missing" must be tested explicitly.
        ok = (run.get("exit") == 70
              and any("E_SAFETY_VIOLATION" in line for line in run.get("stderr", []))
              and str(report.get("rtl_source", "")).startswith(expect_prefix)
              and landing is not None and landing <= 0.5
              and report.get("disarmed_at_s") is not None)
        detail = "exit=%s rtl=%s landing=%s" % (run.get("exit"), report.get("rtl_source"), landing)
    else:
        # The mission ends with its own "return to launch", so RTL firing is
        # expected; what must be absent is a *failsafe*.
        ok = (run.get("exit") == 0 and (report.get("failsafe_count") or 0) == 0
              and report.get("rtl_source") == "command")
        detail = "exit=%s rtl=%s failsafes=%s" % (run.get("exit"), report.get("rtl_source"),
                                                  report.get("failsafe_count"))
    print("  %s  %-14s seed %-3d %s" % ("PASS" if ok else "FAIL", name, seed, detail))
    if not ok:
        failures += 1

for name, args, prefix in cases:
    for seed in seeds:
        one(name, args, prefix, seed, inject=True)
one("control", "", "", seeds[0], inject=False)

if failures:
    print("failure injection: %d of %d failed" % (failures, total))
    sys.exit(1)
print("failure injection: %d of %d passed (RTL on every injected failure)" % (total, total))
PY
