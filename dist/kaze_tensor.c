/* kaze_tensor.c — Phase 12 tensor substrate: storage, dense math, kernels.
 *
 * The memory contract is the Phase 1 arena contract: every result tensor is an
 * arena allocation, so a forward pass is reclaimed by one kaze_arena_reset and
 * nothing in a hot loop calls malloc. Weight tensors are mmap views and are
 * never copied by this file; matmul reads them through kaze_quant.
 *
 * Shape errors are never undefined behaviour: an impossible shape or a
 * non-finite arithmetic result traps with the Phase 12 codes and returns NULL.
 */
#include "kaze_tensor.h"
#include "kaze_quant.h"

#include <math.h>
#include <string.h>

#if defined(__ARM_NEON) || defined(__ARM_NEON__) || defined(__aarch64__)
#include <arm_neon.h>
#define KAZE_HAVE_NEON 1
#else
#define KAZE_HAVE_NEON 0
#endif

/* ------------------------------------------------------------------------- */
/* Dtype geometry                                                             */
/* ------------------------------------------------------------------------- */

size_t kaze_dtype_block_elems(kaze_dtype_t dtype)
{
    switch (dtype) {
    case KAZE_DTYPE_F32:
    case KAZE_DTYPE_F16:
    case KAZE_DTYPE_BF16:
    case KAZE_DTYPE_I8:
    case KAZE_DTYPE_BOOL:
        return 1;
    case KAZE_DTYPE_Q8_0:
    case KAZE_DTYPE_Q4_0:
        return 32;
    case KAZE_DTYPE_Q4_K:
    case KAZE_DTYPE_Q6_K:
        return 256;
    default:
        return 0;
    }
}

size_t kaze_dtype_block_bytes(kaze_dtype_t dtype)
{
    switch (dtype) {
    case KAZE_DTYPE_F32:
        return 4;
    case KAZE_DTYPE_F16:
    case KAZE_DTYPE_BF16:
        return 2;
    case KAZE_DTYPE_I8:
    case KAZE_DTYPE_BOOL:
        return 1;
    case KAZE_DTYPE_Q8_0:
        return 34;
    case KAZE_DTYPE_Q4_0:
        return 18;
    case KAZE_DTYPE_Q4_K:
        return 144;
    case KAZE_DTYPE_Q6_K:
        return 210;
    default:
        return 0;
    }
}

const char *kaze_dtype_name(kaze_dtype_t dtype)
{
    switch (dtype) {
    case KAZE_DTYPE_F32: return "f32";
    case KAZE_DTYPE_F16: return "f16";
    case KAZE_DTYPE_BF16: return "bf16";
    case KAZE_DTYPE_I8: return "i8";
    case KAZE_DTYPE_BOOL: return "bool";
    case KAZE_DTYPE_Q8_0: return "q8_0";
    case KAZE_DTYPE_Q4_0: return "q4_0";
    case KAZE_DTYPE_Q4_K: return "q4_k";
    case KAZE_DTYPE_Q6_K: return "q6_k";
    default: return "unknown";
    }
}

int64_t kaze_tensor_numel(const kaze_tensor_t *t)
{
    int64_t n = 1;
    int32_t d;

    if (t == NULL || t->rank < 0 || t->rank > KAZE_TENSOR_MAX_DIMS) {
        return 0;
    }
    for (d = 0; d < t->rank; d++) {
        if (t->shape[d] < 0) {
            return 0;
        }
        n *= t->shape[d];
    }
    return n;
}

size_t kaze_tensor_bytes(const kaze_tensor_t *t)
{
    size_t be, bb;
    int64_t n;

    if (t == NULL) {
        return 0;
    }
    n = kaze_tensor_numel(t);
    if (t->dtype == KAZE_DTYPE_F32) {
        return (size_t)n * 4u;
    }
    if (t->dtype == KAZE_DTYPE_F16 || t->dtype == KAZE_DTYPE_BF16) {
        return (size_t)n * 2u;
    }
    if (t->dtype == KAZE_DTYPE_I8 || t->dtype == KAZE_DTYPE_BOOL) {
        return (size_t)n;
    }
    be = kaze_dtype_block_elems(t->dtype);
    bb = kaze_dtype_block_bytes(t->dtype);
    if (be == 0) {
        return 0;
    }
    return (size_t)(n / (int64_t)be) * bb;
}

bool kaze_tensor_is_contiguous(const kaze_tensor_t *t)
{
    int64_t expected = 1;
    int32_t d;

    if (t == NULL) {
        return false;
    }
    for (d = t->rank - 1; d >= 0; d--) {
        if (t->shape[d] == 0) {
            return true;
        }
        if (t->strides[d] != expected) {
            return false;
        }
        expected *= t->shape[d];
    }
    return true;
}

bool kaze_tensor_same_shape(const kaze_tensor_t *a, const kaze_tensor_t *b)
{
    int32_t d;

    if (a == NULL || b == NULL || a->rank != b->rank) {
        return false;
    }
    for (d = 0; d < a->rank; d++) {
        if (a->shape[d] != b->shape[d]) {
            return false;
        }
    }
    return true;
}

static bool shape_valid(int32_t rank, const int64_t *shape)
{
    int32_t d;

    if (rank < 1 || rank > KAZE_TENSOR_MAX_DIMS || shape == NULL) {
        return false;
    }
    for (d = 0; d < rank; d++) {
        if (shape[d] <= 0) {
            return false;
        }
    }
    return true;
}

static void row_major_strides(int32_t rank, const int64_t *shape,
                              int64_t *strides)
{
    int64_t step = 1;
    int32_t d;

    for (d = rank - 1; d >= 0; d--) {
        strides[d] = step;
        step *= shape[d];
    }
}

static kaze_tensor_t *tensor_header(kaze_arena_t *arena)
{
    kaze_tensor_t *t;

    if (arena == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "tensor allocation without an arena");
        return NULL;
    }
    t = (kaze_tensor_t *)kmalloc(arena, sizeof(kaze_tensor_t));
    if (t == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "tensor header did not fit the arena");
        return NULL;
    }
    memset(t, 0, sizeof *t);
    return t;
}

/* ------------------------------------------------------------------------- */
/* Tensor construction                                                        */
/* ------------------------------------------------------------------------- */

kaze_tensor_t *kaze_tensor_alloc(kaze_arena_t *arena, kaze_dtype_t dtype,
                                 int32_t rank, const int64_t *shape)
{
    kaze_tensor_t *t;
    size_t nbytes;
    void *data;

    if (!shape_valid(rank, shape)) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "tensor shape is empty or has a non-positive dimension");
        return NULL;
    }
    if (dtype != KAZE_DTYPE_F32 && dtype != KAZE_DTYPE_F16 &&
        dtype != KAZE_DTYPE_BF16 && dtype != KAZE_DTYPE_I8 &&
        dtype != KAZE_DTYPE_BOOL) {
        (void)kaze_trap(KAZE_ERR_QUANT, "quantized tensors are views over GGUF storage, not arena allocations");
        return NULL;
    }
    t = tensor_header(arena);
    if (t == NULL) {
        return NULL;
    }
    t->dtype = dtype;
    t->rank = rank;
    {
        int32_t d;
        for (d = 0; d < rank; d++) {
            t->shape[d] = shape[d];
        }
    }
    row_major_strides(rank, t->shape, t->strides);
    nbytes = kaze_tensor_bytes(t);
    data = kmalloc(arena, nbytes == 0 ? 1u : nbytes);
    if (data == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "tensor data did not fit the arena");
        return NULL;
    }
    memset(data, 0, nbytes);
    t->data = data;
    t->nbytes = nbytes;
    t->flags = KAZE_TENSOR_FLAG_ARENA;
    return t;
}

kaze_tensor_t *kaze_tensor_zeros(kaze_arena_t *arena, kaze_dtype_t dtype,
                                 int32_t rank, const int64_t *shape)
{
    return kaze_tensor_alloc(arena, dtype, rank, shape);
}

kaze_tensor_t *kaze_tensor_ones(kaze_arena_t *arena, kaze_dtype_t dtype,
                                int32_t rank, const int64_t *shape)
{
    kaze_tensor_t *t = kaze_tensor_alloc(arena, dtype, rank, shape);
    if (t != NULL) {
        kaze_tensor_fill(t, 1.0f);
    }
    return t;
}

kaze_tensor_t *kaze_tensor_view(kaze_arena_t *arena, const kaze_tensor_t *base,
                                int32_t rank, const int64_t *shape)
{
    kaze_tensor_t *t;
    int64_t wanted = 1;
    int32_t d;

    if (base == NULL || !shape_valid(rank, shape) ||
        !kaze_tensor_is_contiguous(base)) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "a tensor view needs a contiguous base and a valid shape");
        return NULL;
    }
    for (d = 0; d < rank; d++) {
        wanted *= shape[d];
    }
    if (kaze_tensor_numel(base) != wanted) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "reshaped tensor changes the element count");
        return NULL;
    }
    t = tensor_header(arena);
    if (t == NULL) {
        return NULL;
    }
    t->dtype = base->dtype;
    t->rank = rank;
    {
        int32_t d;
        for (d = 0; d < rank; d++) {
            t->shape[d] = shape[d];
        }
    }
    row_major_strides(rank, t->shape, t->strides);
    t->data = base->data;
    t->nbytes = kaze_tensor_bytes(t);
    t->flags = base->flags | KAZE_TENSOR_FLAG_VIEW;
    t->name = base->name;
    return t;
}

kaze_tensor_t *kaze_tensor_copy(kaze_arena_t *arena, const kaze_tensor_t *src)
{
    kaze_tensor_t *out;

    if (src == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "copy of a null tensor");
        return NULL;
    }
    if (src->dtype != KAZE_DTYPE_F32 && src->dtype != KAZE_DTYPE_F16 &&
        src->dtype != KAZE_DTYPE_BF16 && src->dtype != KAZE_DTYPE_I8 &&
        src->dtype != KAZE_DTYPE_BOOL) {
        return kaze_quant_dequant(arena, src);
    }
    out = kaze_tensor_alloc(arena, src->dtype, src->rank, src->shape);
    if (out == NULL) {
        return NULL;
    }
    memcpy(out->data, src->data, kaze_tensor_bytes(src));
    out->name = src->name;
    return out;
}

kaze_tensor_t *kaze_tensor_from_f32(kaze_arena_t *arena, const float *data,
                                    int32_t rank, const int64_t *shape,
                                    const char *name)
{
    kaze_tensor_t *t = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, rank, shape);
    if (t == NULL) {
        return NULL;
    }
    if (data != NULL) {
        memcpy(t->data, data, kaze_tensor_bytes(t));
    }
    t->name = name;
    return t;
}

/* ------------------------------------------------------------------------- */
/* Element access                                                             */
/* ------------------------------------------------------------------------- */

float kaze_tensor_get(const kaze_tensor_t *t, int64_t index)
{
    int64_t n;

    if (t == NULL || t->data == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "read of a null tensor");
        return 0.0f;
    }
    n = kaze_tensor_numel(t);
    if (index < 0 || index >= n) {
        char msg[96];
        (void)snprintf(msg, sizeof msg, "tensor read %lld outside its %lld elements",
                       (long long)index, (long long)n);
        (void)kaze_trap(KAZE_ERR_BADARG, msg);
        return 0.0f;
    }
    switch (t->dtype) {
    case KAZE_DTYPE_F32:
        return ((const float *)t->data)[index];
    case KAZE_DTYPE_F16:
        return kaze_f16_to_f32(((const uint16_t *)t->data)[index]);
    case KAZE_DTYPE_BF16:
        return kaze_bf16_to_f32(((const uint16_t *)t->data)[index]);
    case KAZE_DTYPE_I8:
        return (float)((const int8_t *)t->data)[index];
    case KAZE_DTYPE_BOOL:
        return ((const uint8_t *)t->data)[index] ? 1.0f : 0.0f;
    default:
        (void)kaze_trap(KAZE_ERR_TYPE, "a quantized tensor is read through dequant, not element access");
        return 0.0f;
    }
}

void kaze_tensor_set(kaze_tensor_t *t, int64_t index, float v)
{
    int64_t n;

    if (t == NULL || t->data == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "write to a null tensor");
        return;
    }
    if (t->flags & KAZE_TENSOR_FLAG_MMAP) {
        (void)kaze_trap(KAZE_ERR_BADARG, "write to a read-only mmap tensor");
        return;
    }
    n = kaze_tensor_numel(t);
    if (index < 0 || index >= n) {
        char msg[96];
        (void)snprintf(msg, sizeof msg, "tensor write %lld outside its %lld elements",
                       (long long)index, (long long)n);
        (void)kaze_trap(KAZE_ERR_BADARG, msg);
        return;
    }
    switch (t->dtype) {
    case KAZE_DTYPE_F32:
        ((float *)t->data)[index] = v;
        break;
    case KAZE_DTYPE_F16:
        ((uint16_t *)t->data)[index] = kaze_f32_to_f16(v);
        break;
    case KAZE_DTYPE_BF16:
        ((uint16_t *)t->data)[index] = kaze_f32_to_bf16(v);
        break;
    case KAZE_DTYPE_I8:
        ((int8_t *)t->data)[index] = (int8_t)v;
        break;
    case KAZE_DTYPE_BOOL:
        ((uint8_t *)t->data)[index] = (v != 0.0f) ? 1u : 0u;
        break;
    default:
        (void)kaze_trap(KAZE_ERR_TYPE, "a quantized tensor cannot be written element-wise");
        break;
    }
}

void kaze_tensor_fill(kaze_tensor_t *t, float v)
{
    int64_t i, n;

    if (t == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "fill of a null tensor");
        return;
    }
    n = kaze_tensor_numel(t);
    for (i = 0; i < n; i++) {
        kaze_tensor_set(t, i, v);
    }
}

/* ------------------------------------------------------------------------- */
/* Dense conversion helper                                                    */
/* ------------------------------------------------------------------------- */

/* Every math kernel works on dense F32. A quantized input is dequantized into
 * the caller's arena once; a dense F32 input is used in place. */
static const kaze_tensor_t *as_f32(kaze_arena_t *arena, const kaze_tensor_t *t)
{
    if (t == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "null tensor operand");
        return NULL;
    }
    if (t->dtype == KAZE_DTYPE_F32) {
        return t;
    }
    return kaze_quant_dequant(arena, t);
}

static bool is_scalar_tensor(const kaze_tensor_t *t)
{
    return t != NULL && kaze_tensor_numel(t) == 1;
}

typedef enum kaze_binop_t {
    KAZE_BIN_ADD = 0,
    KAZE_BIN_SUB,
    KAZE_BIN_MUL,
    KAZE_BIN_DIV
} kaze_binop_t;

static float apply_binop(kaze_binop_t op, float a, float b)
{
    switch (op) {
    case KAZE_BIN_ADD: return a + b;
    case KAZE_BIN_SUB: return a - b;
    case KAZE_BIN_MUL: return a * b;
    case KAZE_BIN_DIV:
        if (b == 0.0f) {
            (void)kaze_trap(KAZE_ERR_DIV0, "tensor division by zero");
            return 0.0f;
        }
        return a / b;
    default: return 0.0f;
    }
}

static kaze_tensor_t *elementwise(kaze_arena_t *arena, const kaze_tensor_t *a,
                                  const kaze_tensor_t *b, kaze_binop_t op)
{
    const kaze_tensor_t *x, *y;
    kaze_tensor_t *out;
    int64_t n, i;
    bool x_scalar, y_scalar;

    x = as_f32(arena, a);
    y = as_f32(arena, b);
    if (x == NULL || y == NULL) {
        return NULL;
    }
    x_scalar = is_scalar_tensor(x);
    y_scalar = is_scalar_tensor(y);
    if (!kaze_tensor_same_shape(x, y) && !x_scalar && !y_scalar) {
        (void)kaze_trap(KAZE_ERR_SHAPE,
                        "elementwise operands must match, or one must be a scalar");
        return NULL;
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32,
                            x_scalar && !y_scalar ? y->rank : x->rank,
                            x_scalar && !y_scalar ? y->shape : x->shape);
    if (out == NULL) {
        return NULL;
    }
    n = x_scalar && !y_scalar ? kaze_tensor_numel(y) : kaze_tensor_numel(x);
    for (i = 0; i < n; i++) {
        float av = x_scalar ? ((const float *)x->data)[0]
                                   : ((const float *)x->data)[i];
        float bv = y_scalar ? ((const float *)y->data)[0]
                                   : ((const float *)y->data)[i];
        float rv = apply_binop(op, av, bv);
        if (!isfinite((double)rv)) {
            (void)kaze_trap(KAZE_ERR_MATH, "tensor arithmetic produced a non-finite value");
            return NULL;
        }
        ((float *)out->data)[i] = rv;
    }
    return out;
}

kaze_tensor_t *kaze_tensor_add(kaze_arena_t *arena, const kaze_tensor_t *a,
                               const kaze_tensor_t *b)
{
    return elementwise(arena, a, b, KAZE_BIN_ADD);
}

kaze_tensor_t *kaze_tensor_sub(kaze_arena_t *arena, const kaze_tensor_t *a,
                               const kaze_tensor_t *b)
{
    return elementwise(arena, a, b, KAZE_BIN_SUB);
}

kaze_tensor_t *kaze_tensor_mul(kaze_arena_t *arena, const kaze_tensor_t *a,
                               const kaze_tensor_t *b)
{
    return elementwise(arena, a, b, KAZE_BIN_MUL);
}

kaze_tensor_t *kaze_tensor_div(kaze_arena_t *arena, const kaze_tensor_t *a,
                               const kaze_tensor_t *b)
{
    return elementwise(arena, a, b, KAZE_BIN_DIV);
}

kaze_tensor_t *kaze_tensor_scale(kaze_arena_t *arena, const kaze_tensor_t *a,
                                 float scalar)
{
    const kaze_tensor_t *x = as_f32(arena, a);
    kaze_tensor_t *out;
    int64_t n, i;

    if (x == NULL) {
        return NULL;
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, x->shape);
    if (out == NULL) {
        return NULL;
    }
    n = kaze_tensor_numel(x);
    for (i = 0; i < n; i++) {
        float rv = ((const float *)x->data)[i] * scalar;
        if (!isfinite((double)rv)) {
            (void)kaze_trap(KAZE_ERR_MATH, "tensor scaling produced a non-finite value");
            return NULL;
        }
        ((float *)out->data)[i] = rv;
    }
    return out;
}

kaze_tensor_t *kaze_tensor_add_bias(kaze_arena_t *arena, const kaze_tensor_t *x,
                                    const kaze_tensor_t *bias)
{
    const kaze_tensor_t *a, *b;
    kaze_tensor_t *out;
    int64_t rows, width, i, j;

    a = as_f32(arena, x);
    b = as_f32(arena, bias);
    if (a == NULL || b == NULL) {
        return NULL;
    }
    if (a->rank < 1 || b->rank != 1 || b->shape[0] != a->shape[a->rank - 1]) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "bias must be rank 1 with the tensor's last dimension");
        return NULL;
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, a->rank, a->shape);
    if (out == NULL) {
        return NULL;
    }
    width = a->shape[a->rank - 1];
    rows = width == 0 ? 0 : kaze_tensor_numel(a) / width;
    for (i = 0; i < rows; i++) {
        for (j = 0; j < width; j++) {
            ((float *)out->data)[i * width + j] =
                ((const float *)a->data)[i * width + j] +
                ((const float *)b->data)[j];
        }
    }
    return out;
}

/* ------------------------------------------------------------------------- */
/* GEMM kernels                                                               */
/* ------------------------------------------------------------------------- */

static float dot_f32(const float *x, const float *y, int64_t n)
{
    int64_t i = 0;
    float acc = 0.0f;

#if KAZE_HAVE_NEON
    {
        float32x4_t a0 = vdupq_n_f32(0.0f);
        float32x4_t a1 = vdupq_n_f32(0.0f);
        float32x4_t a2 = vdupq_n_f32(0.0f);
        float32x4_t a3 = vdupq_n_f32(0.0f);
        for (; i + 16 <= n; i += 16) {
            a0 = vfmaq_f32(a0, vld1q_f32(x + i), vld1q_f32(y + i));
            a1 = vfmaq_f32(a1, vld1q_f32(x + i + 4), vld1q_f32(y + i + 4));
            a2 = vfmaq_f32(a2, vld1q_f32(x + i + 8), vld1q_f32(y + i + 8));
            a3 = vfmaq_f32(a3, vld1q_f32(x + i + 12), vld1q_f32(y + i + 12));
        }
        a0 = vaddq_f32(a0, a1);
        a2 = vaddq_f32(a2, a3);
        a0 = vaddq_f32(a0, a2);
        acc = (float)(vgetq_lane_f32(a0, 0) + vgetq_lane_f32(a0, 1) +
                             vgetq_lane_f32(a0, 2) + vgetq_lane_f32(a0, 3));
    }
#endif
    for (; i < n; i++) {
        acc += x[i] * y[i];
    }
    return acc;
}

/* C[M,N] = A[M,K] * B[K,N], row-major. A 4x4 register microkernel over k; the
 * B panel is read in n-contiguous vectors, which is what the cache wants. */
static void gemm_f32_impl(const float *a, const float *b,
                          float *c, int64_t m, int64_t n, int64_t k)
{
    int64_t i0, j0;

    for (i0 = 0; i0 < m; i0 += 4) {
        int64_t imax = (i0 + 4 < m) ? i0 + 4 : m;
        for (j0 = 0; j0 < n; j0 += 4) {
            int64_t jmax = (j0 + 4 < n) ? j0 + 4 : n;
#if KAZE_HAVE_NEON
            if (imax - i0 == 4 && jmax - j0 == 4) {
                float32x4_t c0 = vdupq_n_f32(0.0f);
                float32x4_t c1 = vdupq_n_f32(0.0f);
                float32x4_t c2 = vdupq_n_f32(0.0f);
                float32x4_t c3 = vdupq_n_f32(0.0f);
                int64_t kk;
                for (kk = 0; kk < k; kk++) {
                    float32x4_t bv = vld1q_f32(b + kk * n + j0);
                    c0 = vfmaq_n_f32(c0, bv, a[(i0 + 0) * k + kk]);
                    c1 = vfmaq_n_f32(c1, bv, a[(i0 + 1) * k + kk]);
                    c2 = vfmaq_n_f32(c2, bv, a[(i0 + 2) * k + kk]);
                    c3 = vfmaq_n_f32(c3, bv, a[(i0 + 3) * k + kk]);
                }
                vst1q_f32(c + (i0 + 0) * n + j0, c0);
                vst1q_f32(c + (i0 + 1) * n + j0, c1);
                vst1q_f32(c + (i0 + 2) * n + j0, c2);
                vst1q_f32(c + (i0 + 3) * n + j0, c3);
                continue;
            }
#endif
            {
                int64_t i, j, kk;
                for (i = i0; i < imax; i++) {
                    for (j = j0; j < jmax; j++) {
                        float acc = 0.0f;
                        for (kk = 0; kk < k; kk++) {
                            acc += a[i * k + kk] * b[kk * n + j];
                        }
                        c[i * n + j] = acc;
                    }
                }
            }
        }
    }
}

void kaze_gemm_f32(const float *a, const float *b,
                   float *c, int64_t m, int64_t n, int64_t k)
{
    if (a == NULL || b == NULL || c == NULL || m < 0 || n < 0 || k < 0) {
        (void)kaze_trap(KAZE_ERR_BADARG, "gemm with a null buffer or a negative extent");
        return;
    }
    gemm_f32_impl(a, b, c, m, n, k);
}

bool kaze_tensor_has_neon(void)
{
#if KAZE_HAVE_NEON
    return true;
#else
    return false;
#endif
}

/* ------------------------------------------------------------------------- */
/* Matrix products                                                            */
/* ------------------------------------------------------------------------- */

kaze_tensor_t *kaze_tensor_matmul(kaze_arena_t *arena, const kaze_tensor_t *a,
                                  const kaze_tensor_t *b)
{
    const kaze_tensor_t *x = as_f32(arena, a);
    const kaze_tensor_t *y = as_f32(arena, b);
    kaze_tensor_t *out;
    int64_t m, n, k;

    if (x == NULL || y == NULL) {
        return NULL;
    }
    if (x->rank == 2 && y->rank == 2) {
        int64_t shape[2];
        m = x->shape[0];
        k = x->shape[1];
        n = y->shape[1];
        if (y->shape[0] != k) {
            (void)kaze_trap(KAZE_ERR_SHAPE, "matmul inner dimensions do not match");
            return NULL;
        }
        shape[0] = m;
        shape[1] = n;
        out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, 2, shape);
        if (out == NULL) {
            return NULL;
        }
        gemm_f32_impl((const float *)x->data, (const float *)y->data,
                      (float *)out->data, m, n, k);
        return out;
    }
    if (x->rank == 3 && (y->rank == 2 || y->rank == 3)) {
        int64_t batch = x->shape[0];
        int64_t ybatch = y->rank == 3 ? y->shape[0] : 1;
        int64_t shape[3];
        m = x->shape[1];
        k = x->shape[2];
        n = y->shape[y->rank - 1];
        if (y->shape[y->rank - 2] != k) {
            (void)kaze_trap(KAZE_ERR_SHAPE, "batched matmul inner dimensions do not match");
            return NULL;
        }
        if (!(ybatch == 1 || ybatch == batch)) {
            (void)kaze_trap(KAZE_ERR_SHAPE, "batched matmul batch dimensions do not broadcast");
            return NULL;
        }
        shape[0] = batch;
        shape[1] = m;
        shape[2] = n;
        out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, 3, shape);
        if (out == NULL) {
            return NULL;
        }
        {
            int64_t bi;
            for (bi = 0; bi < batch; bi++) {
                const float *bp = (const float *)y->data +
                    (ybatch == 1 ? 0 : bi) * k * n;
                gemm_f32_impl((const float *)x->data + bi * m * k, bp,
                              (float *)out->data + bi * m * n, m, n, k);
            }
        }
        return out;
    }
    (void)kaze_trap(KAZE_ERR_SHAPE, "matmul needs rank 2, or rank 3 with a rank-2/3 right side");
    return NULL;
}

kaze_tensor_t *kaze_tensor_matmul_t(kaze_arena_t *arena, const kaze_tensor_t *a,
                                    const kaze_tensor_t *b)
{
    const kaze_tensor_t *x = as_f32(arena, a);
    const kaze_tensor_t *y = as_f32(arena, b);
    kaze_tensor_t *out;
    int64_t batch, m, n, k, bi, i, j;

    if (x == NULL || y == NULL) {
        return NULL;
    }
    if (x->rank == 2 && y->rank == 2) {
        int64_t shape[2];
        m = x->shape[0];
        k = x->shape[1];
        n = y->shape[0];
        if (y->shape[1] != k) {
            (void)kaze_trap(KAZE_ERR_SHAPE, "matmul_t inner dimensions do not match");
            return NULL;
        }
        shape[0] = m;
        shape[1] = n;
        out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, 2, shape);
        if (out == NULL) {
            return NULL;
        }
        for (i = 0; i < m; i++) {
            for (j = 0; j < n; j++) {
                ((float *)out->data)[i * n + j] =
                    dot_f32((const float *)x->data + i * k,
                            (const float *)y->data + j * k, k);
            }
        }
        return out;
    }
    if (x->rank == 3 && y->rank == 3) {
        int64_t shape[3];
        batch = x->shape[0];
        m = x->shape[1];
        k = x->shape[2];
        n = y->shape[1];
        if (y->shape[0] != batch || y->shape[2] != k) {
            (void)kaze_trap(KAZE_ERR_SHAPE, "batched matmul_t batch/inner dimensions do not match");
            return NULL;
        }
        shape[0] = batch;
        shape[1] = m;
        shape[2] = n;
        out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, 3, shape);
        if (out == NULL) {
            return NULL;
        }
        for (bi = 0; bi < batch; bi++) {
            const float *xp = (const float *)x->data + bi * m * k;
            const float *yp = (const float *)y->data + bi * n * k;
            float *op = (float *)out->data + bi * m * n;
            for (i = 0; i < m; i++) {
                for (j = 0; j < n; j++) {
                    op[i * n + j] = dot_f32(xp + i * k, yp + j * k, k);
                }
            }
        }
        return out;
    }
    (void)kaze_trap(KAZE_ERR_SHAPE, "matmul_t needs rank 2 or matching rank 3");
    return NULL;
}

kaze_tensor_t *kaze_tensor_matmul_quant(kaze_arena_t *arena,
                                        const kaze_tensor_t *x,
                                        const kaze_tensor_t *w)
{
    const kaze_tensor_t *a = as_f32(arena, x);
    kaze_tensor_t *out;
    int64_t m, n, k, i, j;
    int64_t shape[2];

    if (a == NULL || w == NULL) {
        return NULL;
    }
    if (a->rank != 2 || w->rank != 2) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "matmul_quant needs rank-2 activations and weights");
        return NULL;
    }
    m = a->shape[0];
    k = a->shape[1];
    n = w->shape[0];
    if (w->shape[1] != k) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "matmul_quant weight row width does not match the activation");
        return NULL;
    }
    shape[0] = m;
    shape[1] = n;
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, 2, shape);
    if (out == NULL) {
        return NULL;
    }
    {
        const char *act_mode = getenv("KAZE_MATMUL_ACT");
        bool use_q8_1 = (act_mode == NULL) || (strcmp(act_mode, "q8_1") == 0);
        if (!use_q8_1) {
            goto f32_activation_path;
        }
    }
    if (w->dtype == KAZE_DTYPE_Q8_0 && k > 0 && (k % 32) == 0) {
        size_t row_bytes = kaze_q8_1_bytes(k);
        uint8_t *q81 = (uint8_t *)kmalloc(arena, row_bytes * (size_t)m);
        if (q81 == NULL) {
            (void)kaze_trap(KAZE_ERR_OOM, "q8_1 activation scratch did not fit the arena");
            return NULL;
        }
        for (i = 0; i < m; i++) {
            kaze_quantize_q8_1((const float *)a->data + i * k, k,
                               q81 + (size_t)i * row_bytes);
        }
        for (i = 0; i < m; i++) {
            for (j = 0; j < n; j++) {
                ((float *)out->data)[i * n + j] =
                    kaze_quant_q8_0_q8_1_dot(w, j, q81 + (size_t)i * row_bytes, k);
            }
        }
        return out;
    }
f32_activation_path:
    for (i = 0; i < m; i++) {
        const float *xp = (const float *)a->data + i * k;
        for (j = 0; j < n; j++) {
            ((float *)out->data)[i * n + j] =
                kaze_quant_row_dot(w, j, xp, k);
        }
    }
    return out;
}

/* ------------------------------------------------------------------------- */
/* Layout operations                                                          */
/* ------------------------------------------------------------------------- */

kaze_tensor_t *kaze_tensor_transpose(kaze_arena_t *arena, const kaze_tensor_t *t,
                                     int32_t dim0, int32_t dim1)
{
    const kaze_tensor_t *x = as_f32(arena, t);
    kaze_tensor_t *out;
    int64_t oshape[KAZE_TENSOR_MAX_DIMS];
    int64_t ostrides[KAZE_TENSOR_MAX_DIMS];
    int64_t coord[KAZE_TENSOR_MAX_DIMS];
    int64_t n, i;
    int32_t d;

    if (x == NULL) {
        return NULL;
    }
    if (x->rank < 2 || dim0 < 0 || dim1 < 0 || dim0 >= x->rank ||
        dim1 >= x->rank || dim0 == dim1) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "transpose needs two different dimensions");
        return NULL;
    }
    for (d = 0; d < x->rank; d++) {
        oshape[d] = x->shape[d];
    }
    {
        int64_t tmp = oshape[dim0];
        oshape[dim0] = oshape[dim1];
        oshape[dim1] = tmp;
    }
    {
        int64_t step = 1;
        for (d = x->rank - 1; d >= 0; d--) {
            ostrides[d] = step;
            step *= oshape[d];
        }
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, oshape);
    if (out == NULL) {
        return NULL;
    }
    n = kaze_tensor_numel(x);
    for (i = 0; i < n; i++) {
        int64_t rem = i;
        int64_t oi = 0;
        int64_t xv;
        for (d = x->rank - 1; d >= 0; d--) {
            coord[d] = rem % x->shape[d];
            rem /= x->shape[d];
        }
        xv = (int64_t)coord[dim0];
        coord[dim0] = coord[dim1];
        coord[dim1] = xv;
        for (d = 0; d < x->rank; d++) {
            oi += coord[d] * ostrides[d];
        }
        ((float *)out->data)[oi] = ((const float *)x->data)[i];
    }
    return out;
}

kaze_tensor_t *kaze_tensor_reshape(kaze_arena_t *arena, const kaze_tensor_t *t,
                                   int32_t rank, const int64_t *shape)
{
    return kaze_tensor_view(arena, t, rank, shape);
}

/* ------------------------------------------------------------------------- */
/* Numerics                                                                   */
/* ------------------------------------------------------------------------- */

kaze_tensor_t *kaze_tensor_softmax(kaze_arena_t *arena, const kaze_tensor_t *t)
{
    const kaze_tensor_t *x = as_f32(arena, t);
    kaze_tensor_t *out;
    int64_t width, rows, r, j;

    if (x == NULL) {
        return NULL;
    }
    if (x->rank < 1) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "softmax needs at least one dimension");
        return NULL;
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, x->shape);
    if (out == NULL) {
        return NULL;
    }
    width = x->shape[x->rank - 1];
    rows = kaze_tensor_numel(x) / width;
    for (r = 0; r < rows; r++) {
        const float *xp = (const float *)x->data + r * width;
        float *op = (float *)out->data + r * width;
        double maxv = -HUGE_VAL;
        double sum = 0.0;
        for (j = 0; j < width; j++) {
            double v = (double)xp[j];
            if (v > maxv) {
                maxv = v;
            }
        }
        for (j = 0; j < width; j++) {
            double e = exp((double)xp[j] - maxv);
            sum += e;
        }
        if (!isfinite(sum) || sum <= 0.0) {
            (void)kaze_trap(KAZE_ERR_MATH, "softmax normalizer is not finite and positive");
            return NULL;
        }
        for (j = 0; j < width; j++) {
            op[j] = (float)(exp((double)xp[j] - maxv) / sum);
        }
    }
    return out;
}

kaze_tensor_t *kaze_tensor_rms_norm(kaze_arena_t *arena, const kaze_tensor_t *x,
                                    const kaze_tensor_t *weight, float eps)
{
    const kaze_tensor_t *a = as_f32(arena, x);
    const kaze_tensor_t *w = as_f32(arena, weight);
    kaze_tensor_t *out;
    int64_t width, rows, r, j;

    if (a == NULL || w == NULL) {
        return NULL;
    }
    if (a->rank < 1 || w->rank != 1 || w->shape[0] != a->shape[a->rank - 1]) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "rms_norm weight must match the last dimension");
        return NULL;
    }
    if (!(eps > 0.0f) || !isfinite((double)eps)) {
        (void)kaze_trap(KAZE_ERR_MATH, "rms_norm epsilon must be finite and positive");
        return NULL;
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, a->rank, a->shape);
    if (out == NULL) {
        return NULL;
    }
    width = a->shape[a->rank - 1];
    rows = kaze_tensor_numel(a) / width;
    for (r = 0; r < rows; r++) {
        const float *xp = (const float *)a->data + r * width;
        float *op = (float *)out->data + r * width;
        double ss = 0.0;
        double inv;
        for (j = 0; j < width; j++) {
            ss += (double)xp[j] * (double)xp[j];
        }
        ss = ss / (double)width + (double)eps;
        if (!(ss > 0.0) || !isfinite(ss)) {
            (void)kaze_trap(KAZE_ERR_MATH, "rms_norm variance is not finite and positive");
            return NULL;
        }
        inv = 1.0 / sqrt(ss);
        for (j = 0; j < width; j++) {
            op[j] = (float)((double)xp[j] * inv * (double)((const float *)w->data)[j]);
        }
    }
    return out;
}

kaze_tensor_t *kaze_tensor_layer_norm(kaze_arena_t *arena, const kaze_tensor_t *x,
                                      const kaze_tensor_t *weight,
                                      const kaze_tensor_t *bias,
                                      float eps)
{
    const kaze_tensor_t *a = as_f32(arena, x);
    const kaze_tensor_t *w = as_f32(arena, weight);
    const kaze_tensor_t *b = (bias == NULL) ? NULL : as_f32(arena, bias);
    kaze_tensor_t *out;
    int64_t width, rows, r, j;

    if (a == NULL || w == NULL) {
        return NULL;
    }
    if (a->rank < 1 || w->rank != 1 || w->shape[0] != a->shape[a->rank - 1]) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "layer_norm weight must match the last dimension");
        return NULL;
    }
    if (b != NULL && (b->rank != 1 || b->shape[0] != w->shape[0])) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "layer_norm bias must match the weight");
        return NULL;
    }
    if (!(eps > 0.0f) || !isfinite((double)eps)) {
        (void)kaze_trap(KAZE_ERR_MATH, "layer_norm epsilon must be finite and positive");
        return NULL;
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, a->rank, a->shape);
    if (out == NULL) {
        return NULL;
    }
    width = a->shape[a->rank - 1];
    rows = kaze_tensor_numel(a) / width;
    for (r = 0; r < rows; r++) {
        const float *xp = (const float *)a->data + r * width;
        float *op = (float *)out->data + r * width;
        double mean = 0.0, var = 0.0, inv;
        for (j = 0; j < width; j++) {
            mean += (double)xp[j];
        }
        mean /= (double)width;
        for (j = 0; j < width; j++) {
            double d = (double)xp[j] - mean;
            var += d * d;
        }
        var = var / (double)width + (double)eps;
        if (!(var > 0.0) || !isfinite(var)) {
            (void)kaze_trap(KAZE_ERR_MATH, "layer_norm variance is not finite and positive");
            return NULL;
        }
        inv = 1.0 / sqrt(var);
        for (j = 0; j < width; j++) {
            double v = ((double)xp[j] - mean) * inv *
                       (double)((const float *)w->data)[j];
            if (b != NULL) {
                v += (double)((const float *)b->data)[j];
            }
            op[j] = (float)v;
        }
    }
    return out;
}

kaze_tensor_t *kaze_tensor_gelu(kaze_arena_t *arena, const kaze_tensor_t *t)
{
    const kaze_tensor_t *x = as_f32(arena, t);
    kaze_tensor_t *out;
    int64_t n, i;

    if (x == NULL) {
        return NULL;
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, x->shape);
    if (out == NULL) {
        return NULL;
    }
    n = kaze_tensor_numel(x);
    for (i = 0; i < n; i++) {
        double v = (double)((const float *)x->data)[i];
        ((float *)out->data)[i] =
            (float)(0.5 * v * (1.0 + erf(v * 0.70710678118654752440)));
    }
    return out;
}

kaze_tensor_t *kaze_tensor_silu(kaze_arena_t *arena, const kaze_tensor_t *t)
{
    const kaze_tensor_t *x = as_f32(arena, t);
    kaze_tensor_t *out;
    int64_t n, i;

    if (x == NULL) {
        return NULL;
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, x->shape);
    if (out == NULL) {
        return NULL;
    }
    n = kaze_tensor_numel(x);
    for (i = 0; i < n; i++) {
        double v = (double)((const float *)x->data)[i];
        ((float *)out->data)[i] = (float)(v / (1.0 + exp(-v)));
    }
    return out;
}

kaze_tensor_t *kaze_tensor_relu(kaze_arena_t *arena, const kaze_tensor_t *t)
{
    const kaze_tensor_t *x = as_f32(arena, t);
    kaze_tensor_t *out;
    int64_t n, i;

    if (x == NULL) {
        return NULL;
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, x->shape);
    if (out == NULL) {
        return NULL;
    }
    n = kaze_tensor_numel(x);
    for (i = 0; i < n; i++) {
        float v = ((const float *)x->data)[i];
        ((float *)out->data)[i] = v > 0.0f ? v : 0.0f;
    }
    return out;
}

kaze_tensor_t *kaze_tensor_masked_fill(kaze_arena_t *arena,
                                       const kaze_tensor_t *t,
                                       const kaze_tensor_t *mask,
                                       float value)
{
    const kaze_tensor_t *x = as_f32(arena, t);
    kaze_tensor_t *out;
    int64_t mask_strides[KAZE_TENSOR_MAX_DIMS];
    int64_t coord[KAZE_TENSOR_MAX_DIMS];
    int64_t n, i;
    int32_t d, md;

    if (x == NULL || mask == NULL) {
        return NULL;
    }
    if (mask->dtype != KAZE_DTYPE_BOOL || !kaze_tensor_is_contiguous(mask)) {
        (void)kaze_trap(KAZE_ERR_TYPE, "masked_fill mask must be a contiguous bool tensor");
        return NULL;
    }
    if (mask->rank > x->rank) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "masked_fill mask has more dimensions than its input");
        return NULL;
    }
    {
        int64_t step = 1;
        for (md = mask->rank - 1; md >= 0; md--) {
            mask_strides[md] = step;
            step *= mask->shape[md];
        }
    }
    for (md = 0; md < mask->rank; md++) {
        int32_t td = x->rank - mask->rank + md;
        if (mask->shape[md] != 1 && mask->shape[md] != x->shape[td]) {
            (void)kaze_trap(KAZE_ERR_SHAPE, "masked_fill mask does not broadcast to its input");
            return NULL;
        }
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, x->shape);
    if (out == NULL) {
        return NULL;
    }
    memcpy(out->data, x->data, kaze_tensor_bytes(x));
    n = kaze_tensor_numel(x);
    for (d = 0; d < x->rank; d++) {
        coord[d] = 0;
    }
    for (i = 0; i < n; i++) {
        int64_t mi = 0;
        bool masked;
        for (md = 0; md < mask->rank; md++) {
            int32_t td = x->rank - mask->rank + md;
            int64_t c = (mask->shape[md] == 1) ? 0 : coord[td];
            mi += c * mask_strides[md];
        }
        masked = ((const uint8_t *)mask->data)[mi] != 0u;
        if (masked) {
            ((float *)out->data)[i] = value;
        }
        for (d = x->rank - 1; d >= 0; d--) {
            coord[d]++;
            if (coord[d] < x->shape[d]) {
                break;
            }
            coord[d] = 0;
        }
    }
    return out;
}

kaze_tensor_t *kaze_tensor_rope(kaze_arena_t *arena, const kaze_tensor_t *t,
                                int32_t heads, int32_t head_dim,
                                size_t pos0, float theta)
{
    const kaze_tensor_t *x = as_f32(arena, t);
    kaze_tensor_t *out;
    int64_t seq, row, p;
    int32_t h, i;

    if (x == NULL) {
        return NULL;
    }
    if (x->rank != 2 || heads <= 0 || head_dim < 2 || (head_dim % 2) != 0 ||
        x->shape[1] != (int64_t)heads * (int64_t)head_dim) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "rope needs [seq, heads*head_dim] with an even head_dim");
        return NULL;
    }
    if (!(theta > 1.0f) || !isfinite((double)theta)) {
        (void)kaze_trap(KAZE_ERR_MATH, "rope theta must be finite and greater than 1");
        return NULL;
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, x->shape);
    if (out == NULL) {
        return NULL;
    }
    seq = x->shape[0];
    row = x->shape[1];
    for (p = 0; p < seq; p++) {
        for (h = 0; h < heads; h++) {
            for (i = 0; i < head_dim / 2; i++) {
                double inv = pow((double)theta, -2.0 * (double)i / (double)head_dim);
                double ang = ((double)pos0 + (double)p) * inv;
                double c = cos(ang);
                double s = sin(ang);
                int64_t i0 = p * row + (int64_t)h * head_dim + 2 * i;
                int64_t i1 = i0 + 1;
                double x0 = (double)((const float *)x->data)[i0];
                double x1 = (double)((const float *)x->data)[i1];
                ((float *)out->data)[i0] = (float)(x0 * c - x1 * s);
                ((float *)out->data)[i1] = (float)(x0 * s + x1 * c);
            }
        }
    }
    return out;
}

/* ------------------------------------------------------------------------- */
/* Reductions                                                                 */
/* ------------------------------------------------------------------------- */

kaze_tensor_t *kaze_tensor_sum_last(kaze_arena_t *arena, const kaze_tensor_t *t)
{
    const kaze_tensor_t *x = as_f32(arena, t);
    kaze_tensor_t *out;
    int64_t oshape[KAZE_TENSOR_MAX_DIMS];
    int64_t width, rows, r, j;
    int32_t d;

    if (x == NULL) {
        return NULL;
    }
    if (x->rank < 1) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "sum_last needs at least one dimension");
        return NULL;
    }
    for (d = 0; d < x->rank; d++) {
        oshape[d] = x->shape[d];
    }
    oshape[x->rank - 1] = 1;
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, oshape);
    if (out == NULL) {
        return NULL;
    }
    width = x->shape[x->rank - 1];
    rows = kaze_tensor_numel(x) / width;
    for (r = 0; r < rows; r++) {
        double acc = 0.0;
        for (j = 0; j < width; j++) {
            acc += (double)((const float *)x->data)[r * width + j];
        }
        ((float *)out->data)[r] = (float)acc;
    }
    return out;
}

kaze_tensor_t *kaze_tensor_mean_last(kaze_arena_t *arena, const kaze_tensor_t *t)
{
    const kaze_tensor_t *x = as_f32(arena, t);
    kaze_tensor_t *out;
    int64_t width, rows, r;

    if (x == NULL) {
        return NULL;
    }
    out = kaze_tensor_sum_last(arena, x);
    if (out == NULL) {
        return NULL;
    }
    width = x->shape[x->rank - 1];
    rows = kaze_tensor_numel(x) / width;
    for (r = 0; r < rows; r++) {
        ((float *)out->data)[r] = (float)((double)((float *)out->data)[r] / (double)width);
    }
    return out;
}

float kaze_tensor_sum_all(const kaze_tensor_t *t)
{
    const float *p;
    double acc = 0.0;
    int64_t n, i;

    if (t == NULL || t->dtype != KAZE_DTYPE_F32) {
        (void)kaze_trap(KAZE_ERR_TYPE, "sum_all needs a dense f32 tensor");
        return 0.0f;
    }
    p = (const float *)t->data;
    n = kaze_tensor_numel(t);
    for (i = 0; i < n; i++) {
        acc += (double)p[i];
    }
    return (float)acc;
}

float kaze_tensor_max_all(const kaze_tensor_t *t)
{
    const float *p;
    float best;
    int64_t n, i;

    if (t == NULL || t->dtype != KAZE_DTYPE_F32) {
        (void)kaze_trap(KAZE_ERR_TYPE, "max_all needs a dense f32 tensor");
        return 0.0f;
    }
    n = kaze_tensor_numel(t);
    if (n == 0) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "max_all of an empty tensor");
        return 0.0f;
    }
    p = (const float *)t->data;
    best = p[0];
    for (i = 1; i < n; i++) {
        if (p[i] > best) {
            best = p[i];
        }
    }
    return best;
}

int64_t kaze_tensor_argmax_last(const kaze_tensor_t *t)
{
    float best;
    int64_t best_i = 0, j;

    if (t == NULL || t->dtype != KAZE_DTYPE_F32 || t->rank < 1) {
        (void)kaze_trap(KAZE_ERR_TYPE, "argmax_last needs a dense f32 tensor with at least one dimension");
        return -1;
    }
    if (t->shape[t->rank - 1] <= 0) {
        (void)kaze_trap(KAZE_ERR_SHAPE, "argmax_last of an empty row");
        return -1;
    }
    best = ((const float *)t->data)[0];
    for (j = 1; j < t->shape[t->rank - 1]; j++) {
        float v = ((const float *)t->data)[j];
        if (v > best) {
            best = v;
            best_i = j;
        }
    }
    return best_i;
}

/* ------------------------------------------------------------------------- */
/* Dequantization entry point                                                 */
/* ------------------------------------------------------------------------- */

kaze_tensor_t *kaze_tensor_dequant(kaze_arena_t *arena, const kaze_tensor_t *t)
{
    if (t == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "dequant of a null tensor");
        return NULL;
    }
    if (t->dtype == KAZE_DTYPE_F32) {
        return kaze_tensor_copy(arena, t);
    }
    return kaze_quant_dequant(arena, t);
}






