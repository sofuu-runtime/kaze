#!/usr/bin/env python3
"""Regenerate the verification-surface table in docs/PHASE-1-6-REPORT.md.

Run it after adding or removing tests so the report cannot drift from the code.
"""
import glob, io, os, re, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPORT = os.path.join(ROOT, 'docs', 'PHASE-1-6-REPORT.md')
FILES = [
    ('tests/strict_semantics.rs', 'strict semantics'),
    ('tests/units.rs', 'units'),
    ('tests/contracts.rs', 'contracts'),
    ('tests/diagnostics.rs', 'diagnostics'),
    ('tests/emitted_c.rs', 'emitted C'),
    ('tests/frontend.rs', 'frontend'),
    ('tests/capability_safety.rs', 'capability safety'),
    ('tests/effector_safety.rs', 'effector safety'),
    ('tests/modules.rs', 'modules'),
    ('tests/lsp.rs', 'LSP'),
    ('tests/compat.rs', 'compat'),
]

def main():
    rows = []
    for path, label in FILES:
        full = os.path.join(ROOT, path)
        if not os.path.exists(full):
            continue
        rows.append('| `%s` | %s | %d |' % (path, label, io.open(full, encoding='utf-8').read().count('#[test]')))
    fixtures = len(glob.glob(os.path.join(ROOT, 'tests', 'fixtures', 'strict', '*.kaze')))
    rows.append('| `tests/fixtures/strict/` | intentionally flawed programs | %d |' % fixtures)
    text = io.open(REPORT, encoding='utf-8').read()
    start = text.index('| File | Area | Tests |')
    end = text.index(chr(10) + chr(10), start)
    text = text[:start] + chr(10).join(['| File | Area | Tests |', '| :--- | :--- | ---: |'] + rows) + text[end:]
    io.open(REPORT, 'w', encoding='utf-8').write(text)
    print('audit-counts: %d rows refreshed' % len(rows))
    return 0

sys.exit(main())