/* kaze_autograd.c — reverse-mode automatic differentiation + AdamW (Phase 12).
 *
 * The tape is a flat array of nodes in forward order. Every value that takes
 * part in a backward pass is a node: parameters are leaves, every taped op
 * appends one node holding its output and the inputs it owns. backward() walks
 * the array in reverse and accumulates a gradient tensor on each node. All
 * allocations come from the tape's arena, so a training loop is one arena reset
 * per step and never touches malloc.
 *
 * This is the runtime substrate; the compiler-side symbolic differentiation
 * (static backward synthesis from a forward routine) layers on top of the same
 * op set.
 */
#include "kaze_tensor.h"

#include <math.h>
#include <string.h>

typedef struct kaze_tape_node_t {
    kaze_tape_op_t op;
    kaze_tensor_t *out;
    kaze_tensor_t *grad;
    const kaze_tensor_t *a;
    const kaze_tensor_t *b;
    const kaze_tensor_t *c;
    float eps;
    int32_t target;
} kaze_tape_node_t;

struct kaze_tape_t {
    kaze_arena_t *arena;
    kaze_tape_node_t *nodes;
    size_t count;
    size_t cap;
};

static void tape_fail(const char *msg)
{
    (void)kaze_trap(KAZE_ERR_BADARG, msg);
}

kaze_tape_t *kaze_tape_new(kaze_arena_t *arena)
{
    kaze_tape_t *t;

    if (arena == NULL) {
        tape_fail("kaze_tape_new needs an arena");
        return NULL;
    }
    t = (kaze_tape_t *)kmalloc(arena, sizeof(kaze_tape_t));
    if (t == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "tape header did not fit the arena");
        return NULL;
    }
    t->arena = arena;
    t->cap = 128;
    t->count = 0;
    t->nodes = (kaze_tape_node_t *)kmalloc(arena, t->cap * sizeof(kaze_tape_node_t));
    if (t->nodes == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "tape node array did not fit the arena");
        return NULL;
    }
    return t;
}

void kaze_tape_clear(kaze_tape_t *tape)
{
    if (tape != NULL) {
        tape->count = 0;
    }
}

static kaze_tape_node_t *tape_push(kaze_tape_t *tape, kaze_tape_op_t op)
{
    kaze_tape_node_t *n;

    if (tape->count == tape->cap) {
        size_t next_cap = tape->cap * 2;
        kaze_tape_node_t *next = (kaze_tape_node_t *)kmalloc(
            tape->arena, next_cap * sizeof(kaze_tape_node_t));
        if (next == NULL) {
            (void)kaze_trap(KAZE_ERR_OOM, "tape growth did not fit the arena");
            return NULL;
        }
        memcpy(next, tape->nodes, tape->count * sizeof(kaze_tape_node_t));
        tape->nodes = next;
        tape->cap = next_cap;
    }
    n = &tape->nodes[tape->count++];
    memset(n, 0, sizeof *n);
    n->op = op;
    return n;
}

static kaze_tape_node_t *tape_find(kaze_tape_t *tape, const kaze_tensor_t *t)
{
    size_t i;

    for (i = tape->count; i > 0; i--) {
        if (tape->nodes[i - 1].out == t) {
            return &tape->nodes[i - 1];
        }
    }
    return NULL;
}

kaze_tensor_t *kaze_tape_leaf(kaze_tape_t *tape, kaze_arena_t *arena,
                              int32_t rank, const int64_t *shape)
{
    kaze_tensor_t *t;
    kaze_tape_node_t *n;

    if (tape == NULL || arena == NULL) {
        tape_fail("kaze_tape_leaf needs a tape and an arena");
        return NULL;
    }
    t = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, rank, shape);
    if (t == NULL) {
        return NULL;
    }
    n = tape_push(tape, KAZE_TAPE_LEAF);
    if (n == NULL) {
        return NULL;
    }
    n->out = t;
    return t;
}

kaze_tensor_t *kaze_tape_watch(kaze_tape_t *tape, kaze_tensor_t *t)
{
    kaze_tape_node_t *n;

    if (tape == NULL || t == NULL) {
        tape_fail("kaze_tape_watch needs a tape and a tensor");
        return NULL;
    }
    n = tape_push(tape, KAZE_TAPE_LEAF);
    if (n == NULL) {
        return NULL;
    }
    n->out = t;
    return t;
}

static kaze_tensor_t *node_grad(kaze_tape_t *tape, kaze_tape_node_t *n)
{
    if (n->grad == NULL) {
        n->grad = kaze_tensor_zeros(tape->arena, KAZE_DTYPE_F32, n->out->rank,
                                    n->out->shape);
    }
    return n->grad;
}

/* Gradient accumulation with scalar broadcast: a scalar target receives the sum
 * of a larger delta, an equal shape adds elementwise. */
static void grad_accumulate(kaze_tape_t *tape, kaze_tape_node_t *n,
                            const kaze_tensor_t *delta)
{
    kaze_tensor_t *g;
    int64_t ng, nd, i;

    if (n == NULL || delta == NULL) {
        return;
    }
    g = node_grad(tape, n);
    if (g == NULL) {
        return;
    }
    ng = kaze_tensor_numel(g);
    nd = kaze_tensor_numel(delta);
    if (ng == nd) {
        for (i = 0; i < ng; i++) {
            kaze_tensor_set(g, i, kaze_tensor_get(g, i) + kaze_tensor_get(delta, i));
        }
        return;
    }
    if (ng == 1) {
        kaze_float_t sum = 0.0f;
        for (i = 0; i < nd; i++) {
            sum += kaze_tensor_get(delta, i);
        }
        kaze_tensor_set(g, 0, kaze_tensor_get(g, 0) + sum);
        return;
    }
    tape_fail("gradient shape does not match its tensor");
}

static bool same_shape(const kaze_tensor_t *a, const kaze_tensor_t *b)
{
    return kaze_tensor_same_shape(a, b);
}

/* Broadcast elementwise product helpers used by MUL/ADD gradients. */
static kaze_tensor_t *elem_mul_broadcast(kaze_arena_t *arena, const kaze_tensor_t *a,
                                         const kaze_tensor_t *b)
{
    return kaze_tensor_mul(arena, a, b);
}

/* ------------------------------------------------------------------------- */
/* Forward ops                                                                */
/* ------------------------------------------------------------------------- */

static kaze_tensor_t *tape_binary(kaze_tape_t *tape, kaze_arena_t *arena,
                                  kaze_tape_op_t op, const kaze_tensor_t *a,
                                  const kaze_tensor_t *b,
                                  kaze_tensor_t *(*forward)(kaze_arena_t *,
                                                            const kaze_tensor_t *,
                                                            const kaze_tensor_t *))
{
    kaze_tensor_t *out;
    kaze_tape_node_t *n;

    if (tape == NULL || arena == NULL || a == NULL || b == NULL) {
        tape_fail("a taped binary op needs a tape, an arena and two tensors");
        return NULL;
    }
    /* Inputs need not all be on the tape: a constant may feed an op. Backward
     * simply skips accumulating into a value that has no node. */
    out = forward(arena, a, b);
    if (out == NULL) {
        return NULL;
    }
    n = tape_push(tape, op);
    if (n == NULL) {
        return NULL;
    }
    n->out = out;
    n->a = a;
    n->b = b;
    return out;
}

kaze_tensor_t *kaze_tape_add(kaze_tape_t *tape, kaze_arena_t *arena,
                             const kaze_tensor_t *a, const kaze_tensor_t *b)
{
    return tape_binary(tape, arena, KAZE_TAPE_ADD, a, b, kaze_tensor_add);
}

kaze_tensor_t *kaze_tape_sub(kaze_tape_t *tape, kaze_arena_t *arena,
                             const kaze_tensor_t *a, const kaze_tensor_t *b)
{
    return tape_binary(tape, arena, KAZE_TAPE_SUB, a, b, kaze_tensor_sub);
}

kaze_tensor_t *kaze_tape_mul(kaze_tape_t *tape, kaze_arena_t *arena,
                             const kaze_tensor_t *a, const kaze_tensor_t *b)
{
    return tape_binary(tape, arena, KAZE_TAPE_MUL, a, b, kaze_tensor_mul);
}

kaze_tensor_t *kaze_tape_matmul(kaze_tape_t *tape, kaze_arena_t *arena,
                                const kaze_tensor_t *a, const kaze_tensor_t *b)
{
    return tape_binary(tape, arena, KAZE_TAPE_MATMUL, a, b, kaze_tensor_matmul);
}

kaze_tensor_t *kaze_tape_add_bias(kaze_tape_t *tape, kaze_arena_t *arena,
                                  const kaze_tensor_t *x, const kaze_tensor_t *bias)
{
    kaze_tensor_t *out;
    kaze_tape_node_t *n;

    if (tape == NULL || arena == NULL || x == NULL || bias == NULL) {
        tape_fail("kaze_tape_add_bias needs a tape, an arena, x and bias");
        return NULL;
    }

    out = kaze_tensor_add_bias(arena, x, bias);
    if (out == NULL) {
        return NULL;
    }
    n = tape_push(tape, KAZE_TAPE_ADD_BIAS);
    if (n == NULL) {
        return NULL;
    }
    n->out = out;
    n->a = x;
    n->b = bias;
    return out;
}

static kaze_tensor_t *tape_unary(kaze_tape_t *tape, kaze_arena_t *arena,
                                 kaze_tape_op_t op, const kaze_tensor_t *x,
                                 kaze_tensor_t *(*forward)(kaze_arena_t *,
                                                           const kaze_tensor_t *))
{
    kaze_tensor_t *out;
    kaze_tape_node_t *n;

    if (tape == NULL || arena == NULL || x == NULL) {
        tape_fail("a taped unary op needs a tape, an arena and a tensor");
        return NULL;
    }
    out = forward(arena, x);
    if (out == NULL) {
        return NULL;
    }
    n = tape_push(tape, op);
    if (n == NULL) {
        return NULL;
    }
    n->out = out;
    n->a = x;
    return out;
}

kaze_tensor_t *kaze_tape_relu(kaze_tape_t *tape, kaze_arena_t *arena,
                              const kaze_tensor_t *x)
{
    return tape_unary(tape, arena, KAZE_TAPE_RELU, x, kaze_tensor_relu);
}

kaze_tensor_t *kaze_tape_silu(kaze_tape_t *tape, kaze_arena_t *arena,
                              const kaze_tensor_t *x)
{
    return tape_unary(tape, arena, KAZE_TAPE_SILU, x, kaze_tensor_silu);
}

kaze_tensor_t *kaze_tape_gelu(kaze_tape_t *tape, kaze_arena_t *arena,
                              const kaze_tensor_t *x)
{
    return tape_unary(tape, arena, KAZE_TAPE_GELU, x, kaze_tensor_gelu);
}

kaze_tensor_t *kaze_tape_softmax(kaze_tape_t *tape, kaze_arena_t *arena,
                                 const kaze_tensor_t *x)
{
    return tape_unary(tape, arena, KAZE_TAPE_SOFTMAX, x, kaze_tensor_softmax);
}

kaze_tensor_t *kaze_tape_rms_norm(kaze_tape_t *tape, kaze_arena_t *arena,
                                  const kaze_tensor_t *x,
                                  const kaze_tensor_t *weight, float eps)
{
    kaze_tensor_t *out;
    kaze_tape_node_t *n;

    if (tape == NULL || arena == NULL || x == NULL || weight == NULL) {
        tape_fail("kaze_tape_rms_norm needs a tape, an arena, x and weight");
        return NULL;
    }
    out = kaze_tensor_rms_norm(arena, x, weight, eps);
    if (out == NULL) {
        return NULL;
    }
    n = tape_push(tape, KAZE_TAPE_RMS_NORM);
    if (n == NULL) {
        return NULL;
    }
    n->out = out;
    n->a = x;
    n->b = weight;
    n->eps = eps;
    return out;
}

kaze_tensor_t *kaze_tape_layer_norm(kaze_tape_t *tape, kaze_arena_t *arena,
                                    const kaze_tensor_t *x,
                                    const kaze_tensor_t *weight,
                                    const kaze_tensor_t *bias, float eps)
{
    kaze_tensor_t *out;
    kaze_tape_node_t *n;

    if (tape == NULL || arena == NULL || x == NULL || weight == NULL) {
        tape_fail("kaze_tape_layer_norm needs a tape, an arena, x and weight");
        return NULL;
    }
    out = kaze_tensor_layer_norm(arena, x, weight, bias, eps);
    if (out == NULL) {
        return NULL;
    }
    n = tape_push(tape, KAZE_TAPE_LAYER_NORM);
    if (n == NULL) {
        return NULL;
    }
    n->out = out;
    n->a = x;
    n->b = weight;
    n->c = bias;
    n->eps = eps;
    return out;
}

kaze_tensor_t *kaze_tape_mse(kaze_tape_t *tape, kaze_arena_t *arena,
                             const kaze_tensor_t *pred, const kaze_tensor_t *target)
{
    kaze_tensor_t *out;
    kaze_tape_node_t *n;
    int64_t shape[1] = { 1 };
    int64_t i, count;
    double acc = 0.0;

    if (tape == NULL || arena == NULL || pred == NULL || target == NULL ||
        !same_shape(pred, target)) {
        tape_fail("kaze_tape_mse needs matching prediction and target tensors");
        return NULL;
    }
    count = kaze_tensor_numel(pred);
    for (i = 0; i < count; i++) {
        double d = (double)kaze_tensor_get(pred, i) - (double)kaze_tensor_get(target, i);
        acc += d * d;
    }
    out = kaze_tensor_zeros(arena, KAZE_DTYPE_F32, 1, shape);
    if (out == NULL) {
        return NULL;
    }
    kaze_tensor_set(out, 0, (kaze_float_t)(acc / (double)count));
    n = tape_push(tape, KAZE_TAPE_MSE_LOSS);
    if (n == NULL) {
        return NULL;
    }
    n->out = out;
    n->a = pred;
    n->b = target;
    return out;
}

kaze_tensor_t *kaze_tape_cross_entropy(kaze_tape_t *tape, kaze_arena_t *arena,
                                       const kaze_tensor_t *logits, int32_t target_class)
{
    kaze_tensor_t *out;
    kaze_tape_node_t *n;
    int64_t shape[1] = { 1 };
    int64_t width, j;
    double maxv = -HUGE_VAL, sum = 0.0;
    const kaze_tensor_t *x;

    if (tape == NULL || arena == NULL || logits == NULL || logits->rank < 1) {
        tape_fail("kaze_tape_cross_entropy needs a tape, an arena and logits");
        return NULL;
    }
    width = logits->shape[logits->rank - 1];
    if (target_class < 0 || target_class >= width) {
        tape_fail("cross_entropy target class is outside the logits width");
        return NULL;
    }
    x = logits;
    for (j = 0; j < width; j++) {
        double v = (double)kaze_tensor_get(x, j);
        if (v > maxv) {
            maxv = v;
        }
    }
    for (j = 0; j < width; j++) {
        sum += exp((double)kaze_tensor_get(x, j) - maxv);
    }
    if (!(sum > 0.0) || !isfinite(sum)) {
        (void)kaze_trap(KAZE_ERR_MATH, "cross_entropy normalizer is not finite");
        return NULL;
    }
    out = kaze_tensor_zeros(arena, KAZE_DTYPE_F32, 1, shape);
    if (out == NULL) {
        return NULL;
    }
    kaze_tensor_set(out, 0, (kaze_float_t)(-(double)kaze_tensor_get(x, target_class) + maxv + log(sum)));
    n = tape_push(tape, KAZE_TAPE_CROSS_ENTROPY);
    if (n == NULL) {
        return NULL;
    }
    n->out = out;
    n->a = logits;
    n->target = target_class;
    return out;
}

/* ------------------------------------------------------------------------- */
/* Backward pass                                                              */
/* ------------------------------------------------------------------------- */

static kaze_tensor_t *elem_mask_positive(kaze_arena_t *arena, const kaze_tensor_t *x)
{
    kaze_tensor_t *mask = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, x->shape);
    int64_t i, n;
    if (mask == NULL) {
        return NULL;
    }
    n = kaze_tensor_numel(x);
    for (i = 0; i < n; i++) {
        kaze_float_t v = kaze_tensor_get(x, i);
        kaze_tensor_set(mask, i, v > 0.0f ? 1.0f : 0.0f);
    }
    return mask;
}

static kaze_tensor_t *elem_deriv_silu(kaze_arena_t *arena, const kaze_tensor_t *x)
{
    kaze_tensor_t *d = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, x->shape);
    int64_t i, n;
    if (d == NULL) {
        return NULL;
    }
    n = kaze_tensor_numel(x);
    for (i = 0; i < n; i++) {
        double v = (double)kaze_tensor_get(x, i);
        double s = 1.0 / (1.0 + exp(-v));
        kaze_tensor_set(d, i, (kaze_float_t)(s * (1.0 + v * (1.0 - s))));
    }
    return d;
}

static kaze_tensor_t *elem_deriv_gelu(kaze_arena_t *arena, const kaze_tensor_t *x)
{
    kaze_tensor_t *d = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, x->shape);
    int64_t i, n;
    if (d == NULL) {
        return NULL;
    }
    n = kaze_tensor_numel(x);
    for (i = 0; i < n; i++) {
        double v = (double)kaze_tensor_get(x, i);
        double cdf = 0.5 * (1.0 + erf(v * 0.70710678118654752440));
        double pdf = exp(-0.5 * v * v) / 2.50662827463100050240;
        kaze_tensor_set(d, i, (kaze_float_t)(cdf + v * pdf));
    }
    return d;
}

static kaze_tensor_t *colsum_into_width(kaze_arena_t *arena, const kaze_tensor_t *g,
                                        int64_t width)
{
    int64_t shape[1];
    int64_t rows, r, j;
    kaze_tensor_t *sum;

    if (g->rank < 1 || g->shape[g->rank - 1] != width) {
        tape_fail("bias gradient needs the last dimension to match the bias");
        return NULL;
    }
    shape[0] = width;
    sum = kaze_tensor_zeros(arena, KAZE_DTYPE_F32, 1, shape);
    if (sum == NULL) {
        return NULL;
    }
    rows = width == 0 ? 0 : kaze_tensor_numel(g) / width;
    for (r = 0; r < rows; r++) {
        for (j = 0; j < width; j++) {
            kaze_tensor_set(sum, j, kaze_tensor_get(sum, j) +
                                     kaze_tensor_get(g, r * width + j));
        }
    }
    return sum;
}

static kaze_tensor_t *softmax_jacobian(kaze_arena_t *arena, const kaze_tensor_t *y,
                                       const kaze_tensor_t *g)
{
    kaze_tensor_t *dx;
    int64_t width, rows, r, j;
    int64_t n;

    if (y->rank < 1 || !same_shape(y, g)) {
        tape_fail("softmax gradient needs matching shapes");
        return NULL;
    }
    width = y->shape[y->rank - 1];
    n = kaze_tensor_numel(y);
    rows = width == 0 ? 0 : n / width;
    dx = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, y->rank, y->shape);
    if (dx == NULL) {
        return NULL;
    }
    for (r = 0; r < rows; r++) {
        double dot = 0.0;
        for (j = 0; j < width; j++) {
            dot += (double)kaze_tensor_get(y, r * width + j) *
                   (double)kaze_tensor_get(g, r * width + j);
        }
        for (j = 0; j < width; j++) {
            double yi = (double)kaze_tensor_get(y, r * width + j);
            double gi = (double)kaze_tensor_get(g, r * width + j);
            kaze_tensor_set(dx, r * width + j, (kaze_float_t)(yi * (gi - dot)));
        }
    }
    return dx;
}

static bool rms_backward(kaze_tape_t *tape, kaze_tape_node_t *n,
                         const kaze_tensor_t *g)
{
    kaze_arena_t *arena = tape->arena;
    const kaze_tensor_t *x = n->a;
    const kaze_tensor_t *w = n->b;
    kaze_tensor_t *dx;
    kaze_tensor_t *dw;
    int64_t width, rows, r, j;
    int64_t shape[1];

    if (x->rank < 1 || w->rank != 1 || w->shape[0] != x->shape[x->rank - 1] ||
        !same_shape(g, n->out)) {
        tape_fail("rms_norm gradient has inconsistent shapes");
        return false;
    }
    width = x->shape[x->rank - 1];
    rows = width == 0 ? 0 : kaze_tensor_numel(x) / width;
    dx = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, x->shape);
    shape[0] = width;
    dw = kaze_tensor_zeros(arena, KAZE_DTYPE_F32, 1, shape);
    if (dx == NULL || dw == NULL) {
        return false;
    }
    for (r = 0; r < rows; r++) {
        double mean_sq = 0.0, inv, inv3, sum_gwx = 0.0, eps = (double)n->eps;
        for (j = 0; j < width; j++) {
            double xv = (double)kaze_tensor_get(x, r * width + j);
            mean_sq += xv * xv;
        }
        mean_sq = mean_sq / (double)width + eps;
        if (!(mean_sq > 0.0) || !isfinite(mean_sq)) {
            (void)kaze_trap(KAZE_ERR_MATH, "rms_norm gradient variance is not finite");
            return false;
        }
        inv = 1.0 / sqrt(mean_sq);
        inv3 = inv * inv * inv;
        for (j = 0; j < width; j++) {
            double xv = (double)kaze_tensor_get(x, r * width + j);
            double wv = (double)kaze_tensor_get(w, j);
            double gv = (double)kaze_tensor_get(g, r * width + j);
            sum_gwx += gv * wv * xv;
            kaze_tensor_set(dw, j, (kaze_float_t)((double)kaze_tensor_get(dw, j) +
                                                   gv * xv * inv));
        }
        for (j = 0; j < width; j++) {
            double xv = (double)kaze_tensor_get(x, r * width + j);
            double wv = (double)kaze_tensor_get(w, j);
            double gv = (double)kaze_tensor_get(g, r * width + j);
            double val = inv * wv * gv - inv3 * xv * sum_gwx / (double)width;
            kaze_tensor_set(dx, r * width + j, (kaze_float_t)val);
        }
    }
    grad_accumulate(tape, tape_find(tape, x), dx);
    grad_accumulate(tape, tape_find(tape, w), dw);
    return true;
}

static bool layer_backward(kaze_tape_t *tape, kaze_tape_node_t *n,
                           const kaze_tensor_t *g)
{
    kaze_arena_t *arena = tape->arena;
    const kaze_tensor_t *x = n->a;
    const kaze_tensor_t *w = n->b;
    const kaze_tensor_t *bias = n->c;
    kaze_tensor_t *dx;
    kaze_tensor_t *dw;
    kaze_tensor_t *db;
    int64_t width, rows, r, j;
    int64_t wshape[1];

    if (x->rank < 1 || w->rank != 1 || w->shape[0] != x->shape[x->rank - 1] ||
        !same_shape(g, n->out)) {
        tape_fail("layer_norm gradient has inconsistent shapes");
        return false;
    }
    width = x->shape[x->rank - 1];
    rows = width == 0 ? 0 : kaze_tensor_numel(x) / width;
    dx = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, x->rank, x->shape);
    wshape[0] = width;
    dw = kaze_tensor_zeros(arena, KAZE_DTYPE_F32, 1, wshape);
    db = kaze_tensor_zeros(arena, KAZE_DTYPE_F32, 1, wshape);
    if (dx == NULL || dw == NULL || db == NULL) {
        return false;
    }
    for (r = 0; r < rows; r++) {
        double mean = 0.0, var = 0.0, inv;
        double sum_gw = 0.0, sum_gwz = 0.0;

        for (j = 0; j < width; j++) {
            mean += (double)kaze_tensor_get(x, r * width + j);
        }
        mean /= (double)width;
        for (j = 0; j < width; j++) {
            double d = (double)kaze_tensor_get(x, r * width + j) - mean;
            var += d * d;
        }
        var = var / (double)width + (double)n->eps;
        if (!(var > 0.0) || !isfinite(var)) {
            (void)kaze_trap(KAZE_ERR_MATH, "layer_norm gradient variance is not finite");
            return false;
        }
        inv = 1.0 / sqrt(var);
        for (j = 0; j < width; j++) {
            double z = ((double)kaze_tensor_get(x, r * width + j) - mean) * inv;
            double gv = (double)kaze_tensor_get(g, r * width + j);
            double wv = (double)kaze_tensor_get(w, j);
            sum_gw += gv * wv;
            sum_gwz += gv * wv * z;
            kaze_tensor_set(dw, j, (kaze_float_t)((double)kaze_tensor_get(dw, j) + gv * z));
            if (bias != NULL) {
                kaze_tensor_set(db, j, (kaze_float_t)((double)kaze_tensor_get(db, j) + gv));
            }
        }
        for (j = 0; j < width; j++) {
            double z = ((double)kaze_tensor_get(x, r * width + j) - mean) * inv;
            double gv = (double)kaze_tensor_get(g, r * width + j);
            double wv = (double)kaze_tensor_get(w, j);
            double val = inv * wv * gv - sum_gw * inv / (double)width -
                         z * sum_gwz * inv / (double)width;
            kaze_tensor_set(dx, r * width + j, (kaze_float_t)val);
        }
    }
    grad_accumulate(tape, tape_find(tape, x), dx);
    grad_accumulate(tape, tape_find(tape, w), dw);
    if (bias != NULL) {
        grad_accumulate(tape, tape_find(tape, bias), db);
    }
    return true;
}

bool kaze_tape_backward(kaze_tape_t *tape, const kaze_tensor_t *loss)
{
    kaze_tape_node_t *loss_node;
    size_t idx;
    int64_t i, n;

    if (tape == NULL || loss == NULL) {
        tape_fail("kaze_tape_backward needs a tape and a loss tensor");
        return false;
    }
    loss_node = tape_find(tape, loss);
    if (loss_node == NULL) {
        tape_fail("the loss tensor is not on the tape");
        return false;
    }
    {
        kaze_tensor_t *seed = node_grad(tape, loss_node);
        if (seed == NULL) {
            return false;
        }
        n = kaze_tensor_numel(seed);
        for (i = 0; i < n; i++) {
            kaze_tensor_set(seed, i, 1.0f);
        }
    }
    for (idx = tape->count; idx > 0; idx--) {
        kaze_tape_node_t *node = &tape->nodes[idx - 1];
        kaze_arena_t *arena = tape->arena;
        const kaze_tensor_t *g = node->grad;
        kaze_tape_node_t *na;
        kaze_tape_node_t *nb;

        if (g == NULL || node->op == KAZE_TAPE_LEAF) {
            continue;
        }
        na = tape_find(tape, node->a);
        nb = tape_find(tape, node->b);
        switch (node->op) {
        case KAZE_TAPE_ADD:
            grad_accumulate(tape, na, g);
            grad_accumulate(tape, nb, g);
            break;
        case KAZE_TAPE_SUB: {
            kaze_tensor_t *neg = kaze_tensor_scale(arena, g, -1.0f);
            grad_accumulate(tape, na, g);
            grad_accumulate(tape, nb, neg);
            break;
        }
        case KAZE_TAPE_MUL: {
            kaze_tensor_t *ga = elem_mul_broadcast(arena, g, node->b);
            kaze_tensor_t *gb = elem_mul_broadcast(arena, g, node->a);
            grad_accumulate(tape, na, ga);
            grad_accumulate(tape, nb, gb);
            break;
        }
        case KAZE_TAPE_MATMUL: {
            kaze_tensor_t *ga;
            kaze_tensor_t *gb;
            kaze_tensor_t *at;
            if (node->a->rank != 2 || node->b->rank != 2) {
                tape_fail("taped matmul gradients support rank-2 tensors");
                return false;
            }
            ga = kaze_tensor_matmul_t(arena, g, node->b);
            at = kaze_tensor_transpose(arena, node->a, 0, 1);
            gb = kaze_tensor_matmul(arena, at, g);
            grad_accumulate(tape, na, ga);
            grad_accumulate(tape, nb, gb);
            break;
        }
        case KAZE_TAPE_ADD_BIAS: {
            kaze_tensor_t *gb = colsum_into_width(arena, g, node->b->shape[0]);
            grad_accumulate(tape, na, g);
            grad_accumulate(tape, nb, gb);
            break;
        }
        case KAZE_TAPE_RELU: {
            kaze_tensor_t *mask = elem_mask_positive(arena, node->a);
            kaze_tensor_t *dx = elem_mul_broadcast(arena, g, mask);
            grad_accumulate(tape, na, dx);
            break;
        }
        case KAZE_TAPE_SILU: {
            kaze_tensor_t *d = elem_deriv_silu(arena, node->a);
            kaze_tensor_t *dx = elem_mul_broadcast(arena, g, d);
            grad_accumulate(tape, na, dx);
            break;
        }
        case KAZE_TAPE_GELU: {
            kaze_tensor_t *d = elem_deriv_gelu(arena, node->a);
            kaze_tensor_t *dx = elem_mul_broadcast(arena, g, d);
            grad_accumulate(tape, na, dx);
            break;
        }
        case KAZE_TAPE_SOFTMAX: {
            kaze_tensor_t *dx = softmax_jacobian(arena, node->out, g);
            grad_accumulate(tape, na, dx);
            break;
        }
        case KAZE_TAPE_RMS_NORM:
            if (!rms_backward(tape, node, g)) {
                return false;
            }
            break;
        case KAZE_TAPE_LAYER_NORM:
            if (!layer_backward(tape, node, g)) {
                return false;
            }
            break;
        case KAZE_TAPE_MSE_LOSS: {
            kaze_tensor_t *dx;
            double scale;
            int64_t count = kaze_tensor_numel(node->a);
            if (count <= 0) {
                break;
            }
            scale = 2.0 * (double)kaze_tensor_get(g, 0) / (double)count;
            dx = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, node->a->rank,
                                   node->a->shape);
            if (dx == NULL) {
                return false;
            }
            for (i = 0; i < count; i++) {
                double d = (double)kaze_tensor_get(node->a, i) -
                           (double)kaze_tensor_get(node->b, i);
                kaze_tensor_set(dx, i, (kaze_float_t)(scale * d));
            }
            grad_accumulate(tape, na, dx);
            break;
        }
        case KAZE_TAPE_CROSS_ENTROPY: {
            kaze_tensor_t *dx;
            int64_t width = node->a->shape[node->a->rank - 1];
            double maxv = -HUGE_VAL, sum = 0.0, gscale;
            for (i = 0; i < width; i++) {
                double v = (double)kaze_tensor_get(node->a, i);
                if (v > maxv) {
                    maxv = v;
                }
            }
            for (i = 0; i < width; i++) {
                sum += exp((double)kaze_tensor_get(node->a, i) - maxv);
            }
            gscale = (double)kaze_tensor_get(g, 0);
            dx = kaze_tensor_alloc(arena, KAZE_DTYPE_F32, node->a->rank,
                                   node->a->shape);
            if (dx == NULL) {
                return false;
            }
            for (i = 0; i < width; i++) {
                double p = exp((double)kaze_tensor_get(node->a, i) - maxv) / sum;
                double onehot = (i == node->target) ? 1.0 : 0.0;
                kaze_tensor_set(dx, i, (kaze_float_t)(gscale * (p - onehot)));
            }
            grad_accumulate(tape, na, dx);
            break;
        }
        case KAZE_TAPE_LEAF:
            break;
        default:
            break;
        }
    }
    return true;
}

const kaze_tensor_t *kaze_tape_grad(const kaze_tape_t *tape, const kaze_tensor_t *leaf)
{
    size_t i;

    if (tape == NULL || leaf == NULL) {
        return NULL;
    }
    for (i = tape->count; i > 0; i--) {
        if (tape->nodes[i - 1].out == leaf) {
            return tape->nodes[i - 1].grad;
        }
    }
    return NULL;
}

/* ------------------------------------------------------------------------- */
/* AdamW                                                                      */
/* ------------------------------------------------------------------------- */

bool kaze_opt_adamw(kaze_tensor_t *leaf, const kaze_tensor_t *grad,
                    kaze_tensor_t *m, kaze_tensor_t *v, float lr,
                    float beta1, float beta2, float eps,
                    float weight_decay, int64_t step)
{
    int64_t i, n;
    double b1c, b2c;

    if (leaf == NULL || grad == NULL || m == NULL || v == NULL ||
        !kaze_tensor_same_shape(leaf, grad) ||
        !kaze_tensor_same_shape(leaf, m) || !kaze_tensor_same_shape(leaf, v)) {
        tape_fail("AdamW needs four tensors of the same shape");
        return false;
    }
    if (step < 1) {
        tape_fail("AdamW step must start at 1");
        return false;
    }
    b1c = 1.0 - pow((double)beta1, (double)step);
    b2c = 1.0 - pow((double)beta2, (double)step);
    n = kaze_tensor_numel(leaf);
    for (i = 0; i < n; i++) {
        double g = (double)kaze_tensor_get(grad, i);
        double mi = (double)beta1 * (double)kaze_tensor_get(m, i) + (1.0 - (double)beta1) * g;
        double vi = (double)beta2 * (double)kaze_tensor_get(v, i) + (1.0 - (double)beta2) * g * g;
        double mh = mi / b1c;
        double vh = vi / b2c;
        double p = (double)kaze_tensor_get(leaf, i);
        double update = mh / (sqrt(vh) + (double)eps) + (double)weight_decay * p;
        kaze_tensor_set(m, i, (kaze_float_t)mi);
        kaze_tensor_set(v, i, (kaze_float_t)vi);
        kaze_tensor_set(leaf, i, (kaze_float_t)(p - (double)lr * update));
    }
    return true;
}

