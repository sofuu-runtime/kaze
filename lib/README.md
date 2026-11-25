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
| `lib/regex.kaze` | `regex_full_match`, `regex_search`, `regex_find_all`, `regex_replace`, `regex_group*` | bounded subset with capture groups, extraction & replacement |
| `lib/http.kaze` | `http_get`, `http_post`, `http_head`, `http_header`, `http_header_map`, `HttpResponse` | C path only; needs `KAZE_HTTP=1` and allowlist |

`lib/regex.kaze` evaluates reachable state sets without backtracking. It supports
literals, `.`, classes (`[a-z]`, `[^...]`), `\d \w \s \D \W \S \n \t \r`, hex
and Unicode escapes (`\xHH`, `\uHHHH`, `\u{...}`), capture groups `(...)` and
non-capturing groups `(?:...)`, group extraction (`regex_group`, `regex_group_span`,
`regex_group_count`), group replacement (`$0`..`$9`, `\0`..`\9`, `$$`), alternation,
quantifiers `* + ?`, and anchors `^` and `$`. Code points are handled via Unicode
escapes and `regex_is_utf8_boundary`. Counted repetition, lookaround, and backreferences
are typed syntax errors. Work is bounded by a step budget (`E_REGEX_BUDGET`), a
1024-byte pattern ceiling, and a 1 MiB text ceiling.

`lib/http.kaze` is a wrapper over the host's `http_fetch` builtin, so a program
that uses it must be run as C (`emit-c` / `kaze-host run`); `compile` refuses it
because format 5 has no HTTP opcode. The host grants the network explicitly:
`KAZE_HTTP=1`, an allowlist in `KAZE_HTTP_ALLOW`, a body ceiling in
`KAZE_HTTP_MAX_BYTES`, and an optional `KAZE_HTTP_CAINFO` bundle. Hostnames are
resolved and validated via `getaddrinfo` and `CURLINFO_PRIMARY_IP` to block
private/reserved IPs (127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16,
169.254.0.0/16, ::1) preventing SSRF and DNS rebinding attacks unless explicitly
granted. Structured response headers are accessible via `http_header_map`. An HTTP
error status is data (`ok` is true, `status` is 404), not a transport failure.

### Encodings, time, spatial maths and vector search (Phase 17)

Added by Phase 17. Each has a differential harness under `scripts/phase17/` that
generates seeded cases and compares every answer with CPython or NumPy; run one
with `python3 scripts/phase17/diff_<module>.py` and add `KAZE_ASAN=1` to build
the harness under the sanitizers.

| Module | Provides | Checked against |
| :--- | :--- | :--- |
| `lib/base64.kaze` | `base64_encode/decode`, URL-safe alphabet, strict padding contract | CPython `base64`, 1,225 cases |
| `lib/hex.kaze` | `hex_encode/decode`, byte/hex both ways | CPython `binascii`, 764 cases |
| `lib/url.kaze` | RFC 3986 parsing into `UrlParts`, percent encode/decode, query building | CPython `urllib.parse`, 898 cases |
| `lib/time.kaze` | monotonic and UTC clock, `time_duration_ms`, ISO 8601 formatting | CPython `datetime`/`date`, 16,959 cases |
| `lib/spatial.kaze` | `Vec3`, `Quat`, `Euler`, `Transform3D`, quaternion SLERP, 6-DoF rotation | NumPy, 100 cases |
| `lib/matrix.kaze` | LU determinant, Gauss-Jordan inverse with partial pivoting and a singular guard | NumPy `linalg`, 120 cases |
| `lib/dsp.kaze` | 1-D Kalman, multi-state filter, IIR low-pass | NumPy, 102 cases |
| `lib/heap.kaze` | arena-backed binary min/max priority queue | CPython `heapq`, 2,022 cases |
| `lib/embeddings.kaze` | `vec_dot`, cosine similarity, euclidean distance, `vec_top_k` | NumPy, 474 cases |

`lib/time.kaze` divides with floor semantics because Hinnant's
civil-from-days requires it; a truncating division returned an invalid date for
day numbers before the epoch (`time_civil_from_days(-719469)` is `0000-02-29`).

### Pervasive Multicore Parallelism

Pure collection transformations can be dispatched across worker threads with guaranteed data-race freedom:

- `parallel_map(collection, worker)`: transforms elements in parallel using pure routine `worker`.
- `parallel_filter(collection, predicate)`: filters elements in parallel using pure predicate `predicate`.
- `parallel for each <item> in <collection>:`: parallel loop syntax over collections.

The capability checker guarantees data-race freedom at compile time: any routine executed across worker threads must declare `needs: []`. Invoking impure routines or peripheral builtins inside parallel scopes triggers `E_KAZE_IMPURE_PARALLEL`. Each worker thread allocates from a private bump arena, which is adopted into the parent arena upon join in $O(1)$ time with zero lock contention and zero memory leaks under AddressSanitizer.

### Hardware Buses, Actuators & Avionics Drivers (Phase 14)

These modules provide production-grade, microsecond-precision hardware drivers and board support packages for bare-metal targets (STM32H7, RP2040) and host simulation. Access to peripheral registers is strictly gated at compile time behind `needs: regs`.

| Module | Provides | Needs | Notes |
| :--- | :--- | :--- | :--- |
| `lib/regs.kaze` | `reg_new`, `reg_read8/16/32`, `reg_write8/16/32`, `reg_field_*` | `regs` | Type-safe MMIO register and bitfield abstraction |
| `lib/pwm.kaze` | `pwm_plan`, `pwm_init`, `pwm_set_duty`, `pwm_set_pulse_ns` | `regs` | 400 Hz ESC timer planner and zero-allocation duty updates |
| `lib/dshot.kaze` | `dshot_frame`, `dshot_pulses`, `dshot_telemetry_decode` | `regs` | 600 kHz DShot framing, GCR line timing, bidirectional telemetry |
| `lib/systick.kaze` | `systick_clock_real/sim`, `systick_micros`, `systick_spin_us` | `regs` | 24-bit down-counter sub-microsecond real-time timebase |
| `lib/spi.kaze` | `spi_init`, `spi_read_burst`, `spi_transfer8` | `regs` | Polled SPI master with bounded timeouts and zero-allocation streaming |
| `lib/imu.kaze` | `imu_probe`, `imu_read_burst`, `imu_poll_run` | `regs` | 1–8 kHz 6-axis IMU burst polling with strict deadline checking |
| `lib/i2c.kaze` | `i2c_init`, `i2c_read_reg_bytes`, `i2c_write_reg_byte` | `regs` | Polled I2C master with register-pointer transactions |
| `lib/baro.kaze` | `baro_probe`, `baro_sample`, `baro_calc_pascals` | `regs` | 24-bit ratiometric barometric pressure/temperature sensor |
| `lib/can.kaze` | `can_init`, `can_transmit_frame`, `can_receive_frame` | `regs` | bxCAN controller driver (11-bit standard and 29-bit extended frames) |
| `lib/dronecan.kaze` | `dronecan_transfer_frames`, `dronecan_reassemble` | `[]` | DroneCAN (UAVCAN v0) message framing and CRC-16 reassembly |
| `lib/uart.kaze` | `uart_init`, `uart_write_bytes`, `uart_dma_send` | `regs` | USART telemetry port with computed BRR and zero-CPU DMA streaming |
| `lib/dma.kaze` | `dma_channel_new`, `dma_prepare_mem2periph`, `dma_start` | `regs` | DMA stream controller for high-throughput peripheral transfers |
| `lib/bsp_rp2040.kaze` | `bsp_rp2040_init`, `bsp_rp2040_clock_init` | `regs` | Raspberry Pi RP2040 125 MHz clock tree and peripheral bring-up |
| `lib/bsp_stm32h7.kaze` | `bsp_stm32h7_init`, `bsp_stm32h7_clock_init` | `regs` | STM32H7 Pixhawk 480 MHz clock tree, GPIOA, and bus bring-up |

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
every tick. A library routine is never the implicit entry (Kaze 1.0 entry rules).

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
