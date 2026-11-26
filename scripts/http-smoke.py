#!/usr/bin/env python3
"""http-smoke.py -- Phase 3 evidence against real local HTTP and HTTPS servers.

Starts a loopback HTTP server and a loopback TLS server with a generated
certificate, grants the client exactly those destinations, and runs
tests/http/http_test.kaze twice. The TLS run passes the generated certificate
as KAZE_HTTP_CAINFO, so peer and hostname verification are actually exercised;
it does not turn verification off.

Also checks eight refusal paths: an unavailable backend, an untrusted
certificate, no grant, a grant that is not exactly 1, a destination outside the
allowlist, an allowlist entry whose port does not match, a redirect to a
host outside the allowlist, and a private IP outside the allowlist. A pass here
is local integration evidence, not a public-network or production-TLS guarantee.
"""
import http.server
import os
import shlex
import shutil
import ssl
import subprocess
import sys
import tempfile
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KAZE = os.environ.get("KAZE_BIN", os.path.join(ROOT, "bin", "kaze"))
HOST = os.environ.get("KAZE_HOST", os.path.join(ROOT, "bin", "kaze-host"))
TEST = os.path.join(ROOT, "tests", "http", "http_test.kaze")


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/hello":
            body = b"hello http"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("X-Test", "yes")
            self.send_header("X-Empty", "")
        elif self.path == "/redirect":
            self.send_response(302)
            self.send_header("Location", "/hello")
            body = b""
        elif self.path == "/redirect-out":
            self.send_response(302)
            self.send_header("Location", "http://127.0.0.2:1/")
            body = b""
        elif self.path == "/located":
            self.send_response(200)
            self.send_header("Location", "http://127.0.0.2:1/")
            body = b"located"
        elif self.path == "/slow":
            time.sleep(0.4)
            self.send_response(200)
            body = b"slow"
        elif self.path == "/big":
            self.send_response(200)
            body = b"x" * 200000
        else:
            self.send_response(404)
            body = b"nope"
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        n = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(n)
        self.send_response(201)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def serve(port, cert=None, key=None):
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
    if cert is not None:
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ctx.load_cert_chain(cert, key)
        srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
    thread = threading.Thread(target=srv.serve_forever, daemon=True)
    thread.start()
    return srv


def run_case(url, extra_env, want, label, failures, allow_fails=False):
    env = dict(os.environ)
    env.update(extra_env)
    cmd = [HOST, "run", BUNDLE, "--arg", "url=" + url]
    out = subprocess.run(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                         text=True, timeout=120)
    text = out.stdout
    if want in text and (allow_fails or "FAIL " not in text):
        print("PASS  " + label)
        return failures
    print("FAIL  " + label)
    for line in text.splitlines()[-8:]:
        print("    " + line)
    return failures + 1


def run_no_backend_case(tmp, failures):
    src = os.path.join(tmp, "no_backend.kaze")
    with open(src, "w") as out:
        out.write('needs: log\n\nuse "%s/lib/http.kaze"\n\n'
                  'action main() -> num:\n'
                  '    let r = http_get("http://127.0.0.1:1/x")\n'
                  '    log.info("error=" + r.error)\n'
                  '    return 0\n' % ROOT)
    emitted = subprocess.run([KAZE, "emit-c", src], stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, text=True)
    if emitted.returncode != 0:
        print("FAIL  a build without libcurl is refused as E_HTTP_BACKEND")
        print("    emit-c: " + emitted.stdout.strip()[-300:])
        return failures + 1
    c_path = os.path.join(tmp, "no_backend.c")
    exe = os.path.join(tmp, "no_backend")
    with open(c_path, "w") as out:
        out.write(emitted.stdout)
    build = subprocess.run(
        shlex.split(os.environ.get("CC", "cc")) +
        ["-std=c99", "-O2", "-w", "-DKAZE_HTTP_NO_CURL",
         "-I", os.path.join(ROOT, "dist"), c_path,
         os.path.join(ROOT, "dist", "kaze_rt.c"), "-lm", "-o", exe],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if build.returncode != 0:
        print("FAIL  a build without libcurl is refused as E_HTTP_BACKEND")
        print("    cc: " + build.stdout.strip()[-300:])
        return failures + 1
    env = dict(os.environ)
    env["KAZE_HTTP"] = "1"
    run = subprocess.run([exe], env=env, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT, text=True, timeout=60)
    if "E_HTTP_BACKEND" in run.stdout:
        print("PASS  a build without libcurl is refused as E_HTTP_BACKEND")
        return failures
    print("FAIL  a build without libcurl is refused as E_HTTP_BACKEND")
    for line in run.stdout.splitlines()[-6:]:
        print("    " + line)
    return failures + 1


def run_emission_case(failures):
    hello = subprocess.run([KAZE, "emit-c", os.path.join(ROOT, "examples", "hello.kaze")],
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    used = subprocess.run([KAZE, "emit-c", BUNDLE],
                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    ok = (hello.returncode == 0 and "kaze_http.h" not in hello.stdout and
          used.returncode == 0 and "kaze_http.h" in used.stdout)
    if ok:
        print("PASS  HTTP is emitted only when a program uses it")
        return failures
    print("FAIL  HTTP is emitted only when a program uses it")
    print("    hello: rc=%d http=%s; http program: rc=%d http=%s" % (
        hello.returncode, "kaze_http.h" in hello.stdout,
        used.returncode, "kaze_http.h" in used.stdout))
    return failures + 1


def main():
    global BUNDLE
    if not os.access(KAZE, os.X_OK) or not os.access(HOST, os.X_OK):
        print("http-smoke: missing bin/kaze or bin/kaze-host", file=sys.stderr)
        return 2

    tmp = tempfile.mkdtemp(prefix="kaze-http-")
    cert = os.path.join(tmp, "cert.pem")
    key = os.path.join(tmp, "key.pem")
    openssl = subprocess.run(
        ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
         "-keyout", key, "-out", cert, "-days", "1", "-subj", "/CN=127.0.0.1",
         "-addext", "subjectAltName=IP:127.0.0.1"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if openssl.returncode != 0:
        print("http-smoke: openssl could not make a test certificate", file=sys.stderr)
        print(openssl.stdout[-400:], file=sys.stderr)
        return 2

    BUNDLE = TEST

    http_srv = serve(0)
    https_srv = serve(0, cert, key)
    http_port = http_srv.server_address[1]
    https_port = https_srv.server_address[1]
    failures = 0
    try:
        failures = run_case(
            "http://127.0.0.1:%d" % http_port,
            {"KAZE_HTTP": "1", "KAZE_HTTP_ALLOW": "127.0.0.1",
             "KAZE_HTTP_MAX_BYTES": "1000"},
            "OK http: 19 checks", "HTTP over loopback", failures)
        failures = run_case(
            "https://127.0.0.1:%d" % https_port,
            {"KAZE_HTTP": "1", "KAZE_HTTP_ALLOW": "127.0.0.1",
             "KAZE_HTTP_MAX_BYTES": "1000", "KAZE_HTTP_CAINFO": cert},
            "OK http: 19 checks", "HTTPS with certificate verification", failures)
        failures = run_case(
            "https://127.0.0.1:%d" % https_port,
            {"KAZE_HTTP": "1", "KAZE_HTTP_ALLOW": "127.0.0.1"},
            "E_HTTP_TLS", "an untrusted certificate is refused", failures, True)
        failures = run_case(
            "http://127.0.0.1:%d" % http_port,
            {"KAZE_HTTP": "1", "KAZE_HTTP_ALLOW": "127.0.0.1:1"},
            "E_HTTP_DENIED", "an allowlisted host with another port is refused", failures, True)
        failures = run_case(
            "http://127.0.0.1:%d" % http_port,
            {"KAZE_HTTP_ALLOW": "127.0.0.1:%d" % http_port},
            "E_HTTP_DENIED", "no grant is refused", failures, True)
        failures = run_case(
            "http://127.0.0.1:%d" % http_port,
            {"KAZE_HTTP": "0", "KAZE_HTTP_ALLOW": "127.0.0.1"},
            "E_HTTP_DENIED", "KAZE_HTTP must be exactly 1 to grant", failures, True)
        failures = run_case(
            "http://127.0.0.1:%d" % http_port,
            {"KAZE_HTTP": "1", "KAZE_HTTP_ALLOW": "example.com"},
            "E_HTTP_DENIED", "a destination outside the allowlist is refused", failures, True)
        failures = run_case(
            "http://127.0.0.2:%d" % http_port,
            {"KAZE_HTTP": "1", "KAZE_HTTP_ALLOW": "127.0.0.1:%d" % http_port},
            "E_HTTP_DENIED", "private IP 127.0.0.2 is refused when not in allowlist", failures, True)
        failures = run_no_backend_case(tmp, failures)
        failures = run_emission_case(failures)
    finally:
        http_srv.shutdown()
        https_srv.shutdown()
        shutil.rmtree(tmp, ignore_errors=True)

    if failures == 0:
        print("http-smoke: 10 of 10 cases passed")
        return 0
    print("http-smoke: %d case(s) failed" % failures)
    return 1


if __name__ == "__main__":
    sys.exit(main())
