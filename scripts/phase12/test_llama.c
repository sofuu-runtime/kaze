/* test_llama.c — Phase 12 inference gate: run a token sequence through the
 * Kaze llama runtime and dump the last-token logits for comparison with
 * llama.cpp (scripts/phase12/compare_logits.py).
 */
#include "kaze_tensor.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv)
{
    kaze_llama_t *model;
    int32_t *tokens;
    int32_t n_tokens = 0;
    float *logits;
    size_t n_vocab;
    FILE *out;
    int32_t argmax = 0;
    int32_t i;

    if (argc < 4) {
        fprintf(stderr, "usage: test_llama <model.gguf> <logits.bin> <id,id,...>\n");
        return 2;
    }
    tokens = (int32_t *)calloc(512, sizeof(int32_t));
    logits = NULL;
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
        fprintf(stderr, "test_llama: no tokens\n");
        return 2;
    }
    model = kaze_llama_open(argv[1]);
    if (model == NULL) {
        fprintf(stderr, "test_llama: cannot open model\n");
        return 1;
    }
    n_vocab = kaze_llama_n_vocab(model);
    logits = (float *)calloc(n_vocab, sizeof(float));
    if (logits == NULL) {
        return 2;
    }
    if (!kaze_llama_forward(model, tokens, (size_t)n_tokens, 0, logits)) {
        fprintf(stderr, "test_llama: forward failed\n");
        return 1;
    }
    for (i = 1; i < (int32_t)n_vocab; i++) {
        if (logits[i] > logits[argmax]) {
            argmax = i;
        }
    }
    out = fopen(argv[2], "wb");
    if (out == NULL) {
        fprintf(stderr, "test_llama: cannot write %s\n", argv[2]);
        return 1;
    }
    if (fwrite(&n_vocab, sizeof(int32_t), 1, out) != 1 ||
        fwrite(logits, sizeof(float), n_vocab, out) != n_vocab) {
        fclose(out);
        return 1;
    }
    fclose(out);
    printf("test_llama: tokens=%d n_vocab=%zu argmax=%d top=%.6f\n",
           n_tokens, n_vocab, argmax, (double)logits[argmax]);
    printf("test_llama: logits[0..5] =");
    for (i = 0; i < 5 && (size_t)i < n_vocab; i++) {
        printf(" %.6f", (double)logits[i]);
    }
    printf("\n");
    kaze_llama_close(model);
    free(logits);
    free(tokens);
    return 0;
}
