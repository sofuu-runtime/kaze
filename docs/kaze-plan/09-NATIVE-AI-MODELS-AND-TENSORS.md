# Kaze Plan 09: Native AI Models, Tensors & Neural Computation

> **The Core Maxim:** *"Zero-overhead, shape-verified, panic-free neural computing from high-level syntax to bare metal."*  
> **Status:** Architecture Blueprint & Extension Roadmap  
> **Parent Blueprint:** [README.md](./README.md)

---

## 1. Vision & Motivation

Modern artificial intelligence software suffers from a stark bifurcation:

* **The Python/PyTorch Tier:** Developer-friendly and expressive, but burdened by heavy runtime overhead, Global Interpreter Lock (GIL) contention, unpredictable garbage collection pauses, multi-gigabyte dependency trees, and **runtime shape mismatch crashes** that occur after hours or days of execution.
* **The C++/CUDA Tier:** Blazing fast and capable of bare-metal GPU control, but plagued by unsafe raw pointer arithmetic, segmentation faults, silent memory corruption, platform lock-in, and complex boilerplate.

Kaze is architecturally positioned to unify these worlds. By pairing **high-level, sentence-like readability** with **zero-GC linear arena memory**, **compile-time dimensional shape algebra**, and **deterministic ANSI C99/SIMD emission**, Kaze enables developers and autonomous agents to author, train, and run neural network models with the speed of C and safety surpassing Rust.

---

## 2. Core Architectural Pillars for AI in Kaze

```
┌────────────────────────────────────────────────────────────────────────┐
│                   KAZE HIGH-LEVEL NEURAL DEFINITION                    │
│   • Readable layer syntax              • Dual-syntax machine S-exprs   │
│   • Explicit dimensional shapes        • Mandatory numerical fallbacks │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│              COMPILE-TIME TENSOR SHAPE & NUMERICAL CHECKER             │
│   • Static shape inference [B, S, D]   • Inner-dimension verification  │
│   • Zero-cost dimension cancellations  • Zero-panic arithmetic proofs  │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│              AUTOMATIC DIFFERENTIATION ENGINE (AUTOGRAD)               │
│   • Symbolic reverse-mode derivation   • Static backward pass synthesis│
│   • Zero runtime computation graph     • Fused forward-backward passes │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│              NATIVE C99 / SIMD / GPU LOWERING ENGINE                   │
│   • Linear bump arena allocations      • ARM NEON & x86 AVX2/AVX-512   │
│   • Apple Metal (MSL) & WebGPU shaders • Zero-copy GGUF/SafeTensors mmap│
└────────────────────────────────────────────────────────────────────────┘
```

---

### Pillar 1: Zero-GC Linear Bump Arena for Tensor Memory

In conventional runtimes, creating millions of intermediate activation tensors and attention maps triggers frequent garbage collection cycles, inducing severe latency jitter.

In Kaze:
1. **Per-Pass Working Arenas**:
   - Forward and backward passes execute within a dedicated linear bump arena (`kaze_arena_t`).
   - Every intermediate activation tensor, scratch matrix, and normalized vector is allocated sequentially via $O(1)$ pointer increments.
2. **Instantaneous Bulk Reclamation**:
   - Once a batch forward-backward pass or inference step completes, the arena offset resets to zero.
   - **Cost:** $O(1)$ pointer assignment. Memory fragmentation is mathematically zero, and GC sweeps are completely eliminated.
3. **Persistent Weight Arenas**:
   - Model parameters (weights and biases) reside in a separate read-only or optimizer-managed persistent arena, isolated from ephemeral activations.

---

### Pillar 2: Compile-Time Tensor Dimension Algebra

In Python, multiplying incompatible matrix shapes (e.g., `[32, 512]` by `[256, 128]`) results in a runtime crash:
`RuntimeError: mat1 and mat2 shapes cannot be multiplied`.

In Kaze, our **Phase 3 Dimensional Unit Algebra** is extended to **Tensor Dimension Algebra**:

```kaze
# Tensor type declaration with symbolic dimension variables
type Matrix<T, Rows: dim, Cols: dim> = tensor<T, [Rows, Cols]>

routine linear_forward(
  input: tensor<float, [Batch, InDim]>,
  weights: tensor<float, [InDim, OutDim]>,
  bias: tensor<float, [OutDim]>
) -> tensor<float, [Batch, OutDim]>:
  requires InDim > 0 and OutDim > 0
  
  # The compiler verifies that inner dimension InDim matches
  let product = input * weights
  return product + bias
```

#### Compile-Time Verification Rules:
1. **Matrix Multiplication**:
   $$\text{tensor}\langle T, [B, S, D]\rangle \times \text{tensor}\langle T, [D, H]\rangle \longrightarrow \text{tensor}\langle T, [B, S, H]\rangle$$
   If the inner dimension $D$ does not unify at compile time, the compiler issues `error[E_KAZE_SHAPE_MISMATCH]` before any code is generated or executed.
2. **Broadcasting Rules**:
   - Tensors can only broadcast across dimensions that are statically known to be $1$ or algebraically identical.
3. **Zero Runtime Cost**:
   - Dimensions exist exclusively within the static type checker. Emitted C99 code works with raw strided pointers and pre-calculated offset strides with zero descriptor overhead.

---

### Pillar 3: Total Numerical Correctness & Panic-Free Activations

Neural networks frequently encounter floating-point instability:
* Division-by-zero during variance calculation in `LayerNorm` or `RMSNorm`.
* Overflow/underflow producing `NaN` or `Inf` in `Softmax` or exponential activations.
* Out-of-bounds vocabulary lookups in token embedding tables.

Kaze applies **Phase 2 Ultra-Strict Semantics** to neural operations:

```kaze
routine rms_norm(x: tensor<float, [B, S, D]>, weight: tensor<float, [D]>, eps: float = 1e-5) -> tensor<float, [B, S, D]>:
  requires eps > 0.0
  
  let mean_square = (x * x).mean(axis: -1)
  # Mandatory fallback prevents division by zero if variance is zero:
  let scale = 1.0 / sqrt(mean_square + eps) otherwise 0.0
  return (x * scale) * weight
```

If an engineer attempts an unguarded division or an out-of-bounds embedding lookup without an `otherwise` fallback, compilation is rejected with `error[E_KAZE_MISSING_FALLBACK]`.

---

### Pillar 4: Symbolic Autograd & Compile-Time Reverse-Mode Differentiation

Unlike PyTorch, which builds a heavy dynamic computation graph in heap memory on every forward pass, Kaze performs **Compile-Time Symbolic Differentiation**:

1. **Declarative Layer Definitions**:
   The engineer writes only the forward pass.
2. **Static Backward Derivation**:
   The Kaze compiler inspects the AST of the forward pass, applies the chain rule symbolically, and emits the exact C99 backward function during compilation.
3. **Fused Passes**:
   Forward and backward loops can be fused at the C level, maximizing CPU L1/L2 cache locality and minimizing memory bandwidth.

---

### Pillar 5: Zero-Copy Weight Ingestion (GGUF & SafeTensors)

Kaze integrates zero-copy binary tensor loaders directly into the runtime substrate:

* **Memory-Mapped Weight Ingestion**:
  ```kaze
  routine load_llama_weights(path: string) -> ModelWeights:
    needs: memory.mmap
    
    let weights = memory.mmap.read_gguf(path) otherwise {
      alert.sink.send("stderr", "failed to map model weights")
      return ModelWeights.empty()
    }
    return weights
  ```
* **Quantization Format Support**:
  Native SIMD decoding and matrix multiplication for standard quantization types:
  - `Q4_K` / `Q4_0` (4-bit block quantization)
  - `Q8_0` (8-bit quantization)
  - `FP16` / `BF16` (half-precision float)
  - `FP32` (single-precision float)
* **Local Weights Only Invariant**:
  Adhering strictly to `AGENTS.md`, all weights must be read from local disk (`~/Downloads/*.gguf`, `/tmp/models/`). Zero network downloads are allowed.

---

### Pillar 6: Hardware Acceleration & Kernel Lowering

Kaze compiles to clean ANSI C99 with targeted SIMD vector intrinsics and optional GPU compute shaders:

1. **CPU SIMD Vectorization**:
   - **Apple Silicon / ARM64**: ARM NEON intrinsics (`vld1q_f32`, `vfmaq_f32`, `vaddq_f32`).
   - **x86_64**: AVX2 and AVX-512 FMA vector intrinsics.
2. **Apple Silicon Metal Shaders (MSL)**:
   - Kaze can emit native Metal Shading Language (`.metal`) compute kernels for massive matrix multiplications, executing directly on Apple Silicon unified memory GPUs with zero host-to-device memory copies.
3. **WebGPU / Vulkan Compute**:
   - For cross-platform and embedded GPU inference on Linux and mobile edge devices.

---

## 3. Concrete Example: Complete Transformer Attention Head in Kaze

```kaze
# Self-Attention Head authored in pure Kaze
routine attention_head(
  query: tensor<float, [Batch, SeqLen, HeadDim]>,
  key:   tensor<float, [Batch, KeyLen, HeadDim]>,
  value: tensor<float, [Batch, KeyLen, ValueDim]>,
  mask:  tensor<bool,  [SeqLen, KeyLen]>
) -> tensor<float, [Batch, SeqLen, ValueDim]>:
  requires HeadDim > 0
  
  # 1. Compute raw attention scores (Q * K^T)
  let scale = 1.0 / sqrt(HeadDim) otherwise 0.0
  let scores = (query * key.transpose(1, 2)) * scale
  
  # 2. Apply causal mask (set masked tokens to -infinity)
  let masked_scores = scores.masked_fill(mask, -1e9)
  
  # 3. Softmax along key dimension
  let weights = softmax(masked_scores, axis: -1) otherwise 0.0
  
  # 4. Multiply by Value tensor
  return weights * value
```

### Equivalent Machine S-Expression Syntax:

```lisp
(kaze (format 1)
  (routine attention_head
    (params
      (param query (type tensor float (dim Batch SeqLen HeadDim)))
      (param key   (type tensor float (dim Batch KeyLen HeadDim)))
      (param value (type tensor float (dim Batch KeyLen ValueDim)))
      (param mask  (type tensor bool  (dim SeqLen KeyLen))))
    (returns (type tensor float (dim Batch SeqLen ValueDim)))
    (requires (gt HeadDim 0))
    (body
      (let scale (div 1.0 (sqrt HeadDim) (otherwise 0.0)))
      (let scores (mul (mul query (transpose key 1 2)) scale))
      (let masked_scores (masked_fill scores mask -1e9))
      (let weights (softmax masked_scores (axis -1) (otherwise 0.0)))
      (return (mul weights value)))))
```

---

## 4. Technical Milestone Roadmap for AI in Kaze

The native AI extension roadmap builds on top of the established 10-phase systems roadmap:

```
Phase 1-6 (COMPLETED ✓) ───► Phase 7: C99 Codegen (CURRENT) ───► Phase 8: Bootstrap
                                                                         │
                                                                         ▼
┌────────────────────────────────────────────────────────────────────────┐
│             AI EXTENSION MILESTONE 1: TENSOR PRIMITIVES                │
│   • Multi-dimensional strided tensor type `tensor<T, [dims]>`          │
│   • Compile-time shape inference and broadcasting validator            │
│   • Linear arena tensor allocation and bulk reset in kaze_rt.h         │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             AI EXTENSION MILESTONE 2: CPU SIMD ACCELERATION            │
│   • ARM NEON and x86 AVX2/AVX-512 GEMM matrix multiplication kernels   │
│   • Vectorized Softmax, RMSNorm, LayerNorm, and GELU activations       │
│   • Quantized block decoding (Q4_K, Q8_0) in native C99                │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             AI EXTENSION MILESTONE 3: ZERO-COPY WEIGHT INGESTION       │
│   • Native GGUF and SafeTensors header parser in pure Kaze             │
│   • Memory-mapped weight streaming without memory duplication          │
│   • Model configuration loader (Llama, Phi, SmolLM architectures)      │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             AI EXTENSION MILESTONE 4: SYMBOLIC AUTOGRAD ENGINE         │
│   • Compile-time reverse-mode automatic differentiation                │
│   • Static backward pass synthesis for loss and optimizer updates      │
│   • AdamW and SGD optimizer implementations in pure Kaze               │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             AI EXTENSION MILESTONE 5: METAL GPU COMPUTE SHADERS        │
│   • Direct emission of Metal Shading Language (.metal) compute kernels │
│   • Zero-copy unified memory GPU execution on Apple Silicon M-series   │
│   • Multi-threaded asynchronous command queue dispatch                 │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 5. Acceptance Gates & Verification Criteria

1. **Shape Safety Gate**:
   - 100% of tensor shape mismatches must be caught at compile time with `error[E_KAZE_SHAPE_MISMATCH]`.
   - Zero runtime shape crashes permitted.
2. **Sanitizer & Leak-Freedom Gate**:
   - 1,000 forward-backward passes must complete under AddressSanitizer and LeakSanitizer with zero leaks and zero dangling pointer dereferences.
3. **Numerical Parity Gate**:
   - In-process Kaze inference must produce numerical outputs matching reference implementations (`llama.cpp`, PyTorch) within standard floating-point tolerance ($\le 1e-4$).
4. **Local Weights Invariant**:
   - Verification harnesses run exclusively against local GGUF models already on disk (`~/Downloads/*.gguf`). Zero remote downloads are permitted.
5. **Binary Footprint Gate**:
   - The compiled model executable must remain self-contained, requiring zero Python dependencies, zero external shared libraries, and minimal memory overhead.
