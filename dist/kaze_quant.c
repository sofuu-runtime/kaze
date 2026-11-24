/* kaze_quant.c — GGUF block quantization decode + fused dot products.
 *
 * The layouts are llama.cpp's on-disk layouts; a file that runs there must
 * decode here. Every decoder is bounds-driven by the tensor's logical shape, so
 * a truncated file cannot read past its mmap: the GGUF parser already validated
 * the byte ranges before a tensor is handed to these functions.
 */
#include "kaze_quant.h"

#include <math.h>
#include <string.h>

#if defined(__ARM_FEATURE_DOTPROD)
#include <arm_neon.h>
#define KAZE_QUANT_DOTPROD 1
#else
#define KAZE_QUANT_DOTPROD 0
#endif

/* ------------------------------------------------------------------------- */
/* IEEE 754 half / bfloat16                                                   */
/* ------------------------------------------------------------------------- */

float kaze_f16_to_f32(uint16_t h)
{
    uint32_t sign = (uint32_t)(h & 0x8000u) << 16;
    uint32_t exp = (uint32_t)(h >> 10) & 0x1Fu;
    uint32_t mant = (uint32_t)h & 0x3FFu;
    uint32_t bits;
    union { uint32_t u; float f; } cvt;

    if (exp == 0) {
        if (mant == 0) {
            bits = sign;
        } else {
            int32_t e = -1;
            do {
                e++;
                mant <<= 1;
            } while ((mant & 0x400u) == 0);
            mant &= 0x3FFu;
            bits = sign | (uint32_t)(127 - 15 - e) << 23 | mant << 13;
        }
    } else if (exp == 31) {
        bits = sign | 0x7F800000u | mant << 13;
    } else {
        bits = sign | (exp + (127 - 15)) << 23 | mant << 13;
    }
    cvt.u = bits;
    return (float)cvt.f;
}

uint16_t kaze_f32_to_f16(float f)
{
    union { uint32_t u; float f; } cvt;
    uint32_t sign, exp, mant;
    int32_t e;

    cvt.f = (float)f;
    sign = (cvt.u >> 16) & 0x8000u;
    exp = (cvt.u >> 23) & 0xFFu;
    mant = cvt.u & 0x7FFFFFu;

    if (exp == 0xFFu) {
        return (uint16_t)(sign | 0x7C00u | (mant ? 0x200u : 0u));
    }
    e = (int32_t)exp - 127 + 15;
    if (e >= 31) {
        return (uint16_t)(sign | 0x7C00u);
    }
    if (e <= 0) {
        uint32_t shift, half, rem, halfway;
        if (e < -10) {
            return (uint16_t)sign;
        }
        mant |= 0x800000u;
        shift = (uint32_t)(14 - e);
        half = mant >> shift;
        rem = mant & ((1u << shift) - 1u);
        halfway = 1u << (shift - 1u);
        if (rem > halfway || (rem == halfway && (half & 1u) != 0u)) {
            half++;
        }
        return (uint16_t)(sign | half);
    }
    {
        uint16_t out = (uint16_t)(sign | ((uint32_t)e << 10) | (mant >> 13));
        uint32_t rem = mant & 0x1FFFu;
        if (rem > 0x1000u || (rem == 0x1000u && (out & 1u) != 0u)) {
            out = (uint16_t)(out + 1u);
        }
        return out;
    }
}

float kaze_bf16_to_f32(uint16_t b)
{
    union { uint32_t u; float f; } cvt;
    cvt.u = (uint32_t)b << 16;
    return (float)cvt.f;
}

uint16_t kaze_f32_to_bf16(float f)
{
    union { uint32_t u; float f; } cvt;
    uint32_t lsb, rounding;
    cvt.f = (float)f;
    lsb = (cvt.u >> 16) & 1u;
    rounding = 0x7FFFu + lsb;
    return (uint16_t)((cvt.u + rounding) >> 16);
}

/* ------------------------------------------------------------------------- */
/* GGUF dtype mapping and geometry                                             */
/* ------------------------------------------------------------------------- */

bool kaze_dtype_from_gguf(uint32_t ggml_type, kaze_dtype_t *out)
{
    switch (ggml_type) {
    case 0: *out = KAZE_DTYPE_F32; return true;
    case 1: *out = KAZE_DTYPE_F16; return true;
    case 2: *out = KAZE_DTYPE_Q4_0; return true;
    case 8: *out = KAZE_DTYPE_Q8_0; return true;
    case 12: *out = KAZE_DTYPE_Q4_K; return true;
    case 14: *out = KAZE_DTYPE_Q6_K; return true;
    case 24: *out = KAZE_DTYPE_I8; return true;
    case 30: *out = KAZE_DTYPE_BF16; return true;
    default: return false;
    }
}

/* ------------------------------------------------------------------------- */
/* Q8_0: 32 elements, fp16 scale + 32 int8                                    */
/* ------------------------------------------------------------------------- */

static float q8_0_dot(const uint8_t *block, const float *x)
{
    uint16_t dh;
    float d;
    const int8_t *qs = (const int8_t *)(block + 2);
    float acc = 0.0f;
    int i;

    memcpy(&dh, block, sizeof dh);
    d = kaze_f16_to_f32(dh);
    for (i = 0; i < KAZE_QK8_0; i++) {
        acc += (float)qs[i] * x[i];
    }
    return d * acc;
}

static void q8_0_row(const uint8_t *block, float *out)
{
    uint16_t dh;
    float d;
    const int8_t *qs = (const int8_t *)(block + 2);
    int i;

    memcpy(&dh, block, sizeof dh);
    d = kaze_f16_to_f32(dh);
    for (i = 0; i < KAZE_QK8_0; i++) {
        out[i] = d * (float)qs[i];
    }
}

/* ------------------------------------------------------------------------- */
/* Q4_0: 32 elements, fp16 scale + 16 packed nibbles (low half first)         */
/* ------------------------------------------------------------------------- */

static float q4_0_dot(const uint8_t *block, const float *x)
{
    uint16_t dh;
    float d;
    const uint8_t *qs = block + 2;
    float acc = 0.0f;
    int i;

    memcpy(&dh, block, sizeof dh);
    d = kaze_f16_to_f32(dh);
    for (i = 0; i < 16; i++) {
        float x0 = (float)((int)qs[i] & 0xF) - 8.0f;
        float x1 = (float)((int)qs[i] >> 4) - 8.0f;
        acc += x0 * x[i];
        acc += x1 * x[i + 16];
    }
    return d * acc;
}

static void q4_0_row(const uint8_t *block, float *out)
{
    uint16_t dh;
    float d;
    const uint8_t *qs = block + 2;
    int i;

    memcpy(&dh, block, sizeof dh);
    d = kaze_f16_to_f32(dh);
    for (i = 0; i < 16; i++) {
        out[i] = d * ((float)((int)qs[i] & 0xF) - 8.0f);
        out[i + 16] = d * ((float)((int)qs[i] >> 4) - 8.0f);
    }
}

/* ------------------------------------------------------------------------- */
/* Q4_K: 256 elements, d/dmin fp16, 12 packed 6-bit scale/min pairs, 128 qs   */
/* ------------------------------------------------------------------------- */

static void q4_k_scale_min(int j, const uint8_t *q, uint8_t *d, uint8_t *m)
{
    if (j < 4) {
        *d = (uint8_t)(q[j] & 63u);
        *m = (uint8_t)(q[j + 4] & 63u);
    } else {
        *d = (uint8_t)((q[j + 4] & 0xFu) | ((q[j - 4] >> 6) << 4));
        *m = (uint8_t)((q[j + 4] >> 4) | ((q[j] >> 6) << 4));
    }
}

static void q4_k_row(const uint8_t *block, float *out)
{
    uint16_t dh, dmh;
    float d, dmin;
    const uint8_t *scales = block + 4;
    const uint8_t *qs = block + 16;
    int j;

    memcpy(&dh, block, sizeof dh);
    memcpy(&dmh, block + 2, sizeof dmh);
    d = kaze_f16_to_f32(dh);
    dmin = kaze_f16_to_f32(dmh);
    for (j = 0; j < 8; j++) {
        uint8_t sc, m;
        float dl, ml;
        int l;
        q4_k_scale_min(j, scales, &sc, &m);
        dl = d * (float)sc;
        ml = dmin * (float)m;
        for (l = 0; l < 32; l++) {
            uint8_t qv = (l < 16) ? (uint8_t)(qs[l] & 0xFu)
                                  : (uint8_t)(qs[l - 16] >> 4);
            out[j * 32 + l] = dl * (float)qv - ml;
        }
        qs += 32;
    }
}

static float q4_k_dot(const uint8_t *block, const float *x)
{
    uint16_t dh, dmh;
    float d, dmin;
    const uint8_t *scales = block + 4;
    const uint8_t *qs = block + 16;
    float acc = 0.0f;
    int j;

    memcpy(&dh, block, sizeof dh);
    memcpy(&dmh, block + 2, sizeof dmh);
    d = kaze_f16_to_f32(dh);
    dmin = kaze_f16_to_f32(dmh);
    for (j = 0; j < 8; j++) {
        uint8_t sc, m;
        float dl, ml, sub = 0.0f;
        int l;
        q4_k_scale_min(j, scales, &sc, &m);
        dl = d * (float)sc;
        ml = dmin * (float)m;
        for (l = 0; l < 32; l++) {
            uint8_t qv = (l < 16) ? (uint8_t)(qs[l] & 0xFu)
                                  : (uint8_t)(qs[l - 16] >> 4);
            sub += (float)qv * x[j * 32 + l];
        }
        acc += (dl - ml) * sub;
    }
    return acc;
}

/* ------------------------------------------------------------------------- */
/* Q6_K: 256 elements, 128 ql + 64 qh + 16 int8 scales + fp16 d              */
/* ------------------------------------------------------------------------- */

static void q6_k_row(const uint8_t *block, float *out)
{
    const uint8_t *ql = block;
    const uint8_t *qh = block + 128;
    const int8_t *sc = (const int8_t *)(block + 192);
    uint16_t dh;
    float d;
    int n;

    memcpy(&dh, block + 208, sizeof dh);
    d = kaze_f16_to_f32(dh);
    for (n = 0; n < 256; n += 128) {
        int l;
        for (l = 0; l < 32; l++) {
            int is = l / 16;
            int8_t q1 = (int8_t)((ql[l] & 0xF) | (((qh[l] >> 0) & 3) << 4)) - 32;
            int8_t q2 = (int8_t)((ql[l + 32] & 0xF) | (((qh[l] >> 2) & 3) << 4)) - 32;
            int8_t q3 = (int8_t)((ql[l] >> 4) | (((qh[l] >> 4) & 3) << 4)) - 32;
            int8_t q4 = (int8_t)((ql[l + 32] >> 4) | (((qh[l] >> 6) & 3) << 4)) - 32;
            out[n + l] = d * (float)sc[is + 0] * (float)q1;
            out[n + l + 32] = d * (float)sc[is + 2] * (float)q2;
            out[n + l + 64] = d * (float)sc[is + 4] * (float)q3;
            out[n + l + 96] = d * (float)sc[is + 6] * (float)q4;
        }
        ql += 64;
        qh += 32;
        sc += 8;
    }
}

static float q6_k_dot(const uint8_t *block, const float *x)
{
    float y[256];
    float acc = 0.0f;
    int i;

    q6_k_row(block, y);
    for (i = 0; i < 256; i++) {
        acc += y[i] * x[i];
    }
    return acc;
}

/* ------------------------------------------------------------------------- */
/* Dispatch                                                                   */
/* ------------------------------------------------------------------------- */

static size_t row_blocks(const kaze_tensor_t *t, int64_t n)
{
    size_t be = kaze_dtype_block_elems(t->dtype);
    if (be == 0) {
        return 0;
    }
    return (size_t)(n / (int64_t)be);
}

float kaze_quant_row_dot(const kaze_tensor_t *t, int64_t row,
                                const float *x, int64_t n)
{
    const uint8_t *base;
    size_t bb = kaze_dtype_block_bytes(t->dtype);
    size_t nb = row_blocks(t, n);
    size_t b;
    float acc = 0.0f;

    if (n < 0 || row < 0) {
        (void)kaze_trap(KAZE_ERR_BADARG, "negative row or length in quant dot");
        return 0.0f;
    }
    if (t->dtype == KAZE_DTYPE_F32) {
        const float *p = (const float *)t->data + (size_t)row * (size_t)n;
        for (b = 0; b < (size_t)n; b++) {
            acc += p[b] * x[b];
        }
        return acc;
    }
    if (t->dtype == KAZE_DTYPE_F16 || t->dtype == KAZE_DTYPE_BF16) {
        const uint16_t *p = (const uint16_t *)t->data + (size_t)row * (size_t)n;
        for (b = 0; b < (size_t)n; b++) {
            acc += ((t->dtype == KAZE_DTYPE_F16) ? kaze_f16_to_f32(p[b])
                                                 : kaze_bf16_to_f32(p[b])) * x[b];
        }
        return acc;
    }
    if (bb == 0 || nb == 0 || n % (int64_t)kaze_dtype_block_elems(t->dtype) != 0) {
        (void)kaze_trap(KAZE_ERR_QUANT, "quantized row length is not a block multiple");
        return 0.0f;
    }
    base = (const uint8_t *)t->data + (size_t)row * nb * bb;
    for (b = 0; b < nb; b++) {
        const uint8_t *blk = base + b * bb;
        switch (t->dtype) {
        case KAZE_DTYPE_Q8_0:
            acc += q8_0_dot(blk, x + b * 32);
            break;
        case KAZE_DTYPE_Q4_0:
            acc += q4_0_dot(blk, x + b * 32);
            break;
        case KAZE_DTYPE_Q4_K:
            acc += q4_k_dot(blk, x + b * 256);
            break;
        case KAZE_DTYPE_Q6_K:
            acc += q6_k_dot(blk, x + b * 256);
            break;
        default:
            (void)kaze_trap(KAZE_ERR_QUANT, "unsupported quantized dtype in dot");
            return 0.0f;
        }
    }
    return acc;
}

size_t kaze_q8_1_bytes(int64_t n)
{
    size_t blocks;
    if (n <= 0 || (n % 32) != 0) {
        return 0;
    }
    blocks = (size_t)(n / 32);
    return blocks * KAZE_Q8_1_BLOCK_BYTES;
}

void kaze_quantize_q8_1(const float *x, int64_t n, uint8_t *out)
{
    int64_t b;
    size_t blocks;

    if (x == NULL || out == NULL || n <= 0 || (n % 32) != 0) {
        (void)kaze_trap(KAZE_ERR_BADARG, "q8_1 quantization needs a positive block-multiple length");
        return;
    }
    blocks = (size_t)(n / 32);
    for (b = 0; b < (int64_t)blocks; b++) {
        uint8_t *blk = out + (size_t)b * KAZE_Q8_1_BLOCK_BYTES;
        const float *xb = x + b * 32;
        float amax = 0.0f;
        uint16_t dh;
        float d, id;
        int j;

        for (j = 0; j < 32; j++) {
            float v = xb[j] < 0.0f ? -xb[j] : xb[j];
            if (v > amax) {
                amax = v;
            }
        }
        /* llama.cpp derives the integer scale from the f32 d and only the stored
         * block scale is rounded through f16; using the rounded value here would
         * change some activation quants by one and shift every logit. */
        d = amax / 127.0f;
        id = (d != 0.0f) ? 1.0f / d : 0.0f;
        dh = kaze_f32_to_f16(d);
        memcpy(blk, &dh, sizeof dh);
        for (j = 0; j < 32; j++) {
            blk[2 + j] = (uint8_t)(int8_t)roundf(xb[j] * id);
        }
    }
}

float kaze_quant_q8_0_q8_1_dot(const kaze_tensor_t *w, int64_t row,
                               const uint8_t *q81, int64_t n)
{
    const uint8_t *base;
    size_t bb = kaze_dtype_block_bytes(w->dtype);
    size_t blocks, b;
    float acc = 0.0f;

    if (w->dtype != KAZE_DTYPE_Q8_0 || q81 == NULL || n <= 0 || (n % 32) != 0) {
        (void)kaze_trap(KAZE_ERR_BADARG, "q8_0/q8_1 dot needs a q8_0 tensor and a block-aligned length");
        return 0.0f;
    }
    blocks = (size_t)(n / 32);
    base = (const uint8_t *)w->data + (size_t)row * blocks * bb;
    for (b = 0; b < blocks; b++) {
        const uint8_t *wb = base + b * bb;
        const uint8_t *ab = q81 + b * KAZE_Q8_1_BLOCK_BYTES;
        uint16_t wdh, adh;
        const int8_t *wq;
        const int8_t *aq;
        kaze_int_t sumi = 0;
        int j;

        memcpy(&wdh, wb, sizeof wdh);
        memcpy(&adh, ab, sizeof adh);
        wq = (const int8_t *)(wb + 2);
        aq = (const int8_t *)(ab + 2);
#if KAZE_QUANT_DOTPROD
        {
            int32x4_t s = vdotq_s32(vdupq_n_s32(0), vld1q_s8(wq), vld1q_s8(aq));
            s = vdotq_s32(s, vld1q_s8(wq + 16), vld1q_s8(aq + 16));
            sumi = (kaze_int_t)vaddvq_s32(s);
            (void)j;
        }
#else
        for (j = 0; j < 32; j++) {
            sumi += (kaze_int_t)wq[j] * (kaze_int_t)aq[j];
        }
#endif
        acc += kaze_f16_to_f32(wdh) * kaze_f16_to_f32(adh) * (float)sumi;
    }
    return acc;
}

void kaze_quant_dequant_row(const kaze_tensor_t *t, int64_t row,
                            float *out, int64_t n)
{
    const uint8_t *base;
    size_t bb = kaze_dtype_block_bytes(t->dtype);
    size_t nb = row_blocks(t, n), b;

    if (n < 0 || row < 0) {
        (void)kaze_trap(KAZE_ERR_BADARG, "negative row or length in dequant");
        return;
    }
    if (t->dtype == KAZE_DTYPE_F32) {
        memcpy(out, (const float *)t->data + (size_t)row * (size_t)n,
               (size_t)n * sizeof(float));
        return;
    }
    if (t->dtype == KAZE_DTYPE_F16 || t->dtype == KAZE_DTYPE_BF16) {
        const uint16_t *p = (const uint16_t *)t->data + (size_t)row * (size_t)n;
        int64_t i;
        for (i = 0; i < n; i++) {
            out[i] = (t->dtype == KAZE_DTYPE_F16) ? kaze_f16_to_f32(p[i])
                                                  : kaze_bf16_to_f32(p[i]);
        }
        return;
    }
    if (t->dtype == KAZE_DTYPE_I8) {
        const int8_t *p = (const int8_t *)t->data + (size_t)row * (size_t)n;
        int64_t i;
        for (i = 0; i < n; i++) {
            out[i] = (float)p[i];
        }
        return;
    }
    if (t->dtype == KAZE_DTYPE_BOOL) {
        const uint8_t *p = (const uint8_t *)t->data + (size_t)row * (size_t)n;
        int64_t i;
        for (i = 0; i < n; i++) {
            out[i] = p[i] ? 1.0f : 0.0f;
        }
        return;
    }
    if (bb == 0 || nb == 0 || n % (int64_t)kaze_dtype_block_elems(t->dtype) != 0) {
        (void)kaze_trap(KAZE_ERR_QUANT, "quantized row length is not a block multiple");
        return;
    }
    base = (const uint8_t *)t->data + (size_t)row * nb * bb;
    for (b = 0; b < nb; b++) {
        const uint8_t *blk = base + b * bb;
        switch (t->dtype) {
        case KAZE_DTYPE_Q8_0:
            q8_0_row(blk, out + b * 32);
            break;
        case KAZE_DTYPE_Q4_0:
            q4_0_row(blk, out + b * 32);
            break;
        case KAZE_DTYPE_Q4_K:
            q4_k_row(blk, out + b * 256);
            break;
        case KAZE_DTYPE_Q6_K:
            q6_k_row(blk, out + b * 256);
            break;
        default:
            (void)kaze_trap(KAZE_ERR_QUANT, "unsupported quantized dtype in dequant");
            return;
        }
    }
}

kaze_tensor_t *kaze_quant_dequant(kaze_arena_t *arena, const kaze_tensor_t *t)
{
    kaze_tensor_t *out;
    int64_t n, i;

    if (t == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "dequant of a null tensor");
        return NULL;
    }
    out = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, t->rank, t->shape);
    if (out == NULL) {
        return NULL;
    }
    n = kaze_tensor_numel(t);
    if (n == 0) {
        return out;
    }
    if (t->rank == 1) {
        kaze_quant_dequant_row(t, 0, (float *)out->data, n);
    } else {
        int64_t rows = 1;
        int32_t d;
        for (d = 0; d + 1 < t->rank; d++) {
            rows *= t->shape[d];
        }
        for (i = 0; i < rows; i++) {
            kaze_quant_dequant_row(t, i,
                                   (float *)out->data + i * t->shape[t->rank - 1],
                                   t->shape[t->rank - 1]);
        }
    }
    if (t->name != NULL) {
        out->name = t->name;
    }
    return out;
}


