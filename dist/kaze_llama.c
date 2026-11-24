/* kaze_llama.c — LLaMA-family forward pass over GGUF weights (Phase 12).
 *
 * Scope: the llama architecture used by SmolLM2/Llama-3.2 GGUF files (GQA,
 * RMSNorm, RoPE, SwiGLU, tied or separate output weights). Weights stay in the
 * mmap; a forward pass allocates every activation from one arena that is reset
 * per token, so a pass never calls malloc. The KV cache is a separate arena
 * allocated once at load.
 *
 * Numerical contract: the quantized matmul path uses the same Q8_1 activation
 * quantization as llama.cpp, so the two implementations compute the same
 * integer dot products. The gate compares the last-token logits against
 * llama.cpp within 1e-4 (scripts/phase12/compare_logits.py).
 */
#include "kaze_tensor.h"
#include "kaze_quant.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define KAZE_LLAMA_META_BYTES ((size_t)128u * 1024u * 1024u)
#define KAZE_LLAMA_SCRATCH_BYTES ((size_t)64u * 1024u * 1024u)
#define KAZE_LLAMA_MAX_DEFAULT_CTX 512

typedef struct kaze_llama_layer_t {
    const float *attn_norm;
    const kaze_tensor_t *wq;
    const kaze_tensor_t *wk;
    const kaze_tensor_t *wv;
    const kaze_tensor_t *wo;
    const float *ffn_norm;
    const kaze_tensor_t *w_gate;
    const kaze_tensor_t *w_up;
    const kaze_tensor_t *w_down;
} kaze_llama_layer_t;

struct kaze_llama_t {
    kaze_arena_t meta_arena;
    kaze_arena_t scratch_arena;
    kaze_arena_t kv_arena;
    kaze_gguf_t *gguf;
    int32_t n_layer;
    int32_t n_embd;
    int32_t n_head;
    int32_t n_head_kv;
    int32_t n_ff;
    int32_t n_vocab;
    int32_t head_dim;
    int32_t rope_dims;
    float rope_theta;
    float eps;
    const kaze_tensor_t *tok_embd;
    const float *out_norm;
    const kaze_tensor_t *out_w;
    kaze_llama_layer_t *layers;
    float **k_cache;
    float **v_cache;
    size_t n_ctx;
    size_t pos;
    kaze_tokenizer_t *tok;
};

static void llama_fail(const char *msg)
{
    (void)kaze_trap(KAZE_ERR_FORMAT, msg);
}

/* Opt-in tensor dump for the numerical-parity gate: KAZE_DUMP_DIR=<dir> writes
 * each named activation as raw f32 little-endian so scripts/phase12 can diff a
 * stage against llama.cpp. Off by default; it is a test hook, not a feature. */
static void dump_f32(const char *name, const float *v, int32_t n)
{
    const char *dir = getenv("KAZE_DUMP_DIR");
    char path[512];
    FILE *f;

    if (dir == NULL || dir[0] == '\0' || v == NULL || n <= 0) {
        return;
    }
    (void)snprintf(path, sizeof path, "%s/%s.bin", dir, name);
    f = fopen(path, "wb");
    if (f == NULL) {
        return;
    }
    (void)fwrite(v, sizeof(float), (size_t)n, f);
    (void)fclose(f);
}

static int64_t need_i64(const kaze_llama_t *m, const char *key)
{
    int64_t v = 0;
    if (!kaze_gguf_meta_i64(m->gguf, key, &v)) {
        llama_fail("required GGUF hyperparameter is missing");
        return 0;
    }
    return v;
}

static int64_t opt_i64(const kaze_llama_t *m, const char *key, int64_t fallback)
{
    int64_t v = 0;
    if (!kaze_gguf_meta_i64(m->gguf, key, &v)) {
        return fallback;
    }
    return v;
}

static float opt_f32(const kaze_llama_t *m, const char *key, float fallback)
{
    float v = 0.0f;
    if (!kaze_gguf_meta_f32(m->gguf, key, &v)) {
        return fallback;
    }
    return v;
}

static const kaze_tensor_t *need_tensor(const kaze_llama_t *m, const char *name)
{
    const kaze_tensor_t *t = kaze_gguf_tensor(m->gguf, name);
    if (t == NULL) {
        llama_fail("a required weight tensor is missing from the GGUF file");
    }
    return t;
}

static const float *need_vec_f32(kaze_llama_t *m, const char *name, int32_t n)
{
    const kaze_tensor_t *t = need_tensor(m, name);
    kaze_tensor_t *f32t;

    if (t == NULL) {
        return NULL;
    }
    if (t->rank != 1 || t->shape[0] != n) {
        llama_fail("a norm weight has the wrong rank or width");
        return NULL;
    }
    if (t->dtype == KAZE_DTYPE_F32) {
        return (const float *)t->data;
    }
    f32t = kaze_quant_dequant(&m->meta_arena, t);
    if (f32t == NULL) {
        return NULL;
    }
    return (const float *)f32t->data;
}

static void require_rank2(const kaze_llama_t *m, const kaze_tensor_t *t,
                          int32_t rows, int32_t cols, const char *what)
{
    if (t == NULL) {
        return;
    }
    if (t->rank != 2 || t->shape[0] != rows || t->shape[1] != cols) {
        (void)m;
        llama_fail(what);
    }
}

static void layer_name(char *buf, size_t cap, int32_t layer, const char *suffix)
{
    (void)snprintf(buf, cap, "blk.%d.%s", (int)layer, suffix);
}

kaze_llama_t *kaze_llama_open(const char *path)
{
    kaze_llama_t *m;
    const char *arch = NULL;
    int32_t i;
    size_t kv_bytes;
    const char *ctx_env;

    if (path == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "kaze_llama_open needs a model path");
        return NULL;
    }
    m = (kaze_llama_t *)calloc(1, sizeof(kaze_llama_t));
    if (m == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "model handle allocation failed");
        return NULL;
    }
    if (!kaze_arena_init_heap(&m->meta_arena, KAZE_LLAMA_META_BYTES) ||
        !kaze_arena_init_heap(&m->scratch_arena, KAZE_LLAMA_SCRATCH_BYTES)) {
        (void)kaze_trap(KAZE_ERR_OOM, "model arenas could not be created");
        return NULL;
    }
    m->gguf = kaze_gguf_open(&m->meta_arena, path);
    if (m->gguf == NULL) {
        return NULL;
    }
    if (!kaze_gguf_meta_str(m->gguf, "general.architecture", &arch) ||
        strcmp(arch, "llama") != 0) {
        (void)kaze_trap(KAZE_ERR_FORMAT, "Phase 12 inference supports the llama architecture only");
        return NULL;
    }
    m->n_layer = (int32_t)need_i64(m, "llama.block_count");
    m->n_embd = (int32_t)need_i64(m, "llama.embedding_length");
    m->n_head = (int32_t)need_i64(m, "llama.attention.head_count");
    m->n_head_kv = (int32_t)opt_i64(m, "llama.attention.head_count_kv", m->n_head);
    m->n_ff = (int32_t)need_i64(m, "llama.feed_forward_length");
    m->n_vocab = (int32_t)need_i64(m, "llama.vocab_size");
    m->head_dim = (int32_t)opt_i64(m, "llama.attention.key_length",
                                   m->n_head > 0 ? m->n_embd / m->n_head : 0);
    m->rope_dims = (int32_t)opt_i64(m, "llama.rope.dimension_count", m->head_dim);
    m->rope_theta = opt_f32(m, "llama.rope.freq_base", 10000.0f);
    m->eps = opt_f32(m, "llama.attention.layer_norm_rms_epsilon", 1e-5f);
    if (m->n_layer <= 0 || m->n_embd <= 0 || m->n_head <= 0 || m->n_head_kv <= 0 ||
        m->n_ff <= 0 || m->n_vocab <= 0 || m->head_dim <= 0 || m->rope_dims <= 0 ||
        m->rope_dims > m->head_dim || (m->rope_dims % 2) != 0 ||
        m->n_head % m->n_head_kv != 0 || !(m->rope_theta > 1.0f) || !(m->eps > 0.0f)) {
        (void)kaze_trap(KAZE_ERR_FORMAT, "inconsistent llama hyperparameters in the GGUF file");
        return NULL;
    }
    m->n_ctx = (size_t)opt_i64(m, "llama.context_length", KAZE_LLAMA_MAX_DEFAULT_CTX);
    if (m->n_ctx > KAZE_LLAMA_MAX_DEFAULT_CTX) {
        m->n_ctx = KAZE_LLAMA_MAX_DEFAULT_CTX;
    }
    ctx_env = getenv("KAZE_N_CTX");
    if (ctx_env != NULL && ctx_env[0] != '\0') {
        long want = strtol(ctx_env, NULL, 10);
        if (want > 0 && (size_t)want <= m->n_ctx) {
            m->n_ctx = (size_t)want;
        }
    }
    m->tok_embd = need_tensor(m, "token_embd.weight");
    require_rank2(m, m->tok_embd, m->n_vocab, m->n_embd, "token_embd.weight has the wrong shape");
    m->out_norm = need_vec_f32(m, "output_norm.weight", m->n_embd);
    m->out_w = kaze_gguf_tensor(m->gguf, "output.weight");
    if (m->out_w == NULL) {
        m->out_w = m->tok_embd; /* tied embeddings */
    } else {
        require_rank2(m, m->out_w, m->n_vocab, m->n_embd, "output.weight has the wrong shape");
    }
    m->layers = (kaze_llama_layer_t *)kmalloc(&m->meta_arena,
                                              (size_t)m->n_layer * sizeof(kaze_llama_layer_t));
    m->k_cache = (float **)kmalloc(&m->meta_arena, (size_t)m->n_layer * sizeof(float *));
    m->v_cache = (float **)kmalloc(&m->meta_arena, (size_t)m->n_layer * sizeof(float *));
    if (m->layers == NULL || m->k_cache == NULL || m->v_cache == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "model layer tables did not fit the metadata arena");
        return NULL;
    }
    kv_bytes = (size_t)m->n_layer * 2u * (size_t)m->n_head_kv * m->n_ctx *
               (size_t)m->head_dim * sizeof(float);
    if (!kaze_arena_init_heap(&m->kv_arena, kv_bytes)) {
        (void)kaze_trap(KAZE_ERR_OOM, "KV cache arena could not be created");
        return NULL;
    }
    for (i = 0; i < m->n_layer; i++) {
        char name[128];
        kaze_llama_layer_t *l = &m->layers[i];
        int32_t qdim = m->n_head * m->head_dim;
        int32_t kdim = m->n_head_kv * m->head_dim;

        layer_name(name, sizeof name, i, "attn_norm.weight");
        l->attn_norm = need_vec_f32(m, name, m->n_embd);
        layer_name(name, sizeof name, i, "attn_q.weight");
        l->wq = need_tensor(m, name);
        require_rank2(m, l->wq, qdim, m->n_embd, "attn_q.weight has the wrong shape");
        layer_name(name, sizeof name, i, "attn_k.weight");
        l->wk = need_tensor(m, name);
        require_rank2(m, l->wk, kdim, m->n_embd, "attn_k.weight has the wrong shape");
        layer_name(name, sizeof name, i, "attn_v.weight");
        l->wv = need_tensor(m, name);
        require_rank2(m, l->wv, kdim, m->n_embd, "attn_v.weight has the wrong shape");
        layer_name(name, sizeof name, i, "attn_output.weight");
        l->wo = need_tensor(m, name);
        require_rank2(m, l->wo, m->n_embd, qdim, "attn_output.weight has the wrong shape");
        layer_name(name, sizeof name, i, "ffn_norm.weight");
        l->ffn_norm = need_vec_f32(m, name, m->n_embd);
        layer_name(name, sizeof name, i, "ffn_gate.weight");
        l->w_gate = need_tensor(m, name);
        require_rank2(m, l->w_gate, m->n_ff, m->n_embd, "ffn_gate.weight has the wrong shape");
        layer_name(name, sizeof name, i, "ffn_up.weight");
        l->w_up = need_tensor(m, name);
        require_rank2(m, l->w_up, m->n_ff, m->n_embd, "ffn_up.weight has the wrong shape");
        layer_name(name, sizeof name, i, "ffn_down.weight");
        l->w_down = need_tensor(m, name);
        require_rank2(m, l->w_down, m->n_embd, m->n_ff, "ffn_down.weight has the wrong shape");
        if (l->attn_norm == NULL || l->ffn_norm == NULL || l->wq == NULL ||
            l->wk == NULL || l->wv == NULL || l->wo == NULL || l->w_gate == NULL ||
            l->w_up == NULL || l->w_down == NULL) {
            return NULL;
        }
        m->k_cache[i] = (float *)kmalloc(&m->kv_arena,
                                         (size_t)m->n_head_kv * m->n_ctx *
                                         (size_t)m->head_dim * sizeof(float));
        m->v_cache[i] = (float *)kmalloc(&m->kv_arena,
                                         (size_t)m->n_head_kv * m->n_ctx *
                                         (size_t)m->head_dim * sizeof(float));
        if (m->k_cache[i] == NULL || m->v_cache[i] == NULL) {
            (void)kaze_trap(KAZE_ERR_OOM, "KV cache did not fit its arena");
            return NULL;
        }
        memset(m->k_cache[i], 0,
               (size_t)m->n_head_kv * m->n_ctx * (size_t)m->head_dim * sizeof(float));
        memset(m->v_cache[i], 0,
               (size_t)m->n_head_kv * m->n_ctx * (size_t)m->head_dim * sizeof(float));
    }
    m->pos = 0;
    m->tok = NULL;
    {
        const char *tok_model = NULL;
        if (kaze_gguf_meta_str(m->gguf, "tokenizer.ggml.model", &tok_model) &&
            strcmp(tok_model, "gpt2") == 0) {
            m->tok = kaze_tokenizer_from_gguf(&m->meta_arena, m->gguf);
        }
    }
    return m;
}

void kaze_llama_close(kaze_llama_t *model)
{
    if (model == NULL) {
        return;
    }
    kaze_gguf_close(model->gguf);
    kaze_arena_free(&model->kv_arena);
    kaze_arena_free(&model->scratch_arena);
    kaze_arena_free(&model->meta_arena);
    free(model);
}

size_t kaze_llama_n_vocab(const kaze_llama_t *model)
{
    return model == NULL ? 0u : (size_t)model->n_vocab;
}

size_t kaze_llama_n_ctx(const kaze_llama_t *model)
{
    return model == NULL ? 0u : model->n_ctx;
}

size_t kaze_llama_pos(const kaze_llama_t *model)
{
    return model == NULL ? 0u : model->pos;
}

const kaze_tokenizer_t *kaze_llama_tokenizer(const kaze_llama_t *model)
{
    return model == NULL ? NULL : model->tok;
}

bool kaze_llama_reset(kaze_llama_t *model)
{
    if (model == NULL) {
        return false;
    }
    model->pos = 0;
    return true;
}

/* ------------------------------------------------------------------------- */
/* Forward-pass helpers                                                       */
/* ------------------------------------------------------------------------- */

static float *arena_f32(kaze_arena_t *arena, int64_t n)
{
    float *p;
    if (n <= 0) {
        return NULL;
    }
    p = (float *)kmalloc(arena, (size_t)n * sizeof(float));
    if (p == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "activation did not fit the scratch arena");
    }
    return p;
}

static kaze_tensor_t *view_1xn(kaze_tensor_t *out, float *data, int64_t n)
{
    memset(out, 0, sizeof *out);
    out->dtype = KAZE_DTYPE_F32;
    out->rank = 2;
    out->shape[0] = 1;
    out->shape[1] = n;
    out->strides[0] = n;
    out->strides[1] = 1;
    out->data = data;
    out->nbytes = (size_t)n * sizeof(float);
    out->flags = KAZE_TENSOR_FLAG_ARENA;
    return out;
}

static void rmsnorm_into(float *out, const float *x, const float *w,
                         int32_t n, float eps)
{
    double ss = 0.0;
    double inv;
    int32_t i;

    for (i = 0; i < n; i++) {
        ss += (double)x[i] * (double)x[i];
    }
    ss = ss / (double)n + (double)eps;
    if (!(ss > 0.0) || !isfinite(ss)) {
        (void)kaze_trap(KAZE_ERR_MATH, "rms_norm variance is not finite and positive");
        return;
    }
    inv = 1.0 / sqrt(ss);
    for (i = 0; i < n; i++) {
        out[i] = (float)((double)x[i] * inv * (double)w[i]);
    }
}

static float silu_f(float v)
{
    return v / (1.0f + expf(-v));
}

static float dot_f32_simple(const float *a, const float *b, int32_t n)
{
    float acc = 0.0f;
    int32_t i;
    for (i = 0; i < n; i++) {
        acc += a[i] * b[i];
    }
    return acc;
}

static void vec_add_into(float *x, const float *y, int32_t n)
{
    int32_t i;
    for (i = 0; i < n; i++) {
        x[i] += y[i];
    }
}

/* LLaMA RoPE: adjacent pairs, per head, first rope_dims dimensions.
 *
 * Computed in f32 in the same order llama.cpp's GGML_ROPE_TYPE_NORMAL path
 * computes its cos/sin cache (theta starts at the position and is multiplied by
 * powf(base, -2/n_dims) once per pair). Doing this in double changes the last
 * bits of q/k, and in a 32-layer quantized network one changed activation
 * quant is enough to break the parity gate. */
static void rope_inplace(const kaze_llama_t *m, float *v, int32_t heads, size_t pos)
{
    int32_t h, i;
    int32_t hd = m->head_dim;
    int32_t rd = m->rope_dims;
    float theta_scale = powf(m->rope_theta, -2.0f / (float)rd);
    float theta_pos = (float)pos;

    for (h = 0; h < heads; h++) {
        float *base = v + (size_t)h * hd;
        float theta = theta_pos;
        for (i = 0; i < rd / 2; i++) {
            float c = cosf(theta);
            float s = sinf(theta);
            float x0 = base[2 * i];
            float x1 = base[2 * i + 1];
            base[2 * i] = x0 * c - x1 * s;
            base[2 * i + 1] = x0 * s + x1 * c;
            theta *= theta_scale;
        }
    }
}

static void cache_store(const kaze_llama_t *m, int32_t layer, const float *k,
                        const float *v, size_t pos)
{
    int32_t h;
    size_t head_stride = m->n_ctx * (size_t)m->head_dim;

    for (h = 0; h < m->n_head_kv; h++) {
        memcpy(m->k_cache[layer] + (size_t)h * head_stride + pos * (size_t)m->head_dim,
               k + (size_t)h * m->head_dim, (size_t)m->head_dim * sizeof(float));
        memcpy(m->v_cache[layer] + (size_t)h * head_stride + pos * (size_t)m->head_dim,
               v + (size_t)h * m->head_dim, (size_t)m->head_dim * sizeof(float));
    }
}

static void attention_into(const kaze_llama_t *m, int32_t layer, const float *q,
                           size_t pos, float *out, kaze_arena_t *arena)
{
    int32_t hq, j, d;
    int32_t nh = m->n_head;
    int32_t nkv = m->n_head_kv;
    int32_t hd = m->head_dim;
    int32_t ratio = nh / nkv;
    float scale = 1.0f / sqrtf((float)hd);
    float *scores = arena_f32(arena, (int64_t)(pos + 1));
    size_t head_stride = m->n_ctx * (size_t)hd;

    if (scores == NULL) {
        return;
    }
    for (hq = 0; hq < nh; hq++) {
        const float *qh = q + (size_t)hq * hd;
        const float *kc = m->k_cache[layer] + (size_t)(hq / ratio) * head_stride;
        const float *vc = m->v_cache[layer] + (size_t)(hq / ratio) * head_stride;
        float maxv = -INFINITY;
        float sum = 0.0f;
        float inv;
        for (j = 0; j <= (int32_t)pos; j++) {
            float s = dot_f32_simple(qh, kc + (size_t)j * hd, hd) * scale;
            scores[j] = s;
            if (s > maxv) {
                maxv = s;
            }
        }
        for (j = 0; j <= (int32_t)pos; j++) {
            float e = expf(scores[j] - maxv);
            scores[j] = e;
            sum += e;
        }
        if (!(sum > 0.0f) || !isfinite((double)sum)) {
            (void)kaze_trap(KAZE_ERR_MATH, "attention softmax normalizer is not finite");
            return;
        }
        inv = 1.0f / sum;
        for (d = 0; d < hd; d++) {
            float acc = 0.0f;
            for (j = 0; j <= (int32_t)pos; j++) {
                acc += scores[j] * inv * vc[(size_t)j * hd + d];
            }
            out[(size_t)hq * hd + d] = acc;
        }
    }
}

bool kaze_llama_forward(kaze_llama_t *model, const int32_t *tokens,
                        size_t n_tokens, size_t start_pos, float *logits)
{
    size_t t;
    int32_t l, i;

    if (model == NULL || tokens == NULL || n_tokens == 0) {
        (void)kaze_trap(KAZE_ERR_BADARG, "kaze_llama_forward needs a model and at least one token");
        return false;
    }
    if (start_pos + n_tokens > model->n_ctx) {
        (void)kaze_trap(KAZE_ERR_BADARG, "token position exceeds the KV cache context");
        return false;
    }
    for (t = 0; t < n_tokens; t++) {
        size_t pos = start_pos + t;
        int32_t token = tokens[t];
        kaze_arena_t *arena = &model->scratch_arena;
        float *x;
        kaze_tensor_t vh, va, vg, vu, vn;
        kaze_tensor_t *q;
        kaze_tensor_t *k;
        kaze_tensor_t *v;
        float *attn;
        float *h;
        float *gate;
        float *up;
        float *act;

        if (token < 0 || token >= model->n_vocab) {
            (void)kaze_trap(KAZE_ERR_BADARG, "token id is outside the vocabulary");
            return false;
        }
        kaze_arena_reset(arena);
        x = arena_f32(arena, model->n_embd);
        if (x == NULL) {
            return false;
        }
        kaze_quant_dequant_row(model->tok_embd, token, x, model->n_embd);
        dump_f32("embd", x, model->n_embd);
        for (l = 0; l < model->n_layer; l++) {
            const kaze_llama_layer_t *layer = &model->layers[l];
            char dbg[64];

            h = arena_f32(arena, model->n_embd);
            if (h == NULL) {
                return false;
            }
            rmsnorm_into(h, x, layer->attn_norm, model->n_embd, model->eps);
            (void)snprintf(dbg, sizeof dbg, "attn_norm-%d-t%zu", (int)l, pos);
            dump_f32(dbg, h, model->n_embd);
            (void)view_1xn(&vh, h, model->n_embd);
            q = kaze_tensor_matmul_quant(arena, &vh, layer->wq);
            k = kaze_tensor_matmul_quant(arena, &vh, layer->wk);
            v = kaze_tensor_matmul_quant(arena, &vh, layer->wv);
            if (q == NULL || k == NULL || v == NULL) {
                return false;
            }
            rope_inplace(model, (float *)q->data, model->n_head, pos);
            rope_inplace(model, (float *)k->data, model->n_head_kv, pos);
            (void)snprintf(dbg, sizeof dbg, "q-%d-t%zu", (int)l, pos);
            dump_f32(dbg, (const float *)q->data, model->n_head * model->head_dim);
            (void)snprintf(dbg, sizeof dbg, "k-%d-t%zu", (int)l, pos);
            dump_f32(dbg, (const float *)k->data, model->n_head_kv * model->head_dim);
            (void)snprintf(dbg, sizeof dbg, "v-%d-t%zu", (int)l, pos);
            dump_f32(dbg, (const float *)v->data, model->n_head_kv * model->head_dim);
            cache_store(model, l, (const float *)k->data, (const float *)v->data, pos);
            attn = arena_f32(arena, model->n_embd);
            if (attn == NULL) {
                return false;
            }
            attention_into(model, l, (const float *)q->data, pos, attn, arena);
            (void)snprintf(dbg, sizeof dbg, "attn_out-%d-t%zu", (int)l, pos);
            dump_f32(dbg, attn, model->n_embd);
            (void)view_1xn(&va, attn, model->n_embd);
            {
                kaze_tensor_t *o = kaze_tensor_matmul_quant(arena, &va, layer->wo);
                if (o == NULL) {
                    return false;
                }
                vec_add_into(x, (const float *)o->data, model->n_embd);
            }
            h = arena_f32(arena, model->n_embd);
            gate = arena_f32(arena, model->n_ff);
            up = arena_f32(arena, model->n_ff);
            act = arena_f32(arena, model->n_ff);
            if (h == NULL || gate == NULL || up == NULL || act == NULL) {
                return false;
            }
            rmsnorm_into(h, x, layer->ffn_norm, model->n_embd, model->eps);
            (void)view_1xn(&vg, h, model->n_embd);
            {
                kaze_tensor_t *g = kaze_tensor_matmul_quant(arena, &vg, layer->w_gate);
                kaze_tensor_t *u = kaze_tensor_matmul_quant(arena, &vg, layer->w_up);
                kaze_tensor_t *down;
                if (g == NULL || u == NULL) {
                    return false;
                }
                for (i = 0; i < model->n_ff; i++) {
                    act[i] = silu_f(((const float *)g->data)[i]) *
                             ((const float *)u->data)[i];
                }
                (void)snprintf(dbg, sizeof dbg, "ffn_gate-%d-t%zu", (int)l, pos);
                dump_f32(dbg, (const float *)g->data, model->n_ff);
                (void)snprintf(dbg, sizeof dbg, "ffn_up-%d-t%zu", (int)l, pos);
                dump_f32(dbg, (const float *)u->data, model->n_ff);
                (void)snprintf(dbg, sizeof dbg, "ffn_swiglu-%d-t%zu", (int)l, pos);
                dump_f32(dbg, act, model->n_ff);
                (void)view_1xn(&vu, act, model->n_ff);
                down = kaze_tensor_matmul_quant(arena, &vu, layer->w_down);
                if (down == NULL) {
                    return false;
                }
                vec_add_into(x, (const float *)down->data, model->n_embd);
                (void)snprintf(dbg, sizeof dbg, "l_out-%d-t%zu", (int)l, pos);
                dump_f32(dbg, x, model->n_embd);
            }
        }
        if (logits != NULL && t + 1 == n_tokens) {
            float *xn = arena_f32(arena, model->n_embd);
            kaze_tensor_t *lg;
            if (xn == NULL) {
                return false;
            }
            rmsnorm_into(xn, x, model->out_norm, model->n_embd, model->eps);
            dump_f32("result_norm", xn, model->n_embd);
            (void)view_1xn(&vn, xn, model->n_embd);
            lg = kaze_tensor_matmul_quant(arena, &vn, model->out_w);
            if (lg == NULL) {
                return false;
            }
            memcpy(logits, lg->data, (size_t)model->n_vocab * sizeof(float));
        }
    }
    model->pos = start_pos + n_tokens;
    return true;
}

