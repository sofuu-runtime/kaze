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

With Rust and Cargo installed, run this standalone implementation from the
`kaze` directory:

```bash
cargo run -- check hello.kaze
cargo run -- run hello.kaze
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

See [docs/SPEC.md](docs/SPEC.md) for the contract, [docs/STRICTNESS.md](docs/STRICTNESS.md)
for the strictness rules and [docs/PHASE-1-6-REPORT.md](docs/PHASE-1-6-REPORT.md)
for what the last roadmap round landed.

## Design

- Kaze is independently designed and implemented; the current compiler is
  written in Rust. Documented external providers may be used where needed.
- External operations require declared capabilities, authorization, and an
  available backend. Hardware support and safety depend on the specific host.
- Kaze has human-oriented and machine-oriented source syntax. The standalone
  implementation includes conversion commands; supported behavior and
  compatibility should be verified for each use case.

This directory contains standalone Kaze. Sofuu also has a separate embedded
Kaze implementation; their supported features and artifacts may differ.

## Learn More

- [Language reference and current support](docs/KAZE-1.0-REFERENCE.md)
- [Roadmap](docs/ROADMAP.md)
- [Version alignment plan](docs/VERSION-ALIGNMENT.md)
- [Examples](examples/)

## License

[MIT](LICENSE)
