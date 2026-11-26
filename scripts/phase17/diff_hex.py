#!/usr/bin/env python3
"""diff_hex.py -- check lib/hex.kaze against CPython's binascii.

Generates inputs with CPython's random module, embeds them in a small Kaze
harness, builds the harness through the real native pipeline (kaze emit-c + cc
against dist/), runs it, and compares every result with CPython's own encoder
and decoder.  The Kaze side never computes an expected value; every expected
value here comes from binascii.

    python3 scripts/phase17/diff_hex.py            # native build
    KAZE_ASAN=1 python3 scripts/phase17/diff_hex.py # + ASan/UBSan

With KAZE_ASAN=1 the harness is compiled with -fsanitize=address,undefined
and the run counts as failed when the sanitized process fails, the same rule
scripts/stdlib-one.sh uses for the library suites.

Exit status is nonzero on any disagreement, on a harness that fails to build or
run, and on a missing summary line.  The last line is
"diff hex: <N> cases, 0 mismatches".
"""
import binascii
import os
import random
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# KAZE_BIN lets a build tree point the script at a specific compiler; the
# default is the one bin/ ships.
KAZE = os.environ.get("KAZE_BIN") or os.path.join(ROOT, "bin", "kaze")
DIST = os.path.join(ROOT, "dist")
LIB = os.path.join(ROOT, "lib", "hex.kaze")
CC = os.environ.get("CC", "cc")
SEED = 20261125

MUTATION_CHARS = "0123456789abcdefABCDEFgx -_"


def kaze_text(text):
    """A Kaze string literal for an ASCII text."""
    out = []
    for ch in text:
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        elif ch == "\r":
            out.append("\\r")
        else:
            out.append(ch)
    return '"' + "".join(out) + '"'


def byte_list(data):
    return "[" + ", ".join(str(b) for b in data) + "]"


def harness(cases):
    lines = [
        "needs: log",
        "",
        'use "' + LIB + '"',
        "",
        "# Independent byte-list dumper: the harness prints what the library decoded",
        "# without using the library to render it.",
        "action dump(bytes) -> text:",
        "    let out = \"\"",
        "    let i = 0",
        "    let n = length of bytes",
        "    while i < n bounded 100000:",
        "        let v = byte_at(bytes, i) & 255",
        "        set out to out + digit(v >> 4) + digit(v & 15)",
        "        set i to i + 1",
        "    return out",
        "",
        "action digit(v) -> text:",
        "    if v < 10:",
        "        return character(48 + v)",
        "    return character(87 + v)",
        "",
        "action show_text(t) -> num:",
        '    log.info("T " + hex_encode(t))',
        "    return 0",
        "",
        "action show_bytes(xs) -> num:",
        '    log.info("B " + hex_encode_bytes(xs))',
        "    return 0",
        "",
        "action show_byte(v) -> num:",
        '    log.info("Y " + hex_encode_byte(v))',
        "    return 0",
        "",
        "action show_decode(t) -> num:",
        "    let r = hex_decode(t)",
        "    if r.ok:",
        '        log.info("D 1 " + dump(r.bytes))',
        "    otherwise:",
        '        log.info("D 0 " + r.err + " " + ("" + r.at))',
        "    return 0",
        "",
        "action main() -> num:",
    ]
    for kind, arg in cases:
        lines.append("    show_" + kind + "(" + arg + ")")
    lines.append("    return 0")
    return "\n".join(lines) + "\n"


def hex_reference(s):
    """The documented contract for hex_decode, applied to CPython's verdict.

    binascii decides whether the spelling is valid and what bytes it means; the
    error code and offset follow the module contract (length first, then the
    first byte outside the alphabet).  A divergence here is a contract bug.
    """
    raw = s.encode("utf-8")
    if len(raw) % 2 != 0:
        return (False, None, "E_HEX_LENGTH", len(raw))
    try:
        data = binascii.unhexlify(raw)
    except (binascii.Error, ValueError):
        for i, byte in enumerate(raw):
            if byte not in b"0123456789abcdefABCDEF":
                return (False, None, "E_HEX_CHARACTER", i)
        return (False, None, "E_HEX_CHARACTER", 0)
    return (True, data, "", 0)


def build_and_run(cases, workdir):
    src = os.path.join(workdir, "harness.kaze")
    with open(src, "w", encoding="utf-8") as fh:
        fh.write(harness(cases))

    check = subprocess.run([KAZE, "check", src], capture_output=True, text=True)
    if "E_KAZE" in check.stdout + check.stderr:
        print("diff hex: the harness does not check\n" + check.stdout + check.stderr, file=sys.stderr)
        return None

    cpath = os.path.join(workdir, "harness.c")
    with open(cpath, "w", encoding="utf-8") as fh:
        emit = subprocess.run([KAZE, "emit-c", src], stdout=fh, stderr=subprocess.PIPE, text=True)
    if emit.returncode != 0:
        print("diff hex: emit-c failed\n" + emit.stderr[-2000:], file=sys.stderr)
        return None

    exe = os.path.join(workdir, "harness")
    cmd = [CC, "-std=c99", "-O2", "-w", "-I", DIST]
    if os.environ.get("KAZE_ASAN") == "1":
        cmd += ["-g", "-fsanitize=address,undefined", "-fno-omit-frame-pointer"]
    cmd += [cpath, os.path.join(DIST, "kaze_rt.c"), "-lm", "-o", exe]
    cc = subprocess.run(cmd, capture_output=True, text=True)
    if cc.returncode != 0:
        print("diff hex: the native build failed\n" + cc.stderr[-3000:], file=sys.stderr)
        return None

    env = dict(os.environ)
    if os.environ.get("KAZE_ASAN") == "1":
        env["ASAN_OPTIONS"] = "detect_leaks=0" if sys.platform == "darwin" else "detect_leaks=1"
    run = subprocess.run([exe], capture_output=True, text=True, env=env)
    if run.returncode != 0:
        print("diff hex: the harness exited %d\n%s\n%s" % (run.returncode, run.stdout[-3000:], run.stderr[-3000:]),
              file=sys.stderr)
        return None
    results = []
    for line in run.stdout.splitlines():
        if line.startswith("[info] "):
            results.append(line[len("[info] "):])
    return results


def main():
    rng = random.Random(SEED)
    cases = []
    expected = []

    # --- hex_encode over text: CPython bytes.hex() -------------------------
    alphabet = "abcXYZ019 ~_-.+%{}[]"
    for _ in range(80):
        n = rng.randint(0, 40)
        text = "".join(rng.choice(alphabet) for _ in range(n))
        cases.append(("text", kaze_text(text)))
        expected.append(("encode", text.encode("utf-8").hex()))
    cases.append(("text", kaze_text("")))
    expected.append(("encode", ""))

    # --- hex_encode_bytes over every byte value ---------------------------
    for _ in range(140):
        n = rng.randint(0, 64)
        data = bytes(rng.randrange(256) for _ in range(n))
        cases.append(("bytes", byte_list(data)))
        expected.append(("encode", data.hex()))

    # --- hex_encode_byte: CPython byte formatting -------------------------
    byte_cases = list(range(256)) + [0, 255, 128, 1]
    byte_expect = ["%02x" % b for b in byte_cases]
    for b in byte_cases:
        cases.append(("byte", str(b)))
    expected.extend(("byte", value) for value in byte_expect)
    # Contract cases: a value that is not a whole number in 0..255 is refused.
    for value in ["-1", "256", "3.5", "-0.5", "1000000"]:
        cases.append(("byte", value))
        expected.append(("byte", ""))

    # --- hex_decode of CPython's own encodings ----------------------------
    for _ in range(140):
        n = rng.randint(0, 64)
        data = bytes(rng.randrange(256) for _ in range(n))
        spelling = data.hex()
        if rng.randrange(2) == 1:
            spelling = spelling.upper()
        cases.append(("decode", kaze_text(spelling)))
        expected.append(("decode", (True, data, "", 0)))

    # --- hex_decode of malformed spellings, expected via the documented
    #     contract on top of binascii's verdict ----------------------------
    malformed = ["6", "666", "g", "gg", "6g", "g6", "66 0", "0x66", "66 ", "-1",
                 "66\\n", "0X66", "6G", "66_00", "abc", "abcd ", " 66", "\t66"]
    for spelling in malformed:
        spelling = spelling.encode().decode("unicode_escape")
        cases.append(("decode", kaze_text(spelling)))
        expected.append(("decode", hex_reference(spelling)))

    # --- mutation fuzz: one to three edits of a canonical spelling --------
    for _ in range(120):
        data = bytes(rng.randrange(256) for _ in range(rng.randint(0, 12)))
        chars = list(data.hex())
        for _ in range(rng.randint(1, 3)):
            op = rng.randrange(3)
            if op == 0 and chars:
                chars[rng.randrange(len(chars))] = rng.choice(MUTATION_CHARS)
            elif op == 1:
                chars.insert(rng.randrange(len(chars) + 1), rng.choice(MUTATION_CHARS))
            elif chars:
                del chars[rng.randrange(len(chars))]
        spelling = "".join(chars)
        cases.append(("decode", kaze_text(spelling)))
        expected.append(("decode", hex_reference(spelling)))

    with tempfile.TemporaryDirectory(prefix="kaze-diff-hex-") as workdir:
        results = build_and_run(cases, workdir)
    if results is None:
        return 2
    if len(results) != len(cases):
        print("diff hex: expected %d results, got %d" % (len(cases), len(results)), file=sys.stderr)
        return 2

    mismatches = 0
    for i, (line, exp) in enumerate(zip(results, expected)):
        kind = exp[0]
        if kind in ("encode", "byte"):
            want = exp[1]
            got = line.split(" ", 1)[1] if " " in line else ""
            if got != want:
                mismatches += 1
                print("FAIL case %d (%s): expected %r, got %r" % (i, kind, want[:80], got[:80]))
        else:
            ok, data, err, at = exp[1]
            parts = line.split(" ")
            if ok:
                got = parts[2] if len(parts) > 2 else ""
                if parts[0:2] != ["D", "1"] or got != data.hex():
                    mismatches += 1
                    print("FAIL case %d (decode): expected ok %r, got %r" % (i, data.hex()[:80], line[:80]))
            else:
                got_err = parts[2] if len(parts) > 2 else ""
                got_at = parts[3] if len(parts) > 3 else ""
                if parts[0] != "D" or parts[1] != "0" or got_err != err or got_at != str(at):
                    mismatches += 1
                    print("FAIL case %d (decode): expected %s at %d, got %r" % (i, err, at, line[:80]))

    print()
    print("diff hex: %d cases, %d mismatches" % (len(cases), mismatches))
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())