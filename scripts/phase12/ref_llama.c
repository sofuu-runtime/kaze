/* ref_llama.c — Phase 12 numerical-parity reference via llama.cpp.
 *
 * This links the locally built llama.cpp (an external reference implementation,
 * not part of the Kaze-owned code) and dumps the last token's logits for an
 * explicit token sequence. scripts/phase12/compare_logits.py compares that dump
 * with the Kaze runtime's logits.
 *
 * Build:
 *   cc -O2 -I ~/llama.cpp/include -I ~/llama.cpp/ggml/include ref_llama.c \
 *      -L ~/llama.cpp/build/bin -lllama -lggml -lggml-base \
 *      -Wl,-rpath,$HOME/llama.cpp/build/bin -o ref_llama
 */
#include "llama.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static bool ref_cb_eval(struct ggml_tensor *t, bool ask, void *user_data)
{
    const char *list = getenv("REF_LIST");
    const char *dir = getenv("REF_DUMP_DIR");
    const char *names = getenv("REF_DUMP_NAMES");
    (void)user_data;
    if (ask) {
        return true;
    }
    if (t == NULL || t->name == NULL) {
        return true;
    }
    if (list != NULL) {
        fprintf(stderr, "tensor: %s type=%d ne=[%lld,%lld,%lld,%lld]\n", t->name,
                (int)t->type, (long long)t->ne[0], (long long)t->ne[1],
                (long long)t->ne[2], (long long)t->ne[3]);
    }
    if (dir != NULL && names != NULL && t->type == GGML_TYPE_F32 && t->data != NULL &&
        strstr(names, t->name) != NULL) {
        char path[1024];
        FILE *f;
        size_t n = (size_t)t->ne[0] * (size_t)t->ne[1] * (size_t)t->ne[2] * (size_t)t->ne[3];
        (void)snprintf(path, sizeof path, "%s/%s.bin", dir, t->name);
        f = fopen(path, "wb");
        if (f != NULL) {
            (void)fwrite(t->data, sizeof(float), n, f);
            (void)fclose(f);
        }
    }
    return true;
}

int main(int argc, char **argv)
{
    struct llama_model *model;
    struct llama_context *ctx;
    struct llama_model_params mparams;
    struct llama_context_params cparams;
    struct llama_batch batch;
    const struct llama_vocab *vocab;
    int32_t n_vocab, n_tokens = 0, i;
    int32_t *tokens;
    float *logits;
    FILE *out;
    int32_t argmax = 0;

    if (argc < 4) {
        fprintf(stderr, "usage: ref_llama <model.gguf> <logits.bin> <id,id,...>\n");
        return 2;
    }
    tokens = (int32_t *)calloc(512, sizeof(int32_t));
    if (tokens == NULL) {
        return 2;
    }
    {
        const char *p = argv[3];
        while (*p != '\0' && n_tokens < 512) {
            char *end = NULL;
            long v = strtol(p, &end, 10);
            if (end == p) {
                break;
            }
            tokens[n_tokens++] = (int32_t)v;
            p = end;
            if (*p == ',') {
                p++;
            }
        }
    }
    if (n_tokens == 0) {
        fprintf(stderr, "ref_llama: no tokens\n");
        return 2;
    }
    llama_backend_init();
    mparams = llama_model_default_params();
    mparams.n_gpu_layers = 0;
    model = llama_model_load_from_file(argv[1], mparams);
    if (model == NULL) {
        fprintf(stderr, "ref_llama: cannot load model\n");
        return 1;
    }
    cparams = llama_context_default_params();
    cparams.n_ctx = 512;
    cparams.n_batch = 512;
    cparams.n_ubatch = 512;
    cparams.n_threads = 1;
    cparams.n_threads_batch = 1;
    cparams.flash_attn_type = LLAMA_FLASH_ATTN_TYPE_DISABLED;
    cparams.offload_kqv = false;
    cparams.cb_eval = ref_cb_eval;
    cparams.cb_eval_user_data = NULL;
    {
        const char *flash = getenv("REF_FLASH");
        const char *kv = getenv("REF_KV");
        const char *threads = getenv("REF_THREADS");
        if (flash != NULL && strcmp(flash, "on") == 0) {
            cparams.flash_attn_type = LLAMA_FLASH_ATTN_TYPE_ENABLED;
        }
        if (kv != NULL && strcmp(kv, "f32") == 0) {
            cparams.type_k = GGML_TYPE_F32;
            cparams.type_v = GGML_TYPE_F32;
        }
        if (threads != NULL) {
            cparams.n_threads = (int32_t)strtol(threads, NULL, 10);
            cparams.n_threads_batch = cparams.n_threads;
        }
    }
    ctx = llama_init_from_model(model, cparams);
    if (ctx == NULL) {
        fprintf(stderr, "ref_llama: cannot create context\n");
        return 1;
    }
    vocab = llama_model_get_vocab(model);
    n_vocab = llama_vocab_n_tokens(vocab);
    batch = llama_batch_init(n_tokens, 0, 1);
    for (i = 0; i < n_tokens; i++) {
        batch.token[i] = tokens[i];
        batch.pos[i] = i;
        batch.n_seq_id[i] = 1;
        batch.seq_id[i][0] = 0;
        batch.logits[i] = (i == n_tokens - 1) ? 1 : 0;
    }
    batch.n_tokens = n_tokens;
    if (llama_decode(ctx, batch) != 0) {
        fprintf(stderr, "ref_llama: decode failed\n");
        return 1;
    }
    logits = llama_get_logits_ith(ctx, -1);
    if (logits == NULL) {
        fprintf(stderr, "ref_llama: no logits\n");
        return 1;
    }
    for (i = 1; i < n_vocab; i++) {
        if (logits[i] > logits[argmax]) {
            argmax = i;
        }
    }
    out = fopen(argv[2], "wb");
    if (out == NULL) {
        fprintf(stderr, "ref_llama: cannot write %s\n", argv[2]);
        return 1;
    }
    if (fwrite(&n_vocab, sizeof n_vocab, 1, out) != 1 ||
        fwrite(logits, sizeof(float), (size_t)n_vocab, out) != (size_t)n_vocab) {
        fclose(out);
        fprintf(stderr, "ref_llama: short write\n");
        return 1;
    }
    fclose(out);
    printf("ref_llama: tokens=%d n_vocab=%d argmax=%d top=%.6f\n",
           n_tokens, n_vocab, argmax, (double)logits[argmax]);
    printf("ref_llama: logits[0..5] =");
    for (i = 0; i < 5 && i < n_vocab; i++) {
        printf(" %.6f", (double)logits[i]);
    }
    printf("\n");
    llama_batch_free(batch);
    llama_free(ctx);
    llama_model_free(model);
    llama_backend_free();
    free(tokens);
    return 0;
}
