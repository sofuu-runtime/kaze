# 03. Type System, Linear Ownership & Dimensional Physics Units

> **Status:** Architecture Specification & Implementation Plan  
> **Core Innovation:** First-class physical units with compile-time dimensional algebra and linear ownership without lifetime syntax.

---

## 1. Type System Overview

Kaze features a strong, statically checked, domain-agnostic type system:

```text
Types (Ty)
├── Primitives
│   ├── num              (64-bit IEEE 754 floating point / 32-bit float profile)
│   ├── str              (immutable UTF-8 string)
│   └── bool             (true or false)
├── Spatial & Physical
│   ├── point            (3D coordinate: latitude, longitude, altitude)
│   └── num with Unit    (e.g., num<meters>, num<m/s>, num<ms>)
├── Composite
│   ├── list of T        (homogeneous dynamically sized sequence)
│   └── record Name      (statically typed unboxed struct with named fields)
└── Fallback / Dynamic
    ├── any              (unconstrained untrusted border type)
    └── nil              (absence of value, strictly opt-in)
```

---

## 2. Linear Ownership & Arena Lifetimes (No Lifetime Sigils)

Rust's primary friction is lifetime syntax: `<'a, 'b: 'a, T: Clone + 'static>`. It requires the programmer to manually annotate pointer relationships.

Kaze eliminates lifetime annotations while retaining 100% memory safety through **Linear Arena Scoping**:

```kaze
# All temporary allocations inside this routine live in a local linear arena
action process_telemetry(raw_data: str):
    let tokens = tokenize(raw_data)
    let ast = parse(tokens)
    return ast.summary   # Only the returned value is promoted or copied
    # The entire arena containing 'tokens' and intermediate AST nodes is wiped in 1 instruction
```

### Safety Guarantees:
1. **Zero Garbage Collection Pauses:** No mark-and-sweep, no stop-the-world GC latency spikes.
2. **Zero Use-After-Free:** Memory cannot outlive its owning scope.
3. **Zero Lifetime Annotations:** The compiler automatically determines allocation lifetimes based on routine scopes.

---

## 3. First-Class Dimensional Physics Units

In robotics and physical systems, software bugs often arise from unit mismatches (e.g., the Mars Climate Orbiter loss caused by pound-force versus newtons).

Kaze treats physical units as **first-class types**, verified at compile time with dimensional algebra:

| Base Dimension | Units Supported | Canonical Representation |
| :--- | :--- | :--- |
| **Length** | `meters`, `m`, `km`, `cm`, `mm` | SI Base: meters ($m$) |
| **Time** | `ms`, `seconds`, `s`, `minutes`, `min`, `hours`, `h` | SI Base: milliseconds ($ms$) |
| **Velocity** | `m/s`, `km/h` | Derived: $m / s$ |
| **Angle** | `degrees`, `deg`, `radians`, `rad` | Base: degrees ($^\circ$) |
| **Ratio** | `percent`, `%` | Fraction: $0.0 .. 1.0$ |
| **Temperature** | `celsius`, `C` | Base: Celsius |

---

## 4. Compile-Time Dimensional Algebra

The Kaze compiler evaluates the arithmetic of physical units using algebraic cancellation:

### Valid Dimensional Operations:
```kaze
let dist = 100 meters
let time = 10 seconds

# Division produces derived velocity unit:
let speed = dist / time              # Type: num<m/s> (Value: 10 m/s)

# Multiplication cancels denominator:
let traveled = speed * 5 seconds     # Type: num<meters> (Value: 50 meters)

# Scaling by dimensionless number:
let double_dist = dist * 2           # Type: num<meters> (Value: 200 meters)

# Unit conversion:
let dist_km = 1500 meters in km      # Type: num<km> (Value: 1.5 km)
```

### Compile-Time Incompatibility Rejections:
```kaze
let dist = 100 meters
let duration = 5 seconds

# INVALID: Cannot add distance to time
let bad = dist + duration
# COMPILE ERROR: E_KAZE_DIMENSION_MISMATCH: cannot add 'num<meters>' and 'num<seconds>'.

# INVALID: Comparing velocity to length
if speed > 100 meters:
# COMPILE ERROR: E_KAZE_DIMENSION_MISMATCH: cannot compare 'num<m/s>' with 'num<meters>'.
```

---

## 5. Record Types (Unboxed Structs)

Records in Kaze are defined with human sentence-like syntax:

```kaze
record TelemetryPacket:
    timestamp: num
    altitude: num
    speed: num
    battery: num

action make_packet(ts, alt, spd, bat):
    return TelemetryPacket(
        timestamp: ts,
        altitude: alt,
        speed: spd,
        battery: bat
    )
```

### Compiler Layout:
* Records are lowered to unboxed C `struct` types with standard word alignment.
* Field accesses (`pkt.altitude`) are resolved to direct offset lookups ($O(1)$ pointer math), matching raw C struct performance.

---

## 6. Implementation Milestones

- [ ] **W1.Units:** Add unit dimensional tags to `Ty::Num(Unit)` in `ast.kaze` and `checker.kaze`.
- [ ] **W1.Algebra:** Implement dimensional arithmetic rules in `checker.rs` and `kaze/compiler/parser.kaze`:
  - `Length / Time` $\rightarrow$ `Velocity`
  - `Velocity * Time` $\rightarrow$ `Length`
  - Incompatible additions/subtractions $\rightarrow$ emit `E_KAZE_DIMENSION_MISMATCH`.
- [ ] **W1.Records:** Expand record declarations to support typed fields with range constraints.
- [ ] **W1.Linear:** Implement local arena scope cleanup in `codegen.kaze`.
