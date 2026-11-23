#!/usr/bin/env python3
'''Freeze a compiler's bytecode modules as regression data.

Phase 10 keeps the Rust engine as the reference oracle. This captures what that oracle
*emits* so the self-hosted compiler's module writer can be checked against it without a
Rust binary present.

Cross-engine byte equality is NOT the goal here: the reference writes origin strings into
the module (the source name and per-item origins) that the self-hosted compiler cannot
know, because it is handed file contents, never a path. What this freezes is the
self-hosted compiler's own output, so the module writer cannot regress silently.

    ./scripts/capture-bytecode.py <compiler-binary>
'''
import glob, os, subprocess, sys

if len(sys.argv) < 2:
    print('usage: capture-bytecode.py <reference-binary>', file=sys.stderr)
    sys.exit(2)
ref = sys.argv[1]
root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
out_dir = os.path.join(root, 'scripts', 'selftest', 'bytecode')
os.makedirs(out_dir, exist_ok=True)

files = sorted(glob.glob(os.path.join(root, 'examples', '*.kaze')))
captured = 0
for path in files:
    name = os.path.basename(path)[:-5]
    rel = os.path.relpath(path, root)
    out = os.path.join(out_dir, name + '.kazebc')
    r = subprocess.run([ref, 'compile', path, '-o', out], capture_output=True, text=True)
    if r.returncode != 0 or not os.path.exists(out):
        print('capture-bytecode: %s refused by the reference (%s)' % (name, (r.stderr or r.stdout).strip()[:60]))
        continue
    with open(os.path.join(out_dir, name + '.path'), 'w') as fh:
        fh.write(rel + '\n')
    captured += 1

print('capture-bytecode: %d modules frozen in scripts/selftest/bytecode/' % captured)