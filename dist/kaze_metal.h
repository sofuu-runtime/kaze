/* kaze_metal.h — optional Metal GPU backend for the Kaze tensor runtime.
 *
 * This is an explicit, documented external provider boundary (Apple Metal /
 * MetalPerformanceShaders), not Kaze-owned code: the portable C99 kernels remain
 * the default and keep a compiled Kaze program self-contained. A host that
 * links kaze_metal.m can dispatch large GEMMs to the GPU.
 */
#ifndef KAZE_METAL_H
#define KAZE_METAL_H

#include "kaze_rt.h"

#ifdef __cplusplus
extern "C" {
#endif

/* A binary that does not link kaze_metal.m does not call these; the runtime's
 * portable kernels are the default. */
bool kaze_metal_available(void);
bool kaze_metal_matmul(const float *a, const float *b, float *c,
                       int64_t m, int64_t n, int64_t k);
/* Hand-written Metal Shading Language GEMM: the Phase 12 kernel, compiled from
 * source with the Metal compiler at runtime. Returns false when no GPU or when
 * the shader failed to build. */
bool kaze_metal_matmul_msl(const float *a, const float *b, float *c,
                           int64_t m, int64_t n, int64_t k);
bool kaze_metal_msl_available(void);
bool kaze_tensor_has_metal(void);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* KAZE_METAL_H */
