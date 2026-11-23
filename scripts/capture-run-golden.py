#!/usr/bin/env python3
'''Freeze each runnable example's behaviour, captured in the *same* environment the
bootstrap verifier runs it in: its own directory, a policy.json, /dev/null on stdin.

Phase 10 retires the Rust engine, so the bootstrap verifier's cross-emitter claim now
compares the self-hosted compiler's emitted binary against this frozen data instead of
against a second live emitter. Libraries and modules with 'use' are not runnable and have
no behaviour golden; the verifier skips them and checks their verdict instead.
'''
import glob, json, os, subprocess, sys, tempfile

if len(sys.argv) < 2:
    print('usage: capture-run-golden.py <reference-binary>', file=sys.stderr)
    sys.exit(2)
ref = sys.argv[1]
root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
out_dir = os.path.join(root, 'scripts', 'selftest', 'run-golden')
os.makedirs(out_dir, exist_ok=True)
cc = os.environ.get('CC', 'cc')

files = sorted(glob.glob(os.path.join(root, 'examples', '*.kaze')))
files += sorted(glob.glob(os.path.join(root, 'examples', 'missions', '*.kaze')))
captured = skipped = 0
for path in files:
    name = os.path.basename(path)[:-5]
    tmp = tempfile.mkdtemp(prefix='kaze-rungold-')
    c_path = os.path.join(tmp, 'p.c')
    bin_path = os.path.join(tmp, 'p')
    r = subprocess.run([ref, 'emit-c', path, '-o', c_path], capture_output=True, text=True)
    if r.returncode != 0 or not os.path.exists(c_path):
        print('capture-run-golden: %s: not runnable (%s)' % (name, (r.stderr or r.stdout).strip()[:60]))
        skipped += 1
        continue
    b = subprocess.run([cc, '-O1', c_path, '-lm', '-o', bin_path], capture_output=True, text=True)
    if b.returncode != 0:
        print('capture-run-golden: %s: did not build' % name)
        skipped += 1
        continue
    with open(os.path.join(tmp, 'policy.json'), 'w') as fh:
        fh.write(json.dumps({'deny': []}) + '\n')
    run = subprocess.run(['./p', '--ticks', '2'], cwd=tmp, capture_output=True, text=True,
                         stdin=subprocess.DEVNULL, timeout=120)
    with open(os.path.join(out_dir, name + '.out'), 'w') as fh:
        fh.write(run.stdout)
    with open(os.path.join(out_dir, name + '.err'), 'w') as fh:
        fh.write(run.stderr)
    captured += 1

print('capture-run-golden: %d behaviours frozen, %d not runnable' % (captured, skipped))