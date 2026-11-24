#!/usr/bin/env python3
"""make_safetensors_fixture.py — write a tiny SafeTensors file for the Phase 12 gate.

Layout: 8-byte little-endian header length, JSON header, then raw little-endian
tensor data. Two tensors: a f32 [2,3] and b f16 [3].
"""
import json
import struct
import sys

path = sys.argv[1]
a = [0.5, 1.0, 1.5, 2.0, 2.5, 3.0]
b = [1.5, -2.25, 0.75]
a_raw = struct.pack("<6f", *a)
b_raw = struct.pack("<3e", *b)
header = {
    "a": {"dtype": "F32", "shape": [2, 3], "data_offsets": [0, len(a_raw)]},
    "b": {"dtype": "F16", "shape": [3],
          "data_offsets": [len(a_raw), len(a_raw) + len(b_raw)]},
    "__metadata__": {"format": "pt"},
}
blob = json.dumps(header, separators=(",", ":")).encode("utf-8")
while len(blob) % 8 != 0:
    blob += b" "
with open(path, "wb") as f:
    f.write(struct.pack("<Q", len(blob)))
    f.write(blob)
    f.write(a_raw)
    f.write(b_raw)
print(f"wrote {path}: header {len(blob)} bytes, data {len(a_raw) + len(b_raw)} bytes")
