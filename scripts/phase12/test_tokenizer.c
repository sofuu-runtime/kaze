/* test_tokenizer.c — Phase 12 tokenizer gate: encode/decode and reference dump. */
#include "kaze_tensor.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv)
{
    static uint8_t meta_buffer[96 * 1024 * 1024];
    static uint8_t work_buffer[16 * 1024 * 1024];
    kaze_arena_t meta, work;
    kaze_gguf_t *g;
    kaze_tokenizer_t *tok;
    int i;

    if (argc < 2) {
        fprintf(stderr, "usage: test_tokenizer <model.gguf> [text ...]\n");
        return 2;
    }
    kaze_arena_init(&meta, meta_buffer, sizeof meta_buffer);
    kaze_arena_init(&work, work_buffer, sizeof work_buffer);
    g = kaze_gguf_open(&meta, argv[1]);
    if (g == NULL) {
        fprintf(stderr, "test_tokenizer: cannot open model metadata\n");
        return 1;
    }
    tok = kaze_tokenizer_from_gguf(&meta, g);
    if (tok == NULL) {
        fprintf(stderr, "test_tokenizer: cannot build tokenizer\n");
        return 1;
    }
    printf("tokenizer: vocab=%zu bos=%d eos=%d\n",
           kaze_tokenizer_vocab_size(tok), (int)kaze_tokenizer_bos(tok),
           (int)kaze_tokenizer_eos(tok));
    for (i = 2; i < argc; i++) {
        size_t n = 0, k;
        const int32_t *ids;
        kaze_arena_reset(&work);
        ids = kaze_tokenize(&work, tok, argv[i], strlen(argv[i]), &n);
        printf("%s\t", argv[i]);
        for (k = 0; k < n; k++) {
            printf("%s%d", k ? "," : "", (int)ids[k]);
        }
        printf("\n");
        {
            char *back = kaze_tokenizer_decode(&work, tok, ids, n);
            if (back == NULL || strcmp(back, argv[i]) != 0) {
                printf("decode-mismatch\t%s\t%s\n", argv[i], back ? back : "(null)");
            }
        }
    }
    {
        char line[4096];
        while (fgets(line, sizeof line, stdin) != NULL) {
            size_t n = 0, k;
            const int32_t *ids;
            size_t len = strlen(line);
            while (len > 0 && (line[len - 1] == '\n' || line[len - 1] == '\r')) {
                line[--len] = '\0';
            }
            kaze_arena_reset(&work);
            ids = kaze_tokenize(&work, tok, line, len, &n);
            printf("%s\t", line);
            for (k = 0; k < n; k++) {
                printf("%s%d", k ? "," : "", (int)ids[k]);
            }
            printf("\n");
            {
                char *back = kaze_tokenizer_decode(&work, tok, ids, n);
                if (back == NULL || strcmp(back, line) != 0) {
                    printf("decode-mismatch\t%s\t%s\n", line, back ? back : "(null)");
                }
            }
        }
    }
    kaze_gguf_close(g);
    return 0;
}
