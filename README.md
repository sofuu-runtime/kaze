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

With any standard C99 compiler (clang or gcc), build and run the standalone Kaze implementation from the `kaze` directory:

```bash
# Build the self-hosted Kaze engine
cc -std=c99 -O3 -o bin/kaze dist/kaze.c

# Check and run
./bin/kaze check hello.kaze
./bin/kaze run hello.kaze
```

Here, `hello.kaze` contains the example above. It returns `5` and needs no
external capabilities.

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
- **Bounded Text and Authorized Network**: A byte-oriented regular-expression engine (`lib/regex.kaze`) with a typed step budget, and an HTTP/HTTPS client (`lib/http.kaze`) over an explicit host grant, destination allowlist, body ceiling, and verified TLS.
- **Native AI Tensors & GGUF Runtime**: Zero-copy mmap ingestion of GGUF and SafeTensors model weights, quantized matrix math (Q4_K, Q8_0), LLaMA forward pass achieving strict parity with `llama.cpp` (logit abs diff < 1e-4), reverse-mode autograd tape, and Apple Silicon Metal MSL GPU shaders.
- **Deterministic Concurrency**: Single-threaded cooperative fibers, bounded tasks, and channel synchronization with sub-50ns context switching and zero data races.
- **Developer Tooling**: Full Language Server Protocol (LSP) daemon, interactive REPL, step debugger, and instruction fuel profiler.

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
- [Libraries, tooling and parallelism report](docs/STDLIB-TOOLING-PARALLELISM-REPORT.md)
- [Roadmap](docs/ROADMAP.md)
- [Version alignment plan](docs/VERSION-ALIGNMENT.md)
- [Examples](examples/)

## License

[MIT](LICENSE)
