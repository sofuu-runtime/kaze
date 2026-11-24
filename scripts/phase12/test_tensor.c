/* test_tensor.c — Phase 12 core tensor kernel gate.
 *
 * Builds with the same strict flags every other Kaze runtime component uses and
 * runs under ASan/UBSan from scripts/phase12/phase12-gates.sh.
 */
#include "kaze_rt.h"
#include "kaze_tensor.h"
#include "kaze_quant.h"

#include <math.h>
#include <stdio.h>
#include <string.h>

static int failures = 0;
static int checks = 0;

#define CHECK(cond, msg) do { \
    checks++; \
    if (!(cond)) { printf("FAIL %s (line %d)\n", msg, __LINE__); failures++; } \
} while (0)

static void check_near(float got, float want, float tol, const char *msg)
{
    checks++;
    if (!(fabsf(got - want) <= tol)) {
        printf("FAIL %s: got %.9g want %.9g (line %d)\n", msg, (double)got, (double)want, __LINE__);
        failures++;
    }
}

static kaze_tensor_t *tensor2x2(kaze_arena_t *a, float a00, float a01, float a10, float a11)
{
    int64_t shape[2];
    float data[4];
    shape[0] = 2;
    shape[1] = 2;
    data[0] = a00; data[1] = a01; data[2] = a10; data[3] = a11;
    return kaze_tensor_from_f32(a, data, 2, shape, NULL);
}

static void test_elementwise(kaze_arena_t *arena)
{
    kaze_tensor_t *a = tensor2x2(arena, 1, 2, 3, 4);
    kaze_tensor_t *b = tensor2x2(arena, 10, 20, 30, 40);
    kaze_tensor_t *sum = kaze_tensor_add(arena, a, b);
    kaze_tensor_t *prod = kaze_tensor_mul(arena, a, b);

    CHECK(sum != NULL && kaze_tensor_get(sum, 0) == 11.0f, "add element 0");
    CHECK(sum != NULL && kaze_tensor_get(sum, 3) == 44.0f, "add element 3");
    CHECK(prod != NULL && kaze_tensor_get(prod, 2) == 90.0f, "mul element 2");

    {
        float scalar_data[1];
        int64_t shape[1];
        kaze_tensor_t *scalar;
        kaze_tensor_t *scaled;
        shape[0] = 1;
        scalar_data[0] = 2.0f;
        scalar = kaze_tensor_from_f32(arena, scalar_data, 1, shape, NULL);
        scaled = kaze_tensor_mul(arena, a, scalar);
        CHECK(scaled != NULL && kaze_tensor_get(scaled, 1) == 4.0f, "scalar broadcast");
    }
}

static void test_transpose_reshape(kaze_arena_t *arena)
{
    int64_t shape[2] = { 2, 3 };
    float data[6] = { 1, 2, 3, 4, 5, 6 };
    kaze_tensor_t *t = kaze_tensor_from_f32(arena, data, 2, shape, NULL);
    kaze_tensor_t *tt = kaze_tensor_transpose(arena, t, 0, 1);
    int64_t newshape[2] = { 6, 1 };
    kaze_tensor_t *r = kaze_tensor_reshape(arena, t, 2, newshape);

    CHECK(tt != NULL && tt->shape[0] == 3 && tt->shape[1] == 2, "transpose shape");
    CHECK(tt != NULL && kaze_tensor_get(tt, 0) == 1.0f, "transpose 0,0");
    CHECK(tt != NULL && kaze_tensor_get(tt, 1) == 4.0f, "transpose 0,1");
    CHECK(tt != NULL && kaze_tensor_get(tt, 5) == 6.0f, "transpose 1,2");
    CHECK(r != NULL && kaze_tensor_get(r, 4) == 5.0f, "reshape view");
}

static void test_softmax(kaze_arena_t *arena)
{
    int64_t shape[2] = { 1, 3 };
    float data[3] = { 1.0f, 2.0f, 3.0f };
    kaze_tensor_t *t = kaze_tensor_from_f32(arena, data, 2, shape, NULL);
    kaze_tensor_t *s = kaze_tensor_softmax(arena, t);
    float total = 0.0f;

    if (s == NULL) {
        CHECK(0, "softmax allocated");
        return;
    }
    total = kaze_tensor_get(s, 0) + kaze_tensor_get(s, 1) + kaze_tensor_get(s, 2);
    check_near(total, 1.0f, 1e-6f, "softmax sums to one");
    CHECK(kaze_tensor_get(s, 2) > kaze_tensor_get(s, 1), "softmax order");
    check_near(kaze_tensor_get(s, 0), 0.090030573f, 1e-6f, "softmax first value");
}

static void test_norms(kaze_arena_t *arena)
{
    int64_t shape[2] = { 2, 4 };
    float data[8] = { 1, 1, 1, 1, 1, 2, 3, 4 };
    int64_t wshape[1] = { 4 };
    float wdata[4] = { 1, 1, 1, 1 };
    kaze_tensor_t *x = kaze_tensor_from_f32(arena, data, 2, shape, NULL);
    kaze_tensor_t *w = kaze_tensor_from_f32(arena, wdata, 1, wshape, NULL);
    kaze_tensor_t *rms = kaze_tensor_rms_norm(arena, x, w, 1e-6f);
    kaze_tensor_t *ln = kaze_tensor_layer_norm(arena, x, w, NULL, 1e-6f);

    if (rms == NULL || ln == NULL) {
        CHECK(0, "norm allocation");
        return;
    }
    check_near(kaze_tensor_get(rms, 0), 0.9999995f, 1e-5f, "rms_norm ones row");
    check_near(kaze_tensor_get(ln, 0) + kaze_tensor_get(ln, 1) +
               kaze_tensor_get(ln, 2) + kaze_tensor_get(ln, 3), 0.0f, 1e-5f, "layer_norm zero mean");
}

static void test_matmul(kaze_arena_t *arena)
{
    int64_t ashape[2] = { 2, 3 };
    int64_t bshape[2] = { 3, 2 };
    float adata[6] = { 1, 2, 3, 4, 5, 6 };
    float bdata[6] = { 7, 8, 9, 10, 11, 12 };
    kaze_tensor_t *a = kaze_tensor_from_f32(arena, adata, 2, ashape, NULL);
    kaze_tensor_t *b = kaze_tensor_from_f32(arena, bdata, 2, bshape, NULL);
    kaze_tensor_t *c = kaze_tensor_matmul(arena, a, b);

    CHECK(c != NULL, "matmul allocated");
    if (c == NULL) {
        return;
    }
    CHECK(c->shape[0] == 2 && c->shape[1] == 2, "matmul shape");
    check_near(kaze_tensor_get(c, 0), 58.0f, 1e-5f, "matmul 0,0");
    check_near(kaze_tensor_get(c, 1), 64.0f, 1e-5f, "matmul 0,1");
    check_near(kaze_tensor_get(c, 2), 139.0f, 1e-5f, "matmul 1,0");
    check_near(kaze_tensor_get(c, 3), 154.0f, 1e-5f, "matmul 1,1");

    {
        int64_t atshape[2] = { 1, 3 };
        int64_t at2shape[2] = { 2, 3 };
        float atdata[3] = { 1, 2, 3 };
        float at2data[6] = { 4, 5, 6, 7, 8, 9 };
        kaze_tensor_t *u = kaze_tensor_from_f32(arena, atdata, 2, atshape, NULL);
        kaze_tensor_t *v = kaze_tensor_from_f32(arena, at2data, 2, at2shape, NULL);
        kaze_tensor_t *scores = kaze_tensor_matmul_t(arena, u, v);
        CHECK(scores != NULL && kaze_tensor_get(scores, 0) == 32.0f, "matmul_t 0,0");
        CHECK(scores != NULL && kaze_tensor_get(scores, 1) == 50.0f, "matmul_t 0,1");
    }
}

static void test_rope(kaze_arena_t *arena)
{
    int64_t shape[2] = { 1, 4 };
    float data[4] = { 1.0f, 0.0f, 1.0f, 0.0f };
    kaze_tensor_t *t = kaze_tensor_from_f32(arena, data, 2, shape, NULL);
    kaze_tensor_t *r = kaze_tensor_rope(arena, t, 1, 4, 0, 10000.0f);
    CHECK(r != NULL, "rope allocated");
    if (r != NULL) {
        check_near(kaze_tensor_get(r, 0), 1.0f, 1e-6f, "rope pos 0 pair 0 unchanged");
        check_near(kaze_tensor_get(r, 2), 1.0f, 1e-6f, "rope pos 0 pair 1 unchanged");
    }
}

static void test_q8_dequant(kaze_arena_t *arena)
{
    uint8_t block[34];
    uint16_t dh = kaze_f32_to_f16(0.5f);
    int i;
    kaze_tensor_t t;
    kaze_tensor_t *out;

    memcpy(block, &dh, sizeof dh);
    for (i = 0; i < 32; i++) {
        block[2 + i] = (uint8_t)(int8_t)(i - 16);
    }
    memset(&t, 0, sizeof t);
    t.dtype = KAZE_DTYPE_Q8_0;
    t.rank = 1;
    t.shape[0] = 32;
    t.strides[0] = 1;
    t.data = block;
    t.nbytes = 34;
    out = kaze_tensor_dequant(arena, &t);
    CHECK(out != NULL, "q8 dequant allocated");
    if (out != NULL) {
        check_near(kaze_tensor_get(out, 0), -8.0f, 1e-6f, "q8 dequant q=-16");
        check_near(kaze_tensor_get(out, 16), 0.0f, 1e-6f, "q8 dequant q=0");
        check_near(kaze_tensor_get(out, 31), 7.5f, 1e-6f, "q8 dequant q=15");
    }
    {
        float x[32];
        float want = 0.0f;
        for (i = 0; i < 32; i++) {
            x[i] = 1.0f;
            want += 0.5f * (float)(i - 16);
        }
        check_near(kaze_quant_row_dot(&t, 0, x, 32), want, 1e-5f, "q8 row dot");
    }
}

static void test_gemm_bench_hook(kaze_arena_t *arena)
{
    int64_t ashape[2] = { 16, 16 };
    int64_t bshape[2] = { 16, 16 };
    kaze_tensor_t *a = kaze_tensor_zeros(arena, KAZE_DTYPE_F32, 2, ashape);
    kaze_tensor_t *b = kaze_tensor_zeros(arena, KAZE_DTYPE_F32, 2, bshape);
    kaze_tensor_t *ref;
    float c[256];
    int i;

    (void)arena;
    for (i = 0; i < 256; i++) {
        kaze_tensor_set(a, i, (float)(i % 7) - 3.0f);
        kaze_tensor_set(b, i, (float)(i % 5) - 2.0f);
    }
    ref = kaze_tensor_matmul(arena, a, b);
    kaze_gemm_f32((const float *)a->data, (const float *)b->data, c, 16, 16, 16);
    CHECK(ref != NULL, "gemm reference allocated");
    if (ref != NULL) {
        for (i = 0; i < 256; i++) {
            check_near(c[i], kaze_tensor_get(ref, i), 1e-4f, "gemm matches matmul");
        }
    }
    CHECK(kaze_tensor_has_neon() == (kaze_tensor_has_neon() ? 1 : 0), "neon hook");
}

int main(void)
{
    static uint8_t buffer[64 * 1024 * 1024];
    kaze_arena_t arena;

    kaze_arena_init(&arena, buffer, sizeof buffer);
    test_elementwise(&arena);
    test_transpose_reshape(&arena);
    test_softmax(&arena);
    test_norms(&arena);
    test_matmul(&arena);
    test_rope(&arena);
    test_q8_dequant(&arena);
    test_gemm_bench_hook(&arena);

    printf("test_tensor: %d checks, %d failures\n", checks, failures);
    return failures == 0 ? 0 : 1;
}
