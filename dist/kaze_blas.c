/* kaze_blas.c — optional platform-BLAS GEMM provider (Apple Accelerate here).
 *
 * Build only for hosts that want it:
 *   cc -O3 -I dist kaze_blas.c -framework Accelerate -o app ...
 * Without this file the weak imports in kaze_blas.h resolve to NULL and the
 * portable kernel stays in charge.
 */
#include "kaze_blas.h"

#if defined(__APPLE__)
#include <Accelerate/Accelerate.h>

bool kaze_blas_available(void)
{
    return true;
}

bool kaze_blas_matmul(const float *a, const float *b, float *c,
                      int64_t m, int64_t n, int64_t k)
{
    if (m <= 0 || n <= 0 || k <= 0 || a == NULL || b == NULL || c == NULL) {
        return false;
    }
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasNoTrans,
                (int)m, (int)n, (int)k, 1.0f, a, (int)k, b, (int)n,
                0.0f, c, (int)n);
#pragma clang diagnostic pop
    return true;
}

#else

bool kaze_blas_available(void)
{
    return false;
}

bool kaze_blas_matmul(const float *a, const float *b, float *c,
                      int64_t m, int64_t n, int64_t k)
{
    (void)a;
    (void)b;
    (void)c;
    (void)m;
    (void)n;
    (void)k;
    return false;
}

#endif
