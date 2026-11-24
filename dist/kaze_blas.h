/* kaze_blas.h — optional platform-BLAS provider for the Kaze tensor runtime.
 *
 * Documented external provider boundary: like Metal or TLS, the platform BLAS is
 * not Kaze-owned code. The portable C99 kernel is the default (self-contained);
 * a host that links kaze_blas.c can route large GEMMs to the OS BLAS. This is
 * what the roadmap's "85% of raw BLAS throughput" gate measures on Apple Silicon,
 * where the platform BLAS uses the AMX units a NEON kernel cannot reach.
 */
#ifndef KAZE_BLAS_H
#define KAZE_BLAS_H

#include "kaze_rt.h"

#ifdef __cplusplus
extern "C" {
#endif

bool kaze_blas_available(void);
bool kaze_blas_matmul(const float *a, const float *b, float *c,
                      int64_t m, int64_t n, int64_t k);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* KAZE_BLAS_H */
