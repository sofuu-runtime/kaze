#!/usr/bin/env python3
"""diff_url.py -- check lib/url.kaze against CPython's urllib.parse.

Generates RFC 3986 URLs, percent-encoding workloads and query strings with
CPython's random module, embeds them in a small Kaze harness, builds that harness
through the real native pipeline (kaze emit-c + cc against dist/), runs it, and
compares every component, formatting, encoding, decoding and query answer with
urllib.parse.

The malformed-URL group is the module's documented strictness: CPython's parser
accepts almost any text, so those expectations are pinned by the module contract
(they are reported as a separate count, and the cases CPython itself rejects are
cross-checked).  Everything else is CPython's answer, not the module's.

    python3 scripts/phase17/diff_url.py
    KAZE_ASAN=1 python3 scripts/phase17/diff_url.py

With KAZE_ASAN=1 the harness is compiled with -fsanitize=address,undefined
and the run counts as failed when the sanitized process fails, the same rule
scripts/stdlib-one.sh uses for the library suites.

Exit status is nonzero on any disagreement, on a harness that fails to build or
run, and on a missing summary line.  The last line is
"diff url: <N> cases, 0 mismatches".
"""
import os
import random
import subprocess
import sys
import tempfile
import urllib.parse

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# KAZE_BIN lets a build tree point the script at a specific compiler; the
# default is the one bin/ ships.
KAZE = os.environ.get("KAZE_BIN") or os.path.join(ROOT, "bin", "kaze")
DIST = os.path.join(ROOT, "dist")
LIB = os.path.join(ROOT, "lib", "url.kaze")
CC = os.environ.get("CC", "cc")
SEED = 20261125

SCHEMES = ["http", "https", "ftp"]
HOSTS = ["example.com", "a.b.c", "host-1", "xn--bcher-kva", "127.0.0.1", "localhost",
         "api.example.org", "sub.domain.test"]
PATH_PIECES = ["abc", "a%20b", "p;x=1", "a:b@c", "%7Euser", "v1.2", "x_y-z", "A9",
               "seg%2Fment", "name+tag", "k=v", "!$&'()*+,;="]
QUERY_PIECES = ["k=v", "a=1", "q=a%20b", "x=%3D", "empty=", "flag", "p=a+b",
                "u=%C3%A9", "n=1&n=2", "a:b=c@d"]
FRAGMENT_PIECES = ["frag", "f%20g", "a/b", "q?x", "sec-1"]

STRICT_MALFORMED = [
    ("http://user@h/", "E_URL_USERINFO"),
    ("http://user:pass@h/", "E_URL_USERINFO"),
    ("1http://x", "E_URL_SCHEME"),
    (":x", "E_URL_SCHEME"),
    ("http://h:abc/", "E_URL_PORT"),
    ("http://h:70000/", "E_URL_PORT"),
    ("http://h:65536/", "E_URL_PORT"),
    ("http://h/%zz", "E_URL_PERCENT"),
    ("http://h/a b", "E_URL_CHARACTER"),
    ("http://[::1", "E_URL_HOST"),
    ("http://[]/", "E_URL_HOST"),
    ("http://[zz]/", "E_URL_HOST"),
    ("http://[::1]x/", "E_URL_PORT"),
    ("http://h/[]", "E_URL_CHARACTER"),
    ("http://h/p#a#b", "E_URL_CHARACTER"),
    ("http://h/a%2", "E_URL_PERCENT"),
    ("http://h/p?q=%", "E_URL_PERCENT"),
    ("http://h/p#f%", "E_URL_PERCENT"),
    ("//h:bad/", "E_URL_PORT"),
    ("a b", "E_URL_CHARACTER"),
    ("http://h/<", "E_URL_CHARACTER"),
    ("http://h/p?q=<", "E_URL_CHARACTER"),
    ("http://h/p#f>", "E_URL_CHARACTER"),
]


def kaze_text(text):
    """A Kaze string literal for a text (source is UTF-8, escapes as needed)."""
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
        "# Independent dumpers: results are printed as hex over their bytes, never",
        "# through the library under test.",
        "action digit(v) -> text:",
        "    if v < 10:",
        "        return character(48 + v)",
        "    return character(87 + v)",
        "",
        "action dump(t) -> text:",
        "    let out = \"\"",
        "    let i = 0",
        "    let n = length of t",
        "    while i < n bounded 100000:",
        "        let v = byte_at(t, i) & 255",
        "        set out to out + digit(v >> 4) + digit(v & 15)",
        "        set i to i + 1",
        "    return out",
        "",
        "action dump_list(bytes) -> text:",
        "    let out = \"\"",
        "    let i = 0",
        "    let n = length of bytes",
        "    while i < n bounded 100000:",
        "        let v = byte_at(bytes, i) & 255",
        "        set out to out + digit(v >> 4) + digit(v & 15)",
        "        set i to i + 1",
        "    return out",
        "",
        "action bytes_text(xs) -> text:",
        "    let out = \"\"",
        "    let i = 0",
        "    let n = length of xs",
        "    while i < n bounded 100000:",
        "        set out to out + character(byte_at(xs, i) & 255)",
        "        set i to i + 1",
        "    return out",
        "",
        "action show_parse(u) -> num:",
        "    let p = url_parse(u)",
        "    if p.ok:",
        "        let auth = 0",
        "        if p.has_authority:",
        "            set auth to 1",
        "        log.info(\"P 1 \" + dump(p.scheme) + \" \" + dump(p.host) + \" \" + (\"\" + p.port) + \" \" + dump(p.path) + \" \" + dump(p.query) + \" \" + dump(p.fragment) + \" \" + (\"\" + auth))",
        "    otherwise:",
        "        log.info(\"P 0 \" + p.err + \" \" + (\"\" + p.at))",
        "    return 0",
        "",
        "action show_format(u) -> num:",
        "    let p = url_parse(u)",
        "    if p.ok:",
        "        log.info(\"F \" + dump(url_format(p)))",
        "    otherwise:",
        "        log.info(\"F !\")",
        "    return 0",
        "",
        "action show_encode(xs) -> num:",
        "    log.info(\"E \" + dump(url_encode(bytes_text(xs))))",
        "    return 0",
        "",
        "action show_decode(t) -> num:",
        "    log.info(\"D \" + dump_list(url_decode_bytes(t)))",
        "    return 0",
        "",
        "action show_get(q, k) -> num:",
        "    log.info(\"Q \" + dump(url_query_get(q, k)))",
        "    return 0",
        "",
        "action show_has(q, k) -> num:",
        "    let found = 0",
        "    if url_query_has(q, k):",
        "        set found to 1",
        "    log.info(\"H \" + (\"\" + found))",
        "    return 0",
        "",
        "action main() -> num:",
    ]
    for kind, args in cases:
        lines.append("    show_" + kind + "(" + args + ")")
    lines.append("    return 0")
    return "\n".join(lines) + "\n"


def authority_of(raw):
    """The raw authority text of a URL, or "" when there is none."""
    if raw.startswith("//"):
        rest = raw[2:]
        return rest.split("/", 1)[0].split("?", 1)[0].split("#", 1)[0]
    colon = raw.find(":")
    if colon < 0:
        return ""
    rest = raw[colon + 1:]
    if not rest.startswith("//"):
        return ""
    rest = rest[2:]
    return rest.split("/", 1)[0].split("?", 1)[0].split("#", 1)[0]


def has_authority(raw):
    """True when an authority marker ("//") follows the scheme or starts the reference."""
    if raw.startswith("//"):
        return True
    colon = raw.find(":")
    if colon < 0:
        return False
    return raw[colon + 1:].startswith("//")


def reference_parts(raw):
    """The RFC 3986 split as CPython's urlsplit sees it, in the module's shape."""
    sp = urllib.parse.urlsplit(raw)
    authority = authority_of(raw)
    auth_flag = has_authority(raw)
    host = sp.hostname or ""
    if authority.startswith("["):
        host = "[" + host + "]"
    port = sp.port or 0
    return {
        "scheme": sp.scheme,
        "host": host,
        "port": port,
        "path": sp.path,
        "query": sp.query,
        "fragment": sp.fragment,
        "has_authority": auth_flag,
    }


def parse_parts_line(line):
    parts = line.split(" ")
    if len(parts) == 3 and parts[0] == "P" and parts[1] == "0":
        return ("reject", parts[2], int(parts[3]) if len(parts) > 3 else -1)
    if len(parts) != 9 or parts[0] != "P" or parts[1] != "1":
        return None
    return {
        "ok": True,
        "scheme": bytes.fromhex(parts[2]).decode("utf-8"),
        "host": bytes.fromhex(parts[3]).decode("utf-8"),
        "port": int(parts[4]),
        "path": bytes.fromhex(parts[5]).decode("utf-8"),
        "query": bytes.fromhex(parts[6]).decode("utf-8"),
        "fragment": bytes.fromhex(parts[7]).decode("utf-8"),
        "has_authority": parts[8] == "1",
    }


def unquote(data):
    return urllib.parse.unquote_to_bytes(data)


def query_get_ref(query, key):
    """url_query_get's documented rule, with urllib.parse doing the decoding."""
    raw = query.encode("utf-8")
    key_bytes = key.encode("utf-8")
    n = len(raw)
    start = 0
    while start <= n:
        end = raw.find(b"&", start)
        if end < 0:
            end = n
        pair = raw[start:end]
        eq = pair.find(b"=")
        name = pair[:eq] if eq >= 0 else pair
        if unquote(name) == key_bytes:
            if eq < 0:
                return b""
            value = unquote(pair[eq + 1:])
            if b"\x00" in value:
                return b""
            return value
        if end >= n:
            break
        start = end + 1
    return b""


def query_has_ref(query, key):
    raw = query.encode("utf-8")
    key_bytes = key.encode("utf-8")
    n = len(raw)
    start = 0
    while start <= n:
        end = raw.find(b"&", start)
        if end < 0:
            end = n
        pair = raw[start:end]
        eq = pair.find(b"=")
        name = pair[:eq] if eq >= 0 else pair
        if unquote(name) == key_bytes:
            return True
        if end >= n:
            break
        start = end + 1
    return False


def build_and_run(cases, workdir):
    src = os.path.join(workdir, "harness.kaze")
    with open(src, "w", encoding="utf-8") as fh:
        fh.write(harness(cases))
    check = subprocess.run([KAZE, "check", src], capture_output=True, text=True)
    if "E_KAZE" in check.stdout + check.stderr:
        print("diff url: the harness does not check\n" + check.stdout + check.stderr, file=sys.stderr)
        return None
    cpath = os.path.join(workdir, "harness.c")
    with open(cpath, "w", encoding="utf-8") as fh:
        emit = subprocess.run([KAZE, "emit-c", src], stdout=fh, stderr=subprocess.PIPE, text=True)
    if emit.returncode != 0:
        print("diff url: emit-c failed\n" + emit.stderr[-2000:], file=sys.stderr)
        return None
    exe = os.path.join(workdir, "harness")
    cmd = [CC, "-std=c99", "-O2", "-w", "-I", DIST]
    if os.environ.get("KAZE_ASAN") == "1":
        cmd += ["-g", "-fsanitize=address,undefined", "-fno-omit-frame-pointer"]
    cmd += [cpath, os.path.join(DIST, "kaze_rt.c"), "-lm", "-o", exe]
    cc = subprocess.run(cmd, capture_output=True, text=True)
    if cc.returncode != 0:
        print("diff url: the native build failed\n" + cc.stderr[-3000:], file=sys.stderr)
        return None
    env = dict(os.environ)
    if os.environ.get("KAZE_ASAN") == "1":
        env["ASAN_OPTIONS"] = "detect_leaks=0" if sys.platform == "darwin" else "detect_leaks=1"
    run = subprocess.run([exe], capture_output=True, text=True, env=env)
    if run.returncode != 0:
        print("diff url: the harness exited %d\n%s\n%s" % (run.returncode, run.stdout[-3000:], run.stderr[-3000:]),
              file=sys.stderr)
        return None
    results = []
    for line in run.stdout.splitlines():
        if line.startswith("[info] "):
            results.append(line[len("[info] "):])
    return results


def generated_url(rng):
    parts = []
    scheme = rng.choice(SCHEMES)
    host = rng.choice(HOSTS)
    port = rng.choice([None, None, None, rng.randint(1, 65535)])
    raw = scheme + "://" + host
    if port is not None:
        raw += ":" + str(port)
    pieces = [rng.choice(PATH_PIECES) for _ in range(rng.randint(0, 3))]
    if pieces:
        raw += "/" + "/".join(pieces)
    if rng.randrange(2) == 0:
        raw += "?" + "&".join(rng.choice(QUERY_PIECES) for _ in range(rng.randint(1, 3)))
    if rng.randrange(2) == 0:
        raw += "#" + rng.choice(FRAGMENT_PIECES)
    return raw


def main():
    rng = random.Random(SEED)
    cases = []
    expected = []
    strictness_pins = 0

    # --- generated absolute URLs: compare every parsed component ----------
    urls = [generated_url(rng) for _ in range(110)]
    for raw in urls:
        ref = reference_parts(raw)
        cases.append(("parse", kaze_text(raw)))
        expected.append(("parts", ref, raw))
        cases.append(("format", kaze_text(raw)))
        expected.append(("format", raw, raw))

    # --- fixed references, including relative and scheme-less forms -------
    fixed = [
        ("", "", ""),
        ("path", "path", "path"),
        ("path/to", "path/to", "path/to"),
        ("path/to?x=1", "path/to?x=1", "path/to?x=1"),
        ("?q=1", "?q=1", "?q=1"),
        ("#f", "#f", "#f"),
        ("//host/p", "//host/p", "//host/p"),
        ("//host:99/p?q#f", "//host:99/p?q#f", "//host:99/p?q#f"),
        ("a:/b", "a:/b", "a:/b"),
        ("mailto:x@y", "mailto:x@y", "mailto:x@y"),
        ("file:///etc/hosts", "file:///etc/hosts", "file:///etc/hosts"),
        ("file:/etc/hosts", "file:/etc/hosts", "file:/etc/hosts"),
        ("http:", "http:", "http:"),
        ("http://", "http://", "http://"),
        ("http:///path", "http:///path", "http:///path"),
    ]
    for raw, fmt, _label in fixed:
        ref = reference_parts(raw)
        cases.append(("parse", kaze_text(raw)))
        expected.append(("parts", ref, raw))
        cases.append(("format", kaze_text(raw)))
        expected.append(("format", fmt, raw))

    # --- IP literals: host keeps its brackets -----------------------------
    for raw in ["http://[::1]:8080/x", "http://[2001:db8::1]/p",
                "http://[::ffff:192.168.0.1]:9/", "http://[fe80::1]/"]:
        ref = reference_parts(raw)
        cases.append(("parse", kaze_text(raw)))
        expected.append(("parts", ref, raw))
        cases.append(("format", kaze_text(raw)))
        expected.append(("format", raw, raw))

    # --- percent-encoding: CPython's quote_from_bytes is the reference ----
    encode_payloads = []
    for _ in range(70):
        encode_payloads.append(bytes(rng.choice([rng.randrange(1, 256), 32, 47, 63, 37, 43])
                                     for _ in range(rng.randint(0, 48))))
    for _ in range(20):
        encode_payloads.append("".join(rng.choice("abcXYZ019 ~_-.+%/!") for _ in range(rng.randint(0, 30)))
                               .encode("ascii"))
    for data in encode_payloads:
        cases.append(("encode", byte_list(data)))
        expected.append(("encode", urllib.parse.quote_from_bytes(data, safe="-._~").encode("ascii")))

    # --- percent-decoding: CPython's unquote_to_bytes is the reference -----
    decode_sources = ["", "abc", "a%20b", "%41%42", "%zz", "%4", "%", "50%", "%E9",
                      "%C3%A9", "%25", "a+b", "%2F%3F%23", "%00", "x%00y"]
    for _ in range(110):
        pieces = []
        for _ in range(rng.randint(0, 8)):
            pick = rng.randrange(6)
            if pick == 0:
                pieces.append(rng.choice("abcXYZ019-._~!$&'()*+,;=:@/?#% "))
            elif pick == 1:
                pieces.append("%%%02X" % rng.randrange(256))
            elif pick == 2:
                pieces.append("%%%02x" % rng.randrange(256))
            elif pick == 3:
                pieces.append(rng.choice(["%z", "%4", "%", "%%"]))
            elif pick == 4:
                pieces.append("é")
            else:
                pieces.append(rng.choice(["+", "%2B", "%20", " "]))
        decode_sources.append("".join(pieces))
    for source in decode_sources:
        cases.append(("decode", kaze_text(source)))
        expected.append(("decode", unquote(source)))

    # --- query helpers ----------------------------------------------------
    keys = ["a", "b", "k", "flag", "a b", "a+b", "enc key", "missing", "", "x"]
    queries = ["", "a=1", "a=1&b=2", "a=1&a=2", "flag", "empty=&x=1", "a%20b=7",
               "enc%20key=9", "plus=a%2Bb", "sp=a+b", "k=%25", "k=50%", "k=%00x",
               "a=%zz", "=v", "&&", "a=1&&b=2"]
    for _ in range(50):
        pairs = []
        for _ in range(rng.randint(0, 4)):
            kind = rng.randrange(5)
            if kind == 0:
                pairs.append(rng.choice(["a=1", "b=2", "k=3", "flag"]))
            elif kind == 1:
                pairs.append(rng.choice(keys) + "=" + rng.choice(["1", "x%20y", "%2B", "%00", "%zz", ""]))
            elif kind == 2:
                pairs.append(rng.choice(["sp=a+b", "plain=value", "q=%C3%A9"]))
            elif kind == 3:
                pairs.append("k=")
            else:
                pairs.append(rng.choice(["a=1&b=2", "a=1"]))
        queries.append("&".join(pairs))
    for query in queries:
        for key in rng.sample(keys, 3):
            cases.append(("get", kaze_text(query) + ", " + kaze_text(key)))
            expected.append(("query", query_get_ref(query, key), query, key))
            cases.append(("has", kaze_text(query) + ", " + kaze_text(key)))
            expected.append(("has", query_has_ref(query, key), query, key))

    # --- malformed URLs: the documented strictness ------------------------
    for raw, err in STRICT_MALFORMED:
        # Where CPython rejects the same input, that is a cross-check; the rest
        # are the module's documented refusal of text CPython tolerates.
        python_rejects = False
        try:
            sp = urllib.parse.urlsplit(raw)
            sp.port
        except ValueError:
            python_rejects = True
        if not python_rejects:
            strictness_pins += 1
        cases.append(("parse", kaze_text(raw)))
        expected.append(("reject", err, raw))

    with tempfile.TemporaryDirectory(prefix="kaze-diff-url-") as workdir:
        results = build_and_run(cases, workdir)
    if results is None:
        return 2
    if len(results) != len(cases):
        print("diff url: expected %d results, got %d" % (len(cases), len(results)), file=sys.stderr)
        return 2

    mismatches = 0
    for i, (line, exp) in enumerate(zip(results, expected)):
        kind = exp[0]
        if kind == "parts":
            ref = exp[1]
            got = parse_parts_line(line)
            if got is None or "ok" not in got:
                mismatches += 1
                print("FAIL case %d (parse %r): expected accept, got %r" % (i, exp[2][:60], line[:100]))
                continue
            for field in ("scheme", "host", "port", "path", "query", "fragment", "has_authority"):
                if got[field] != ref[field]:
                    mismatches += 1
                    print("FAIL case %d (parse %r): %s expected %r, got %r" % (i, exp[2][:60], field, ref[field], got[field]))
                    break
        elif kind == "format":
            want = exp[1]
            got = line.split(" ", 1)[1] if line.startswith("F ") else ""
            try:
                got_text = bytes.fromhex(got).decode("utf-8")
            except ValueError:
                got_text = "<not hex>"
            if got_text != want:
                mismatches += 1
                print("FAIL case %d (format %r): expected %r, got %r" % (i, exp[2][:60], want, got_text))
        elif kind == "encode":
            want = exp[1].hex()
            got = line.split(" ", 1)[1] if line.startswith("E ") else ""
            if got != want:
                mismatches += 1
                print("FAIL case %d (encode): expected %r, got %r" % (i, want[:80], got[:80]))
        elif kind == "decode":
            want = exp[1].hex()
            got = line.split(" ", 1)[1] if line.startswith("D ") else ""
            if got != want:
                mismatches += 1
                print("FAIL case %d (decode): expected %r, got %r" % (i, want[:80], got[:80]))
        elif kind == "query":
            want = exp[1].hex()
            got = line.split(" ", 1)[1] if line.startswith("Q ") else ""
            if got != want:
                mismatches += 1
                print("FAIL case %d (get %r, %r): expected %r, got %r" % (i, exp[2][:40], exp[3], want[:80], got[:80]))
        elif kind == "has":
            want = exp[1]
            got = line == "H 1"
            if got != want:
                mismatches += 1
                print("FAIL case %d (has %r, %r): expected %r, got %r" % (i, exp[2][:40], exp[3], want, got))
        else:
            want_err = exp[1]
            raw = exp[2]
            parts = line.split(" ")
            if len(parts) < 3 or parts[0] != "P" or parts[1] != "0":
                mismatches += 1
                print("FAIL case %d (reject %r): expected %s, got %r" % (i, raw[:60], want_err, line[:100]))
                continue
            if parts[2] != want_err:
                mismatches += 1
                print("FAIL case %d (reject %r): expected %s, got %s" % (i, raw[:60], want_err, parts[2]))
                continue
            at = int(parts[3]) if len(parts) > 3 else -1
            if at < 0 or at > len(raw.encode("utf-8")):
                mismatches += 1
                print("FAIL case %d (reject %r): offset %d outside the input" % (i, raw[:60], at))

    print()
    if strictness_pins:
        print("strictness: %d malformed URLs CPython's permissive parser accepts but the"
              " documented RFC 3986 grammar refuses" % strictness_pins)
    print("diff url: %d cases, %d mismatches" % (len(cases), mismatches))
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())