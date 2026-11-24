#!/usr/bin/env python3
"""kaze_sitl.py -- the Kaze digital twin: a MAVLink 2 vehicle on a UDP socket.

This is the vehicle side of the Phase 15 SITL bridge. A Kaze program compiled
with -DKAZE_SITL_LINK talks to it through dist/kaze_sitl_client.c, and every
frame on the wire is MAVLink 2 built by pymavlink, so the same client speaks to
ArduPilot or PX4 SITL unchanged (this file is the stand-in when neither is
installed -- and it says which one ran).

What the twin models:

  * flight: a guided quad that holds altitude, flies legs at a bounded speed and
    lands at 0.5 m/s, driven only by MAVLink commands;
  * sensors: GPS lock delay, fix dilution, multipath jitter and HDOP
    degradation; barometer drift; magnetometer interference on yaw; IMU
    vibration noise on roll and pitch;
  * atmosphere: a steady wind, seeded gusts and a thermal updraft column;
  * failures: motor cutout (failsafe RTL), GCS link loss (failsafe RTL) and
    geofence / speed / altitude violations recorded as data, never clamped.

Everything runs in simulation time with a fixed step and a seeded generator, so
a run is reproducible: same inputs, same report.

Report (JSON, --report PATH):

  scenario, seed, waypoints_reached, sim_seconds, max_alt_m, max_speed_ms,
  landing_error_m, rtl_fired, rtl_source, failsafe_count, wind_max_ms,
  sensor gps_fix_at_s / hdop_max / baro_drift_max_m / mag_bias_max_deg /
  vib_max_deg, telemetry sent counts, and violations: [str], which must be
  empty for a clean run.

Exit status: 0 when the run finished normally, 2 on an internal error.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import select
import socket
import sys
import time

from pymavlink.dialects.v20 import ardupilotmega as mavlink

# MAVLink constants used here (values from the dialect, not from memory).
MAV_CMD_NAV_RETURN_TO_LAUNCH = 20
MAV_CMD_NAV_LAND = 21
MAV_CMD_NAV_TAKEOFF = 22
MAV_CMD_DO_PAUSE_CONTINUE = 193
MAV_CMD_COMPONENT_ARM_DISARM = 400
MAV_RESULT_ACCEPTED = 0
MAV_RESULT_DENIED = 2
MAV_RESULT_UNSUPPORTED = 3
MODE_GUIDED = 4
MODE_RTL = 6
MODE_LAND = 9
ARMED_FLAG = 128
MAV_STATE_ACTIVE = 4
MAV_STATE_STANDBY = 3


class Vehicle:
    """A guided quad in local metres (north, east, up), turned into lat/lon for telemetry."""

    def __init__(self, args: argparse.Namespace) -> None:
        self.a = args
        self.rng = random.Random(args.seed)
        self.home_lat = args.home_lat
        self.home_lon = args.home_lon
        self.home_alt_msl = args.home_alt
        self.lat = args.home_lat
        self.lon = args.home_lon
        self.n = 0.0
        self.e = 0.0
        self.up = 0.0
        self.vn = 0.0
        self.ve = 0.0
        self.vu = 0.0
        self.yaw = 0.0
        self.armed = False
        self.mode = MODE_GUIDED
        self.target_n = 0.0
        self.target_e = 0.0
        self.target_up = 0.0
        self.has_target = False
        self.landing = False
        self.rtl = False
        self.failsafe = False
        self.disarmed_at: float | None = None
        self.t = 0.0
        self.battery = 100.0
        self.max_alt = 0.0
        self.max_speed = 0.0
        self.waypoints_reached = 0
        self.landing_error = 0.0
        self.rtl_source = ""
        self.failsafe_count = 0
        self.violations: list[str] = []
        self.wind_max = 0.0
        # sensor model state
        self.hdop_max = 0.0
        self.baro_drift_max = 0.0
        self.mag_bias_max = 0.0
        self.vib_max = 0.0
        self.gps_fix_at: float | None = None
        self._baro_phase = self.rng.random() * 6.28
        self.arm_pending = False
        self.landing_ref_n = 0.0
        self.landing_ref_e = 0.0
        self.landed_at_target = False
        self.armed_at: float | None = None
        self.goto_commands = 0
        self.arrival_pending = False
        self.armed_once = False
        self.failsafe_reasons: set[str] = set()
        self.hold_n = 0.0
        self.hold_e = 0.0
        self.hold_up = 0.0
        self.on_target = 0.0
        self.leg_seq = 0
        self.wind_est_n = 0.0
        self.wind_est_e = 0.0

    # -- geography -------------------------------------------------------
    def _to_latlon(self) -> tuple[float, float]:
        lat = self.home_lat + self.n / 111320.0
        lon = self.home_lon + self.e / (111320.0 * math.cos(math.radians(self.home_lat)))
        return lat, lon

    def _from_latlon(self, lat: float, lon: float) -> tuple[float, float]:
        n = (lat - self.home_lat) * 111320.0
        e = (lon - self.home_lon) * 111320.0 * math.cos(math.radians(self.home_lat))
        return n, e

    # -- sensor models ---------------------------------------------------
    def gps_ok(self) -> bool:
        return self.t >= self.a.gps_lock

    def sensor_view(self) -> dict:
        """The measurement channels, all seeded: what the vehicle believes."""
        jitter = 0.0
        if self.a.multipath > 0.0:
            jitter = self.rng.uniform(-self.a.multipath, self.a.multipath)
        # GPS dilution: a bad window after --hdop-bad-at degrades the fix.
        hdop = self.a.hdop
        if self.a.hdop_bad_at is not None and self.a.hdop_bad_at <= self.t < self.a.hdop_bad_at + 10.0:
            hdop = hdop * 3.5
        self.hdop_max = max(self.hdop_max, hdop)
        # Barometer drift: a slow sinusoid plus noise, in metres.
        drift = self.a.baro_drift * math.sin(0.11 * self.t + self._baro_phase) + \
            self.rng.gauss(0.0, 0.03)
        self.baro_drift_max = max(self.baro_drift_max, abs(drift))
        # Magnetometer interference: a growing yaw bias inside its window.
        mag_bias = 0.0
        if self.a.mag_emi_at is not None and self.a.mag_emi_at <= self.t < self.a.mag_emi_at + 15.0:
            age = self.t - self.a.mag_emi_at
            mag_bias = min(1.0, age / 5.0) * math.radians(18.0)
        self.mag_bias_max = max(self.mag_bias_max, abs(math.degrees(mag_bias)))
        # IMU vibration: roll and pitch noise.
        vib = self.rng.gauss(0.0, self.a.vibration) if self.a.vibration > 0 else 0.0
        self.vib_max = max(self.vib_max, abs(vib))
        return {"jitter_m": jitter, "hdop": hdop, "baro_m": drift, "mag_rad": mag_bias,
                "vib_deg": vib}

    def wind_at(self) -> tuple[float, float, float]:
        """Wind as (north, east, up) m/s: a steady wind, seeded gusts, an updraft column."""
        base = math.radians(self.a.wind_dir)
        wn = self.a.wind_speed * math.cos(base)
        we = self.a.wind_speed * math.sin(base)
        if self.a.gust > 0.0:
            g = self.a.gust * (0.5 + 0.5 * math.sin(0.7 * self.t + self._baro_phase))
            g += self.rng.gauss(0.0, self.a.gust * 0.15)
            wn += g * math.cos(base)
            we += g * math.sin(base)
        wu = 0.0
        if self.a.updraft > 0.0:
            d = math.hypot(self.n - self.a.updraft_n, self.e - self.a.updraft_e)
            if d < self.a.updraft_radius:
                wu = self.a.updraft * math.cos(0.5 * math.pi * d / self.a.updraft_radius)
        speed = math.hypot(wn, we)
        self.wind_max = max(self.wind_max, speed)
        return wn, we, wu

    # -- flight ----------------------------------------------------------
    def command(self, name: str, **kw) -> bool:
        if name == "arm":
            # ArduPilot refuses to arm without a position fix; a patient operator
            # retries, so the twin marks the request and completes it at lock.
            if self.gps_ok() is False:
                self.arm_pending = True
                return True
            self.armed = True
            return True
        if name == "disarm":
            self.armed = False
            self.disarmed_at = self.t
            return True
        if name == "takeoff":
            self.on_target = 0.0
            self.mode = MODE_GUIDED
            self.landing = False
            self.rtl = False
            self.target_up = float(kw.get("alt", 10.0))
            self.has_target = True
            self.target_n, self.target_e = self.n, self.e
            return True
        if name == "goto":
            # A new leg starts with the arrival flag down, or a client that trusts
            # the flag would accept the new leg while still parked on the old one.
            self.on_target = 0.0
            self.leg_seq += 1
            self.mode = MODE_GUIDED
            self.landing = False
            self.rtl = False
            self.target_n = float(kw["n"])
            self.target_e = float(kw["e"])
            self.target_up = float(kw.get("up", self.target_up))
            self.has_target = True
            self.arrival_pending = True
            self.goto_commands += 1
            self.landing_ref_n = self.target_n
            self.landing_ref_e = self.target_e
            return True
        if name == "land":
            self.on_target = 0.0
            self.mode = MODE_LAND
            self.landing = True
            self.has_target = False
            return True
        if name == "rtl":
            self._start_rtl("command")
            return True
        if name == "pause":
            self.has_target = False
            self.hold_n, self.hold_e, self.hold_up = self.n, self.e, self.up
            return True
        return False

    def _start_rtl(self, source: str) -> None:
        if not self.rtl:
            self.rtl_source = source
        self.rtl = True
        self.on_target = 0.0
        self.mode = MODE_RTL
        self.landing = False
        self.has_target = False
        self.landing_ref_n = 0.0
        self.landing_ref_e = 0.0

    def _failsafe(self, reason: str) -> None:
        """Record a failsafe once per reason.

        A condition that persists (a battery under the floor stays under it) must
        not append a violation every step: the report would drown the flight it
        describes. The first record is the evidence.
        """
        if not self.armed:
            return
        if reason in self.failsafe_reasons:
            return
        self.failsafe_reasons.add(reason)
        self.failsafe = True
        self.failsafe_count += 1
        if len(self.violations) < 40:
            self.violations.append("failsafe:%s:at=%.2f" % (reason, self.t))
        if not self.rtl:
            self._start_rtl("failsafe:" + reason)

    def step(self, dt: float) -> None:
        self.t += dt
        wn, we, wu = self.wind_at()
        # The controller estimates the wind from its own velocity error and
        # cancels most of it, with a two-second lag: a gust still moves the
        # vehicle, a steady wind does not walk it out of the geofence.
        alpha = min(1.0, dt / 2.0)
        self.wind_est_n += (wn - self.wind_est_n) * alpha
        self.wind_est_e += (we - self.wind_est_e) * alpha
        view = self.sensor_view()

        if self.armed:
            # 0.09 %/s is about eighteen minutes of flight: enough that a survey
            # finishes with margin, small enough that the floor is still reachable.
            self.battery = max(0.0, self.battery - dt * 0.09)
            if self.battery <= self.a.battery_floor:
                self._failsafe("battery")

        if self.armed and self.a.motor_cutout_at is not None and \
                self.a.motor_cutout_at <= self.t < self.a.motor_cutout_at + dt:
            self._failsafe("motor-cutout")

        # GCS silence: the twin stops hearing the ground station for a window.
        if self.a.link_loss_at is not None and \
                self.a.link_loss_at <= self.t < self.a.link_loss_at + self.a.link_loss_for:
            if self.armed:
                self._failsafe("link-loss")

        if self.arm_pending and self.gps_ok() and not self.armed:
            self.armed = True
            self.arm_pending = False
            self.armed_once = True
            self.armed_at = self.t
        if self.armed:
            self.armed_once = True
            if self.armed_at is None:
                self.armed_at = self.t
        if not self.armed:
            self.vn = self.ve = self.vu = 0.0
            return

        max_speed = self.a.max_speed
        if self.rtl:
            home_dist = math.hypot(self.n, self.e)
            if home_dist > 1.0:
                self.target_n = self.target_e = 0.0
                self.target_up = max(self.a.rtl_altitude, self.up)
                self.has_target = True
            else:
                self.landing = True
                self.has_target = False
        elif self.landing or (self.mode == MODE_LAND):
            self.has_target = False

        self.armed_once = True

        if self.landing:
            # Held over the landing reference while it descends: the controller
            # corrects the wind, so the touchdown point is the reference, not
            # wherever thirty seconds of drift left it.
            self.vu = -min(self.a.land_speed, max(0.15, self.up * 1.5))
            self.vn = max(-2.0, min(2.0, (self.landing_ref_n - self.n) * 4.0))
            self.ve = max(-2.0, min(2.0, (self.landing_ref_e - self.e) * 4.0))
            self.vn -= self.wind_est_n * 0.12
            self.ve -= self.wind_est_e * 0.12
        elif not self.has_target and not self.rtl:
            # Hover is a position hold, not a drift: the controller keeps the
            # point the last leg ended on, so wind shows up as a small offset
            # instead of an unbounded walk out of the geofence.
            dn = self.hold_n - self.n
            de = self.hold_e - self.e
            self.vn = max(-1.5, min(1.5, dn * 2.0)) - self.wind_est_n * 0.12
            self.ve = max(-1.5, min(1.5, de * 2.0)) - self.wind_est_e * 0.12
            self.vu = max(-0.5, min(0.5, (self.hold_up - self.up) * 0.5))
        elif self.has_target:
            dn = self.target_n - self.n
            de = self.target_e - self.e
            dist = math.hypot(dn, de)
            speed = min(max_speed, max(1.0, dist * 1.5))
            if dist > 0.05:
                self.vn = speed * dn / dist
                self.ve = speed * de / dist
            else:
                self.vn = self.ve = 0.0
            du = self.target_up - self.up
            self.vu = max(-2.0, min(2.0, du * 0.8))

        # Wind pushes a light quad; the controller only partly compensates.
        self.vn += wn * 0.12
        self.ve += we * 0.12
        self.vu += wu * 0.10
        if self.landing and self.up < 0.3:
            # On the ground: friction holds the skids, so a gust cannot slide a
            # landed vehicle off its touchdown point.
            self.vn = 0.0
            self.ve = 0.0

        self.n += self.vn * dt
        self.e += self.ve * dt
        self.up += self.vu * dt
        if self.up < 0.0:
            self.up = 0.0
        self.yaw = math.atan2(self.ve, self.vn) if (abs(self.vn) + abs(self.ve)) > 0.1 else self.yaw

        speed_now = math.hypot(self.vn, self.ve)
        self.max_speed = max(self.max_speed, speed_now)
        self.max_alt = max(self.max_alt, self.up)
        lat, lon = self._to_latlon()
        self.lat, self.lon = lat, lon

        # Arrival bookkeeping: one count per commanded leg, not per step inside
        # tolerance, and takeoff or RTL never count as a waypoint.
        # Two different questions, answered separately:
        #   * did the mission reach the commanded point? counted once, on the
        #     twin's true state, with a tolerance that survives the modelled
        #     sensing error (the client only ever sees a noisy measurement);
        #   * has the vehicle finished the leg? it keeps closing until it is
        #     parked on the point, because a measured 1 m can be a true 2 m.
        if self.has_target:
            dist_now = math.hypot(self.target_n - self.n, self.target_e - self.e)
            dz_now = abs(self.target_up - self.up)
            if self.arrival_pending and dist_now <= 2.0 and dz_now <= 2.0:
                self.waypoints_reached += 1
                self.arrival_pending = False
            guided_leg = (not self.rtl) and (not self.landing) and self.mode == MODE_GUIDED
            self.on_target = 1.0 if (guided_leg and dist_now <= 0.3 and dz_now <= 0.5) else 0.0
            if dist_now <= 0.1 and dz_now <= 0.2:
                self.has_target = False
                self.hold_n, self.hold_e, self.hold_up = self.target_n, self.target_e, self.target_up

        if self.up <= 0.01 and (self.landing or self.rtl):
            self.landing_error = math.hypot(self.n - self.landing_ref_n,
                                            self.e - self.landing_ref_e)
            self.armed = False
            self.disarmed_at = self.t
            self.rtl = False
            self.landing = False

        self._check_violations(speed_now)

    def _check_violations(self, speed_now: float) -> None:
        def note(msg: str) -> None:
            if len(self.violations) < 40:
                self.violations.append("%s:at=%.2f" % (msg, self.t))

        if math.hypot(self.n, self.e) > self.a.geofence:
            if not any(v.startswith("geofence-breach") for v in self.violations):
                note("geofence-breach")
        if self.up > self.a.max_alt + 1.0:
            if not any(v.startswith("altitude-ceiling") for v in self.violations):
                note("altitude-ceiling")
        if speed_now > self.a.max_speed + 1.0:
            if not any(v.startswith("overspeed") for v in self.violations):
                note("overspeed")


class Twin:
    """The MAVLink side: frames in, frames out, and the report at the end."""

    def __init__(self, args: argparse.Namespace) -> None:
        self.a = args
        self.v = Vehicle(args)
        self.mav = mavlink.MAVLink(None, srcSystem=args.sysid, srcComponent=1)
        self.mav.seq = 0
        # Junk on the socket must not kill a simulator: report it and resync.
        self.mav.robust_parsing = True
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind((args.bind, args.port))
        self.sock.setblocking(False)
        self.gcs_addr: tuple[str, int] | None = None
        self.leftover = b""
        self.counts: dict[str, int] = {}
        self.tx_state = {"pos": 0.0, "att": 0.0, "sys": 0.0, "hud": 0.0, "gps": 0.0,
                         "hb": 0.0, "named": 0.0}
        self.last_rx_real = 0.0
        self.start_real = time.time()
        self.armed_once = False
        self.mission_done_at: float | None = None
        self.sim_dt = 1.0 / args.rate_hz

    # -- wire ------------------------------------------------------------
    def send(self, msg) -> None:
        if self.gcs_addr is None:
            return
        try:
            self.sock.sendto(msg.pack(self.mav), self.gcs_addr)
            self.counts[msg.get_type()] = self.counts.get(msg.get_type(), 0) + 1
        except OSError:
            pass

    def send_heartbeat(self) -> None:
        base_mode = ARMED_FLAG if self.v.armed else 0
        self.send(mavlink.MAVLink_heartbeat_message(
            type=2, autopilot=3, base_mode=base_mode, custom_mode=self.v.mode,
            system_status=MAV_STATE_ACTIVE if self.v.armed else MAV_STATE_STANDBY,
            mavlink_version=3))

    def send_telemetry(self) -> None:
        v = self.v
        t = v.t
        view = v.sensor_view()
        if t - self.tx_state["hb"] >= 1.0 / max(1.0, self.a.hb_hz):
            self.tx_state["hb"] = t
            self.send_heartbeat()
        if t - self.tx_state["pos"] >= 0.1:
            self.tx_state["pos"] = t
            lat, lon = v.lat + view["jitter_m"] / 111320.0, v.lon + view["jitter_m"] / 111320.0
            self.send(mavlink.MAVLink_global_position_int_message(
                time_boot_ms=int(t * 1000), lat=int(lat * 1e7), lon=int(lon * 1e7),
                alt=int((v.home_alt_msl + v.up) * 1000),
                relative_alt=int((v.up + view["baro_m"]) * 1000),
                vx=int(v.vn * 100), vy=int(v.ve * 100), vz=int(-v.vu * 100),
                hdg=int((math.degrees(v.yaw) % 360.0) * 100)))
        if t - self.tx_state["att"] >= 0.1:
            self.tx_state["att"] = t
            roll = self.a.vibration and view["vib_deg"] * 0.0175 or 0.0
            self.send(mavlink.MAVLink_attitude_message(
                time_boot_ms=int(t * 1000), roll=roll, pitch=-roll * 0.6,
                yaw=v.yaw + view["mag_rad"], rollspeed=0.0, pitchspeed=0.0, yawspeed=0.0))
        if t - self.tx_state["sys"] >= 1.0:
            self.tx_state["sys"] = t
            self.send(mavlink.MAVLink_sys_status_message(
                onboard_control_sensors_present=0, onboard_control_sensors_enabled=0,
                onboard_control_sensors_health=0, load=120, voltage_battery=12000,
                current_battery=800, drop_rate_comm=0, errors_comm=0, errors_count1=0,
                errors_count2=0, errors_count3=0, errors_count4=0,
                battery_remaining=int(v.battery)))
        if t - self.tx_state["gps"] >= 0.2:
            self.tx_state["gps"] = t
            fix = 3 if v.gps_ok() else 0
            sats = 12 if v.gps_ok() else int(min(12, max(0, (t / max(0.1, self.a.gps_lock)) * 12)))
            if fix == 3 and v.gps_fix_at is None:
                v.gps_fix_at = t
            self.send(mavlink.MAVLink_gps_raw_int_message(
                time_usec=int(t * 1e6), lat=int(v.lat * 1e7), lon=int(v.lon * 1e7),
                alt=int((v.home_alt_msl + v.up) * 1000), eph=int(view["hdop"] * 100),
                epv=int(view["hdop"] * 150), vel=int(math.hypot(v.vn, v.ve) * 100),
                cog=0, fix_type=fix, satellites_visible=sats))
        if t - self.tx_state["hud"] >= 0.2:
            self.tx_state["hud"] = t
            self.send(mavlink.MAVLink_vfr_hud_message(
                airspeed=float(math.hypot(v.vn, v.ve)), groundspeed=float(math.hypot(v.vn, v.ve)),
                alt=float(v.up), climb=float(v.vu),
                heading=int(math.degrees(v.yaw) % 360.0),
                throttle=50 if v.armed else 0))
        if t - self.tx_state["named"] >= 0.5:
            self.tx_state["named"] = t
            wn, we, wu = v.wind_at()
            self.send(mavlink.MAVLink_named_value_float_message(
                time_boot_ms=int(t * 1000), name=b"wind_speed",
                value=float(math.hypot(wn, we))))
            self.send(mavlink.MAVLink_named_value_float_message(
                time_boot_ms=int(t * 1000), name=b"wind_dir",
                value=float(math.degrees(math.atan2(we, wn)) % 360.0)))
            self.send(mavlink.MAVLink_named_value_float_message(
                time_boot_ms=int(t * 1000), name=b"failsafe",
                value=1.0 if v.failsafe else 0.0))
            # An explicit arrival flag: the twin knows its true state, so a client
            # does not have to infer arrival from a noisy measurement. A vehicle
            # that does not publish one falls back to judging position.
            self.send(mavlink.MAVLink_named_value_float_message(
                time_boot_ms=int(t * 1000), name=b"on_target",
                value=float(v.on_target)))
            self.send(mavlink.MAVLink_named_value_float_message(
                time_boot_ms=int(t * 1000), name=b"leg_seq", value=float(v.leg_seq)))

    def _ack(self, command: int, result: int) -> None:
        self.send(mavlink.MAVLink_command_ack_message(
            command=command, result=result, progress=0, result_param2=0,
            target_system=255, target_component=190))

    def on_message(self, msg) -> None:
        v = self.v
        kind = msg.get_type()
        if kind == "COMMAND_LONG":
            cmd = msg.command
            if cmd == MAV_CMD_COMPONENT_ARM_DISARM:
                want = msg.param1 >= 0.5
                if want:
                    ok = v.command("arm")
                else:
                    ok = v.command("disarm")
                self._ack(cmd, MAV_RESULT_ACCEPTED if ok else MAV_RESULT_DENIED)
            elif cmd == MAV_CMD_NAV_TAKEOFF:
                ok = v.command("takeoff", alt=msg.param7)
                self._ack(cmd, MAV_RESULT_ACCEPTED if ok else MAV_RESULT_DENIED)
            elif cmd == MAV_CMD_NAV_LAND:
                ok = v.command("land")
                self._ack(cmd, MAV_RESULT_ACCEPTED if ok else MAV_RESULT_DENIED)
            elif cmd == MAV_CMD_NAV_RETURN_TO_LAUNCH:
                ok = v.command("rtl")
                self._ack(cmd, MAV_RESULT_ACCEPTED if ok else MAV_RESULT_DENIED)
            elif cmd == MAV_CMD_DO_PAUSE_CONTINUE:
                ok = v.command("pause")
                self._ack(cmd, MAV_RESULT_ACCEPTED if ok else MAV_RESULT_DENIED)
            else:
                self._ack(cmd, MAV_RESULT_UNSUPPORTED)
        elif kind == "SET_POSITION_TARGET_GLOBAL_INT":
            if not v.armed:
                return
            lat = msg.lat_int / 1e7
            lon = msg.lon_int / 1e7
            n, e = v._from_latlon(lat, lon)
            self.v.command("goto", n=n, e=e, up=msg.alt)
        elif kind == "HEARTBEAT":
            pass

    def pump(self) -> None:
        for _ in range(64):
            try:
                data, addr = self.sock.recvfrom(4096)
            except BlockingIOError:
                return
            except OSError:
                return
            self.gcs_addr = addr
            self.last_rx_real = time.time()
            buf = self.leftover + data
            try:
                msgs = self.mav.parse_buffer(buf)
            except Exception:  # noqa: BLE001 - a hostile datagram is a data point
                msgs = None
            self.leftover = b""
            if msgs is None:
                # Incomplete frame: keep the tail for the next datagram, but never
                # more than one maximum frame, so junk cannot grow without bound.
                self.leftover = buf[-280:] if len(buf) > 280 else buf
                continue
            for msg in msgs:
                if isinstance(msg, mavlink.MAVLink_bad_data):
                    continue
                self.on_message(msg)

    # -- run -------------------------------------------------------------
    def wait_for_gcs(self) -> bool:
        """Simulation time starts when a ground station speaks, not before.

        A twin that ran its clock out while nobody was listening would report an
        empty mission as a success, which is exactly the kind of fabricated pass
        this project refuses.
        """
        deadline = time.time() + self.a.connect_timeout
        while time.time() < deadline:
            ready, _, _ = select.select([self.sock], [], [], 0.05)
            if ready:
                self.pump()
                if self.gcs_addr is not None:
                    return True
        return False

    def run(self) -> dict:
        if not self.wait_for_gcs():
            print("kaze-sitl: no ground station connected within %.1f s"
                  % self.a.connect_timeout, file=sys.stderr)
            self.v.violations.append("no-gcs")
            return self.report()
        stop_at = self.v.t + self.a.duration
        real_deadline = time.time() + self.a.real_timeout
        idle_after = self.a.idle_exit
        while self.v.t < stop_at:
            if time.time() > real_deadline:
                self.v.violations.append("real-timeout")
                break
            self.pump()
            self.v.step(self.sim_dt)
            self.send_telemetry()
            if self.a.inject_link_loss_real and self.gcs_addr is not None:
                if time.time() - self.last_rx_real > self.a.link_loss_for:
                    self.v._failsafe("link-loss")
            if self.v.armed:
                self.mission_done_at = None
            elif self.v.armed_once and self.mission_done_at is None:
                self.mission_done_at = self.v.t
            if self.mission_done_at is not None and self.v.t - self.mission_done_at > idle_after:
                break
            if self.a.speed > 0:
                time.sleep(max(0.0, self.sim_dt / self.a.speed))
        return self.report()

    def report(self) -> dict:
        v = self.v
        return {
            "scenario": self.a.scenario,
            "seed": self.a.seed,
            "sim_seconds": round(v.t, 3),
            "waypoints_reached": v.waypoints_reached,
            "max_alt_m": round(v.max_alt, 3),
            "max_speed_ms": round(v.max_speed, 3),
            "landing_error_m": round(v.landing_error, 3),
            "rtl_fired": bool(v.rtl_source),
            "rtl_source": v.rtl_source,
            "armed_at_s": v.armed_at,
            "disarmed_at_s": v.disarmed_at,
            "landing_ref": [round(v.landing_ref_n, 2), round(v.landing_ref_e, 2)],
            "failsafe_count": v.failsafe_count,
            "failsafe_reasons": sorted(v.failsafe_reasons),
            "wind_max_ms": round(v.wind_max, 3),
            "armed_once": v.armed_once,
            "goto_commands": v.goto_commands,
            "arrivals": v.waypoints_reached,
            "sensor": {
                "gps_fix_at_s": v.gps_fix_at,
                "hdop_max": round(v.hdop_max, 3),
                "baro_drift_max_m": round(v.baro_drift_max, 3),
                "mag_bias_max_deg": round(v.mag_bias_max, 3),
                "vib_max_deg": round(v.vib_max, 3),
            },
            "telemetry_sent": self.counts,
            "violations": list(v.violations),
        }


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Kaze digital twin (MAVLink 2 over UDP)")
    p.add_argument("--port", type=int, default=14550)
    p.add_argument("--bind", default="127.0.0.1")
    p.add_argument("--sysid", type=int, default=1)
    p.add_argument("--scenario", default="manual")
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--duration", type=float, default=300.0, help="simulation seconds")
    p.add_argument("--speed", type=float, default=20.0,
                   help="sim seconds per real second; 0 = as fast as the CPU allows")
    p.add_argument("--connect-timeout", type=float, default=30.0,
                   help="real seconds to wait for the first ground-station datagram")
    p.add_argument("--real-timeout", type=float, default=180.0,
                   help="real seconds the whole run may take, so a stuck harness cannot hang")
    p.add_argument("--rate-hz", type=float, default=50.0)
    p.add_argument("--hb-hz", type=float, default=1.0, help="heartbeat rate in simulation time")
    p.add_argument("--idle-exit", type=float, default=5.0)
    p.add_argument("--report", default="")
    p.add_argument("--home-lat", type=float, default=47.6)
    p.add_argument("--home-lon", type=float, default=8.5)
    p.add_argument("--home-alt", type=float, default=500.0)
    # flight envelope
    p.add_argument("--max-speed", type=float, default=8.0)
    p.add_argument("--land-speed", type=float, default=0.5)
    p.add_argument("--rtl-altitude", type=float, default=15.0)
    p.add_argument("--max-alt", type=float, default=120.0)
    p.add_argument("--geofence", type=float, default=300.0)
    p.add_argument("--battery-floor", type=float, default=20.0)
    # sensors
    p.add_argument("--gps-lock", type=float, default=3.0)
    p.add_argument("--hdop", type=float, default=0.8)
    p.add_argument("--hdop-bad-at", type=float, default=None)
    p.add_argument("--multipath", type=float, default=0.4)
    p.add_argument("--baro-drift", type=float, default=0.6)
    p.add_argument("--mag-emi-at", type=float, default=None)
    p.add_argument("--vibration", type=float, default=0.3)
    # atmosphere
    p.add_argument("--wind-speed", type=float, default=0.0)
    p.add_argument("--wind-dir", type=float, default=270.0)
    p.add_argument("--gust", type=float, default=0.0)
    p.add_argument("--updraft", type=float, default=0.0)
    p.add_argument("--updraft-radius", type=float, default=20.0)
    p.add_argument("--updraft-n", type=float, default=40.0)
    p.add_argument("--updraft-e", type=float, default=0.0)
    # failures
    p.add_argument("--motor-cutout-at", type=float, default=None)
    p.add_argument("--link-loss-at", type=float, default=None)
    p.add_argument("--link-loss-for", type=float, default=4.0)
    p.add_argument("--inject-link-loss-real", action="store_true",
                   help="also fail safe when the GCS stops sending for --link-loss-for seconds")
    return p.parse_args(argv)


def main(argv: list[str]) -> int:
    a = parse_args(argv)
    try:
        twin = Twin(a)
    except OSError as exc:
        print("kaze-sitl: cannot bind %s:%d (%s)" % (a.bind, a.port, exc), file=sys.stderr)
        return 2
    print("kaze-sitl: twin listening on udp:%s:%d scenario=%s seed=%d"
          % (a.bind, a.port, a.scenario, a.seed), file=sys.stderr)
    report = twin.run()
    text = json.dumps(report, sort_keys=True)
    if a.report:
        with open(a.report, "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
    print("kaze-sitl: " + text, file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
