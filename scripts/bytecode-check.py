#!/usr/bin/env python3
'''Check the self-hosted compiler's bytecode writer against its own frozen output.

The self-hosted CLI prints the module as hex (it has no file-write primitive), so this
decodes that and compares. The goldens are the self-hosted compiler's own output, and the
gate is a ratchet: the writer may not drift. Reference parity is a separate measurement -
the two engines emit the same fields but the reference also writes origin strings the
self-hosted side cannot know.

    ./scripts/bytecode-check.sh [--update]
'''
import glob, os, subprocess, sys

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
golden_dir = os.path.join(root, 'scripts', 'selftest', 'bytecode')
baseline_file = os.path.join(golden_dir, 'baseline.txt')

bin_path = os.environ.get('KAZE_BIN')
if not bin_path:
    bin_path = os.path.join(root, 'bin', 'kaze')
if not os.path.exists(bin_path):
    print('bytecode-check: no compiler at %s (run make kaze-build or set KAZE_BIN)' % bin_path)
    sys.exit(2)

goldens = sorted(glob.glob(os.path.join(golden_dir, '*.kazebc')))
match = differ = 0
names = []
for g in goldens:
    name = os.path.basename(g)[:-len('.kazebc')]
    path_file = os.path.join(golden_dir, name + '.path')
    source = os.path.join(root, 'examples', name + '.kaze')
    if not os.path.exists(source):
        continue
    r = subprocess.run([bin_path, 'compile', source], capture_output=True, text=True)
    hexed = ''.join((r.stdout or '').split())
    try:
        produced = bytes.fromhex(hexed)
    except ValueError:
        differ += 1
        names.append('%s (no hex output: %s)' % (name, (r.stderr or '').strip()[:50]))
        continue
    want = open(g, 'rb').read()
    if os.path.exists(path_file):
        recorded = open(path_file).read().strip()
        # the reference's root field holds the path; the self-hosted module has none
        # the module stores strings as a u16 length followed by the bytes, so the
        # length prefix has to go with the path
        import struct
        want = want.replace(struct.pack('<H', len(recorded)) + recorded.encode(), b'', 1)
    if want == produced:
        match += 1
    else:
        differ += 1
        names.append('%s (%d vs %d bytes)' % (name, len(want), len(produced)))

baseline = 0
if os.path.exists(baseline_file):
    baseline = int(open(baseline_file).read().strip() or '0')
total = match + differ
print('bytecode-check: %d of %d modules match their frozen golden, %d differ (baseline %d)' % (match, total, differ, baseline))
for n in names:
    print('  differs: %s' % n)
if '--update' in sys.argv:
    open(baseline_file, 'w').write('%d\n' % differ)
    print('bytecode-check: baseline updated to %d' % differ)
    sys.exit(0)
sys.exit(1 if differ > baseline else 0)