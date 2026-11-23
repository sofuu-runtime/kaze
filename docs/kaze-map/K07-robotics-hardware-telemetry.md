# Division 07: Robotics, Effector Drivers & Telemetry (K07)

> **Maturity:** Preview (Protocol codec & safety gates verified; live physical actuation unverified)  
> **Parent Blueprint:** [kaze-map/README.md](README.md)  
> **Primary Locations:** `kaze/src/hardware.rs`, `kaze/src/mavlink.rs`, `kaze/src/gpio.rs`, `kaze/src/gpio_chardev.rs`, `kaze/src/pwm.rs`, `kaze/src/geo.rs`, `kaze/src/link.rs`

---

## 1. Mission & Scope

Division 07 provides the mission-critical hardware interface layer for autonomous physical systems, drones, rovers, and embedded edge devices. It encapsulates low-level communication protocols (MAVLink 2.0), Linux character device GPIO pulse timers, PWM servo drivers, geodesy navigation mathematics, and physical safety watchdog systems.

Physical systems operate under unforgiving real-world constraints where runtime delays or missed signals can result in hardware destruction or safety hazards. Division 07 enforces strict hardware safety barriers: pulse timers must de-assert cleanly on exit, telemetry must arrive within rigid freshness bounds, and any link loss or protocol breach must trigger immediate fail-closed return-to-launch (RTL) or motor cutoffs.

---

## 2. Code Manifest & Owned Files

| Path | Purpose |
|---|---|
| `kaze/src/hardware.rs` | Hardware capability policy gate, actuator authority verifier, heartbeat watchdog |
| `kaze/src/mavlink.rs` | MAVLink 2.0 binary protocol encoder, decoder, frame parser, CRC verification |
| `kaze/src/gpio.rs` | Linux GPIO pin abstraction and pulse-width timing interfaces |
| `kaze/src/gpio_chardev.rs` | High-precision character device (`/dev/gpiochip*`) event line monitoring and pulse timers |
| `kaze/src/pwm.rs` | Pulse-Width Modulation (PWM) duty cycle controllers for servos and electronic speed controls (ESCs) |
| `kaze/src/geo.rs` | Geodesy math: Vincenty and Haversine distance, bearing calculations, geofence polygon checks |
| `kaze/src/link.rs` | Physical radio telemetry link monitors, packet loss tracking, link-loss failsafes |

---

## 3. Architectural Specifications & Invariants

1. **Unified 1500 ms Heartbeat Safety Watchdog**:
   - Telemetry packets and operator heartbeats are verified against a hard freshness limit:
     $$\Delta t \le 1500\text{ ms}$$
   - Any heartbeat or critical sensor stream older than $1500\text{ ms}$ immediately triggers the fail-closed safety state: motor disarm or autonomous Return-to-Launch (RTL).
2. **Mandatory Actuator De-Assertion on Termination**:
   - Actuator lines (GPIO output pins, PWM duty generators) automatically register OS exit traps and signal handlers (`SIGINT`, `SIGTERM`, abnormal panic).
   - Actuators are immediately grounded and de-asserted upon termination to prevent stuck thrusters or runaway servos (`F-09`).
3. **Geodesy Math & 3D Geofence Containment**:
   - GPS coordinates (latitude, longitude, altitude) are continuously checked against 3D convex geofence polygons.
   - Any trajectory command attempting to breach an active geofence boundary is rejected at the hardware gate before reaching the flight controller.
4. **Explicit Physical Simulator Labelling**:
   - Software-in-the-loop (SITL) simulation is reachable solely through explicit, labelled configuration (`drone=sim` / `rover=sim`).
   - Every simulated telemetry frame and execution report is explicitly tagged as simulated; simulation may never masquerade as physical hardware verification (`RULES.md` §1.9).

---

## 4. Implementation Details & Safety Gate Sequence

### MAVLink 2.0 Frame Structure & Verification

```
┌──────┬────────┬────────┬───────┬───────┬──────┬──────┬───────┬─────────┬───────┐
│ STX  │ LEN    │ INC_F  │ CMP_F │ SEQ   │ SYS  │ COMP │ MSG   │ PAYLOAD │ CHKSUM│
│ 0xFD │ (1 B)  │ (1 B)  │ (1 B) │ (1 B) │(1 B) │(1 B) │ (3 B) │ (0-255B)│ (2 B) │
└──────┴────────┴────────┴───────┴───────┴──────┴──────┴───────┴─────────┴───────┘
```

### Telemetry Freshness Watchdog Logic

```text
# Planned spelling: v1.0 declares `needs: drone, log`, not `hardware.*`.
# Routine handling drone waypoint navigation with 1500ms heartbeat invariant
routine navigate_waypoint(target_lat: deg, target_lon: deg, altitude: meter) -> bool:
  needs: hardware.drone
  needs: hardware.telemetry
  
  let current = hardware.telemetry.last_frame() otherwise { return false }
  let age = current.timestamp_ms - hardware.clock.now_ms()
  
  if age > 1500:
    hardware.drone.failsafe_rtl()
    return false
    
  hardware.drone.goto(target_lat, target_lon, altitude)
  return true
```

---

## 5. Verification & Testing

```bash
# Verify hardware safety policy gates
cargo test --manifest-path kaze/Cargo.toml --lib hardware

# Verify MAVLink 2.0 encoder and decoder
cargo test --manifest-path kaze/Cargo.toml --lib mavlink

# Verify geodesy distance, bearing, and geofence math
cargo test --manifest-path kaze/Cargo.toml --lib geo

# Verify GPIO pulse timing
cargo test --manifest-path kaze/Cargo.toml --lib gpio
```

---

## 6. Active & Completed Tasks

- [x] Implement MAVLink 2.0 binary protocol parser and frame generator (`mavlink.rs`).
- [x] Unify 1500 ms telemetry freshness watchdog with embedded Sofuu engine (`bb98aaa65`).
- [x] Implement GPIO chardev pulse timer with automatic de-assert handlers (`F-09`).
- [x] Extract unified flight and ground vehicle safety rules (`safety.rs`).
- [ ] Connect physical testbed hardware (flight controller / companion computer) for hardware-in-the-loop (HIL) certification.
- [ ] Implement CAN-bus protocol frame decoding for automotive edge controllers.

## Roadmap phase 1-6 update (2026-11-22)

- The 1500 ms heartbeat and per-field telemetry freshness veto now gate live actuation: a stale link or a stale field is `E_SAFETY_VIOLATION` and no command leaves the process.
- `kaze/dist/kaze_rt.c` provides the watchdog primitive (`kaze_watchdog_t`, 1499 ms fresh vs 1500 ms stale) and the async-signal-safe de-assert path used by the GPIO/PWM drivers.
- Live actuation is enabled; the dependency table decides which fields each command needs (arm/land/rtl/hold/stop: heartbeat; takeoff: lat/lon/alt; goto: the target fields), and a stale or missing field refuses with uncatchable `E_SAFETY_VIOLATION`.
- GPIO/PWM outputs de-assert on SIGINT/SIGTERM (async-signal-safe) and on normal exit; verified with a fake sysfs root and the real binary.
- Hardware-in-the-loop and a real 32-bit target remain unverified here; see the phase report.
- Verification: `cargo test --manifest-path kaze/Cargo.toml --test live --test effector_safety`.
