#!/usr/bin/env python3
"""diff_base64.py -- check lib/base64.kaze against CPython's base64 module.

Generates payloads and spellings with CPython's random module, embeds them in a
small Kaze harness, builds that harness through the real native pipeline
(kaze emit-c + cc against dist/), and compares every answer with CPython's own
encoder and decoder.

The strict decoder contract is also a reference implementation here: the module
documents exactly which malformed shapes it refuses and with which code and
offset, so the script applies that contract to the same byte strings CPython
sees, and cross-checks the contract against CPython for the standard alphabet
(where CPython's validate=True implements the same accept/reject set, including
its acceptance of non-zero pad bits).

    python3 scripts/phase17/diff_base64.py
    KAZE_ASAN=1 python3 scripts/phase17/diff_base64.py

With KAZE_ASAN=1 the harness is compiled with -fsanitize=address,undefined
and the run counts as failed when the sanitized process fails, the same rule
scripts/stdlib-one.sh uses for the library suites.

Exit status is nonzero on any disagreement, on a harness that fails to build or
run, and on a missing summary line.  The last line is
"diff base64: <N> cases, 0 mismatches".
"""
import base64
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
LIB = os.path.join(ROOT, "lib", "base64.kaze")
CC = os.environ.get("CC", "cc")
SEED = 20261125

STD_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
URL_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
MUTATION_CHARS = STD_ALPHABET + "=$-_. 	"


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
        "# Independent byte-list dumper: the harness prints decoded bytes without",
        "# asking the library under test to render them.",
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
        "action show_std_text(t) -> num:",
        '    log.info("T " + base64_encode(t))',
        "    return 0",
        "",
        "action show_url_text(t) -> num:",
        '    log.info("U " + base64_url_encode(t))',
        "    return 0",
        "",
        "action show_std_bytes(xs) -> num:",
        '    log.info("B " + base64_encode_bytes(xs))',
        "    return 0",
        "",
        "action show_url_bytes(xs) -> num:",
        '    log.info("C " + base64_url_encode_bytes(xs))',
        "    return 0",
        "",
        "action show_std_decode(t) -> num:",
        "    let r = base64_decode(t)",
        "    if r.ok:",
        '        log.info("D 1 " + dump(r.bytes))',
        "    otherwise:",
        '        log.info("D 0 " + r.err + " " + ("" + r.at))',
        "    return 0",
        "",
        "action show_url_decode(t) -> num:",
        "    let r = base64_url_decode(t)",
        "    if r.ok:",
        '        log.info("V 1 " + dump(r.bytes))',
        "    otherwise:",
        '        log.info("V 0 " + r.err + " " + ("" + r.at))',
        "    return 0",
        "",
        "action main() -> num:",
    ]
    for kind, arg in cases:
        lines.append("    show_" + kind + "(" + arg + ")")
    lines.append("    return 0")
    return "\n".join(lines) + "\n"


def decode_std_model(raw):
    """The module's documented strict standard decoder, on bytes -> (ok, data, err, at)."""
    if len(raw) == 0:
        return (True, b"", "", 0)
    if len(raw) % 4 != 0:
        return (False, None, "E_BASE64_LENGTH", len(raw))
    pad = 0
    for k in range(min(4, len(raw))):
        if raw[len(raw) - 1 - k] == 0x3D:
            pad += 1
    if pad > 2:
        return (False, None, "E_BASE64_PADDING", len(raw) - pad)
    data_len = len(raw) - pad
    if data_len == 0:
        return (False, None, "E_BASE64_PADDING", 0)
    dr = data_len % 4
    if pad > 0 and dr != 4 - pad:
        return (False, None, "E_BASE64_PADDING", data_len)
    if dr == 1:
        return (False, None, "E_BASE64_LENGTH", len(raw))
    for i in range(data_len):
        b = raw[i]
        if b == 0x3D:
            return (False, None, "E_BASE64_PADDING", i)
        if chr(b) not in STD_ALPHABET:
            return (False, None, "E_BASE64_CHARACTER", i)
    return (True, base64.b64decode(raw, validate=True), "", 0)


def decode_url_model(raw):
    """The module's documented URL-safe decoder (padding optional)."""
    if len(raw) == 0:
        return (True, b"", "", 0)
    if len(raw) % 4 == 1:
        return (False, None, "E_BASE64_LENGTH", len(raw))
    pad = 0
    for k in range(min(4, len(raw))):
        if raw[len(raw) - 1 - k] == 0x3D:
            pad += 1
    if pad > 2:
        return (False, None, "E_BASE64_PADDING", len(raw) - pad)
    data_len = len(raw) - pad
    if data_len == 0:
        return (False, None, "E_BASE64_PADDING", 0)
    dr = data_len % 4
    if pad > 0 and dr != 4 - pad:
        return (False, None, "E_BASE64_PADDING", data_len)
    if dr == 1:
        return (False, None, "E_BASE64_LENGTH", len(raw))
    for i in range(data_len):
        b = raw[i]
        if b == 0x3D:
            return (False, None, "E_BASE64_PADDING", i)
        if chr(b) not in URL_ALPHABET:
            return (False, None, "E_BASE64_CHARACTER", i)
    padded = raw + b"=" * ((4 - len(raw) % 4) % 4)
    return (True, base64.b64decode(padded, altchars=b"-_", validate=True), "", 0)


def std_reference(raw):
    """CPython's verdict for a standard-alphabet spelling: (ok, data)."""
    try:
        return (True, base64.b64decode(raw, validate=True))
    except Exception:
        return (False, None)


def url_reference(raw):
    """CPython's verdict after restoring the optional padding."""
    padded = raw + b"=" * ((4 - len(raw) % 4) % 4)
    try:
        return (True, base64.b64decode(padded, altchars=b"-_", validate=True))
    except Exception:
        return (False, None)


def build_and_run(cases, workdir):
    src = os.path.join(workdir, "harness.kaze")
    with open(src, "w", encoding="utf-8") as fh:
        fh.write(harness(cases))
    bundle = subprocess.run(
        [sys.executable, os.path.join(ROOT, "scripts", "bundle-imports.py"), src],
        capture_output=True, text=True)
    if bundle.returncode != 0:
        print("diff base64: bundling the harness failed\n" + bundle.stderr, file=sys.stderr)
        return None
    flat = os.path.join(workdir, "flat.kaze")
    with open(flat, "w", encoding="utf-8") as fh:
        fh.write(bundle.stdout)
    check = subprocess.run([KAZE, "check", flat], capture_output=True, text=True)
    if "E_KAZE" in check.stdout + check.stderr:
        print("diff base64: the harness does not check\n" + check.stdout + check.stderr, file=sys.stderr)
        return None
    cpath = os.path.join(workdir, "harness.c")
    with open(cpath, "w", encoding="utf-8") as fh:
        emit = subprocess.run([KAZE, "emit-c", flat], stdout=fh, stderr=subprocess.PIPE, text=True)
    if emit.returncode != 0:
        print("diff base64: emit-c failed\n" + emit.stderr[-2000:], file=sys.stderr)
        return None
    exe = os.path.join(workdir, "harness")
    cmd = [CC, "-std=c99", "-O2", "-w", "-I", DIST]
    if os.environ.get("KAZE_ASAN") == "1":
        cmd += ["-g", "-fsanitize=address,undefined", "-fno-omit-frame-pointer"]
    cmd += [cpath, os.path.join(DIST, "kaze_rt.c"), "-lm", "-o", exe]
    cc = subprocess.run(cmd, capture_output=True, text=True)
    if cc.returncode != 0:
        print("diff base64: the native build failed\n" + cc.stderr[-3000:], file=sys.stderr)
        return None
    env = dict(os.environ)
    if os.environ.get("KAZE_ASAN") == "1":
        env["ASAN_OPTIONS"] = "detect_leaks=0" if sys.platform == "darwin" else "detect_leaks=1"
    run = subprocess.run([exe], capture_output=True, text=True, env=env)
    if run.returncode != 0:
        print("diff base64: the harness exited %d\n%s\n%s" % (run.returncode, run.stdout[-3000:], run.stderr[-3000:]),
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
    # Inputs CPython's validate=True tolerates but the module's documented strict
    # contract refuses: a length that is not a multiple of four, which CPython
    # completes with implicit padding. They are pinned, not ignored.
    strict_pins = []

    # --- encoders: CPython is the reference -------------------------------
    payloads = [bytes(rng.randrange(256) for _ in range(rng.randint(0, 96))) for _ in range(160)]
    for data in payloads:
        cases.append(("std_bytes", byte_list(data)))
        expected.append(("std_enc", base64.b64encode(data).decode("ascii")))
        cases.append(("url_bytes", byte_list(data)))
        expected.append(("url_enc", base64.urlsafe_b64encode(data).decode("ascii").rstrip("=")))
    ascii_payloads = ["".join(rng.choice("abcXYZ019 ~_-.+%/\\n") for _ in range(rng.randint(0, 40)))
                      for _ in range(64)]
    for text in ascii_payloads:
        cases.append(("std_text", kaze_text(text)))
        expected.append(("std_enc", base64.b64encode(text.encode("utf-8")).decode("ascii")))
        cases.append(("url_text", kaze_text(text)))
        expected.append(("url_enc", base64.urlsafe_b64encode(text.encode("utf-8")).decode("ascii").rstrip("=")))

    # --- standard decoder: valid spellings --------------------------------
    for data in payloads:
        cases.append(("std_decode", kaze_text(base64.b64encode(data).decode("ascii"))))
        expected.append(("std_dec", (True, data, "", 0)))
    # Non-zero pad bits: CPython's validate=True accepts these and so must the
    # module (RFC 4648 3.5 permits either behaviour; the module documents this).
    for _ in range(40):
        n = rng.randint(1, 2)
        data = bytes(rng.randrange(256) for _ in range(n))
        canonical = base64.b64encode(data)
        chars = list(canonical.decode("ascii"))
        last = len(chars) - 1 - (2 if n == 1 else 1)
        sextet = STD_ALPHABET.index(chars[last])
        if n == 1:
            chars[last] = STD_ALPHABET[(sextet & 0x30) | rng.randrange(16)]
        else:
            chars[last] = STD_ALPHABET[(sextet & 0x3C) | rng.randrange(4)]
        spelling = "".join(chars)
        got = std_reference(spelling.encode("ascii"))
        if got[0]:
            cases.append(("std_decode", kaze_text(spelling)))
            expected.append(("std_dec", (True, got[1], "", 0)))

    # --- standard decoder: malformed and mutated spellings ----------------
    malformed_std = ["Z", "Zg", "Zg=", "Zg===", "====", "Z===", "Z=g=", "AB=C",
                     "AAAA====", "Zm9v====", "Zm$v", "Zm9!", "-_8=", "AA==A", "A=AA",
                     "AAAA=", "ZZZ=", "ZZZZZZ", " " "Zg==", "Zg== ", "Zg==\\n"]
    for spelling in malformed_std:
        raw = spelling.encode("ascii")
        ref = std_reference(raw)
        model = decode_std_model(raw)
        if ref[0] != model[0] or (ref[0] and ref[1] != model[1]):
            if model[0] or not ref[0] or len(raw) % 4 == 0:
                print("diff base64: reference model disagrees with CPython on %r" % spelling, file=sys.stderr)
                return 2
            strict_pins.append((spelling, ref[1]))
        cases.append(("std_decode", kaze_text(spelling)))
        expected.append(("std_dec", model))
    for _ in range(90):
        data = bytes(rng.randrange(256) for _ in range(rng.randint(0, 20)))
        chars = list(base64.b64encode(data).decode("ascii"))
        for _ in range(rng.randint(1, 3)):
            op = rng.randrange(3)
            if op == 0 and chars:
                chars[rng.randrange(len(chars))] = rng.choice(MUTATION_CHARS)
            elif op == 1:
                chars.insert(rng.randrange(len(chars) + 1), rng.choice(MUTATION_CHARS))
            elif chars:
                del chars[rng.randrange(len(chars))]
        spelling = "".join(chars)
        raw = spelling.encode("ascii")
        ref = std_reference(raw)
        model = decode_std_model(raw)
        if ref[0] != model[0] or (ref[0] and ref[1] != model[1]):
            if model[0] or not ref[0] or len(raw) % 4 == 0:
                print("diff base64: reference model disagrees with CPython on %r" % spelling, file=sys.stderr)
                return 2
            strict_pins.append((spelling, ref[1]))
        cases.append(("std_decode", kaze_text(spelling)))
        expected.append(("std_dec", model))

    # --- URL-safe decoder: valid padded and unpadded spellings ------------
    for data in payloads:
        padded = base64.urlsafe_b64encode(data).decode("ascii")
        unpadded = padded.rstrip("=")
        cases.append(("url_decode", kaze_text(padded)))
        expected.append(("url_dec", (True, data, "", 0)))
        cases.append(("url_decode", kaze_text(unpadded)))
        expected.append(("url_dec", (True, data, "", 0)))
    for _ in range(40):
        n = rng.randint(1, 2)
        data = bytes(rng.randrange(256) for _ in range(n))
        chars = list(base64.urlsafe_b64encode(data).decode("ascii"))
        last = len(chars) - 1 - (2 if n == 1 else 1)
        sextet = URL_ALPHABET.index(chars[last])
        if n == 1:
            chars[last] = URL_ALPHABET[(sextet & 0x30) | rng.randrange(16)]
        else:
            chars[last] = URL_ALPHABET[(sextet & 0x3C) | rng.randrange(4)]
        spelling = "".join(chars)
        got = url_reference(spelling.encode("ascii"))
        if got[0]:
            cases.append(("url_decode", kaze_text(spelling)))
            expected.append(("url_dec", (True, got[1], "", 0)))

    # --- URL-safe decoder: malformed and mutated spellings ----------------
    malformed_url = ["Z", "Zg=", "Zg===", "====", "AAAA==", "AAAA====", "+/8=", "Z$", "Zg!",
                     "=AAA", "A=AA", "ZZ=", "-_8===", "-w=", "AA A", " AA"]
    for spelling in malformed_url:
        raw = spelling.encode("ascii")
        model = decode_url_model(raw)
        if model[0]:
            print("diff base64: malformed url case %r was accepted by the model" % spelling, file=sys.stderr)
            return 2
        cases.append(("url_decode", kaze_text(spelling)))
        expected.append(("url_dec", model))
    for _ in range(90):
        data = bytes(rng.randrange(256) for _ in range(rng.randint(0, 20)))
        chars = list(base64.urlsafe_b64encode(data).decode("ascii").rstrip("="))
        for _ in range(rng.randint(1, 3)):
            op = rng.randrange(3)
            if op == 0 and chars:
                chars[rng.randrange(len(chars))] = rng.choice(MUTATION_CHARS)
            elif op == 1:
                chars.insert(rng.randrange(len(chars) + 1), rng.choice(MUTATION_CHARS))
            elif chars:
                del chars[rng.randrange(len(chars))]
        spelling = "".join(chars)
        raw = spelling.encode("ascii")
        model = decode_url_model(raw)
        if model[0]:
            ref = url_reference(raw)
            if not ref[0] or ref[1] != model[1]:
                print("diff base64: reference model disagrees with CPython on %r" % spelling, file=sys.stderr)
                return 2
        cases.append(("url_decode", kaze_text(spelling)))
        expected.append(("url_dec", model))

    with tempfile.TemporaryDirectory(prefix="kaze-diff-base64-") as workdir:
        results = build_and_run(cases, workdir)
    if results is None:
        return 2
    if len(results) != len(cases):
        print("diff base64: expected %d results, got %d" % (len(cases), len(results)), file=sys.stderr)
        return 2

    mismatches = 0
    for i, (line, exp) in enumerate(zip(results, expected)):
        kind = exp[0]
        if kind in ("std_enc", "url_enc"):
            want = exp[1]
            got = line.split(" ", 1)[1] if " " in line else ""
            if got != want:
                mismatches += 1
                print("FAIL case %d (%s): expected %r, got %r" % (i, kind, want[:80], got[:80]))
            continue
        ok, data, err, at = exp[1]
        parts = line.split(" ")
        if ok:
            got = parts[2] if len(parts) > 2 else ""
            if parts[0:2] != [("D" if kind == "std_dec" else "V"), "1"] or got != data.hex():
                mismatches += 1
                print("FAIL case %d (%s): expected ok %r, got %r" % (i, kind, data.hex()[:80], line[:80]))
        else:
            got_err = parts[2] if len(parts) > 2 else ""
            got_at = parts[3] if len(parts) > 3 else ""
            if parts[1:2] != ["0"] or got_err != err or got_at != str(at):
                mismatches += 1
                print("FAIL case %d (%s): expected %s at %d, got %r" % (i, kind, err, at, line[:80]))

    print()
    if strict_pins:
        print("strictness: %d inputs CPython's implicit padding tolerates but the documented"
              " RFC 4648 contract refuses (length not a multiple of four)" % len(strict_pins))
    print("diff base64: %d cases, %d mismatches" % (len(cases), mismatches))
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())