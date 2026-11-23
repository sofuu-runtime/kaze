#!/usr/bin/env python3
"""lsp-smoke.py -- drive the pure-Kaze LSP engine end to end.

What is tested, and how the transport really works
--------------------------------------------------
The stage-1 compiler is emitted C that has no streaming stdio: it can slurp
stdin once (or read an argument path) and it prints main's return value once,
when the process exits. lsp.kaze therefore speaks the protocol as a batch
session: one framed conversation in, every framed reply out, process exits.
This script drives exactly that, and it never calls the result an interactive
server.

Transport forms checked:

  T1  <binary> lsp <session-file>       argument path (kz_read_file_or_string)
  T2  <binary> --entry lsp < <pipe>     the one stdin slurp path (parameter 0)

T1 and T2 must produce identical replies. A third form, <binary> lsp
/dev/stdin with stdin redirected from a regular file, is also exercised by
hand in the report; it cannot take a pipe because the runtime's file reader
seeks to the end first.

Protocol steps checked: initialize capabilities, initialized (no reply),
didOpen valid (no diagnostics), hover, definition, hover on an unknown word
(null), didOpen with a deliberate error (one diagnostic), full-sync didChange
through contentChanges and through textDocument.text, unknown request
(-32601), unknown notification (no reply), shutdown, exit, Content-Type
tolerance, and clean stdout (frames plus the emitter's trailing newline).

Latency: the batch engine's per-response cost is isolated by running N extra
hover/definition requests against a baseline session and dividing the
difference by N. That is engine latency inside the batch, NOT an interactive
round trip; the interactive 5 ms gate is reported as SKIP with the reason.

Exit status: 0 when every checked step passes, 1 otherwise.
"""

import argparse
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time

HEADER = b"Content-Length:"
FENCE = b"\r\n\r\n"
LF_FENCE = b"\n\n"


def frame(message, with_content_type=False):
    body = json.dumps(message, separators=(",", ":")).encode("utf-8")
    head = HEADER + b" " + str(len(body)).encode("ascii") + b"\r\n"
    if with_content_type:
        head += b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n"
    return head + b"\r\n" + body


def parse_frames(data):
    """Return (messages, error). Every byte must be a frame or trailing space."""
    messages = []
    i = 0
    n = len(data)
    while i < n:
        while i < n and data[i:i + 1] in (b" ", b"\t", b"\r", b"\n"):
            i += 1
        if i >= n:
            break
        rest = data[i:]
        if rest[:len(HEADER)].lower() != HEADER.lower():
            return messages, "unexpected non-frame bytes at offset %d: %r" % (i, rest[:40])
        crlf = data.find(FENCE, i)
        lf = data.find(LF_FENCE, i)
        if crlf < 0 and lf < 0:
            return messages, "no blank line after headers at offset %d" % i
        if crlf >= 0 and (lf < 0 or crlf < lf):
            head_end, body_start = crlf, crlf + len(FENCE)
        else:
            head_end, body_start = lf, lf + len(LF_FENCE)
        head = data[i:head_end].decode("utf-8", "replace")
        length = None
        for line in head.splitlines():
            if line.lower().startswith("content-length:"):
                try:
                    length = int(line.split(":", 1)[1].strip())
                except ValueError:
                    return messages, "bad Content-Length: %r" % line
        if length is None:
            return messages, "frame without Content-Length at offset %d" % i
        if body_start + length > n:
            return messages, "truncated body: need %d bytes from %d of %d" % (length, body_start, n)
        body = data[body_start:body_start + length]
        try:
            messages.append(json.loads(body))
        except ValueError as exc:
            return messages, "invalid JSON body: %s" % exc
        i = body_start + length
    return messages, None


def responses(messages):
    return [m for m in messages if "id" in m]


def notifications(messages):
    return [m for m in messages if "id" not in m]


def response_for(messages, wanted_id):
    for m in messages:
        if m.get("id") == wanted_id:
            return m
    return None


def diagnostics_for(messages, uri):
    out = []
    for m in messages:
        if m.get("method") == "textDocument/publishDiagnostics":
            params = m.get("params") or {}
            if params.get("uri") == uri:
                out.append(params.get("diagnostics"))
    return out


class Report(object):
    def __init__(self):
        self.passed = 0
        self.failed = 0
        self.skipped = 0

    def check(self, name, ok, detail=""):
        if ok:
            self.passed += 1
            print("[PASS] %s%s" % (name, (" -- " + detail) if detail else ""))
        else:
            self.failed += 1
            print("[FAIL] %s%s" % (name, (" -- " + detail) if detail else ""))

    def skip(self, name, detail=""):
        self.skipped += 1
        print("[SKIP] %s%s" % (name, (" -- " + detail) if detail else ""))

    def summary(self):
        total = self.passed + self.failed
        print("summary: %d/%d checks passed, %d failed, %d skipped" % (self.passed, total, self.failed, self.skipped))
        return 0 if self.failed == 0 else 1


VALID = (
    "action greet(name):\n"
    "    return \"hello \" + name\n"
    "\n"
    "action main():\n"
    "    return greet(\"world\")\n"
)
BROKEN = "action main():\n    return 1 / 0\n"
CHANGED_A = "action main():\n    return 42\n"
CHANGED_B = "action main():\n    return 7\n"
VALID_URI = "doc://valid.kaze"
BROKEN_URI = "doc://broken.kaze"

# Line 4 (0-based) is '    return greet("world")'; character 12 is inside greet.
GREET_LINE = 4
GREET_CHAR = 12


def build_main_session():
    messages = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"capabilities": {}}},
        {"jsonrpc": "2.0", "method": "initialized", "params": {}},
        {"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": VALID_URI, "languageId": "kaze", "version": 1, "text": VALID}}},
        {"jsonrpc": "2.0", "id": 2, "method": "textDocument/hover", "params": {
            "textDocument": {"uri": VALID_URI}, "position": {"line": GREET_LINE, "character": GREET_CHAR}}},
        {"jsonrpc": "2.0", "id": 3, "method": "textDocument/definition", "params": {
            "textDocument": {"uri": VALID_URI}, "position": {"line": GREET_LINE, "character": GREET_CHAR}}},
        {"jsonrpc": "2.0", "id": 4, "method": "textDocument/hover", "params": {
            "textDocument": {"uri": VALID_URI}, "position": {"line": 0, "character": 2}}},
        {"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": BROKEN_URI, "languageId": "kaze", "version": 1, "text": BROKEN}}},
        {"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
            "textDocument": {"uri": BROKEN_URI, "version": 2},
            "contentChanges": [{"text": CHANGED_A}]}},
        {"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
            "textDocument": {"uri": BROKEN_URI, "version": 3, "text": CHANGED_B}}},
        {"jsonrpc": "2.0", "id": 5, "method": "textDocument/completion", "params": {}},
        {"jsonrpc": "2.0", "method": "textDocument/didSave", "params": {"textDocument": {"uri": VALID_URI}}},
        {"jsonrpc": "2.0", "id": 6, "method": "shutdown", "params": None},
        {"jsonrpc": "2.0", "method": "exit", "params": None},
    ]
    return messages


def build_payload(messages):
    parts = []
    for index, message in enumerate(messages):
        # The initialize request carries Content-Type on purpose: a server has
        # to ignore headers it does not use rather than refuse the frame.
        parts.append(frame(message, with_content_type=(index == 0)))
    return b"".join(parts)


def run_file(binary, payload, workdir):
    fd, path = tempfile.mkstemp(prefix="kaze-lsp-", suffix=".jsonrpc", dir=workdir)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(payload)
        started = time.perf_counter()
        proc = subprocess.run([binary, "lsp", path], stdin=subprocess.DEVNULL,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120)
        elapsed = (time.perf_counter() - started) * 1000.0
        return elapsed, proc.stdout, proc.stderr, proc.returncode
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass


def run_pipe(binary, payload, workdir):
    started = time.perf_counter()
    proc = subprocess.run([binary, "--entry", "lsp"], input=payload,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120)
    elapsed = (time.perf_counter() - started) * 1000.0
    return elapsed, proc.stdout, proc.stderr, proc.returncode


def latency_document():
    lines = []
    for i in range(30):
        lines.append("action step%d(x):" % i)
        lines.append("    return x + %d" % i)
        lines.append("")
    lines.append("action main():")
    lines.append("    return step7(1)")
    return "\n".join(lines) + "\n", len(lines) - 1


def latency_session(document, last_line, repeats, method):
    messages = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
        {"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": "doc://latency.kaze", "languageId": "kaze", "version": 1,
                             "text": document}}},
    ]
    for i in range(repeats):
        messages.append({"jsonrpc": "2.0", "id": 1000 + i, "method": method, "params": {
            "textDocument": {"uri": "doc://latency.kaze"},
            "position": {"line": last_line, "character": 12}}})
    messages += [{"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": None},
                 {"jsonrpc": "2.0", "method": "exit", "params": None}]
    return build_payload(messages)


def median_ms(binary, payload, workdir, trials=3):
    samples = []
    for _ in range(trials):
        elapsed, _out, err, code = run_file(binary, payload, workdir)
        if code != 0 or err.strip():
            return None, "exit %s stderr %r" % (code, err[:120])
        samples.append(elapsed)
    return statistics.median(samples), ""


def build_stage1(repo_root):
    """Build the pure-Kaze compiler from the monolith; used by --build."""
    kaze_dir = os.path.join(repo_root, "kaze")
    compiler = os.path.join(kaze_dir, "target", "debug", "kaze")
    monolith = os.path.join(kaze_dir, "compiler", "kaze_self.kaze")
    if not os.path.exists(compiler):
        raise SystemExit("lsp-smoke: --build needs %s" % compiler)
    out_dir = tempfile.mkdtemp(prefix="kaze-lsp-build-")
    c_path = os.path.join(out_dir, "stage1.c")
    bin_path = os.path.join(out_dir, "stage1")
    subprocess.run([compiler, "emit-c", monolith, "-o", c_path], cwd=kaze_dir, check=True)
    subprocess.run(["cc", "-std=c99", "-O2", "-w", "-I", os.path.join(kaze_dir, "dist"),
                    c_path, os.path.join(kaze_dir, "dist", "kaze_rt.c"), "-o", bin_path], check=True)
    return bin_path


def main(argv=None):
    parser = argparse.ArgumentParser(description="Drive the pure-Kaze LSP engine (batch transport).")
    parser.add_argument("--binary", default=os.environ.get("KAZE_LSP_BIN", "/tmp/stage1"),
                        help="stage-1 compiler binary (default /tmp/stage1)")
    parser.add_argument("--build", action="store_true",
                        help="build the stage-1 binary from compiler/kaze_self.kaze first")
    parser.add_argument("--requests", type=int, default=2000,
                        help="repetitions used to isolate per-response latency (default 2000)")
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--keep", action="store_true", help="keep the temporary work directory")
    args = parser.parse_args(argv)

    repo_root = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
    binary = args.binary
    if args.build:
        binary = build_stage1(repo_root)
    if not os.path.exists(binary):
        print("lsp-smoke: binary not found: %s" % binary)
        print("  build it with:")
        print("    cd kaze && ./bin/kaze emit-c compiler/kaze_self.kaze -o /tmp/st.c")
        print("    cc -std=c99 -O2 -w -I dist /tmp/st.c dist/kaze_rt.c -o /tmp/stage1")
        print("  or rerun with --build")
        return 2

    workdir = tempfile.mkdtemp(prefix="kaze-lsp-smoke-")
    report = Report()
    print("lsp-smoke: binary %s" % binary)

    messages = build_main_session()
    payload = build_payload(messages)

    # ---- transport T1: argument path -------------------------------------
    elapsed_file, out_file, err_file, code_file = run_file(binary, payload, workdir)
    parsed, parse_error = parse_frames(out_file)
    report.check("T1 transport: lsp <session-file> produces only framed replies",
                 code_file == 0 and parse_error is None and len(parsed) > 0,
                 "exit %s, %d frames%s" % (code_file, len(parsed),
                                           (", " + parse_error) if parse_error else ""))
    report.check("T1 stderr is empty", err_file.strip() == b"", repr(err_file[:160]))

    # ---- transport T2: whole session through a real stdin pipe -----------
    elapsed_pipe, out_pipe, err_pipe, code_pipe = run_pipe(binary, payload, workdir)
    parsed_pipe, parse_error_pipe = parse_frames(out_pipe)
    report.check("T2 transport: whole session through a stdin pipe (--entry lsp)",
                 code_pipe == 0 and parse_error_pipe is None and len(parsed_pipe) > 0,
                 "exit %s, %d frames%s" % (code_pipe, len(parsed_pipe),
                                           (", " + parse_error_pipe) if parse_error_pipe else ""))
    report.check("T2 pipe replies match T1 file replies", parsed_pipe == parsed,
                 "file %d frames, pipe %d frames" % (len(parsed), len(parsed_pipe)))
    report.check("T2 pipe stderr is empty", err_pipe.strip() == b"", repr(err_pipe[:160]))

    # ---- protocol steps --------------------------------------------------
    initialize = response_for(parsed, 1)
    caps = ((initialize or {}).get("result") or {}).get("capabilities") or {}
    report.check("1 initialize capabilities",
                 caps.get("textDocumentSync") == 1 and caps.get("hoverProvider") is True
                 and caps.get("definitionProvider") is True,
                 json.dumps(caps))

    report.check("2 initialized notification is not answered",
                 response_for(parsed, 0) is None and len(notifications(parsed)) == 4,
                 "%d notifications" % len(notifications(parsed)))

    valid_diags = diagnostics_for(parsed, VALID_URI)
    report.check("3 didOpen valid document publishes no diagnostics",
                 len(valid_diags) >= 1 and valid_diags[0] == [],
                 "%d publish(es), first=%s" % (len(valid_diags), json.dumps(valid_diags[0] if valid_diags else None)))

    hover = response_for(parsed, 2)
    hover_result = (hover or {}).get("result") or {}
    hover_value = (hover_result.get("contents") or {}).get("value", "")
    report.check("4 hover on a known action returns markdown",
                 "action greet(name)" in hover_value and hover_value.startswith(chr(96) * 3 + "kaze"),
                 json.dumps(hover_value))
    hover_range = hover_result.get("range") or {}
    report.check("4b hover range covers the cursor word",
                 hover_range.get("start") == {"line": GREET_LINE, "character": 11}
                 and hover_range.get("end") == {"line": GREET_LINE, "character": 16},
                 json.dumps(hover_range))

    definition = response_for(parsed, 3)
    def_result = (definition or {}).get("result") or {}
    def_range = def_result.get("range") or {}
    report.check("5 definition points at the declaration name",
                 def_result.get("uri") == VALID_URI
                 and def_range.get("start") == {"line": 0, "character": 7}
                 and def_range.get("end") == {"line": 0, "character": 12},
                 json.dumps(def_result))

    unknown_hover = response_for(parsed, 4)
    report.check("6 hover on a word that is not declared returns null",
                 unknown_hover is not None and unknown_hover.get("result", "missing") is None,
                 json.dumps(unknown_hover))

    broken_diags = diagnostics_for(parsed, BROKEN_URI)
    first_broken = broken_diags[0] if broken_diags else None
    report.check("7 didOpen with a deliberate error publishes exactly one diagnostic",
                 first_broken is not None and len(first_broken) == 1
                 and first_broken[0].get("severity") == 1
                 and first_broken[0].get("range", {}).get("start", {}).get("line") == 1
                 and first_broken[0].get("range", {}).get("start", {}).get("character") == 0
                 and "divide by zero" in first_broken[0].get("message", "")
                 and first_broken[0].get("code") == "E_KAZE_MISSING_FALLBACK",
                 json.dumps(first_broken))

    report.check("8 didChange via contentChanges (full sync) clears the diagnostic",
                 len(broken_diags) >= 2 and broken_diags[1] == [],
                 "%d publish(es): %s" % (len(broken_diags), json.dumps(broken_diags[1] if len(broken_diags) > 1 else None)))
    report.check("9 didChange via textDocument.text (full sync) clears the diagnostic",
                 len(broken_diags) >= 3 and broken_diags[2] == [],
                 "%d publish(es): %s" % (len(broken_diags), json.dumps(broken_diags[2] if len(broken_diags) > 2 else None)))

    method_error = response_for(parsed, 5)
    error_obj = (method_error or {}).get("error") or {}
    report.check("10 unknown request answers JSON-RPC -32601",
                 error_obj.get("code") == -32601 and "textDocument/completion" in error_obj.get("message", ""),
                 json.dumps(method_error))

    report.check("11 unknown notification is never answered",
                 all(m.get("id") != 5 for m in notifications(parsed)) and len(responses(parsed)) == 6,
                 "%d responses" % len(responses(parsed)))

    shutdown = response_for(parsed, 6)
    report.check("12 shutdown replies with null",
                 shutdown is not None and shutdown.get("result", "missing") is None,
                 json.dumps(shutdown))

    report.check("13 exit stops the session (no reply, no later messages)",
                 parsed[-1].get("id") == 6 and len(parsed) == 10,
                 "last message id=%s, total=%d" % (parsed[-1].get("id") if parsed else None, len(parsed)))

    report.check("14 stdout has no bytes outside frames except trailing whitespace",
                 parse_error is None, parse_error or "clean")

    # ---- latency ---------------------------------------------------------
    document, last_line = latency_document()
    baseline_session = latency_session(document, last_line, 0, "textDocument/hover")
    baseline_ms, baseline_err = median_ms(binary, baseline_session, workdir, args.trials)
    report.check("15 latency baseline session runs cleanly",
                 baseline_ms is not None, baseline_err or ("%.2f ms" % (baseline_ms or 0.0)))

    measured = {}
    for label, method in (("hover", "textDocument/hover"), ("definition", "textDocument/definition")):
        repeated = latency_session(document, last_line, args.requests, method)
        total_ms, total_err = median_ms(binary, repeated, workdir, args.trials)
        if total_ms is None or baseline_ms is None:
            report.check("16 latency %s batch session runs cleanly" % label, False, total_err)
            continue
        per_request = (total_ms - baseline_ms) / float(args.requests)
        if per_request < 0.0:
            per_request = 0.0
        measured[label] = per_request
        report.check("16 latency %s per response under 5 ms (batch engine)" % label,
                     per_request < 5.0,
                     "%.4f ms/response over %d requests (%.0f ms total, %.2f ms baseline, doc %d lines)"
                     % (per_request, args.requests, total_ms, baseline_ms, document.count("\n")))

    one_shot = {}
    for label, method in (("hover", "textDocument/hover"), ("definition", "textDocument/definition")):
        single = latency_session(document, last_line, 1, method)
        samples = []
        for _ in range(5):
            elapsed, _out, err, code = run_file(binary, single, workdir)
            if code != 0 or err.strip():
                samples = []
                break
            samples.append(elapsed)
        one_shot[label] = statistics.median(samples) if samples else None
        print("latency %-10s one-shot end-to-end %.2f ms (process start + whole session)"
              % (label, one_shot[label] if one_shot[label] is not None else float("nan")))

    report.check("17 measured both hover and definition latencies",
                 len(measured) == 2, json.dumps(measured))

    report.skip("18 interactive round-trip 5 ms gate",
                "batch transport: the binary cannot read a message while running, so an "
                "interactive request/reply round trip does not exist to time")

    print("")
    print("transport: batch session (one framed conversation in, all replies out, process exits)")
    print("interactive: not implemented; needs streaming stdio, which the language and runtime lack")
    return report.summary()


if __name__ == "__main__":
    sys.exit(main())
