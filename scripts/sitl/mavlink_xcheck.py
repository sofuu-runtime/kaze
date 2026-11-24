#!/usr/bin/env python3
"""mavlink_xcheck.py -- prove the C codec and pymavlink agree, both directions.

The bridge is only worth having if its frames are real MAVLink 2: a vehicle
built here must be replaceable by ArduPilot or PX4 without touching the client.
So every frame is checked twice:

  * frames pymavlink builds are handed to the C decoder (dist/kaze_mavlink.c),
    and the fields it reports must equal the fields pymavlink encoded;
  * frames the C codec builds are parsed by pymavlink, and the fields must equal
    what the C code asked for;
  * the HEARTBEAT frame must match pymavlink's bytes exactly for the same
    sequence and ids, because that pins the framing, the field order and the
    CRC_EXTRA in one comparison.

Exit status 0 when every comparison holds.

  python3 scripts/sitl/mavlink_xcheck.py [--keep]
"""

from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

from pymavlink.dialects.v20 import ardupilotmega as mavlink

ROOT = pathlib.Path(__file__).resolve().parents[2]
DIST = ROOT / "dist"

FAILURES: list[str] = []
CHECKS = 0


def check(name: str, ok: bool, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    if ok:
        return
    FAILURES.append("%s%s" % (name, (" (" + detail + ")") if detail else ""))


def build_probe(workdir: pathlib.Path) -> pathlib.Path:
    binary = workdir / "mavlink_probe"
    cmd = ["cc", "-std=c99", "-pedantic", "-Wall", "-Wextra", "-Werror", "-O2",
           "-I", str(DIST), str(ROOT / "scripts" / "sitl" / "mavlink_probe.c"),
           str(DIST / "kaze_mavlink.c"), "-o", str(binary)]
    result = subprocess.run(cmd, capture_output=True)
    if result.returncode != 0:
        raise SystemExit("mavlink_probe did not build: " + result.stderr.decode())
    return binary


def parse_with_c(probe: pathlib.Path, frame: bytes) -> dict[str, str]:
    result = subprocess.run([str(probe), "parse", frame.hex()], capture_output=True, text=True)
    fields: dict[str, str] = {}
    for line in result.stdout.splitlines():
        for token in line.split():
            if "=" in token:
                key, _, value = token.partition("=")
                fields[key] = value
    return fields


def main(argv: list[str]) -> int:
    workdir = pathlib.Path(tempfile.mkdtemp(prefix="kaze-mavlink-"))
    probe = build_probe(workdir)
    enc = mavlink.MAVLink(None, srcSystem=1, srcComponent=1)

    # 1. pymavlink builds, the C codec reads.
    cases = [
        (mavlink.MAVLink_heartbeat_message(type=2, autopilot=3, base_mode=128, custom_mode=4,
                                           system_status=4, mavlink_version=3),
         {"id": "0", "base_mode": "128", "custom_mode": "4"}),
        (mavlink.MAVLink_global_position_int_message(time_boot_ms=1234, lat=476000000,
                                                     lon=85000000, alt=500000, relative_alt=15000,
                                                     vx=100, vy=-100, vz=50, hdg=18000),
         {"id": "33", "lat": "476000000", "lon": "85000000", "rel": "15000", "hdg": "18000"}),
        (mavlink.MAVLink_command_ack_message(command=400, result=0, progress=0, result_param2=0,
                                             target_system=1, target_component=1),
         {"id": "77", "command": "400", "result": "0"}),
        (mavlink.MAVLink_named_value_float_message(time_boot_ms=1, name=b"wind_speed", value=4.5),
         {"id": "251", "name": "wind_speed", "value": "4.5000"}),
        (mavlink.MAVLink_attitude_message(time_boot_ms=1, roll=0.5, pitch=-0.25, yaw=1.5,
                                          rollspeed=0, pitchspeed=0, yawspeed=0),
         {"id": "30", "roll": "0.5000", "yaw": "1.5000"}),
        (mavlink.MAVLink_vfr_hud_message(airspeed=0, groundspeed=8.5, alt=15.0, climb=0.5,
                                         heading=90, throttle=50),
         {"id": "74", "groundspeed": "8.500"}),
        (mavlink.MAVLink_statustext_message(7, b"AB", 0x1234, 5), {"id": "253"}),
    ]
    for msg, expect in cases:
        frame = msg.pack(enc)
        got = parse_with_c(probe, frame)
        for key, value in expect.items():
            check("c-decodes %s %s" % (msg.get_type(), key), got.get(key) == value,
                  "got %r want %r" % (got.get(key), value))

    # 2. the C codec builds, pymavlink reads, and HEARTBEAT matches byte for byte.
    built = subprocess.run([str(probe), "build"], capture_output=True, text=True)
    lines = [ln.strip() for ln in built.stdout.splitlines() if ln.strip()]
    check("c-builds three frames", len(lines) == 3, "got %d" % len(lines))

    dec = mavlink.MAVLink(None, srcSystem=1, srcComponent=1)
    dec.robust_parsing = True
    if len(lines) >= 1:
        hb = dec.parse_char(bytes.fromhex(lines[0]))
        check("py-decodes C heartbeat", hb is not None and hb.get_type() == "HEARTBEAT")
        if hb is not None:
            check("C heartbeat base_mode", getattr(hb, "base_mode", None) == 128)
            check("C heartbeat custom_mode", getattr(hb, "custom_mode", None) == 4)
            check("C heartbeat seq/sys/comp", (hb.get_srcSystem(), hb.get_srcComponent()) == (1, 1))
        ref = mavlink.MAVLink(None, srcSystem=1, srcComponent=1)
        ref.seq = 7
        ref_msg = mavlink.MAVLink_heartbeat_message(type=2, autopilot=3, base_mode=128,
                                                    custom_mode=4, system_status=4,
                                                    mavlink_version=3)
        check("C heartbeat bytes == pymavlink bytes", ref_msg.pack(ref).hex() == lines[0],
              "%s != %s" % (lines[0], ref_msg.pack(ref).hex()))
    if len(lines) >= 2:
        cmd = dec.parse_char(bytes.fromhex(lines[1]))
        check("py-decodes C command_long", cmd is not None and cmd.get_type() == "COMMAND_LONG")
        if cmd is not None:
            check("C arm command id", getattr(cmd, "command", None) == 400)
            check("C arm param1", abs(getattr(cmd, "param1", 0.0) - 1.0) < 1e-6)
    if len(lines) >= 3:
        pos = dec.parse_char(bytes.fromhex(lines[2]))
        check("py-decodes C set_position_target", pos is not None
              and pos.get_type() == "SET_POSITION_TARGET_GLOBAL_INT")
        if pos is not None:
            check("C target lat", getattr(pos, "lat_int", None) == 476000000)
            check("C target alt", abs(getattr(pos, "alt", 0.0) - 15.0) < 1e-6)
            check("C target frame", getattr(pos, "coordinate_frame", None) == 6)

    print("mavlink xcheck: %d checks, %d failed" % (CHECKS, len(FAILURES)))
    for failure in FAILURES:
        print("  FAIL " + failure)
    return 0 if not FAILURES else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
