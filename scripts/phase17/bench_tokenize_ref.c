/* Phase 17 benchmark reference: single-pass JSON/text tokenizer, C99. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

static double scan(const char *buf, size_t n) {
    size_t i = 0;
    uint64_t tokens = 0, hash = 0;
    int in_string = 0;
    while (i < n) {
        unsigned char c = (unsigned char)buf[i];
        if (c == '"') {
            tokens += 1;
            in_string = !in_string;
        } else if (in_string) {
            hash = (hash * 33 + (uint64_t)c) & 0xffffffffULL;
        } else if (c == '{' || c == '}' || c == '[' || c == ']' || c == ',' || c == ':') {
            tokens += 1;
        }
        i++;
    }
    return (double)tokens * 1000003.0 + (double)hash;
}

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    FILE *f = fopen(argv[1], "rb");
    if (!f) return 2;
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    char *buf = (char *)malloc((size_t)n);
    if (!buf) return 2;
    if (fread(buf, 1, (size_t)n, f) != (size_t)n) return 2;
    fclose(f);
    printf("%.17g\n", scan(buf, (size_t)n));
    free(buf);
    return 0;
}
