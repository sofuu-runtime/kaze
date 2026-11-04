# Field Checklist — Kaze v0.3 Kinematics & Spatial Navigation

Everything that can be checked deterministically without a vehicle is verified by
`cargo test` (167 unit, integration, and live tests with default features). This
checklist covers exercising spatial math and navigation against real flight
controllers and SITL.

## 1. SITL Kinematics Mission (ArduCopter over UDP)

```bash
# Terminal 1 — ArduCopter SITL
~/ardupilot/build/SITL/bin/arducopter --model quad -I 0 \
  --home 47.6000,8.5000,500,0 --serial0 udpclient:127.0.0.1:14550

# Terminal 2 — Run in-VM navigation mission
cargo run --release -- run - --cap drone=udp:127.0.0.1:14550 --plain <<'KAZE'
needs: drone, log

action main():
    arm drone
    takeoff to 15 meters
    let wp = vector(47.6004, 8.5004, 520)
    log "bearing to waypoint:"
    log bearing to wp
    log "slant distance to waypoint:"
    log slant distance between vector(latitude, longitude, 500) and wp
    fly to wp at 20 meters
    return to launch
KAZE
```

Expected trace:
- Vehicle arms and climbs to 15 m.
- `bearing to wp` reads live latitude and longitude through the ambient capability, computing ~33.97° heading.
- `slant distance` calculates 3D distance including the climb (~57.2 m).
- `fly to wp at 20 meters` transmits a guided position setpoint frame with coordinates unpacked from the vector.
- Vehicle flies to the waypoint, climbs to 20 m, then activates RTL.

## 2. In-Memory Vector & Trigonometry Tests (No Autopilot)

```bash
cargo run --release -- run examples/kinematics.kaze --cap log=sim --plain
```

Expected:
- Output matches `examples/golden/kinematics.txt` bit-for-bit:
  - Haversine distance: 53.645 m
  - Slant range with climb: 57.252 m
  - Initial bearing: 33.99°
  - Vector arithmetic and 3D Euclidean magnitude (`length of doubled` = 40.0)
  - Offset compass direction (`bearing of doubled` = 45.0°)
  - Trigonometry (`sin(30 in radians)` = 0.5)

## 3. Vector Component Access Ergonomics

```bash
cargo run --release -- run - --cap log=sim --plain <<'KAZE'
needs: log

action main():
    let target = vector(47.6004, 8.5004, 520)
    # Numerical indexing
    log target[0]
    log target[1]
    log target[2]
    # Dot-notation field names
    log target.lat
    log target.lon
    log target.alt
    log target.x
    log target.y
    log target.z
KAZE
```

Expected:
- Output prints latitude (47.6004), longitude (8.5004), and altitude (520) via both index access and dot-notation aliases.
- Non-coordinate field access (e.g. `target.invalid`) is rejected at compile time (`E_KAZE_CHECK`).
