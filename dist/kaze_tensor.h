/* kaze_tensor.h — the Kaze tensor and native-AI substrate (roadmap Phase 12).
 *
 * This is the runtime half of Phase 12. The compiler half lowers tensor
 * declarations, shape algebra and the neural builtins to the functions below.
 *
 * Design rules (same discipline as kaze_rt.h):
 *   - ANSI/ISO C99, no compiler extensions required.
 *   - Nothing here aborts: a failed shape, quant block or arena request is a
 *     trap through kaze_trap with the Phase 12 codes.
 *   - Weights are ingested zero-copy by mmap; every activation tensor is
 *     allocated from a caller-owned linear arena and dies at kaze_arena_reset.
 *   - Kernels are deterministic: no threads, no unordered accumulation.
 *
 * Dtypes:
 *   F32, F16, BF16, I8 and BOOL are dense. Q8_0, Q4_0, Q4_K and Q6_K are the
 *   quantized GGUF layouts, kept in their on-disk blocks; matmul reads them
 *   through the fused quantized dot paths instead of dequantizing first.
 */
#ifndef KAZE_TENSOR_H
#define KAZE_TENSOR_H

#include "kaze_rt.h"

#ifdef __cplusplus
extern "C" {
#endif

#define KAZE_TENSOR_MAX_DIMS 8

/* data points into an mmap'd GGUF/SafeTensors file: never written, never freed */
#define KAZE_TENSOR_FLAG_MMAP 0x1u
/* data points into an arena allocation (tensor header included) */
#define KAZE_TENSOR_FLAG_ARENA 0x2u
/* header is a view over another tensor's data; do not free the storage */
#define KAZE_TENSOR_FLAG_VIEW 0x4u

typedef enum kaze_dtype_t {
    KAZE_DTYPE_F32 = 0,
    KAZE_DTYPE_F16 = 1,
    KAZE_DTYPE_BF16 = 2,
    KAZE_DTYPE_I8 = 3,
    KAZE_DTYPE_BOOL = 4,
    KAZE_DTYPE_Q8_0 = 5,
    KAZE_DTYPE_Q4_0 = 6,
    KAZE_DTYPE_Q4_K = 7,
    KAZE_DTYPE_Q6_K = 8
} kaze_dtype_t;

/* One strided tensor. shape/strides are always in elements, row-major unless a
 * view says otherwise; strides[d] is the step between consecutive indices on
 * dimension d. A quantized tensor's data is a sequence of blocks: its shape is
 * the logical shape, its nbytes the on-disk block bytes. */
typedef struct kaze_tensor_t {
    kaze_dtype_t dtype;
    int32_t rank;
    int64_t shape[KAZE_TENSOR_MAX_DIMS];
    int64_t strides[KAZE_TENSOR_MAX_DIMS];
    void *data;
    size_t nbytes;
    uint32_t flags;
    const char *name; /* optional, arena- or mmap-backed; never freed here */
} kaze_tensor_t;

/* Block geometry. A dense dtype is one element per block. */
size_t kaze_dtype_block_elems(kaze_dtype_t dtype);
size_t kaze_dtype_block_bytes(kaze_dtype_t dtype);
const char *kaze_dtype_name(kaze_dtype_t dtype);

int64_t kaze_tensor_numel(const kaze_tensor_t *t);
size_t kaze_tensor_bytes(const kaze_tensor_t *t);
bool kaze_tensor_is_contiguous(const kaze_tensor_t *t);
bool kaze_tensor_same_shape(const kaze_tensor_t *a, const kaze_tensor_t *b);

/* Arena-backed dense tensors. The header and the data both come from arena.
 * An unsupported dtype or a failing arena allocation traps and returns NULL. */
kaze_tensor_t *kaze_tensor_alloc(kaze_arena_t *arena, kaze_dtype_t dtype,
                                 int32_t rank, const int64_t *shape);
kaze_tensor_t *kaze_tensor_zeros(kaze_arena_t *arena, kaze_dtype_t dtype,
                                 int32_t rank, const int64_t *shape);
kaze_tensor_t *kaze_tensor_ones(kaze_arena_t *arena, kaze_dtype_t dtype,
                                int32_t rank, const int64_t *shape);
/* A view over base's storage; requires base to be contiguous. */
kaze_tensor_t *kaze_tensor_view(kaze_arena_t *arena, const kaze_tensor_t *base,
                                int32_t rank, const int64_t *shape);
/* Dense copy into the arena. F32 tensors are the workhorse; other dense dtypes
 * copy their bytes, quantized tensors are dequantized to F32. */
kaze_tensor_t *kaze_tensor_copy(kaze_arena_t *arena, const kaze_tensor_t *src);
kaze_tensor_t *kaze_tensor_from_f32(kaze_arena_t *arena, const float *data,
                                    int32_t rank, const int64_t *shape,
                                    const char *name);

/* Element access. get returns 0.0 and traps E_KAZE_BADARG when the dtype is not
 * dense; quantized values are read through kaze_tensor_dequant. */
float kaze_tensor_get(const kaze_tensor_t *t, int64_t linear_index);
void kaze_tensor_set(kaze_tensor_t *t, int64_t linear_index, float v);
void kaze_tensor_fill(kaze_tensor_t *t, float v);

/* Elementwise. Shapes must match exactly, or one side may be a one-element
 * (scalar) tensor. Any other pair traps E_KAZE_SHAPE_MISMATCH. Division traps on
 * a zero denominator instead of producing inf/NaN. */
kaze_tensor_t *kaze_tensor_add(kaze_arena_t *arena, const kaze_tensor_t *a,
                               const kaze_tensor_t *b);
kaze_tensor_t *kaze_tensor_sub(kaze_arena_t *arena, const kaze_tensor_t *a,
                               const kaze_tensor_t *b);
kaze_tensor_t *kaze_tensor_mul(kaze_arena_t *arena, const kaze_tensor_t *a,
                               const kaze_tensor_t *b);
kaze_tensor_t *kaze_tensor_div(kaze_arena_t *arena, const kaze_tensor_t *a,
                               const kaze_tensor_t *b);
kaze_tensor_t *kaze_tensor_scale(kaze_arena_t *arena, const kaze_tensor_t *a,
                                 float scalar);
/* bias: rank-1 shape [last dim]. x's last dim must match. */
kaze_tensor_t *kaze_tensor_add_bias(kaze_arena_t *arena, const kaze_tensor_t *x,
                                    const kaze_tensor_t *bias);

/* Matrix products (F32, or a quantized right-hand side for _quant):
 *   matmul(a [M,K], b [K,N])        -> [M,N]
 *   matmul_t(a [M,K], b [N,K])      -> [M,N]   (b transposed)
 *   matmul_quant(x [M,K], w quant)  -> [M,N]   (w stored [N,K], GGUF layout)
 * Rank-3 batched forms treat the leading dimension as a batch and require the
 * batch of either side to be 1 or equal to the other. */
kaze_tensor_t *kaze_tensor_matmul(kaze_arena_t *arena, const kaze_tensor_t *a,
                                  const kaze_tensor_t *b);
kaze_tensor_t *kaze_tensor_matmul_t(kaze_arena_t *arena, const kaze_tensor_t *a,
                                    const kaze_tensor_t *b);
kaze_tensor_t *kaze_tensor_matmul_quant(kaze_arena_t *arena,
                                        const kaze_tensor_t *x,
                                        const kaze_tensor_t *w);

/* Copy-transpose dimensions dim0/dim1 of an F32 tensor. */
kaze_tensor_t *kaze_tensor_transpose(kaze_arena_t *arena, const kaze_tensor_t *t,
                                     int32_t dim0, int32_t dim1);
/* Reinterpret the layout. Requires a contiguous source and equal element count. */
kaze_tensor_t *kaze_tensor_reshape(kaze_arena_t *arena, const kaze_tensor_t *t,
                                   int32_t rank, const int64_t *shape);

/* Numerics. Every kernel works on the last dimension and keeps an F32 result.
 * softmax/rms_norm/layer_norm are numerically guarded: a non-finite value traps
 * E_KAZE_MATH instead of leaking NaN into the rest of a model. */
kaze_tensor_t *kaze_tensor_softmax(kaze_arena_t *arena, const kaze_tensor_t *t);
kaze_tensor_t *kaze_tensor_rms_norm(kaze_arena_t *arena, const kaze_tensor_t *x,
                                    const kaze_tensor_t *weight, float eps);
kaze_tensor_t *kaze_tensor_layer_norm(kaze_arena_t *arena, const kaze_tensor_t *x,
                                      const kaze_tensor_t *weight,
                                      const kaze_tensor_t *bias,
                                      float eps);
kaze_tensor_t *kaze_tensor_gelu(kaze_arena_t *arena, const kaze_tensor_t *t);
kaze_tensor_t *kaze_tensor_silu(kaze_arena_t *arena, const kaze_tensor_t *t);
kaze_tensor_t *kaze_tensor_relu(kaze_arena_t *arena, const kaze_tensor_t *t);
/* mask: BOOL tensor broadcastable to t; masked positions become value. */
kaze_tensor_t *kaze_tensor_masked_fill(kaze_arena_t *arena,
                                       const kaze_tensor_t *t,
                                       const kaze_tensor_t *mask,
                                       float value);
/* Rotary embedding over a [seq, heads*head_dim] F32 tensor, LLaMA pairing. */
kaze_tensor_t *kaze_tensor_rope(kaze_arena_t *arena, const kaze_tensor_t *t,
                                int32_t heads, int32_t head_dim,
                                size_t pos0, float theta);

/* Reductions over the last dimension. sum/mean return [..., 1]. */
kaze_tensor_t *kaze_tensor_sum_last(kaze_arena_t *arena, const kaze_tensor_t *t);
kaze_tensor_t *kaze_tensor_mean_last(kaze_arena_t *arena, const kaze_tensor_t *t);
float kaze_tensor_sum_all(const kaze_tensor_t *t);
float kaze_tensor_max_all(const kaze_tensor_t *t);
int64_t kaze_tensor_argmax_last(const kaze_tensor_t *t);

/* Dequantize any supported dtype to a dense F32 arena tensor. */
kaze_tensor_t *kaze_tensor_dequant(kaze_arena_t *arena, const kaze_tensor_t *t);
/* Dot product of one quantized row with one F32 row; both have n elements. */
float kaze_quant_dot(const kaze_tensor_t *q, int64_t row,
                            const float *x, int64_t n);

/* ------------------------------------------------------------------------- */
/* GGUF / SafeTensors weight ingestion                                        */
/* ------------------------------------------------------------------------- */

typedef struct kaze_gguf_t kaze_gguf_t;

/* Open and mmap a GGUF file, parse its header, metadata and tensor table into
 * meta. Returns NULL after a trap when the file is unreadable or malformed. */
kaze_gguf_t *kaze_gguf_open(kaze_arena_t *meta, const char *path);
void kaze_gguf_close(kaze_gguf_t *g);

size_t kaze_gguf_tensor_count(const kaze_gguf_t *g);
const kaze_tensor_t *kaze_gguf_tensor_at(const kaze_gguf_t *g, size_t index);
const kaze_tensor_t *kaze_gguf_tensor(const kaze_gguf_t *g, const char *name);
size_t kaze_gguf_meta_count(const kaze_gguf_t *g);
const char *kaze_gguf_meta_key(const kaze_gguf_t *g, size_t index);
bool kaze_gguf_meta_i64(const kaze_gguf_t *g, const char *key, int64_t *out);
bool kaze_gguf_meta_f32(const kaze_gguf_t *g, const char *key, float *out);
bool kaze_gguf_meta_str(const kaze_gguf_t *g, const char *key, const char **out);
bool kaze_gguf_meta_bool(const kaze_gguf_t *g, const char *key, bool *out);
/* String-array metadata (tokenizer vocab/merges/token types). */
bool kaze_gguf_meta_str_array(const kaze_gguf_t *g, const char *key,
                              const char ***items, size_t *count);
bool kaze_gguf_meta_i32_array(const kaze_gguf_t *g, const char *key,
                              const int32_t **items, size_t *count);

/* Minimal JSON header reader for SafeTensors (mmap'd file, named f32/f16/bf16
 * tensors only). Same zero-copy contract as GGUF. */
kaze_gguf_t *kaze_safetensors_open(kaze_arena_t *meta, const char *path);

/* ------------------------------------------------------------------------- */
/* Byte-pair tokenizer                                                        */
/* ------------------------------------------------------------------------- */

typedef struct kaze_tokenizer_t kaze_tokenizer_t;

kaze_tokenizer_t *kaze_tokenizer_from_gguf(kaze_arena_t *arena,
                                           const kaze_gguf_t *g);
/* Encode UTF-8 text. Returned ids live in arena; n_out receives the count. */
const int32_t *kaze_tokenize(kaze_arena_t *arena, const kaze_tokenizer_t *tok,
                             const char *text, size_t len, size_t *n_out);
/* Decode ids back to UTF-8 text (arena-backed, NUL-terminated). */
char *kaze_tokenizer_decode(kaze_arena_t *arena, const kaze_tokenizer_t *tok,
                            const int32_t *ids, size_t n);
int32_t kaze_tokenizer_bos(const kaze_tokenizer_t *tok);
int32_t kaze_tokenizer_eos(const kaze_tokenizer_t *tok);
size_t kaze_tokenizer_vocab_size(const kaze_tokenizer_t *tok);

/* ------------------------------------------------------------------------- */
/* LLaMA-family inference (GGUF weights, zero-copy)                            */
/* ------------------------------------------------------------------------- */

typedef struct kaze_llama_t kaze_llama_t;

/* Open a local GGUF model. The model owns its metadata arena, its mmap and one
 * persistent KV-cache arena; a second scratch arena is reset on every forward
 * pass so activations never accumulate and never touch malloc while running. */
kaze_llama_t *kaze_llama_open(const char *path);
void kaze_llama_close(kaze_llama_t *model);

size_t kaze_llama_n_vocab(const kaze_llama_t *model);
size_t kaze_llama_n_ctx(const kaze_llama_t *model);
size_t kaze_llama_pos(const kaze_llama_t *model);
const kaze_tokenizer_t *kaze_llama_tokenizer(const kaze_llama_t *model);
/* Forget the KV cache and restart at position 0. */
bool kaze_llama_reset(kaze_llama_t *model);
/* Run tokens[start_pos..] and write the last token's logits (n_vocab f32
 * values) to logits. Returns false after a trap. */
bool kaze_llama_forward(kaze_llama_t *model, const int32_t *tokens,
                        size_t n_tokens, size_t start_pos,
                        float *logits);

/* ------------------------------------------------------------------------- */
/* Reverse-mode automatic differentiation (training substrate)                 */
/* ------------------------------------------------------------------------- */

typedef struct kaze_tape_t kaze_tape_t;
typedef enum kaze_tape_op_t {
    KAZE_TAPE_LEAF = 0,
    KAZE_TAPE_ADD,
    KAZE_TAPE_SUB,
    KAZE_TAPE_MUL,
    KAZE_TAPE_MATMUL,
    KAZE_TAPE_ADD_BIAS,
    KAZE_TAPE_RELU,
    KAZE_TAPE_SILU,
    KAZE_TAPE_GELU,
    KAZE_TAPE_SOFTMAX,
    KAZE_TAPE_RMS_NORM,
    KAZE_TAPE_LAYER_NORM,
    KAZE_TAPE_MSE_LOSS,
    KAZE_TAPE_CROSS_ENTROPY
} kaze_tape_op_t;

kaze_tape_t *kaze_tape_new(kaze_arena_t *arena);
/* Clear the tape for a new forward pass; leaves parameters' data untouched. */
void kaze_tape_clear(kaze_tape_t *tape);
kaze_tensor_t *kaze_tape_leaf(kaze_tape_t *tape, kaze_arena_t *arena,
                              int32_t rank, const int64_t *shape);
/* Register an existing tensor (typically a model parameter allocated once) as a
 * tape leaf for the current pass. Returns t. */
kaze_tensor_t *kaze_tape_watch(kaze_tape_t *tape, kaze_tensor_t *t);
kaze_tensor_t *kaze_tape_add(kaze_tape_t *tape, kaze_arena_t *arena,
                             const kaze_tensor_t *a, const kaze_tensor_t *b);
kaze_tensor_t *kaze_tape_sub(kaze_tape_t *tape, kaze_arena_t *arena,
                             const kaze_tensor_t *a, const kaze_tensor_t *b);
kaze_tensor_t *kaze_tape_mul(kaze_tape_t *tape, kaze_arena_t *arena,
                             const kaze_tensor_t *a, const kaze_tensor_t *b);
kaze_tensor_t *kaze_tape_matmul(kaze_tape_t *tape, kaze_arena_t *arena,
                                const kaze_tensor_t *a, const kaze_tensor_t *b);
kaze_tensor_t *kaze_tape_add_bias(kaze_tape_t *tape, kaze_arena_t *arena,
                                  const kaze_tensor_t *x,
                                  const kaze_tensor_t *bias);
kaze_tensor_t *kaze_tape_relu(kaze_tape_t *tape, kaze_arena_t *arena,
                              const kaze_tensor_t *x);
kaze_tensor_t *kaze_tape_silu(kaze_tape_t *tape, kaze_arena_t *arena,
                              const kaze_tensor_t *x);
kaze_tensor_t *kaze_tape_gelu(kaze_tape_t *tape, kaze_arena_t *arena,
                              const kaze_tensor_t *x);
kaze_tensor_t *kaze_tape_softmax(kaze_tape_t *tape, kaze_arena_t *arena,
                                 const kaze_tensor_t *x);
kaze_tensor_t *kaze_tape_rms_norm(kaze_tape_t *tape, kaze_arena_t *arena,
                                  const kaze_tensor_t *x,
                                  const kaze_tensor_t *weight,
                                  float eps);
kaze_tensor_t *kaze_tape_layer_norm(kaze_tape_t *tape, kaze_arena_t *arena,
                                    const kaze_tensor_t *x,
                                    const kaze_tensor_t *weight,
                                    const kaze_tensor_t *bias,
                                    float eps);
/* Scalar losses: mse over every element, cross-entropy from logits [1,V]. */
kaze_tensor_t *kaze_tape_mse(kaze_tape_t *tape, kaze_arena_t *arena,
                             const kaze_tensor_t *pred,
                             const kaze_tensor_t *target);
kaze_tensor_t *kaze_tape_cross_entropy(kaze_tape_t *tape, kaze_arena_t *arena,
                                       const kaze_tensor_t *logits,
                                       int32_t target_class);
/* Reverse pass from the scalar loss. Gradients accumulate on tape leaves. */
bool kaze_tape_backward(kaze_tape_t *tape, const kaze_tensor_t *loss);
const kaze_tensor_t *kaze_tape_grad(const kaze_tape_t *tape,
                                    const kaze_tensor_t *leaf);
/* AdamW update in place: leaf -= lr * (m_hat / (sqrt(v_hat) + eps) + wd * leaf).
 * m/v are caller arenas sized like leaf. Returns false on a shape mismatch. */
bool kaze_opt_adamw(kaze_tensor_t *leaf, const kaze_tensor_t *grad,
                    kaze_tensor_t *m, kaze_tensor_t *v, float lr,
                    float beta1, float beta2, float eps,
                    float weight_decay, int64_t step);

/* ------------------------------------------------------------------------- */
/* Benchmark hooks (the Phase 12 gates measure these)                         */
/* ------------------------------------------------------------------------- */

/* FMA GEMM over dense F32; the SIMD path is selected at compile time. */
void kaze_gemm_f32(const float *a, const float *b,
                   float *c, int64_t m, int64_t n, int64_t k);
bool kaze_tensor_has_neon(void);
bool kaze_tensor_has_metal(void);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* KAZE_TENSOR_H */
