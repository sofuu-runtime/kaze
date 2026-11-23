#!/usr/bin/env python3
"""Flatten a Kaze file's transitive imports into one program.

The pure-Kaze checker reads a single file and has no loader, so it treats any file
with a `use` line as open-world: a call to something it cannot see is accepted. That
is how a missing action survived a single-file `check` while the concatenated
monolith refused it. This bundler gives the checker the whole program -- the imported
files expanded first, in use order, de-duplicated, with `needs:` merged and the `use`
lines dropped (the order docs/MODULES.md specifies).
"""
import os, re, sys

USE_RE = re.compile(r'^\s*use\s+\"([^\"]+)\"\s*$')
NEEDS_RE = re.compile(r'^\s*needs:\s*(.*)$')


def collect(path, seen, order, needs, missing):
    path = os.path.realpath(path)
    if path in seen:
        return
    seen.add(path)
    body = []
    for line in open(path, encoding='utf-8', errors='replace').read().splitlines():
        m = USE_RE.match(line)
        if m:
            target = m.group(1)
            if not os.path.isabs(target):
                target = os.path.join(os.path.dirname(path), target)
            if os.path.exists(target):
                collect(target, seen, order, needs, missing)
            else:
                missing.append((m.group(1), path))
            continue
        m = NEEDS_RE.match(line)
        if m:
            for name in m.group(1).split(','):
                name = name.strip()
                if name and name not in needs:
                    needs.append(name)
            continue
        body.append(line)
    order.append('\n'.join(body))


def main():
    if len(sys.argv) < 2:
        print('usage: bundle-imports.py <file.kaze>', file=sys.stderr)
        return 2
    seen, order, needs, missing = set(), [], [], []
    collect(sys.argv[1], seen, order, needs, missing)
    out = []
    if needs:
        out += ['needs: ' + ', '.join(needs), '']
    for part in order:
        out += [part.strip('\n'), '']
    sys.stdout.write('\n'.join(out))
    for name, where in missing:
        print('bundle: %s imports %s, which does not exist' % (where, name), file=sys.stderr)
        return 1
    return 0


sys.exit(main())