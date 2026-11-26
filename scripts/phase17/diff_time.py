#!/usr/bin/env python3
"""diff_time.py -- check lib/time.kaze against CPython's datetime.

Generates UTC timestamps and day numbers with CPython's random module, embeds
them in a small Kaze harness, builds that harness through the real native
pipeline (kaze emit-c + cc against dist/), runs it, and compares every answer
with CPython's own calendar arithmetic.  The Kaze side never computes an
expected value; every expected value comes from datetime, from timedelta
arithmetic, or from plain integer formatting.

    python3 scripts/phase17/diff_time.py            # native build
    KAZE_ASAN=1 python3 scripts/phase17/diff_time.py # + ASan/UBSan

With KAZE_ASAN=1 the harness is compiled with -fsanitize=address,undefined and
the run counts as failed when the sanitized process fails or writes anything
from a sanitizer, the same rule scripts/stdlib-one.sh uses for the library
suites.

time_monotonic_now and time_utc_now have no reference value.  The harness logs
two readings of each around every arithmetic case, and the script checks only
their documented contract: finite, monotonic and non-decreasing, nanosecond
scale, and a UTC reading inside a generous window around time.time().

Exit status is nonzero on any disagreement, on a harness that fails to build or
run, and on a missing summary line.  The last line is
"diff time: <N> cases, 0 mismatches".
"""
import calendar
import datetime
import math
import os
import random
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# KAZE_BIN lets a build tree point the script at a specific compiler; the
# default is the one bin/ ships.
KAZE = os.environ.get("KAZE_BIN") or os.path.join(ROOT, "bin", "kaze")
DIST = os.path.join(ROOT, "dist")
LIB = os.path.join(ROOT, "lib", "time.kaze")
CC = os.environ.get("CC", "cc")
SEED = 20261125

UTC = datetime.timezone.utc
EPOCH = datetime.datetime(1970, 1, 1, tzinfo=UTC)
EPOCH_DAY = datetime.date(1970, 1, 1)
# The widest day numbers CPython's date can name with timedelta arithmetic.
MIN_DAY = (datetime.date.min - EPOCH_DAY).days
MAX_DAY = (datetime.date.max - EPOCH_DAY).days
UTC_WINDOW_SECONDS = 600
SANITIZER_MARKERS = ("AddressSanitizer", "LeakSanitizer", "UndefinedBehaviorSanitizer",
                     "runtime error:", "SEGV")


def num_arg(value):
    """A Kaze number literal for a Python int or float."""
    return repr(value) if isinstance(value, float) else str(value)


def ts_of(year, month, day, hour=0, minute=0, second=0):
    """Seconds from the UNIX epoch to a UTC instant, from CPython's calendar."""
    return int((datetime.datetime(year, month, day, hour, minute, second, tzinfo=UTC)
                - EPOCH).total_seconds())


def iso_reference(ts):
    """CPython's UTC rendering: the reference for time_format_iso8601."""
    return datetime.datetime.fromtimestamp(ts, UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def civil_reference(days):
    """CPython's proleptic Gregorian date: the reference for time_civil_from_days."""
    day = EPOCH_DAY + datetime.timedelta(days=days)
    return "%d-%d-%d" % (day.year, day.month, day.day)


def harness(cases):
    lines = [
        "needs: log",
        "",
        'use "' + LIB + '"',
        "",
        "# Reader for the three civil fields: the harness prints what the library",
        "# computed without asking the library to render the date.",
        "action show_iso(ts) -> num:",
        '    log.info("I " + time_format_iso8601(ts))',
        "    return 0",
        "",
        "action show_civil(days) -> num:",
        "    let civil = time_civil_from_days(days)",
        "    let year = civil[0] otherwise 0",
        "    let month = civil[1] otherwise 0",
        "    let day = civil[2] otherwise 0",
        '    log.info("C " + year + "-" + month + "-" + day)',
        "    return 0",
        "",
        "action show_duration(start, end) -> num:",
        '    log.info("M " + time_duration_ms(start, end))',
        "    return 0",
        "",
        "action show_pads(n) -> num:",
        '    log.info("P " + n + " " + time_pad2(n) + " " + time_pad3(n) + " " + time_pad4(n))',
        "    return 0",
        "",
        "# The clock readings bracket every arithmetic case, so the contract is",
        "# checked across the whole run and not only between adjacent calls.",
        "action main() -> num:",
        "    let mono_start = time_monotonic_now()",
        "    let utc_start = time_utc_now()",
    ]
    for kind, arg in cases:
        lines.append("    show_" + kind + "(" + arg + ")")
    lines += [
        "    let mono_end = time_monotonic_now()",
        "    let utc_end = time_utc_now()",
        '    log.info("K " + mono_start + " " + utc_start + " " + mono_end + " " + utc_end'
        ' + " " + time_duration_ms(mono_start, mono_end))',
        "    return 0",
    ]
    return "\n".join(lines) + "\n"


def clock_verdict(line, before, after):
    """The documented clock contract, applied to the readings the harness logged."""
    parts = line.split(" ")
    if len(parts) != 6 or parts[0] != "K":
        return [("the clock line is malformed", False)]
    try:
        mono1, utc1, mono2, utc2, span = (float(part) for part in parts[1:])
    except ValueError:
        return [("the clock readings are numbers", False)]
    return [
        ("monotonic readings are finite", math.isfinite(mono1) and math.isfinite(mono2)),
        ("monotonic clock does not go backwards", mono2 >= mono1),
        ("monotonic clock is nanosecond scale", mono1 >= 1000000 and mono2 >= 1000000),
        ("utc readings are finite", math.isfinite(utc1) and math.isfinite(utc2)),
        ("utc readings sit inside a generous window of time.time()",
         before - UTC_WINDOW_SECONDS <= utc1 <= after + UTC_WINDOW_SECONDS and
         before - UTC_WINDOW_SECONDS <= utc2 <= after + UTC_WINDOW_SECONDS),
        ("duration over the live clock readings", span == mono2 - mono1),
    ]


def build_and_run(cases, workdir):
    src = os.path.join(workdir, "harness.kaze")
    with open(src, "w", encoding="utf-8") as fh:
        fh.write(harness(cases))

    check = subprocess.run([KAZE, "check", src], capture_output=True, text=True)
    if "E_KAZE" in check.stdout + check.stderr:
        print("diff time: the harness does not check\n" + check.stdout + check.stderr, file=sys.stderr)
        return None

    cpath = os.path.join(workdir, "harness.c")
    with open(cpath, "w", encoding="utf-8") as fh:
        emit = subprocess.run([KAZE, "emit-c", src], stdout=fh, stderr=subprocess.PIPE, text=True)
    if emit.returncode != 0:
        print("diff time: emit-c failed\n" + emit.stderr[-2000:], file=sys.stderr)
        return None

    asan = os.environ.get("KAZE_ASAN") == "1"
    exe = os.path.join(workdir, "harness")
    cmd = [CC, "-std=c99", "-O2", "-w", "-I", DIST]
    if asan:
        cmd += ["-g", "-fsanitize=address,undefined", "-fno-omit-frame-pointer"]
    cmd += [cpath, os.path.join(DIST, "kaze_rt.c"), "-lm", "-o", exe]
    cc = subprocess.run(cmd, capture_output=True, text=True)
    if cc.returncode != 0:
        print("diff time: the native build failed\n" + cc.stderr[-3000:], file=sys.stderr)
        return None

    env = dict(os.environ)
    if asan:
        env["ASAN_OPTIONS"] = "detect_leaks=0" if sys.platform == "darwin" else "detect_leaks=1"
    before = time.time()
    run = subprocess.run([exe], capture_output=True, text=True, env=env)
    after = time.time()
    # A sanitizer report is a failure even when the process still exits zero:
    # UBSan without -fno-sanitize-recover prints and carries on.
    if asan:
        reported = [marker for marker in SANITIZER_MARKERS if marker in run.stderr]
        if reported:
            print("diff time: the sanitizer reported %s\n%s" % (", ".join(reported), run.stderr[-3000:]),
                  file=sys.stderr)
            return None
    if run.returncode != 0:
        print("diff time: the harness exited %d\n%s\n%s" % (run.returncode, run.stdout[-3000:], run.stderr[-3000:]),
              file=sys.stderr)
        return None
    results = []
    for line in run.stdout.splitlines():
        if line.startswith("[info] "):
            results.append(line[len("[info] "):])
    return results, before, after


def main():
    rng = random.Random(SEED)
    cases = []
    expected = []

    # --- ISO 8601 over UTC timestamps: CPython datetime is the reference ---
    stamps = [0, 1, 2, 59, 60, 61, 3599, 3600, 3601, 86399, 86400, 86401]
    # Leap days and the days on either side of them.
    for year in (1972, 1996, 2000, 2024, 2048, 2096):
        stamps += [ts_of(year, 2, 28, 23, 59, 59), ts_of(year, 2, 29),
                   ts_of(year, 2, 29, 23, 59, 59), ts_of(year, 3, 1)]
    # 2100 is divisible by 100 but not by 400: its February has 28 days.
    stamps += [ts_of(2100, 2, 28), ts_of(2100, 2, 28, 23, 59, 59), ts_of(2100, 3, 1)]
    # Month ends and year ends across the years the module has to spell.
    for year in (1970, 1999, 2000, 2023, 2024, 2037, 2038, 2099, 2100):
        for month in range(1, 13):
            last = calendar.monthrange(year, month)[1]
            stamps.append(ts_of(year, month, last, 23, 59, 59))
            stamps.append(ts_of(year, month, 1))
        stamps += [ts_of(year, 1, 1), ts_of(year, 12, 31, 23, 59, 59)]
    # The 2038 int32 rollover and the storage boundaries around it.
    stamps += [2147483646, 2147483647, 2147483648, 2147483649, 4294967295, 4294967296]
    # The widest year a four-digit pad can render.
    stamps += [253402300799]
    # A few thousand random timestamps over the whole [0, 4102444800] range.
    stamps += [rng.randrange(4102444801) for _ in range(3000)]
    # And day boundaries, where the hour/minute/second split turns over.
    for _ in range(400):
        day = rng.randrange(4102444800 // 86400 + 1)
        stamps.append(day * 86400 + rng.choice([0, 1, 3599, 3600, 86399, rng.randrange(86400)]))
    for ts in stamps:
        cases.append(("iso", str(ts)))
        expected.append(("iso", iso_reference(ts)))

    # --- civil_from_days: CPython date arithmetic is the reference ---------
    days = [0, 1, 2, 58, 59, 60, 364, 365, 366, 730, 1460, 1461,
            -1, -2, -365, -366, MIN_DAY, MIN_DAY + 1, MAX_DAY - 1, MAX_DAY]
    # Era boundaries of the 400-year Gregorian cycle.
    for era in (0, 1, 4, 5, 20):
        days += [era * 146097 - 1, era * 146097, era * 146097 + 1]
    # Leap days and the century rule, as day numbers.
    for year, month, day in ((2000, 2, 28), (2000, 2, 29), (2000, 3, 1),
                             (2024, 2, 29), (2100, 2, 28), (2100, 3, 1),
                             (9999, 12, 31)):
        offset = (datetime.date(year, month, day) - EPOCH_DAY).days
        days += [offset - 1, offset, offset + 1]
    # A few thousand day numbers across the whole datetime.date range.
    days += [rng.randrange(MIN_DAY, MAX_DAY + 1) for _ in range(3000)]
    # Offsets next to date.min and date.max fall off CPython's range; CPython
    # cannot state a reference there, so the differential stops at its edge.
    days = [d for d in days if MIN_DAY <= d <= MAX_DAY]
    for d in days:
        cases.append(("civil", str(d)))
        expected.append(("civil", civil_reference(d)))

    # --- duration_ms: plain arithmetic is the reference -------------------
    pairs = [(0, 0), (0, 1), (1, 0), (1000, 2500), (2500, 1000), (0, -1500),
             (-1500, 0), (0, 86400000), (86400000, 0), (1, 4102444800),
             (4102444800, 1), (2147483647, 2147483648),
             (1.5, 2.75), (0.1, 0.2), (0.2, 0.1), (-0.5, 0.5), (1234.25, 9876.5)]
    for _ in range(200):
        shape = rng.randrange(3)
        if shape == 0:
            start, end = rng.randrange(-10 ** 6, 10 ** 6), rng.randrange(-10 ** 6, 10 ** 6)
        elif shape == 1:
            start, end = rng.randrange(4102444801), rng.randrange(4102444801)
        else:
            start = rng.randrange(-10 ** 7, 10 ** 7) / 4.0
            end = rng.randrange(-10 ** 7, 10 ** 7) / 4.0
        pairs.append((start, end))
    for start, end in pairs:
        cases.append(("duration", num_arg(start) + ", " + num_arg(end)))
        expected.append(("duration", float(end - start)))

    # --- pad helpers: plain zero padding is the reference -----------------
    for n in range(0, 10001):
        cases.append(("pads", str(n)))
        expected.append(("pads", (str(n), "%02d" % n, "%03d" % n, "%04d" % n)))

    with tempfile.TemporaryDirectory(prefix="kaze-diff-time-") as workdir:
        built = build_and_run(cases, workdir)
    if built is None:
        return 2
    results, before, after = built
    if len(results) != len(cases) + 1:
        print("diff time: expected %d results, got %d" % (len(cases) + 1, len(results)), file=sys.stderr)
        return 2

    clock = clock_verdict(results[-1], before, after)
    mismatches = 0
    for i, (line, exp) in enumerate(zip(results[:-1], expected)):
        kind, want = exp
        if kind in ("iso", "civil"):
            tag = "I" if kind == "iso" else "C"
            got = line[len(tag) + 1:] if line.startswith(tag + " ") else ""
            if got != want:
                mismatches += 1
                print("FAIL case %d (%s): expected %r, got %r" % (i, kind, want, got))
        elif kind == "duration":
            parts = line.split(" ")
            got = None
            try:
                got = float(parts[1])
            except (IndexError, ValueError):
                pass
            if parts[0] != "M" or got != want:
                mismatches += 1
                print("FAIL case %d (duration): expected %r, got %r" % (i, want, line[:80]))
        else:
            parts = line.split(" ")
            if parts[0] != "P" or tuple(parts[1:5]) != want:
                mismatches += 1
                print("FAIL case %d (pads): expected %r, got %r" % (i, want, line[:80]))

    for label, ok in clock:
        if not ok:
            mismatches += 1
            print("FAIL clock: %s" % label)

    print()
    print("diff time: %d cases, %d mismatches" % (len(cases) + len(clock), mismatches))
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
