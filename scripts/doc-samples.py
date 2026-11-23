#!/usr/bin/env python3
"""Every ```kaze block in the product docs must parse.

Snippets that show *planned* syntax are fenced as ```text instead, so anything
fenced as ```kaze is a claim that this is valid Kaze. Fragments that are not whole
programs are skipped (a block that does not declare a top-level item).
"""
import glob, os, re, subprocess, sys, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KAZE = os.path.join(ROOT, 'target', 'debug', 'kaze')
BLOCK = re.compile(r'```kaze\n(.*?)```', re.S)
DECL = re.compile(r'(?m)^(action|when|every|check|filter|routine|record)\b')

def main():
    if not os.path.exists(KAZE):
        print('doc-samples: the kaze binary is not built; skipping')
        return 0
    paths = [p for p in sorted(glob.glob(os.path.join(ROOT, 'docs', '**', '*.md'), recursive=True)) if 'kaze-plan' not in p]
    checked = skipped = failed = 0
    for path in paths:
        text = open(path, encoding='utf-8', errors='replace').read()
        for m in BLOCK.finditer(text):
            body = m.group(1)
            if not DECL.search(body):
                skipped += 1
                continue
            checked += 1
            with tempfile.NamedTemporaryFile('w', suffix='.kaze', delete=False, encoding='utf-8') as fh:
                fh.write(body + '\n')
                tmp = fh.name
            quiet = subprocess.run([KAZE, 'fmt', tmp], capture_output=True, text=True)
            os.unlink(tmp)
            if quiet.returncode != 0:
                failed += 1
                head = (quiet.stderr or quiet.stdout).strip().split('\n')[0][:110]
                print('doc-sample-fail %s: %s' % (os.path.relpath(path, ROOT), head))
    print('doc-samples: %d checked, %d skipped (fragments), %d failed' % (checked, skipped, failed))
    return 1 if failed else 0

sys.exit(main())