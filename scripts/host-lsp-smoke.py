#!/usr/bin/env python3
'''Smoke-test the interactive LSP served by dist/kaze-host.c.'''
import json, os, subprocess, sys, time

BIN = sys.argv[1] if len(sys.argv) > 1 else '/tmp/kaze-host'
ENV = dict(os.environ, KAZE_BIN=os.environ.get('KAZE_BIN', '/tmp/kaze2'))

class Client:
    def __init__(self):
        self.p = subprocess.Popen([BIN, 'lsp'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=ENV, bufsize=0)
        self.n = 0
    def send(self, method, params=None, notify=False):
        msg = {'jsonrpc': '2.0', 'method': method}
        if params is not None: msg['params'] = params
        if not notify:
            self.n += 1
            msg['id'] = self.n
        body = json.dumps(msg).encode()
        self.p.stdin.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
        self.p.stdin.flush()
        return self.n
    def read(self):
        header = b''
        while b'\r\n\r\n' not in header:
            c = self.p.stdout.read(1)
            if not c: return None
            header += c
        length = int([l for l in header.split(b'\r\n') if l.lower().startswith(b'content-length')][0].split(b':')[1])
        body = b''
        while len(body) < length:
            chunk = self.p.stdout.read(length - len(body))
            if not chunk: return None
            body += chunk
        return json.loads(body)

c = Client()
checks = 0; failures = 0
def check(name, ok, detail=''):
    global checks, failures
    checks += 1
    if ok: print('PASS  %-40s %s' % (name, detail))
    else:
        failures += 1
        print('FAIL  %-40s %s' % (name, detail))

# 1. initialize
c.send('initialize', {'processId': None, 'rootUri': None})
r = c.read()
caps = r['result']['capabilities'] if r and 'result' in r else {}
check('initialize', caps.get('hoverProvider') is True and caps.get('definitionProvider') is True, str(caps)[:70])
c.send('initialized', {}, notify=True)

# 2. a valid document -> no diagnostics
good = 'action main() -> num:\n    return 1 + 2\n'
c.send('textDocument/didOpen', {'textDocument': {'uri': 'file:///good.kaze', 'languageId': 'kaze', 'version': 1, 'text': good}}, notify=True)
r = c.read()
diags = r['params']['diagnostics'] if r and 'params' in r else None
check('didOpen valid -> no diagnostics', diags == [], str(diags)[:70])

# 3. a broken document -> diagnostics
bad = 'action main() -> num:\n    return missing_name\n'
t0 = time.time()
c.send('textDocument/didChange', {'textDocument': {'uri': 'file:///bad.kaze', 'version': 2}, 'contentChanges': [{'text': bad}]}, notify=True)
r = c.read()
dt_ms = (time.time() - t0) * 1000
diags = r['params']['diagnostics'] if r and 'params' in r else []
check('didChange error -> diagnostic', len(diags) >= 1, '%.2f ms, %d diagnostic(s)' % (dt_ms, len(diags)))

# 4. hover on a declared action
doc = 'action frobnicate(x: num) -> num:\n    return x\n'
c.send('textDocument/didOpen', {'textDocument': {'uri': 'file:///h.kaze', 'languageId': 'kaze', 'version': 1, 'text': doc}}, notify=True)
c.read()
t0 = time.time()
c.send('textDocument/hover', {'textDocument': {'uri': 'file:///h.kaze'}, 'position': {'line': 0, 'character': 9}})
r = c.read()
hover_ms = (time.time() - t0) * 1000
val = r['result']['contents']['value'] if r and r.get('result') else None
check('hover on declaration', val is not None and 'frobnicate' in val, '%.2f ms' % hover_ms)

# 5. definition
t0 = time.time()
c.send('textDocument/definition', {'textDocument': {'uri': 'file:///h.kaze'}, 'position': {'line': 0, 'character': 9}})
r = c.read()
def_ms = (time.time() - t0) * 1000
loc = r['result']['range']['start']['line'] if r and r.get('result') else None
check('definition', loc == 0, '%.2f ms, line %s' % (def_ms, loc))

# 6. unknown method
c.send('workspace/nonsense')
r = c.read()
check('unknown method -> -32601', r and r.get('error', {}).get('code') == -32601, str(r)[:60])

# 7. shutdown / exit
c.send('shutdown')
r = c.read()
check('shutdown', r is not None and r.get('result') is None, str(r)[:40])
c.send('exit', notify=True)
c.p.wait(timeout=5)
check('exit', True, 'process ended %s' % c.p.returncode)

gate = 5.0
check('latency under %.0f ms' % gate, max(hover_ms, def_ms, dt_ms) < gate,
      'hover %.2f, definition %.2f, diagnostics %.2f ms' % (hover_ms, def_ms, dt_ms))
print()
print('host-lsp: %d checks, %d failures' % (checks, failures))
sys.exit(1 if failures else 0)