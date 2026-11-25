# Kaze

Kaze is a programming language for AI and machines.

It is designed for AI applications, agents, robotics, drones, and other
AI-powered systems. Its goal is to be general-purpose within AI, not across
all computing. Support for individual applications and devices varies.

## Example

```kaze
action main():
    return 2 + 3
```

With any standard C99 compiler (clang or gcc), build the unified Kaze CLI from
the `kaze` directory. The one executable owns checking, running, building,
formatting and verification; it needs no sibling compiler binary at run time.

```bash
# Build bin/kaze, with the compiler, verifier and runtime embedded
bash scripts/build-cli.sh

# Check and run
./bin/kaze check hello.kaze
./bin/kaze run hello.kaze

# Desktop fast path: QBE SSA -> assembly -> link, no C compiler in the hot path
bash scripts/build-native.sh
./bin/kaze run --backend=native hello.kaze
```

Here, `hello.kaze` contains the example above. It returns `5` and needs no
external capabilities.

To install it for use outside the checkout:

```bash
bash scripts/install.sh                 # ~/.local, no sudo
PREFIX=/usr/local bash scripts/install.sh
```

The installed tree is self-contained (`bin/kaze`, `lib/kaze/*.a`,
`share/kaze/std`, `share/kaze/grammars`). `scripts/install.sh --uninstall`
removes it again.

The same command surface works in-tree and after installation:

```bash
kaze check src/main.kaze --json   # diagnostics; exit 1 on E_KAZE_*
kaze fmt --check src              # exit 1 when a file would change
kaze test                         # every tests/**/*_test.kaze suite
kaze verify src/main.kaze         # contracts and proof obligations
```

`bash scripts/cli-dx-gates.sh` exercises the installed binary from an isolated
prefix with every `KAZE_*` variable cleared.

## The language, shortly

A routine names what it may touch, and the checker refuses what it cannot prove:

```kaze
needs: log

action duty_check(duty: percent) -> bool:
    requires duty >= 0
    guard duty <= 100
    let reading = sensor.temperature() otherwise 0 Celsius
    if reading > 30 Celsius and duty > 50:
        log "hot and loaded"
        return true
    return false
```

- `requires`/`ensures` are contracts; `guard` is an executed assertion.
- A division, a dynamic index or a device read the checker cannot prove safe
  needs an inline `otherwise <fallback>`.
- `while` loops declare `bounded <n>`; recursive routines declare
  `recursion bounded <n>`.
- Numbers carry physical dimensions (`1 km + 500 meters` is `1500`), checked at
  compile time and free at run time.
- Parameters resolve as `--arg` > `AGENT_<PARAM>` > `--config` > signature default.

## Architecture & Subsystems

- **Pure Self-Hosted Compiler**: The Kaze compiler is written in pure Kaze (`compiler/kaze_self.kaze`) and bootstraps into C99 (`dist/kaze.c`), verified via exact Stage 2 == Stage 3 SHA-256 fixed-point verification.
- **Zero-Heap Standard Library**: Arena-backed linear collections (`lib/vector.kaze`, `lib/map.kaze`, `lib/text.kaze`, `lib/ring.kaze`), RFC 8259 JSON parser, binary wire protocols (Protobuf, MAVLink, CRC), and cryptographic suites (SHA-256, SHA-512, HMAC, Ed25519).
- **Bounded Text & Secure Network**: A non-backtracking regular-expression engine (`lib/regex.kaze`) with capture groups `(...)`, group extraction APIs, `$0`..`$9` replacement, Unicode escapes (`\uHHHH`, `\u{...}`), and step budget protection; and an HTTP/HTTPS client (`lib/http.kaze`) over explicit host grants, destination allowlists, pre-DNS and post-connect IP validation against SSRF / DNS rebinding, structured header mapping, and verified TLS.
- **Native AI Tensors & Multicore GGUF Runtime**: Zero-copy mmap ingestion of GGUF and SafeTensors model weights, quantized matrix math (Q4_K, Q8_0), multicore row-partitioned parallel GEMM (`dist/kaze_tensor.c`), LLaMA forward pass achieving strict parity with `llama.cpp` (logit abs diff < 1e-4), reverse-mode autograd tape, and Apple Silicon Metal MSL GPU shaders.
- **Unboxed Scalar Code Generation & Binary Slicing**: Numeric routines are emitted with unboxed C locals and operators (`double`, `long long`, `uint8_t`) instead of 32-byte tagged values, with whole-program dead-code elimination from `main`, supervisor hooks and fibers. Text parameters and byte reads go through a `kz_str_view_t` view, so a scan allocates once at the file boundary and never per byte. Measured against `clang -O3` C99 with identical IEEE semantics: N-body 1,000,000 steps at **0.98x**, 7.9 MB JSON tokenization at **0.90x**, with bit-identical output; `examples/hello.kaze` builds to a **34 KB** stripped binary. Gates: `bash scripts/phase17-gates.sh`.
- **Pervasive Multicore Parallelism**: First-class `parallel for each`, `parallel_map`, and `parallel_filter` primitives powered by compile-time capability purity verification (`needs: []`). Dynamic work-stealing thread pool over worker-isolated bump arenas with $O(1)$ block chain adoption (`kaze_arena_adopt`), zero allocation contention, 0 memory leaks under AddressSanitizer, and up to $54.8\times$ multicore speedup.
- **Deterministic Concurrency**: Single-threaded cooperative fibers, bounded tasks, and channel synchronization with sub-50ns context switching and zero data races.
- **Production Hardware Buses & Bare-Metal Real-Time**: Memory-mapped register abstraction (`needs: regs`), 400 Hz PWM, 600 kHz DShot ESC driver, SysTick timebase, 1–8 kHz SPI IMU burst polling (0 overruns across 1,000,000 cycles), I2C barometer, bxCAN / DroneCAN (UAVCAN v0), UART DMA, and board support packages for STM32H7 (Pixhawk) and RP2040 (Raspberry Pi Pico) targeting bare-metal ARM Cortex-M with 20 ns PWM jitter.
- **SITL Physics Simulator & Digital Twin**: Direct UDP MAVLink 2.0 bridge (`dist/kaze_sitl_client.c`, `dist/kaze_mavlink.*`), 3D guided quad aerodynamics, sensor simulation (GPS jitter, baro drift, magnetometer EMI, IMU vibration noise), wind gusts, updrafts, and automated headless mission regression validating zero flight violations and 100% emergency RTL on motor cutout and link loss.
- **Formal Verification & SMT/Z3 Proofs**: Pure-Kaze verifier (`bin/kaze-verify`) extracting linear forms, loop-counter invariants, interval bounds, and contract obligations (`requires` $\Rightarrow$ `ensures`) into SMT-LIB2 and Horn formulas. Embedded Fourier-Motzkin prover and Z3 bridge certifying 17/17 stdlib modules (< 50ms/module) and driving proof-carrying builds that elide over 80% of runtime boundary checks.
- **Developer Tooling**: Full Language Server Protocol (LSP) daemon, interactive REPL, step debugger with source-line mapping (`#line`) and line breakpoints, and instruction fuel profiler.

## Design

- Kaze is independently designed and self-hosted; the compiler compiles down to portable C99 and native bytecode.
- External operations require declared capabilities, authorization, and an
  available backend. Hardware support and safety depend on the specific host.
- Kaze has human-oriented and machine-oriented source syntax. The standalone
  implementation includes conversion commands; supported behavior and
  compatibility should be verified for each use case.

This directory contains standalone Kaze. Sofuu also has a separate embedded
Kaze implementation; their supported features and artifacts may differ.

## Learn More

- [Language reference and current support](docs/KAZE-1.0-REFERENCE.md)
- [Standard library](lib/README.md)
- [Hardware buses and bare-metal real-time report](docs/PHASE-14-REPORT.md)
- [SITL physics simulator and digital twin report](docs/PHASE-15-REPORT.md)
- [Formal verification and SMT solver report](docs/PHASE-16-REPORT.md)
- [Unboxing, zero-copy views, dead-code elimination and stdlib expansion report](docs/PHASE-17-REPORT.md)
- [Pervasive parallelism, performance and stdlib master plan](docs/PLAN-KAZE-PERVASIVE-PARALLELISM-PERFORMANCE-STDLIB.md)
- [Roadmap](docs/ROADMAP.md)
- [Version alignment plan](docs/VERSION-ALIGNMENT.md)
- [Examples](examples/)

## License

[MIT](LICENSE)
