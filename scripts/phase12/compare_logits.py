#!/usr/bin/env python3
"""compare_logits.py — Phase 12 numerical parity gate.

Compares two logits dumps written as: int32 n_vocab followed by n_vocab float32.
Usage: compare_logits.py <reference.bin> <kaze.bin> [tolerance]
Exit 0 when max |diff| <= tolerance and the argmax agrees.
"""
import struct
import sys

import numpy as np


def load(path):
    with open(path, "rb") as f:
        raw = f.read(4)
        if len(raw) != 4:
            raise SystemExit(f"{path}: empty logits file")
        n = struct.unpack("<i", raw)[0]
        data = np.frombuffer(f.read(n * 4), dtype=np.float32)
        if data.size != n:
            raise SystemExit(f"{path}: truncated logits file")
        return data


def main():
    if len(sys.argv) < 3:
        raise SystemExit("usage: compare_logits.py <reference.bin> <kaze.bin> [tolerance]")
    tol = float(sys.argv[3]) if len(sys.argv) > 3 else 1e-4
    ref = load(sys.argv[1])
    got = load(sys.argv[2])
    if ref.shape != got.shape:
        raise SystemExit(f"shape mismatch: {ref.shape} vs {got.shape}")
    diff = np.abs(ref.astype(np.float64) - got.astype(np.float64))
    idx = int(np.argmax(diff))
    ref_top = int(np.argmax(ref))
    got_top = int(np.argmax(got))
    print(f"parity: n={ref.size} max_abs_diff={diff.max():.6g} at {idx} "
          f"(ref {ref[idx]:.6f} kaze {got[idx]:.6f})")
    print(f"parity: mean_abs_diff={diff.mean():.6g} rms={np.sqrt((diff ** 2).mean()):.6g}")
    print(f"parity: argmax ref={ref_top} ({ref[ref_top]:.6f}) "
          f"kaze={got_top} ({got[got_top]:.6f}) "
          f"{'MATCH' if ref_top == got_top else 'MISMATCH'}")
    print(f"parity: top5 ref={np.argsort(-ref)[:5].tolist()}")
    print(f"parity: top5 kaze={np.argsort(-got)[:5].tolist()}")
    ok = diff.max() <= tol and ref_top == got_top
    print(f"parity: {'PASS' if ok else 'FAIL'} (tolerance {tol:g})")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
