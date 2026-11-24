/* test_metal_msl.c — correctness gate for the hand-written MSL GEMM kernel. */
#include "kaze_tensor.h"
#include "kaze_metal.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>

static void fill(float *p, int64_t n, unsigned seed)
{
    int64_t i;
    for (i = 0; i < n; i++) {
        seed = seed * 1103515245u + 12345u;
        p[i] = (float)((int)(seed >> 16) % 2000) / 1000.0f - 1.0f;
    }
}

static int run(int64_t m, int64_t n, int64_t k)
{
    float *a = (float *)malloc((size_t)(m * k) * sizeof(float));
    float *b = (float *)malloc((size_t)(k * n) * sizeof(float));
    float *c = (float *)malloc((size_t)(m * n) * sizeof(float));
    float *ref = (float *)malloc((size_t)(m * n) * sizeof(float));
    int64_t i;
    double maxdiff = 0.0;

    if (a == NULL || b == NULL || c == NULL || ref == NULL) {
        return 1;
    }
    fill(a, m * k, (unsigned)(m + 7));
    fill(b, k * n, (unsigned)(n + 13));
    kaze_gemm_f32(a, b, ref, m, n, k);
    if (!kaze_metal_matmul_msl(a, b, c, m, n, k)) {
        printf("FAIL msl dispatch %dx%dx%d\n", (int)m, (int)n, (int)k);
        return 1;
    }
    for (i = 0; i < m * n; i++) {
        double d = fabs((double)c[i] - (double)ref[i]);
        if (d > maxdiff) {
            maxdiff = d;
        }
    }
    printf("msl %4dx%4dx%4d maxdiff %.4g %s\n", (int)m, (int)n, (int)k,
           maxdiff, (maxdiff < 1e-2) ? "OK" : "FAIL");
    free(a);
    free(b);
    free(c);
    free(ref);
    return maxdiff < 1e-2 ? 0 : 1;
}

int main(void)
{
    int failures = 0;
    if (!kaze_metal_msl_available()) {
        printf("SKIP: no Metal device or shader build failed\n");
        return 77;
    }
    failures += run(16, 16, 16);
    failures += run(64, 64, 64);
    failures += run(100, 70, 55);
    failures += run(256, 256, 256);
    printf("test_metal_msl: %s\n", failures == 0 ? "OK" : "FAILED");
    return failures == 0 ? 0 : 1;
}
