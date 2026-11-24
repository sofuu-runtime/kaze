#!/usr/bin/env python3
"""harness.py -- build a Kaze mission, fly it against the twin, return the report.

The harness owns the three pieces the gate needs to be honest about:

  * the build: emit C with the self-hosted compiler, then compile it with
    -DKAZE_SITL_LINK so the drone capability has a host;
  * the vehicle: scripts/sitl/kaze_sitl.py on a free UDP port, with the scenario
    flags the caller asked for, and its report read back as data;
  * the flight: the mission binary with KAZE_SITL set, its exit status and
    stderr kept.

Usage (also importable):

  python3 scripts/sitl/harness.py --mission scripts/selftest/phase15/survey.kaze
      [--twin "--wind-speed 4 --gust 2.5"] [--speed 20] [--seed 7]
      [--timeout 60] [--json]

prints one JSON object: mission exit, stderr, report, build_ok, seconds.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import shlex
import socket
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
KAZE = ROOT / "bin" / "kaze"
DIST = ROOT / "dist"
TWIN = ROOT / "scripts" / "sitl" / "kaze_sitl.py"


def free_port() -> int:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def build_mission(source: pathlib.Path, out_dir: pathlib.Path, sitl_link: bool) -> tuple[pathlib.Path, str]:
    """Emit C for a mission and compile it. Returns (binary, log)."""
    c_file = out_dir / (source.stem + ".c")
    binary = out_dir / (source.stem + ".bin")
    with open(c_file, "wb") as fh:
        emit = subprocess.run([str(KAZE), "emit-c", str(source)], stdout=fh, stderr=subprocess.PIPE)
    if emit.returncode != 0 or c_file.stat().st_size == 0:
        return binary, "emit-c failed: " + emit.stderr.decode("utf-8", "replace")[:400]
    cmd = ["cc", "-std=c99", "-pedantic", "-Wall", "-Wextra", "-Werror", "-O2", "-I", str(DIST)]
    if sitl_link:
        cmd.append("-DKAZE_SITL_LINK")
    cmd += [str(c_file), str(DIST / "kaze_mavlink.c"), str(DIST / "kaze_rt.c"), "-lm", "-o", str(binary)]
    cc = subprocess.run(cmd, capture_output=True)
    if cc.returncode != 0:
        return binary, "cc failed: " + cc.stderr.decode("utf-8", "replace")[:800]
    return binary, ""


def prepare(source: pathlib.Path, sitl_link: bool = True) -> tuple[pathlib.Path, str]:
    """Build a mission once so a gate can fly it across many seeds and scenarios."""
    work = pathlib.Path(os.environ.get("KAZE_HARNESS_DIR", "/tmp/kaze-harness"))
    work.mkdir(parents=True, exist_ok=True)
    return build_mission(source, work, sitl_link)


def fly(source: pathlib.Path, twin_args: str = "", seed: int = 7, speed: float = 20.0,
        timeout: int = 60, sitl_link: bool = True, env_extra: dict | None = None,
        real_timeout: float = 180.0, binary: pathlib.Path | None = None,
        build_log: str = "") -> dict:
    """Build and fly one mission against a fresh twin. Returns a result dict."""
    work = pathlib.Path(os.environ.get("KAZE_HARNESS_DIR", "/tmp/kaze-harness"))
    work.mkdir(parents=True, exist_ok=True)
    started = time.time()
    if binary is None:
        binary, build_log = build_mission(source, work, sitl_link)
    if build_log:
        return {"mission": source.name, "build_ok": False, "build_log": build_log,
                "exit": None, "report": None, "seconds": round(time.time() - started, 2)}

    port = free_port()
    report_path = work / ("report-%d-%d.json" % (port, seed))
    if report_path.exists():
        report_path.unlink()
    twin_cmd = [sys.executable, str(TWIN), "--port", str(port), "--seed", str(seed),
                "--speed", str(speed), "--real-timeout", str(real_timeout),
                "--duration", "900", "--idle-exit", "3", "--report", str(report_path),
                "--scenario", source.stem]
    twin_cmd += shlex.split(twin_args)
    twin_log = work / ("twin-%d.log" % port)
    with open(twin_log, "wb") as fh:
        twin = subprocess.Popen(twin_cmd, stdout=fh, stderr=subprocess.STDOUT)
    # Wait for the socket to exist before the mission starts.
    deadline = time.time() + 10.0
    up = False
    while time.time() < deadline:
        try:
            with open(twin_log, "r", encoding="utf-8", errors="replace") as fh:
                if "listening on" in fh.read():
                    up = True
                    break
        except FileNotFoundError:
            pass
        time.sleep(0.02)
    env = dict(os.environ)
    env["KAZE_SITL"] = "udp:127.0.0.1:%d" % port
    env["KAZE_SITL_TIMEOUT_MS"] = str(int(timeout * 1000))
    if env_extra:
        env.update(env_extra)
    mission = subprocess.run([str(binary)], capture_output=True, timeout=timeout + 30, env=env)
    twin.wait(timeout=real_timeout + 60)
    report = None
    if report_path.exists():
        try:
            report = json.loads(report_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            report = None
    twin_text = twin_log.read_text(encoding="utf-8", errors="replace")
    return {
        "mission": source.name,
        "build_ok": True,
        "twin_up": up,
        "port": port,
        "seed": seed,
        "exit": mission.returncode,
        "stdout": mission.stdout.decode("utf-8", "replace").strip().splitlines(),
        "stderr": mission.stderr.decode("utf-8", "replace").strip().splitlines(),
        "report": report,
        "twin_log_tail": twin_text.strip().splitlines()[-3:],
        "seconds": round(time.time() - started, 2),
    }


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--mission", required=True)
    p.add_argument("--twin", default="", help="extra flags for kaze_sitl.py")
    p.add_argument("--seed", type=int, default=7)
    p.add_argument("--speed", type=float, default=20.0)
    p.add_argument("--timeout", type=int, default=60)
    p.add_argument("--no-sitl-link", action="store_true", help="build without the SITL host")
    p.add_argument("--unset-sitl", action="store_true", help="build with the host but do not select a vehicle")
    p.add_argument("--json", action="store_true")
    a = p.parse_args(argv)
    env_extra = {}
    if a.unset_sitl:
        env_extra["KAZE_SITL"] = ""
    result = fly(pathlib.Path(a.mission), a.twin, a.seed, a.speed, a.timeout,
                 sitl_link=not a.no_sitl_link, env_extra=env_extra or None)
    if a.json:
        print(json.dumps(result, sort_keys=True))
    else:
        print(json.dumps(result, indent=1, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
