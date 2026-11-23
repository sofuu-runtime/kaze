#!/usr/bin/env python3
'''Hostile-client checks for the Kaze host LSP: the cases a code review found.'''
import json, os, subprocess, sys

BIN = sys.argv[1]
ENV = dict(os.environ)

def session(frames):
    p = subprocess.run([BIN, 'lsp'], input=b''.join(frames), capture_output=True, env=ENV, timeout=20)
    return p

def frame(obj):
    body = obj if isinstance(obj, bytes) else json.dumps(obj).encode()
    return b'Content-Length: %d\r\n\r\n' % len(body) + body

checks = 0; fails = 0
def check(name, ok, detail=''):
    global checks, fails
    checks += 1
    if ok: print('PASS  %-46s %s' % (name, detail))
    else:
        fails += 1
        print('FAIL  %-46s %s' % (name, detail))

# 1. an empty frame must not kill the server
p = session([b'Content-Length: 0\r\n\r\n', frame({'jsonrpc':'2.0','id':1,'method':'initialize','params':{}})])
check('empty frame does not kill the server', p.returncode == 0 and b'initialize' not in p.stdout[:0] and b'capabilities' in p.stdout, 'exit %d' % p.returncode)

# 2. a notification carrying params.id must NOT be answered
p = session([frame({'jsonrpc':'2.0','method':'$/setTrace','params':{'value':'off','id':9}})])
check('notification with params.id unanswered', b'"id"' not in p.stdout, p.stdout[:60].decode(errors='replace'))

# 3. a hostile position with no colon must not crash
doc = 'action main() -> num:\n    return 1\n'
p = session([frame({'jsonrpc':'2.0','id':1,'method':'initialize','params':{}}),
             frame({'jsonrpc':'2.0','method':'textDocument/didOpen','params':{'textDocument':{'uri':'file:///a.kaze','languageId':'kaze','version':1,'text':doc}}}),
             b'Content-Length: 96\r\n\r\n{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"position":{"character":0,"line"}}}'])
check('position without colon does not crash', p.returncode in (0, 1), 'exit %d' % p.returncode)

# 4. a uri containing a quote must still produce valid JSON
weird = 'file:///a"b.kaze'
p = session([frame({'jsonrpc':'2.0','method':'textDocument/didOpen','params':{'textDocument':{'uri':weird,'languageId':'kaze','version':1,'text':doc}}})])
ok = True; detail = ''
try:
    hdr, body = p.stdout.split(b'\r\n\r\n', 1)
    json.loads(body)
except Exception as e:
    ok = False; detail = str(e)[:60]
check('uri with a quote yields valid JSON', ok, detail)

# 5. a string id is echoed verbatim
p = session([frame({'jsonrpc':'2.0','id':'abc','method':'initialize','params':{}})])
check('string id echoed verbatim', b'"id":"abc"' in p.stdout, p.stdout[:70].decode(errors='replace'))

# 6. long diagnostics do not overflow the reply buffer
longdoc = 'action main() -> num:\n    return ' + ('x' * 900) + '\n'
p = session([frame({'jsonrpc':'2.0','method':'textDocument/didOpen','params':{'textDocument':{'uri':'file:///l.kaze','languageId':'kaze','version':1,'text':longdoc}}})])
check('long diagnostic does not crash', p.returncode == 0, 'exit %d' % p.returncode)

# 7-8. `run` works and leaves nothing behind (a review found pid-named temp dirs
# never removed; the host now uses mkdtemp and cleans up on every exit path).
import glob, tempfile
before = set(glob.glob('/tmp/kaze-run-*'))
hello = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), 'examples', 'hello.kaze')
r = subprocess.run([BIN, 'run', hello], capture_output=True, env=ENV, timeout=60)
after = set(glob.glob('/tmp/kaze-run-*'))
check('run executes the program', b'10' in r.stdout, 'output: %r' % r.stdout[-40:]) 
check('run leaves no temp directory', before == after, '%d left' % len(after - before))

# 9. a burst of edits produces one publishDiagnostics, not one per keystroke
burst = [frame({'jsonrpc':'2.0','id':1,'method':'initialize','params':{}})]
for v in (1,2,3):
    burst.append(frame({'jsonrpc':'2.0','method':'textDocument/didChange','params':{'textDocument':{'uri':'file:///a.kaze','version':v},'contentChanges':[{'text':doc}]}}))
p = session(burst)
check('burst of edits coalesces to one publish', p.stdout.count(b'publishDiagnostics') == 1, '%d publishes' % p.stdout.count(b'publishDiagnostics'))

print()
print('host-hostile: %d checks, %d failures' % (checks, fails))
sys.exit(1 if fails else 0)