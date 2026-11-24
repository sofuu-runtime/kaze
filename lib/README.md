# Kaze Standard Library (v1.0)

Canonical, composable Kaze modules. Every file is a normal module: `use` it,
merge its `needs:`, and select a routine with `--entry` or let its hooks run.
The three robotics modules below are the original mission library; the data,
text and wire-format modules are used by both missions and ordinary programs.

| Module | Provides | Needs |
| :--- | :--- | :--- |
| `lib/safety.kaze` | Low-battery and link-loss RTL hooks, both best-effort | `drone`, `log` |
| `lib/guardrail.kaze` | `filter user_prompt(prompt)` — length + secret checks | `policy` |
| `lib/survey.kaze` | `action eight_point_survey()` — arm, climb, 8 waypoints, RTL | `drone`, `log` |

### Data, text and wire formats

These modules are self-contained (`needs:` is empty). Each has a suite under
`tests/stdlib/`; `bash scripts/stdlib-test.sh` runs all thirteen through the C
path. The format-5 bytecode writer is a separate surface and is not claimed
here.

| Module | Provides | Notes |
| :--- | :--- | :--- |
| `lib/vector.kaze` | `vector_new/push/get/set/size`, slice, search, sort helpers | dynamic vectors over lists |
| `lib/text.kaze` | `text_length/char_at/slice/index_of/contains/split/trim/replace` | fallible indexes always carry `otherwise` |
| `lib/map.kaze` | `map_new/set/get/has/delete/keys/size` | linear-probing hash map, deterministic order |
| `lib/ring.kaze` | `ring_new/push/pop/peek/size/full` | fixed capacity, never resizes |
| `lib/json.kaze` | `json_parse`, `json_stringify`, `JVal` accessors | strict RFC 8259, depth 64, ordered members |
| `lib/crc.kaze` | `crc16_ccitt`, `crc16_xmodem`, accumulate/step forms | table-free, 16-bit |
| `lib/protobuf.kaze` | varint, zigzag, `field_varint/bytes`, decode | schema-free wire format |
| `lib/mavlink.kaze` | `mavlink2_frame`, `mavlink2_parse` | MAVLink 2.0, unsigned frames |
| `lib/sha256.kaze` | `sha256`, `sha256_hex`, `sha256_of_bytes` | FIPS 180-4 |
| `lib/sha512.kaze` | `sha512`, `sha512_hex` | 64-bit words as `[hi, lo]` pairs |
| `lib/hmac.kaze` | `hmac_sha256`, `hmac_sha256_hex` | RFC 2104, 64-byte block |
| `lib/ed25519.kaze` | `ed25519_verify`, `ed25519_verify_bytes` | RFC 8032 verification only |
| `lib/regex.kaze` | `regex_full_match`, `regex_search`, `regex_find_all`, `regex_replace` | bounded subset; see limits below |
| `lib/http.kaze` | `http_get`, `http_post`, `http_head`, `http_header`, `HttpResponse` | C path only; needs `KAZE_HTTP=1` |

`lib/regex.kaze` matches bytes, not code points. It has literals, `.`, classes,
`\d \w \s` and their negations, grouping, alternation, `* + ?`, `^` and `$`;
counted repetition, lookaround, backreferences and lazy quantifiers are syntax
errors, not silently different matches. Work is bounded by a step budget
(`E_REGEX_BUDGET`), a 1024-byte pattern limit and a 1 MiB text limit.

`lib/http.kaze` is a wrapper over the host's `http_fetch` builtin, so a program
that uses it must be run as C (`emit-c` / `kaze-host run`); `compile` refuses it
because format 5 has no HTTP opcode. The host grants the network explicitly:
`KAZE_HTTP=1`, an allowlist in `KAZE_HTTP_ALLOW`, a body ceiling in
`KAZE_HTTP_MAX_BYTES`, and an optional `KAZE_HTTP_CAINFO` bundle. A `use` line
grants no authority, and an HTTP error status is data (`ok` is true, `status` is
404), not a transport failure.

## Compose a mission

```kaze
use "lib/safety.kaze"
use "lib/survey.kaze"

needs: log

action main():
    log "composed mission: select eight_point_survey with --entry"
```

```bash
kaze run mission.kaze --entry eight_point_survey \
  --cap drone --cap log --ticks 2 --tick-ms 100 --plain
```

`--entry eight_point_survey` runs the library routine; the safety hooks run on
every tick. A library routine is never the implicit entry (v0.5 entry rules).

## Guardrail

```bash
kaze run mission.kaze --entry user_prompt \
  --arg prompt='"my api_key is sk-live-123"' --json
```

The report's `verdict` is `Reject` for a leaked credential and `Allow` for a
normal prompt.

## Rules the library follows

- `needs:` is explicit in every file; the loader merges and deduplicates it.
- No file depends on another library file (no import cycles, no hidden
  ordering).
- Hooks are idempotent: a lost link can fire the RTL hook on every tick.
- The RTL in the safety hooks is wrapped in `try ... otherwise`; losing the
  link must not lose the supervisor.
- Every item keeps its file origin, so a failure names `lib/safety.kaze`, not
  the mission that imported it.
