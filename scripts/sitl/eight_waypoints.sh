#!/usr/bin/env bash
# eight_waypoints.sh -- the Phase 15 acceptance gate: fly the eight-waypoint
# survey headless and require zero flight violations.
#
# The vehicle is whichever one KAZE_SITL_VEHICLE names:
#
#   twin        (default) scripts/sitl/kaze_sitl.py, the in-repo digital twin
#   ardupilot   a running ArduCopter/PX4 SITL on udp:127.0.0.1:$KAZE_SITL_PORT
#
# Both speak MAVLink 2 to the same client, so the mission binary is identical.
# The twin path is exercised by CI here; the ArduPilot path needs an ArduPilot
# checkout on the machine and is reported, not assumed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

MISSION="${KAZE_P15_MISSION:-$ROOT/scripts/selftest/phase15/survey.kaze}"
VEHICLE="${KAZE_SITL_VEHICLE:-twin}"
SEED="${KAZE_SITL_SEED:-7}"

if [ ! -x bin/kaze ]; then
    echo "sitl: bin/kaze is missing; run 'make kaze-build' first" >&2
    exit 2
fi

if [ "$VEHICLE" = "ardupilot" ]; then
    PORT="${KAZE_SITL_PORT:-14550}"
    echo "sitl: vehicle=ardupilot udp:127.0.0.1:$PORT (external process; not started here)"
    KAZE_HARNESS_PORT="$PORT" python3 - "$MISSION" "$PORT" <<'PY'
import os, pathlib, subprocess, sys
sys.path.insert(0, os.path.join(os.getcwd(), "scripts", "sitl"))
import harness
mission = pathlib.Path(sys.argv[1]); port = int(sys.argv[2])
binary, log = harness.prepare(mission)
if log:
    print("sitl: FAIL -- " + log[:400], file=sys.stderr); sys.exit(1)
env = dict(os.environ, KAZE_SITL="udp:127.0.0.1:%d" % port, KAZE_SITL_TIMEOUT_MS="60000")
result = subprocess.run([str(binary)], capture_output=True, env=env)
print("sitl: mission exit %d" % result.returncode)
print(result.stderr.decode("utf-8", "replace").strip())
sys.exit(0 if result.returncode == 0 else 1)
PY
    exit $?
fi

python3 - "$MISSION" "$SEED" <<'PY'
import json, os, pathlib, sys
sys.path.insert(0, os.path.join(os.getcwd(), "scripts", "sitl"))
import harness

mission = pathlib.Path(sys.argv[1])
seed = int(sys.argv[2])
result = harness.fly(mission, seed=seed, speed=float(os.environ.get("KAZE_SITL_SPEED", "20")))
report = result.get("report") or {}
if not result.get("build_ok"):
    print("sitl: FAIL -- the mission did not build: " + (result.get("build_log") or "")[:400],
          file=sys.stderr)
    sys.exit(1)
problems = []
if result.get("exit") != 0:
    problems.append("mission exit %s, stderr %s" % (result.get("exit"), result.get("stderr")))
if report.get("goto_commands") != 8:
    problems.append("expected 8 commanded legs, saw %s" % report.get("goto_commands"))
if report.get("waypoints_reached") != 8:
    problems.append("expected 8 arrivals, saw %s" % report.get("waypoints_reached"))
if report.get("violations"):
    problems.append("flight violations: %s" % report.get("violations")[:4])
if not report.get("rtl_fired"):
    problems.append("the mission never returned to launch")
landing = report.get("landing_error_m")
# 0.0 is a real touchdown error; only a missing field counts as "no landing".
if landing is None:
    problems.append("the twin reported no touchdown point")
elif landing > 0.5:
    problems.append("landing error %.3f m exceeds 0.5 m" % landing)
print("sitl: vehicle=twin seed=%d legs=%s arrivals=%s landing=%.3f m violations=%d" % (
    seed, report.get("goto_commands"), report.get("waypoints_reached"),
    report.get("landing_error_m") or 0.0, len(report.get("violations") or [])))
if problems:
    print("sitl: FAIL -- " + "; ".join(problems), file=sys.stderr)
    sys.exit(1)
print("sitl: PASS -- eight-waypoint mission completed headless with zero flight violations")
PY
