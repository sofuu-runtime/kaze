/* ref_tokenize.c — llama.cpp tokenizer reference for Phase 12 parity checks.
 * Prints "text<TAB>id,id,..." for each argument or stdin line, so a script can
 * compare the Kaze tokenizer against the reference implementation.
 */
#include "llama.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv)
{
    struct llama_model *model;
    struct llama_model_params mparams;
    const struct llama_vocab *vocab;
    llama_token tokens[512];
    char line[4096];
    int i;

    if (argc < 2) {
        fprintf(stderr, "usage: ref_tokenize <model.gguf> [text ...]\n");
        return 2;
    }
    llama_backend_init();
    mparams = llama_model_default_params();
    mparams.n_gpu_layers = 0;
    mparams.vocab_only = true;
    model = llama_model_load_from_file(argv[1], mparams);
    if (model == NULL) {
        fprintf(stderr, "ref_tokenize: cannot load model\n");
        return 1;
    }
    vocab = llama_model_get_vocab(model);
    for (i = 2; i < argc; i++) {
        int32_t n = llama_tokenize(vocab, argv[i], (int32_t)strlen(argv[i]),
                                   tokens, 512, false, false);
        int32_t k;
        printf("%s", argv[i]);
        printf("\t");
        for (k = 0; k < n; k++) {
            printf("%s%d", k ? "," : "", (int)tokens[k]);
        }
        printf("\n");
    }
    while (fgets(line, sizeof line, stdin) != NULL) {
        size_t len = strlen(line);
        int32_t n, k;
        while (len > 0 && (line[len - 1] == '\n' || line[len - 1] == '\r')) {
            line[--len] = '\0';
        }
        n = llama_tokenize(vocab, line, (int32_t)len, tokens, 512, false, false);
        printf("%s", line);
        printf("\t");
        for (k = 0; k < n; k++) {
            printf("%s%d", k ? "," : "", (int)tokens[k]);
        }
        printf("\n");
    }
    llama_model_free(model);
    llama_backend_free();
    return 0;
}
