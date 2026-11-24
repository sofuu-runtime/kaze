/* test_autograd.c — Phase 12 autograd gate.
 *
 * Three checks:
 *   1. finite-difference gradient checks for the differentiable ops;
 *   2. gradient accumulation across two backward passes;
 *   3. a 2-layer MLP training loop: 1000 forward+backward+AdamW passes under
 *      ASan/UBSan with fixed arenas (no heap allocation in the loop), plus a
 *      convergence assertion.
 */
#include "kaze_tensor.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int failures = 0;
static int checks = 0;

#define CHECK(cond, msg) do { \
    checks++; \
    if (!(cond)) { printf("FAIL %s (line %d)\n", msg, __LINE__); failures++; } \
} while (0)

static uint8_t g_params_mem[2 * 1024 * 1024];
static uint8_t g_pass_mem[16 * 1024 * 1024];
static uint8_t g_const_mem[2 * 1024 * 1024];
static kaze_arena_t g_params;
static kaze_arena_t g_pass;
static kaze_arena_t g_const;

typedef enum case_id_t {
    CASE_MATMUL = 0,
    CASE_ADDBIAS,
    CASE_RELU,
    CASE_SILU,
    CASE_GELU,
    CASE_SOFTMAX,
    CASE_RMS,
    CASE_LAYER,
    CASE_MSE,
    CASE_CE
} case_id_t;

static kaze_tensor_t *mk_f32(kaze_arena_t *a, const double *x, int n,
                             int32_t rank, const int64_t *dims)
{
    kaze_tensor_t *t = kaze_tensor_zeros(a, KAZE_DTYPE_F32, rank, dims);
    int i;
    if (t == NULL) {
        return NULL;
    }
    for (i = 0; i < n; i++) {
        kaze_tensor_set(t, i, (float)x[i]);
    }
    return t;
}

static kaze_tensor_t *mk_target(kaze_arena_t *a, int64_t width, int64_t rows)
{
    int64_t dims[2];
    dims[0] = rows;
    dims[1] = width;
    return kaze_tensor_zeros(a, KAZE_DTYPE_F32, 2, dims);
}

/* Runs one forward/backward for a fixed op case. x is the concatenation of the
 * differentiable inputs; grads (when non-NULL) receives the analytic gradient of
 * each element of x. Returns the scalar loss. */
static double run_case(case_id_t id, const double *x, double *grads, int *n_in)
{
    kaze_tape_t *tape;
    kaze_tensor_t *t[3] = { NULL, NULL, NULL };
    kaze_tensor_t *target = NULL;
    kaze_tensor_t *out = NULL;
    kaze_tensor_t *loss = NULL;
    int64_t d1[2], d2[2], d3[2], d4[2];
    int count = 0;
    int i;

    kaze_arena_reset(&g_pass);
    kaze_arena_reset(&g_const);
    tape = kaze_tape_new(&g_pass);
    if (tape == NULL) {
        return NAN;
    }
    switch (id) {
    case CASE_MATMUL:
        d1[0] = 2; d1[1] = 3;
        d2[0] = 3; d2[1] = 2;
        t[0] = mk_f32(&g_pass, x, 6, 2, d1);
        t[1] = mk_f32(&g_pass, x + 6, 6, 2, d2);
        target = mk_target(&g_const, 2, 2);
        count = 12;
        break;
    case CASE_ADDBIAS:
        d1[0] = 2; d1[1] = 3;
        d2[0] = 3;
        t[0] = mk_f32(&g_pass, x, 6, 2, d1);
        t[1] = mk_f32(&g_pass, x + 6, 3, 1, d2);
        target = mk_target(&g_const, 3, 2);
        count = 9;
        break;
    case CASE_RELU:
    case CASE_SILU:
    case CASE_GELU:
        d1[0] = 5;
        t[0] = mk_f32(&g_pass, x, 5, 1, d1);
        target = mk_target(&g_const, 5, 1);
        count = 5;
        break;
    case CASE_SOFTMAX:
        d1[0] = 2; d1[1] = 3;
        t[0] = mk_f32(&g_pass, x, 6, 2, d1);
        target = mk_target(&g_const, 3, 2);
        count = 6;
        break;
    case CASE_RMS:
        d1[0] = 2; d1[1] = 3;
        d2[0] = 3;
        t[0] = mk_f32(&g_pass, x, 6, 2, d1);
        t[1] = mk_f32(&g_pass, x + 6, 3, 1, d2);
        target = mk_target(&g_const, 3, 2);
        count = 9;
        break;
    case CASE_LAYER:
        d1[0] = 2; d1[1] = 3;
        d2[0] = 3;
        t[0] = mk_f32(&g_pass, x, 6, 2, d1);
        t[1] = mk_f32(&g_pass, x + 6, 3, 1, d2);
        t[2] = mk_f32(&g_pass, x + 9, 3, 1, d2);
        target = mk_target(&g_const, 3, 2);
        count = 12;
        break;
    case CASE_MSE:
        d1[0] = 4;
        t[0] = mk_f32(&g_pass, x, 4, 1, d1);
        target = mk_f32(&g_const, (const double[]){ 0.5, -0.5, 1.0, 0.25 }, 4, 1, d1);
        count = 4;
        break;
    case CASE_CE:
        d1[0] = 4;
        t[0] = mk_f32(&g_pass, x, 4, 1, d1);
        count = 4;
        break;
    default:
        return NAN;
    }
    if (t[0] == NULL || (t[1] == NULL && (id == CASE_MATMUL || id == CASE_ADDBIAS ||
                                          id == CASE_RMS || id == CASE_LAYER))) {
        return NAN;
    }
    kaze_tape_watch(tape, t[0]);
    if (t[1] != NULL) {
        kaze_tape_watch(tape, t[1]);
    }
    if (t[2] != NULL) {
        kaze_tape_watch(tape, t[2]);
    }
    switch (id) {
    case CASE_MATMUL:
        out = kaze_tape_matmul(tape, &g_pass, t[0], t[1]);
        break;
    case CASE_ADDBIAS:
        out = kaze_tape_add_bias(tape, &g_pass, t[0], t[1]);
        break;
    case CASE_RELU:
        out = kaze_tape_relu(tape, &g_pass, t[0]);
        break;
    case CASE_SILU:
        out = kaze_tape_silu(tape, &g_pass, t[0]);
        break;
    case CASE_GELU:
        out = kaze_tape_gelu(tape, &g_pass, t[0]);
        break;
    case CASE_SOFTMAX:
        out = kaze_tape_softmax(tape, &g_pass, t[0]);
        break;
    case CASE_RMS:
        out = kaze_tape_rms_norm(tape, &g_pass, t[0], t[1], 1e-5f);
        break;
    case CASE_LAYER:
        out = kaze_tape_layer_norm(tape, &g_pass, t[0], t[1], t[2], 1e-5f);
        break;
    case CASE_MSE:
        loss = kaze_tape_mse(tape, &g_pass, t[0], target);
        out = loss;
        break;
    case CASE_CE:
        loss = kaze_tape_cross_entropy(tape, &g_pass, t[0], 2);
        out = loss;
        break;
    default:
        break;
    }
    (void)d3;
    (void)d4;
    if (out == NULL) {
        return NAN;
    }
    if (loss == NULL) {
        kaze_tensor_t *zeros = kaze_tensor_zeros(&g_const, KAZE_DTYPE_F32,
                                                 out->rank, out->shape);
        loss = kaze_tape_mse(tape, &g_pass, out, zeros);
        if (loss == NULL) {
            return NAN;
        }
    }
    if (!kaze_tape_backward(tape, loss)) {
        return NAN;
    }
    if (grads != NULL) {
        int offset = 0;
        int k;
        for (k = 0; k < 3; k++) {
            const kaze_tensor_t *g;
            int numel;
            if (t[k] == NULL) {
                continue;
            }
            numel = (int)kaze_tensor_numel(t[k]);
            g = kaze_tape_grad(tape, t[k]);
            for (i = 0; i < numel; i++) {
                grads[offset + i] = (g != NULL) ? (double)kaze_tensor_get(g, i) : 0.0;
            }
            offset += numel;
        }
    }
    if (n_in != NULL) {
        *n_in = count;
    }
    return (double)kaze_tensor_get(loss, 0);
}

static void check_case(const char *name, case_id_t id, const double *x, int n)
{
    double analytic[16];
    double xp[16];
    double h = 1e-3;
    double worst = 0.0;
    int i;

    memset(analytic, 0, sizeof analytic);
    if (!(run_case(id, x, analytic, NULL) == run_case(id, x, analytic, NULL))) {
        CHECK(0, name);
        return;
    }
    for (i = 0; i < n; i++) {
        double up, down, numeric, err;
        memcpy(xp, x, (size_t)n * sizeof(double));
        xp[i] = x[i] + h;
        up = run_case(id, xp, NULL, NULL);
        xp[i] = x[i] - h;
        down = run_case(id, xp, NULL, NULL);
        numeric = (up - down) / (2.0 * h);
        err = fabs(numeric - analytic[i]);
        if (err > worst) {
            worst = err;
        }
        if (err > 2e-3 + 2e-2 * fabs(numeric)) {
            printf("FAIL %s[%d]: analytic %.6f numeric %.6f (line %d)\n",
                   name, i, analytic[i], numeric, __LINE__);
            failures++;
        }
        checks++;
    }
    printf("autograd: gradcheck %-10s n=%2d max_err=%.3g\n", name, n, worst);
}

static void check_accumulation(void)
{
    int64_t pdims[1] = { 2 };
    int64_t tdims[1] = { 2 };
    double target[2] = { 0.5, -0.5 };
    kaze_tensor_t *p;
    kaze_tensor_t *t;
    kaze_tensor_t *loss;
    kaze_tape_t *tape;
    const kaze_tensor_t *g1;
    const kaze_tensor_t *g2;
    double first0, first1;

    kaze_arena_reset(&g_pass);
    tape = kaze_tape_new(&g_pass);
    p = kaze_tape_leaf(tape, &g_pass, 1, pdims);
    t = mk_f32(&g_const, target, 2, 1, tdims);
    kaze_tensor_set(p, 0, 0.25f);
    kaze_tensor_set(p, 1, -0.75f);
    loss = kaze_tape_mse(tape, &g_pass, p, t);
    CHECK(loss != NULL, "accumulation loss");
    CHECK(kaze_tape_backward(tape, loss), "first backward");
    g1 = kaze_tape_grad(tape, p);
    CHECK(g1 != NULL, "first gradient");
    first0 = kaze_tensor_get(g1, 0);
    first1 = kaze_tensor_get(g1, 1);
    CHECK(kaze_tape_backward(tape, loss), "second backward");
    g2 = kaze_tape_grad(tape, p);
    CHECK(g2 != NULL, "second gradient");
    checks++;
    if (fabs(kaze_tensor_get(g2, 0) - 2.0 * first0) > 1e-6 ||
        fabs(kaze_tensor_get(g2, 1) - 2.0 * first1) > 1e-6) {
        printf("FAIL gradient accumulation: %.6f vs %.6f, %.6f vs %.6f\n",
               (double)kaze_tensor_get(g2, 0), 2.0 * first0,
               (double)kaze_tensor_get(g2, 1), 2.0 * first1);
        failures++;
    }
}

static void train_stress(void)
{
    kaze_arena_t params;
    kaze_arena_t consts;
    static uint8_t params_mem[4 * 1024 * 1024];
    static uint8_t consts_mem[1024 * 1024];
    kaze_tensor_t *x;
    kaze_tensor_t *y;
    kaze_tensor_t *w1;
    kaze_tensor_t *b1;
    kaze_tensor_t *w2;
    kaze_tensor_t *b2;
    kaze_tensor_t *m[4];
    kaze_tensor_t *v[4];
    double first_loss = 0.0;
    double last_loss = 0.0;
    size_t peak = 0;
    int step;
    int i, j;
    int64_t xd[2], yd[2], w1d[2], b1d[1], w2d[2], b2d[1];

    kaze_arena_init(&params, params_mem, sizeof params_mem);
    kaze_arena_init(&consts, consts_mem, sizeof consts_mem);
    xd[0] = 8; xd[1] = 4;
    yd[0] = 8; yd[1] = 1;
    x = kaze_tensor_zeros(&consts, KAZE_DTYPE_F32, 2, xd);
    y = kaze_tensor_zeros(&consts, KAZE_DTYPE_F32, 2, yd);
    for (i = 0; i < 8; i++) {
        for (j = 0; j < 4; j++) {
            kaze_tensor_set(x, i * 4 + j, (float)((((i * 4 + j) % 7) - 3) * 0.3));
        }
    }
    for (i = 0; i < 8; i++) {
        double yv = 0.5 * (double)kaze_tensor_get(x, i * 4) -
                    0.25 * (double)kaze_tensor_get(x, i * 4 + 1) + 0.1;
        kaze_tensor_set(y, i, (float)yv);
    }
    w1d[0] = 4; w1d[1] = 4;
    b1d[0] = 4;
    w2d[0] = 4; w2d[1] = 1;
    b2d[0] = 1;
    w1 = kaze_tensor_zeros(&params, KAZE_DTYPE_F32, 2, w1d);
    b1 = kaze_tensor_zeros(&params, KAZE_DTYPE_F32, 1, b1d);
    w2 = kaze_tensor_zeros(&params, KAZE_DTYPE_F32, 2, w2d);
    b2 = kaze_tensor_zeros(&params, KAZE_DTYPE_F32, 1, b2d);
    for (i = 0; i < 16; i++) {
        kaze_tensor_set(w1, i, (float)(((i * 3) % 5 - 2) * 0.2));
    }
    for (i = 0; i < 4; i++) {
        kaze_tensor_set(w2, i, (float)(((i * 5) % 5 - 2) * 0.2));
    }
    m[0] = kaze_tensor_zeros(&params, KAZE_DTYPE_F32, 2, w1d);
    m[1] = kaze_tensor_zeros(&params, KAZE_DTYPE_F32, 1, b1d);
    m[2] = kaze_tensor_zeros(&params, KAZE_DTYPE_F32, 2, w2d);
    m[3] = kaze_tensor_zeros(&params, KAZE_DTYPE_F32, 1, b2d);
    v[0] = kaze_tensor_zeros(&params, KAZE_DTYPE_F32, 2, w1d);
    v[1] = kaze_tensor_zeros(&params, KAZE_DTYPE_F32, 1, b1d);
    v[2] = kaze_tensor_zeros(&params, KAZE_DTYPE_F32, 2, w2d);
    v[3] = kaze_tensor_zeros(&params, KAZE_DTYPE_F32, 1, b2d);
    CHECK(x != NULL && y != NULL && w1 != NULL && b1 != NULL && w2 != NULL &&
          b2 != NULL && m[0] != NULL && v[3] != NULL, "training tensors");

    for (step = 1; step <= 1000; step++) {
        kaze_tape_t *tape;
        kaze_tensor_t *h;
        kaze_tensor_t *mid;
        kaze_tensor_t *out;
        kaze_tensor_t *loss;
        kaze_tensor_t *params4[4];
        int k;

        kaze_arena_reset(&g_pass);
        tape = kaze_tape_new(&g_pass);
        kaze_tape_watch(tape, w1);
        kaze_tape_watch(tape, b1);
        kaze_tape_watch(tape, w2);
        kaze_tape_watch(tape, b2);
        h = kaze_tape_matmul(tape, &g_pass, x, w1);
        h = kaze_tape_add_bias(tape, &g_pass, h, b1);
        h = kaze_tape_relu(tape, &g_pass, h);
        mid = kaze_tape_matmul(tape, &g_pass, h, w2);
        out = kaze_tape_add_bias(tape, &g_pass, mid, b2);
        loss = kaze_tape_mse(tape, &g_pass, out, y);
        if (loss == NULL || !kaze_tape_backward(tape, loss)) {
            printf("FAIL training pass %d\n", step);
            failures++;
            return;
        }
        last_loss = (double)kaze_tensor_get(loss, 0);
        if (step == 1) {
            first_loss = last_loss;
        }
        if (!isfinite(last_loss)) {
            printf("FAIL training loss is not finite at step %d\n", step);
            failures++;
            return;
        }
        params4[0] = w1; params4[1] = b1; params4[2] = w2; params4[3] = b2;
        for (k = 0; k < 4; k++) {
            const kaze_tensor_t *g = kaze_tape_grad(tape, params4[k]);
            if (g == NULL) {
                printf("FAIL missing gradient for parameter %d at step %d\n", k, step);
                failures++;
                return;
            }
            if (!kaze_opt_adamw(params4[k], g, m[k], v[k], 0.01f, 0.9f, 0.999f,
                                1e-8f, 0.0f, step)) {
                printf("FAIL AdamW step %d\n", step);
                failures++;
                return;
            }
        }
        if (kaze_arena_used(&g_pass) > peak) {
            peak = kaze_arena_used(&g_pass);
        }
    }
    checks++;
    printf("autograd: 1000 fwd/bwd/AdamW passes, pass-arena peak %zu bytes\n", peak);
    printf("autograd: training loss %.6f -> %.6f\n", first_loss, last_loss);
    if (!(last_loss < first_loss * 0.2)) {
        printf("FAIL training did not converge: %.6f -> %.6f\n", first_loss, last_loss);
        failures++;
    }
    CHECK(isfinite((double)kaze_tensor_get(w1, 0)), "trained weights are finite");
}

int main(void)
{
    double matmul_x[12] = { 0.5, -1.0, 2.0, 0.25, 1.5, -0.5,
                            -0.75, 1.0, 0.5, 2.0, -1.5, 0.25 };
    double addbias_x[9] = { 0.5, -1.0, 2.0, 0.25, 1.5, -0.5, 0.1, -0.2, 0.3 };
    double unary_x[5] = { 0.5, -1.0, 2.0, -0.25, 0.125 };
    double softmax_x[6] = { 0.5, -1.0, 2.0, 0.25, 1.5, -0.5 };
    double rms_x[9] = { 0.5, -1.0, 2.0, 0.25, 1.5, -0.5, 0.1, -0.2, 0.3 };
    double layer_x[12] = { 0.5, -1.0, 2.0, 0.25, 1.5, -0.5,
                           0.1, -0.2, 0.3, 1.0, -1.0, 0.5 };
    double mse_x[4] = { 0.25, -0.75, 1.5, 0.0 };
    double ce_x[4] = { 0.5, -1.0, 2.0, 0.25 };

    kaze_arena_init(&g_params, g_params_mem, sizeof g_params_mem);
    kaze_arena_init(&g_pass, g_pass_mem, sizeof g_pass_mem);
    kaze_arena_init(&g_const, g_const_mem, sizeof g_const_mem);

    check_case("matmul", CASE_MATMUL, matmul_x, 12);
    check_case("add_bias", CASE_ADDBIAS, addbias_x, 9);
    check_case("relu", CASE_RELU, unary_x, 5);
    check_case("silu", CASE_SILU, unary_x, 5);
    check_case("gelu", CASE_GELU, unary_x, 5);
    check_case("softmax", CASE_SOFTMAX, softmax_x, 6);
    check_case("rms_norm", CASE_RMS, rms_x, 9);
    check_case("layer_norm", CASE_LAYER, layer_x, 12);
    check_case("mse", CASE_MSE, mse_x, 4);
    check_case("cross_entropy", CASE_CE, ce_x, 4);
    check_accumulation();
    train_stress();

    printf("test_autograd: %d checks, %d failures\n", checks, failures);
    return failures == 0 ? 0 : 1;
}

