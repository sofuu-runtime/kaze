/* test_safetensors.c — Phase 12 SafeTensors zero-copy ingestion gate.
 *
 * Reads a tiny fixture written by scripts/phase12/make_safetensors_fixture.py:
 * two tensors (f32 [2,3] and f16 [3]) in one mmap'd file, and checks that the
 * header parser found them with the right dtype, shape and values.
 */
#include "kaze_tensor.h"

#include <math.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv)
{
    static uint8_t meta_buffer[8 * 1024 * 1024];
    kaze_arena_t meta;
    kaze_gguf_t *g;
    const kaze_tensor_t *a;
    const kaze_tensor_t *b;
    int failures = 0;
    int i;

    if (argc < 2) {
        fprintf(stderr, "usage: test_safetensors <fixture.safetensors>\n");
        return 2;
    }
    kaze_arena_init(&meta, meta_buffer, sizeof meta_buffer);
    g = kaze_safetensors_open(&meta, argv[1]);
    if (g == NULL) {
        printf("FAIL open\n");
        return 1;
    }
    if (kaze_gguf_tensor_count(g) != 2) {
        printf("FAIL tensor count %zu\n", kaze_gguf_tensor_count(g));
        failures++;
    }
    a = kaze_gguf_tensor(g, "a");
    b = kaze_gguf_tensor(g, "b");
    if (a == NULL || a->dtype != KAZE_DTYPE_F32 || a->rank != 2 ||
        a->shape[0] != 2 || a->shape[1] != 3) {
        printf("FAIL tensor a shape/dtype\n");
        failures++;
    }
    if (b == NULL || b->dtype != KAZE_DTYPE_F16 || b->rank != 1 || b->shape[0] != 3) {
        printf("FAIL tensor b shape/dtype\n");
        failures++;
    }
    if (a != NULL) {
        for (i = 0; i < 6; i++) {
            float want = (float)(i + 1) * 0.5f;
            float got = kaze_tensor_get(a, i);
            if (fabsf(got - want) > 1e-6f) {
                printf("FAIL a[%d] got %.6f want %.6f\n", i, (double)got, (double)want);
                failures++;
            }
        }
    }
    if (b != NULL) {
        float want[3] = { 1.5f, -2.25f, 0.75f };
        for (i = 0; i < 3; i++) {
            float got = kaze_tensor_get(b, i);
            if (fabsf(got - want[i]) > 1e-3f) {
                printf("FAIL b[%d] got %.6f want %.6f\n", i, (double)got, (double)want[i]);
                failures++;
            }
        }
    }
    kaze_gguf_close(g);
    printf("test_safetensors: %s\n", failures == 0 ? "OK" : "FAILED");
    return failures == 0 ? 0 : 1;
}
