# Field checklist — Kaze v0.2 on real hardware

Everything that can be checked without hardware is checked by `cargo test`
here. This file is the part only a board and a flight controller can answer.
Write down what you saw for each line; a line that fails is a bug report with a
command already attached.

## 1. SITL over UDP (no hardware)

```bash
# terminal 1 — ArduPilot SITL
~/ardupilot/build/SITL/bin/arducopter --model quad -I 0 \
  --home 47.6000,8.5000,500,0 --serial0 udpclient:127.0.0.1:14550

# terminal 2 — wait ~25 s for the vehicle to finish initialising, then
cargo run --release -- run mission.kaze --cap drone=udp:127.0.0.1:14550 --plain
```

Expected: `arm`, `takeoff`, `goto`, `rtl` traced, exit 0. Before the vehicle is
ready the script fails with `E_COMMAND_REJECTED ... answered FAILED for
MAV_CMD_DO_SET_MODE` or `E_ARMING_DENIED` and the autopilot's own reason —
never a silent success.

## 2. GPIO on a Raspberry Pi (sysfs build)

```bash
cargo build --release
# a pin with an LED or a relay on it, named by you:
./target/release/kaze run - --cap device=gpio --pin fan=17 --plain <<'KAZE'
needs: fan

action main():
    turn on fan
    pulse fan for 1 seconds
KAZE
```

Expected: the line goes high, then low a second later. If the board has no
sysfs GPIO the error says so and names the character device.

## 3. GPIO on a Raspberry Pi 5 (chardev build)

```bash
cargo build --release --features chardev
./target/release/kaze run - --cap device=gpio --pin fan=17 --plain <<'KAZE'
needs: fan

action main():
    turn on fan
KAZE
```

The chardev path compiles against the documented GPIO v2 UAPI and its request
layout and ioctl numbers are unit-tested, but it has not run on a chip yet.
If the line does not move, the error text says which ioctl the kernel refused —
report that line.

## 4. PWM (any board with `/sys/class/pwm`)

```bash
cargo build --release
./target/release/kaze run - --cap device=gpio --pwm fan=0:0 --plain <<'KAZE'
needs: fan

action main():
    device.set_duty("fan", 60)
    device.set_duty("fan", 0)
KAZE
```

Expected: 50 Hz at 60% duty, then the channel switched off and unexported when
the process exits. `duty_cycle` should read `12000000` ns in between.

## 5. Serial flight controller

```bash
cargo build --release --features uart
./target/release/kaze run mission.kaze --cap drone=uart:/dev/ttyAMA0@57600 --plain
```

Expected: the same flight as SITL. The line is configured with `stty` (raw, no
echo) before use, so a port that is already open elsewhere fails with a named
error rather than corrupting frames.

## What to report back

* the exact command, the exit code, and the first error line if any;
* for a Pi: `ls /sys/class/gpio` and `ls /dev/gpiochip*` so the interface is known;
* for a serial FC: baud rate and firmware (ArduPilot/PX4) of the autopilot.
