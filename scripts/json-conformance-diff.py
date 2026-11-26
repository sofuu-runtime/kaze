#!/usr/bin/env python3
"""json-conformance-diff.py -- check the Kaze JSON library against CPython's json.

The library's own suite states expectations that were written by hand. This script adds a
second opinion from an implementation nobody here wrote: it generates documents with
CPython, hands them to the Kaze parser, and compares both verdicts and values.

    python3 scripts/json-conformance-diff.py

Exits 0 when every generated document parses to a value equal to CPython's, every
re-serialisation reads back equal in CPython, and every malformed document is refused by
both.
"""
import json
import os
import random
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KAZE = os.path.join(ROOT, "bin", "kaze")
HOST = os.path.join(ROOT, "bin", "kaze-host")

SEED = 20261124
WORDS = ["", "a", "alt", "quote\"inside", "back\\slash", "tab\there", "line\nfeed",
         "\x01ctrl", "del\x7f", "unicode\u00e9", "\u4e16\u754c", "\U0001f600",
         "0123456789" * 3]


def gen_value(rng, depth):
    pick = rng.randint(0, 9 if depth < 4 else 5)
    if pick == 0:
        return None
    if pick == 1:
        return rng.choice([True, False])
    if pick == 2:
        return rng.randint(-10 ** 9, 10 ** 9)
    if pick == 3:
        return rng.choice([0.0, -0.5, 1.5, 1.0 / 3.0, 1e300, 5e-324, 1234.5678,
                           rng.uniform(-1e6, 1e6)])
    if pick == 4:
        return rng.choice(WORDS)
    if pick == 5:
        return rng.randint(0, 2 ** 53 - 1)
    if pick == 6:
        return [gen_value(rng, depth + 1) for _ in range(rng.randint(0, 4))]
    if pick == 7:
        return {}
    if pick == 8:
        return {rng.choice(WORDS) or "k": gen_value(rng, depth + 1)
                for _ in range(rng.randint(1, 4))}
    return "".join(rng.choice("abc \t\r\n\"\\/\x01") for _ in range(rng.randint(0, 8)))


def kaze_literal(text):
    out = text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")
    out = out.replace("\t", "\\t").replace("\r", "\\r")
    return '"' + out + '"'


MALFORMED = [
    "", " ", "\t\n", "{", "[", "]", "}", '{"a"}', '{"a":}', '{"a":1,}', "[1,]", "[,1]",
    "{,}", "01", "-01", "+1", ".5", "1.", "1e", "1e+", "--1", "00", "1 2", "tru", "nul",
    "True", "NULL", "'a'", "{'a':1}", "{\"a\":1}x", "nulll", "truex", "0x1F", "Infinity",
    "-Infinity", "NaN", "1_000", "[1 2]", "{\"a\" 1}", "{\"a\":1 \"b\":2}",
    "\"unterminated", "\"bad\\qescape\"", "\"\\u12\"", "\"\\uZZZZ\"", "\"tab\there\"",
    "\xef\xbb\xbf{}", "//comment\n1", "[1,2,3", "{\"a\":[1,2}",
]

# Deliberate divergence, not a disagreement. A literal that overflows a double has no
# JSON value (RFC 8259 numbers are IEEE 754 doubles), CPython parses 1e400 to infinity and
# then cannot serialise it as JSON at all, and the Kaze parser refuses it with E_JSON_EXP.
EXPECTED_DIVERGENCE = ["1e400", "-1e400", "1e999999999999999999"]


def main() -> int:
    rng = random.Random(SEED)
    values = [gen_value(rng, 0) for _ in range(220)]
    docs = [json.dumps(v, ensure_ascii=True, allow_nan=False) for v in values]

    lines = ["needs: log", "", 'use "./../lib/json.kaze"', "", "action main() -> num:"]
    # Kaze has no inner scopes, so every binding in root needs its own name.
    for n, d in enumerate(docs + MALFORMED + EXPECTED_DIVERGENCE):
        name = "jd" + str(n)
        lines.append("    let " + name + " = json_parse(" + kaze_literal(d) + ")")
        lines.append("    if " + name + ".ok:")
        lines.append('        log.info("ACCEPT " + json_stringify(' + name + '.val))')
        lines.append("    otherwise:")
        lines.append('        log.info("REJECT " + ' + name + '.err)')
    lines.append("    return 0")
    program = "\n".join(lines) + "\n"

    with tempfile.TemporaryDirectory() as tmp:
        src = os.path.join(tmp, "json_diff.kaze")
        prog_text = program.replace('use "./../lib/json.kaze"', f'use "{os.path.join(ROOT, "lib", "json.kaze")}"')
        with open(src, "w", encoding="utf-8") as fh:
            fh.write(prog_text)
        run = subprocess.run([HOST, "run", src, "--ticks", "8"], capture_output=True, text=True)
    if run.returncode != 0:
        print("json-diff: the program failed\n" + run.stdout[-2000:] + run.stderr[-2000:], file=sys.stderr)
        return 2

    verdicts = []
    for line in run.stdout.splitlines():
        if line.startswith("[info] "):
            verdicts.append(line[7:])
    expected = len(docs) + len(MALFORMED) + len(EXPECTED_DIVERGENCE)
    if len(verdicts) != expected:
        print("json-diff: expected %d verdicts, got %d" % (expected, len(verdicts)), file=sys.stderr)
        return 2

    bad = 0
    for i, value in enumerate(values):
        got = verdicts[i]
        if not got.startswith("ACCEPT"):
            bad += 1
            print("FAIL  accepted by CPython, refused by Kaze: %r -> %s" % (docs[i][:80], got[:80]))
            continue
        rendered = got[7:]
        try:
            back = json.loads(rendered)
        except Exception as exc:                      # noqa: BLE001 - report anything
            bad += 1
            print("FAIL  Kaze emitted text CPython cannot read: %r (%s)" % (rendered[:80], exc))
            continue
        if not same(value, back):
            bad += 1
            print("FAIL  round trip changed the value: %r -> %r" % (docs[i][:80], rendered[:80]))

    for k, doc in enumerate(EXPECTED_DIVERGENCE):
        got = verdicts[len(docs) + len(MALFORMED) + k]
        if got.startswith("ACCEPT"):
            bad += 1
            print("FAIL  %r should have been refused as out of range, got %s" % (doc, got[:60]))

    for j, doc in enumerate(MALFORMED):
        got = verdicts[len(docs) + j]
        try:
            json.loads(doc, parse_constant=raise_constant)
            ref = "ACCEPT"
        except Exception:                              # noqa: BLE001 - a refusal either way
            ref = "REJECT"
        if ref != ("ACCEPT" if got.startswith("ACCEPT") else "REJECT"):
            bad += 1
            print("FAIL  verdict differs for %r: CPython %s, Kaze %s" % (doc[:60], ref, got[:60]))

    print()
    print("json-diff: %d generated documents, %d malformed, %d expected divergences, %d disagreements (seed %d)"
          % (len(docs), len(MALFORMED), len(EXPECTED_DIVERGENCE), bad, SEED))
    return 1 if bad else 0


def raise_constant(name):
    raise ValueError(name)


def same(a, b):
    """Structural equality that also pins numbers to the same double."""
    if isinstance(a, bool) or isinstance(b, bool):
        return a is b
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return float(a) == float(b)
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    if isinstance(a, dict) and isinstance(b, dict):
        return list(a.keys()) == list(b.keys()) and all(same(a[k], b[k]) for k in a)
    return a == b


if __name__ == "__main__":
    sys.exit(main())
