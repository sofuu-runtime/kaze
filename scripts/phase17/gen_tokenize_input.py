#!/usr/bin/env python3
"""Generate the Phase 17 tokenization input (deterministic, no network)."""
import json
import sys

path = sys.argv[1] if len(sys.argv) > 1 else "/tmp/kaze17_tokens.json"
rows = []
for i in range(60000):
    rows.append({
        "id": i,
        "name": "sensor-%08d" % i,
        "values": [i % 977, (i * 7) % 991, (i * 13) % 997],
        "ok": (i % 3) != 0,
        "note": 'telemetry frame %d with a quoted "inner" token' % i,
    })
with open(path, "w") as f:
    json.dump(rows, f, separators=(",", ":"))
print(path)
