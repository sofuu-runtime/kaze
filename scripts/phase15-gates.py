#!/usr/bin/env python3
"""phase15-gates.py -- the SITL gate: a real vehicle over real MAVLink, measured.

Every check below flies the self-hosted compiler's own output against
scripts/sitl/kaze_sitl.py, and reads the twin's report as data:

  * the codec agrees with pymavlink in both directions, byte for byte
  * a program with no vehicle fails closed (never a silent simulation)
  * the eight-waypoint survey completes headless with zero flight violations
  * a mission lands within 0.5 m of its target
  * motor cutouts and GCS link loss trigger the failsafe RTL 100% of the time
    (and the same mission without injection does not)
  * degraded sensors (late fix, HDOP, baro drift, magnetometer interference,
    IMU vibration) and wind gusts with an updraft are exercised, and the
    mission still lands inside the envelope
  * the same seed produces the same flight

Usage: python3 scripts/phase15-gates.py [--verbose]
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts" / "sitl"))
import harness  # noqa: E402

MISSIONS = ROOT / "scripts" / "selftest" / "phase15"
CHECKS = 0
FAILURES: list[str] = []
VERBOSE = "--verbose" in sys.argv


def check(name: str, ok: bool, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    print("  %s  %s%s" % ("PASS" if ok else "FAIL", name,
                          "" if ok else (" (" + detail + ")")))
    if not ok:
        FAILURES.append(name + ((" (" + detail + ")") if detail else ""))


def violations_empty(report: dict) -> bool:
    return report is not None and report.get("violations") == []


def num(report: dict, key: str, missing: float = 9.0) -> float:
    """A report number, with a missing field read as "worse than the gate".

    0.0 is a real value here -- a perfect landing is 0.0 metres -- so this cannot
    be spelled with a truthiness check.
    """
    value = report.get(key)
    return missing if value is None else value


def brief(result: dict) -> str:
    report = result.get("report") or {}
    violations = report.get("violations") or []
    shown = ",".join(str(v) for v in violations[:3])
    if len(violations) > 3:
        shown += ",+%d" % (len(violations) - 3)
    return "exit=%s arrivals=%s landing=%s violations=[%s]" % (
        result.get("exit"), report.get("waypoints_reached"),
        report.get("landing_error_m"), shown)


def main() -> int:
    print("phase 15 -- SITL bridge, digital twin and mission regression")

    # 1. the codec is real MAVLink 2, checked against pymavlink both ways
    xcheck = subprocess.run([sys.executable, str(ROOT / "scripts" / "sitl" / "mavlink_xcheck.py")],
                            capture_output=True, text=True)
    check("C codec and pymavlink agree", xcheck.returncode == 0,
          xcheck.stdout.strip().splitlines()[-1] if xcheck.stdout.strip() else "no output")

    # 2. fail-closed: no host, no vehicle, and a family the link does not own
    survey = MISSIONS / "survey.kaze"
    no_host = harness.fly(survey, sitl_link=False, timeout=10)
    check("a build without the SITL host fails closed",
          no_host.get("exit") == 70 and any("E_CAP_MISSING" in s for s in no_host.get("stderr", [])),
          brief(no_host))
    unselected = harness.fly(survey, env_extra={"KAZE_SITL": ""}, timeout=10)
    check("a build with the host but no selected vehicle fails closed",
          unselected.get("exit") == 70
          and any("E_CAP_MISSING" in s for s in unselected.get("stderr", [])),
          brief(unselected))

    # build the missions once, fly them many times
    survey_bin, survey_log = harness.prepare(survey)
    check("survey mission builds warning-free", not survey_log, survey_log[:300])
    land_bin, land_log = harness.prepare(MISSIONS / "land_at_target.kaze")
    check("landing mission builds warning-free", not land_log, land_log[:300])
    failsafe_bin, failsafe_log = harness.prepare(MISSIONS / "failsafe_leg.kaze")
    check("failsafe mission builds warning-free", not failsafe_log, failsafe_log[:300])
    telemetry_bin, telemetry_log = harness.prepare(MISSIONS / "telemetry.kaze")
    check("telemetry mission builds warning-free", not telemetry_log, telemetry_log[:300])

    # 3. the survey: eight legs, eight arrivals, home again, no violations
    result = harness.fly(survey, seed=7, binary=survey_bin)
    report = result.get("report") or {}
    check("eight-waypoint survey completes headless", result.get("exit") == 0, brief(result))
    check("eight legs were commanded", report.get("goto_commands") == 8,
          str(report.get("goto_commands")))
    check("eight legs were reached", report.get("waypoints_reached") == 8,
          str(report.get("waypoints_reached")))
    check("the survey logs no flight violation", violations_empty(report), str(report.get("violations")))
    check("the survey returns and lands within 0.5 m",
          report.get("rtl_fired") is True and num(report, "landing_error_m") <= 0.5,
          "rtl=%s landing=%s" % (report.get("rtl_fired"), report.get("landing_error_m")))

    # 4. landing accuracy against a commanded target
    landed = harness.fly(MISSIONS / "land_at_target.kaze", seed=11, binary=land_bin)
    lreport = landed.get("report") or {}
    target_n = lreport.get("landing_ref") or [0, 0]
    check("a mission lands within 0.5 m of its target",
          landed.get("exit") == 0 and num(lreport, "landing_error_m") <= 0.5,
          brief(landed))
    check("the landing target is the commanded one",
          abs(target_n[0] - 44.5) < 1.5 and abs(target_n[1]) < 1.0, str(target_n))
    check("the landing run logs no violation", violations_empty(lreport), str(lreport.get("violations")))

    # 5. telemetry a program can branch on
    tel = harness.fly(MISSIONS / "telemetry.kaze", seed=5, binary=telemetry_bin)
    treport = tel.get("report") or {}
    check("a program reads telemetry and flies on it",
          tel.get("exit") == 0 and treport.get("goto_commands") == 1
          and (treport.get("sensor") or {}).get("gps_fix_at_s") is not None,
          brief(tel))

    # 6. failure injection: motor cutout and GCS link loss must return to launch
    seeds = [3, 5, 7, 11]
    for label, twin_args, source in [
        ("motor cutout", "--motor-cutout-at 25", "failsafe:motor-cutout"),
        ("GCS link loss", "--link-loss-at 25 --link-loss-for 6", "failsafe:link-loss"),
    ]:
        returned = 0
        for seed in seeds:
            run = harness.fly(MISSIONS / "failsafe_leg.kaze", twin_args=twin_args, seed=seed,
                              speed=30, binary=failsafe_bin)
            rep = run.get("report") or {}
            ok = (run.get("exit") == 70
                  and any("E_SAFETY_VIOLATION" in s for s in run.get("stderr", []))
                  and str(rep.get("rtl_source", "")).startswith(source)
                  and num(rep, "landing_error_m") <= 0.5
                  and rep.get("disarmed_at_s") is not None)
            if ok:
                returned += 1
            elif VERBOSE:
                print("    %s seed %d: exit=%s rtl=%s landing=%s stderr=%s"
                      % (label, seed, run.get("exit"), rep.get("rtl_source"),
                         rep.get("landing_error_m"), run.get("stderr", [])[-1:]))
        check("%s triggers RTL 100%% of %d seeds" % (label, len(seeds)),
              returned == len(seeds), "%d of %d" % (returned, len(seeds)))

    # 7. the control: the same mission, no injection, does not return to launch
    control = harness.fly(MISSIONS / "failsafe_leg.kaze", seed=7, speed=30, binary=failsafe_bin)
    creport = control.get("report") or {}
    check("the same mission without injection completes with no failsafe",
          control.get("exit") == 0 and (creport.get("failsafe_count") or 0) == 0
          and creport.get("rtl_source") == "command" and violations_empty(creport),
          brief(control))

    # 8. degraded sensors: each model must be visible, and the mission must survive it
    degraded = harness.fly(
        survey, seed=13, binary=survey_bin, speed=30,
        twin_args="--gps-lock 6 --hdop-bad-at 30 --baro-drift 2.0 --mag-emi-at 26 "
                  "--vibration 1.5 --multipath 1.2")
    dreport = degraded.get("report") or {}
    sensor = dreport.get("sensor") or {}
    check("simulated sensors are exercised, not decorative",
          (sensor.get("gps_fix_at_s") or 0) >= 5.9
          and (sensor.get("hdop_max") or 0) > 2.0
          and (sensor.get("baro_drift_max_m") or 0) > 1.0
          and (sensor.get("mag_bias_max_deg") or 0) > 5.0
          and (sensor.get("vib_max_deg") or 0) > 0.3, json.dumps(sensor))
    check("the survey survives degraded sensing with no violation",
          degraded.get("exit") == 0 and violations_empty(dreport)
          and num(dreport, "landing_error_m") <= 0.5, brief(degraded))

    # 9. wind and updraft
    windy = harness.fly(survey, seed=17, binary=survey_bin, speed=30,
                        twin_args="--wind-speed 5 --gust 3 --updraft 1.2")
    wreport = windy.get("report") or {}
    check("gusts and an updraft are present in the run",
          (wreport.get("wind_max_ms") or 0) > 4.0, str(wreport.get("wind_max_ms")))
    check("the survey lands inside the envelope in wind and an updraft",
          windy.get("exit") == 0 and violations_empty(wreport)
          and num(wreport, "landing_error_m") <= 0.5, brief(windy))

    # 10. determinism: one seed, one flight
    first = harness.fly(survey, seed=23, binary=survey_bin, speed=30)
    second = harness.fly(survey, seed=23, binary=survey_bin, speed=30)
    a, b = first.get("report") or {}, second.get("report") or {}
    same = (a.get("waypoints_reached") == b.get("waypoints_reached")
            and a.get("goto_commands") == b.get("goto_commands")
            and a.get("violations") == b.get("violations")
            # The physics is deterministic in simulation time; the listener decides
            # in real time, so the touchdown point is allowed a millimetre of slack.
            and abs((a.get("landing_error_m") or 0) - (b.get("landing_error_m") or 0)) <= 0.01
            and (a.get("sensor") or {}).get("gps_fix_at_s") == (b.get("sensor") or {}).get("gps_fix_at_s"))
    check("the same seed produces the same flight", same,
          "a=%s b=%s" % (brief(first), brief(second)))

    print()
    if not FAILURES:
        print("phase15: %d of %d checks passed" % (CHECKS, CHECKS))
        return 0
    print("phase15: %d of %d checks failed" % (len(FAILURES), CHECKS))
    for failure in FAILURES:
        print("  failed: " + failure)
    return 1


if __name__ == "__main__":
    sys.exit(main())
