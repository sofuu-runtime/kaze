#!/usr/bin/env python3
"""Compare two engine outputs: identical text, or equal to a tolerance when both
sides are numbers; a refusal counts as equal when the diagnostic code matches."""
import re, sys

num = re.compile(r'-?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?')

def clean(line):
    return line.rstrip('\n')

def same(a, b):
    a, b = clean(a), clean(b)
    if a == b:
        return True
    ca = re.match(r'\s*(E_[A-Z_]+)', a)
    cb = re.match(r'\s*(E_[A-Z_]+)', b)
    if ca and cb:
        return ca.group(1) == cb.group(1)
    na = num.findall(a)
    nb = num.findall(b)
    if len(na) != len(nb) or not na:
        return False
    # the text around the numbers must match too
    if num.sub('#', a) != num.sub('#', b):
        return False
    for x, y in zip(na, nb):
        try:
            fx, fy = float(x), float(y)
        except ValueError:
            return False
        scale = max(abs(fx), abs(fy), 1.0)
        if abs(fx - fy) / scale > 1e-9:
            return False
    return True

def main():
    left = open(sys.argv[1]).read().split('\n')
    right = open(sys.argv[2]).read().split('\n')
    if len(left) != len(right):
        print('line count differs: %d vs %d' % (len(left), len(right)))
        return 1
    bad = 0
    for i, (a, b) in enumerate(zip(left, right)):
        if not same(a, b):
            bad += 1
            if bad <= 3:
                print('  line %d:\n    rust: %s\n    kaze: %s' % (i + 1, a, b))
    return 1 if bad else 0

sys.exit(main())