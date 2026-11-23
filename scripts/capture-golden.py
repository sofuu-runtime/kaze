#!/usr/bin/env python3
'''Capture the reference engine's verdict for every corpus file as golden data.

Phase 10 wants the Rust implementation retired. The obstacle is that the Rust engine is
the oracle: 414 tests and every differential harness compare against it. Frozen
verdicts turn that oracle from code into data, so a Rust-free harness can keep checking
the self-hosted compiler against what the reference decided.

    ./scripts/capture-golden.py <path-to-reference-binary>

Writes scripts/selftest/golden/<name>.txt with one line per file:
    ok                      the reference accepts it
    E_KAZE_X,E_KAZE_Y       the diagnostic codes the reference reports
'''
import glob, os, re, subprocess, sys

if len(sys.argv) < 2:
    print('usage: capture-golden.py <reference-binary>', file=sys.stderr)
    sys.exit(2)
ref = sys.argv[1]
root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
out_dir = os.path.join(root, 'scripts', 'selftest', 'golden')
os.makedirs(out_dir, exist_ok=True)

CODE = re.compile(r'(E_[A-Z_]+)')
files = []
for pattern in ('examples/*.kaze', 'examples/missions/*.kaze', 'examples/modular/*.kaze',
                'tests/fixtures/strict/*.kaze', 'tests/fixtures/parity/*.kaze',
                'examples/modular/lib/*.kaze'):
    files.extend(sorted(glob.glob(os.path.join(root, pattern))))

captured = 0
for path in files:
    name = os.path.basename(path)[:-5]
    r = subprocess.run([ref, 'check', path], capture_output=True, text=True)
    text = (r.stdout or '') + (r.stderr or '')
    codes = []
    for c in CODE.findall(text):
        if c not in codes:
            codes.append(c)
    if r.returncode == 0 and not codes:
        verdict = 'ok'
    elif codes:
        verdict = ','.join(codes)
    else:
        verdict = 'ok' if 'check: ok' in text else 'refused'
    with open(os.path.join(out_dir, name + '.txt'), 'w') as fh:
        fh.write(verdict + '\n')
    captured += 1

print('capture-golden: %d verdicts written to scripts/selftest/golden/' % captured)