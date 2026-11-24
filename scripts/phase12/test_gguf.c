/* test_gguf.c — Phase 12 GGUF ingestion gate: open a real local model, read
 * metadata, confirm the tensor table and dtypes, and sample a dequantized row.
 */
#include "kaze_rt.h"
#include "kaze_tensor.h"
#include "kaze_quant.h"

#include <stdio.h>
#include <string.h>

int main(int argc, char **argv)
{
    static uint8_t meta_buffer[16 * 1024 * 1024];
    static uint8_t work_buffer[32 * 1024 * 1024];
    kaze_arena_t meta, work;
    kaze_gguf_t *g;
    const char *arch = NULL;
    int64_t blocks = 0, embd = 0, heads = 0, heads_kv = 0, ffn = 0, vocab = 0;
    size_t i;
    int failures = 0;

    if (argc < 2) {
        printf("usage: test_gguf <model.gguf> [tensor-name]\n");
        return 2;
    }
    kaze_arena_init(&meta, meta_buffer, sizeof meta_buffer);
    kaze_arena_init(&work, work_buffer, sizeof work_buffer);
    g = kaze_gguf_open(&meta, argv[1]);
    if (g == NULL) {
        printf("FAIL open %s\n", argv[1]);
        return 1;
    }
    printf("gguf: tensors=%zu metadata=%zu\n",
           kaze_gguf_tensor_count(g), kaze_gguf_meta_count(g));
    if (!kaze_gguf_meta_str(g, "general.architecture", &arch)) {
        printf("FAIL architecture metadata\n");
        failures++;
    } else {
        printf("gguf: architecture=%s\n", arch);
    }
    (void)kaze_gguf_meta_i64(g, "llama.block_count", &blocks);
    (void)kaze_gguf_meta_i64(g, "llama.embedding_length", &embd);
    (void)kaze_gguf_meta_i64(g, "llama.attention.head_count", &heads);
    (void)kaze_gguf_meta_i64(g, "llama.attention.head_count_kv", &heads_kv);
    (void)kaze_gguf_meta_i64(g, "llama.feed_forward_length", &ffn);
    (void)kaze_gguf_meta_i64(g, "llama.vocab_size", &vocab);
    printf("gguf: layers=%lld embd=%lld heads=%lld kv=%lld ff=%lld vocab=%lld\n",
           (long long)blocks, (long long)embd, (long long)heads,
           (long long)heads_kv, (long long)ffn, (long long)vocab);
    if (blocks == 0 || embd == 0 || heads == 0 || vocab == 0) {
        printf("FAIL expected llama hyperparameters\n");
        failures++;
    }
    for (i = 0; i < kaze_gguf_tensor_count(g) && i < 5; i++) {
        const kaze_tensor_t *t = kaze_gguf_tensor_at(g, i);
        printf("gguf: tensor[%zu] %s %s rank=%d shape=[", i, t->name,
               kaze_dtype_name(t->dtype), (int)t->rank);
        {
            int32_t d;
            for (d = 0; d < t->rank; d++) {
                printf("%lld%s", (long long)t->shape[d], d + 1 < t->rank ? "," : "");
            }
        }
        printf("] bytes=%zu\n", t->nbytes);
    }
    {
        const kaze_tensor_t *w = (argc > 2) ? kaze_gguf_tensor(g, argv[2])
                                            : kaze_gguf_tensor(g, "token_embd.weight");
        if (w == NULL) {
            printf("FAIL tensor not found\n");
            failures++;
        } else {
            kaze_tensor_t *row;
            int64_t rshape[2];
            printf("gguf: sampled %s %s rank=%d nbytes=%zu\n", w->name,
                   kaze_dtype_name(w->dtype), (int)w->rank, w->nbytes);
            if (w->rank != 2 || w->dtype == KAZE_DTYPE_F32) {
                row = kaze_quant_dequant(&work, w);
            } else {
                rshape[0] = 1;
                rshape[1] = w->shape[1];
                row = kaze_tensor_alloc(&work, KAZE_DTYPE_F32, 2, rshape);
                if (row != NULL) {
                    kaze_quant_dequant_row(w, 0, (float *)row->data, rshape[1]);
                }
            }
            if (row == NULL) {
                printf("FAIL dequant allocation\n");
                failures++;
            } else {
                printf("gguf: dequant[0]=%.9g dequant[1]=%.9g dequant[2]=%.9g\n",
                       (double)kaze_tensor_get(row, 0),
                       (double)kaze_tensor_get(row, 1),
                       (double)kaze_tensor_get(row, 2));
            }
        }
    }
    kaze_gguf_close(g);
    printf("test_gguf: %s\n", failures == 0 ? "OK" : "FAILED");
    return failures == 0 ? 0 : 1;
}
