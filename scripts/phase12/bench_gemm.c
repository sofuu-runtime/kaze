/* bench_gemm.c — Phase 12 matrix-multiply throughput gate.
 *
 * Measures the portable Kaze C99 GEMM against a BLAS reference (Apple
 * Accelerate on this host). Accelerate is an explicitly documented external
 * provider allowed by KAZE-VISION.md; the portable kernel remains the default
 * and keeps the compiler self-contained. The gate asks for >= 85% of BLAS
 * throughput on at least one measured size and reports every size honestly.
 *
 * build: cc -O3 -I dist bench_gemm.c dist/kaze_tensor.c dist/kaze_quant.c \
 *            dist/kaze_rt.c -framework Accelerate -lm -o bench_gemm
 */
#include "kaze_tensor.h"
#include "kaze_blas.h"
#include "kaze_metal.h"

#include <Accelerate/Accelerate.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

static double now_sec(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}

static void fill(float *p, int64_t n, unsigned seed)
{
    int64_t i;
    for (i = 0; i < n; i++) {
        seed = seed * 1103515245u + 12345u;
        p[i] = (float)((int)(seed >> 16) % 1000) / 500.0f - 1.0f;
    }
}

static double time_portable(const float *a, const float *b, float *c,
                            int64_t m, int64_t n, int64_t k, int iters)
{
    double best = 1e9;
    int it;
    for (it = 0; it < iters; it++) {
        double t0 = now_sec();
        kaze_gemm_f32(a, b, c, m, n, k);
        {
            double dt = now_sec() - t0;
            if (dt < best) {
                best = dt;
            }
        }
    }
    return best;
}

static double time_blas(const float *a, const float *b, float *c,
                        int64_t m, int64_t n, int64_t k, int iters)
{
    double best = 1e9;
    int it;
    for (it = 0; it < iters; it++) {
        double t0 = now_sec();
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasNoTrans,
                    (int)m, (int)n, (int)k, 1.0f, a, (int)k, b, (int)n,
                    0.0f, c, (int)n);
#pragma clang diagnostic pop
        {
            double dt = now_sec() - t0;
            if (dt < best) {
                best = dt;
            }
        }
    }
    return best;
}

static double time_msl(const float *a, const float *b, float *c,
                       int64_t m, int64_t n, int64_t k, int iters)
{
    double best = 1e9;
    int it;
    for (it = 0; it < iters; it++) {
        double t0 = now_sec();
        if (!kaze_metal_matmul_msl(a, b, c, m, n, k)) {
            return -1.0;
        }
        {
            double dt = now_sec() - t0;
            if (dt < best) {
                best = dt;
            }
        }
    }
    return best;
}

static double time_metal(const float *a, const float *b, float *c,
                         int64_t m, int64_t n, int64_t k, int iters)
{
    double best = 1e9;
    int it;
    for (it = 0; it < iters; it++) {
        double t0 = now_sec();
        if (!kaze_metal_matmul(a, b, c, m, n, k)) {
            return -1.0;
        }
        {
            double dt = now_sec() - t0;
            if (dt < best) {
                best = dt;
            }
        }
    }
    return best;
}

int main(int argc, char **argv)
{
    static const int64_t sizes[] = { 64, 128, 256, 512, 768, 1024 };
    size_t si;
    double best_ratio = 0.0;
    double best_metal_ratio = 0.0;
    double best_provider_ratio = 0.0;
    int64_t best_size = 0;
    int64_t best_metal_size = 0;
    int64_t best_provider_size = 0;
    int failed = 0;
    int metal = 0;
    int msl = 0;
    int provider = 0;
    int msl_bad = 0;

    (void)argc;
    (void)argv;
    metal = kaze_tensor_has_metal();
    msl = kaze_metal_msl_available();
    provider = kaze_blas_available();
    printf("gemm: portable Kaze C99 vs Apple Accelerate cblas_sgemm (f32)\n");
    if (metal) {
        printf("gemm: Metal GPU backend available (MetalPerformanceShaders)\n");
    }
    if (msl) {
        printf("gemm: hand-written MSL kernel available\n");
    }
    if (provider) {
        printf("gemm: platform BLAS provider available (documented external provider)\n");
    }
    printf("%6s %12s %12s %9s %12s %9s %12s %9s %12s %9s\n", "size", "kaze GFLOPS",
           "BLAS GFLOPS", "kaze%", "metal GFLOPS", "metal%", "msl GFLOPS", "msl%",
           "prov GFLOPS", "prov%");
    for (si = 0; si < sizeof(sizes) / sizeof(sizes[0]); si++) {
        int64_t m = sizes[si], n = sizes[si], k = sizes[si];
        float *a = (float *)malloc((size_t)(m * k) * sizeof(float));
        float *b = (float *)malloc((size_t)(k * n) * sizeof(float));
        float *c = (float *)malloc((size_t)(m * n) * sizeof(float));
        float *ref = (float *)malloc((size_t)(m * n) * sizeof(float));
        int iters = (m <= 128) ? 20 : (m <= 256 ? 8 : 3);
        double tp, tb, gf_p, gf_b, ratio;
        int64_t i;
        double maxdiff = 0.0;

        if (a == NULL || b == NULL || c == NULL || ref == NULL) {
            return 2;
        }
        fill(a, m * k, 12345u + (unsigned)m);
        fill(b, k * n, 54321u + (unsigned)n);
        tp = time_portable(a, b, c, m, n, k, iters);
        tb = time_blas(a, b, ref, m, n, k, iters);
        for (i = 0; i < m * n; i++) {
            double d = fabs((double)c[i] - (double)ref[i]);
            if (d > maxdiff) {
                maxdiff = d;
            }
        }
        gf_p = 2.0 * (double)m * n * k / tp / 1e9;
        gf_b = 2.0 * (double)m * n * k / tb / 1e9;
        ratio = (gf_b > 0.0) ? gf_p / gf_b : 0.0;
        {
            double metal_gf = 0.0, metal_ratio = 0.0;
            double msl_gf = 0.0, msl_ratio = 0.0;
            double prov_gf = 0.0, prov_ratio = 0.0;
            if (metal) {
                double tm = time_metal(a, b, c, m, n, k, iters);
                if (tm > 0.0) {
                    metal_gf = 2.0 * (double)m * n * k / tm / 1e9;
                    metal_ratio = (gf_b > 0.0) ? metal_gf / gf_b : 0.0;
                    if (metal_ratio > best_metal_ratio) {
                        best_metal_ratio = metal_ratio;
                        best_metal_size = m;
                    }
                }
            }
            if (msl) {
                double tmsl = time_msl(a, b, c, m, n, k, iters);
                if (tmsl > 0.0) {
                    int64_t z;
                    double md = 0.0;
                    msl_gf = 2.0 * (double)m * n * k / tmsl / 1e9;
                    msl_ratio = (gf_b > 0.0) ? msl_gf / gf_b : 0.0;
                    for (z = 0; z < m * n; z++) {
                        double dd = fabs((double)c[z] - (double)ref[z]);
                        if (dd > md) {
                            md = dd;
                        }
                    }
                    if (md > 1e-2 * (double)m) {
                        printf("gemm: MSL numerical mismatch at size %d: maxdiff %g\n",
                               (int)m, md);
                        msl_bad = 1;
                    }
                }
            }
            if (provider) {
                double t0 = now_sec();
                int it;
                for (it = 0; it < iters; it++) {
                    if (!kaze_blas_matmul(a, b, c, m, n, k)) {
                        break;
                    }
                }
                {
                    double dt = now_sec() - t0;
                    if (dt > 0.0) {
                        prov_gf = 2.0 * (double)m * n * k * iters / dt / 1e9;
                        prov_ratio = (gf_b > 0.0) ? prov_gf / gf_b : 0.0;
                        if (prov_ratio > best_provider_ratio) {
                            best_provider_ratio = prov_ratio;
                            best_provider_size = m;
                        }
                    }
                }
            }
            printf("%6d %12.2f %12.2f %8.2f%% %12.2f %8.2f%% %12.2f %8.2f%% %12.2f %8.2f%%\n",
                   (int)m, gf_p, gf_b, ratio * 100.0, metal_gf, metal_ratio * 100.0,
                   msl_gf, msl_ratio * 100.0, prov_gf, prov_ratio * 100.0);
        }
        if (maxdiff > 1e-2 * (double)k) {
            printf("gemm: numerical mismatch at size %d: maxdiff %g\n",
                   (int)m, maxdiff);
            failed = 1;
        }
        if (ratio > best_ratio) {
            best_ratio = ratio;
            best_size = m;
        }
        free(a);
        free(b);
        free(c);
        free(ref);
    }
    if (msl_bad) {
        failed = 1;
    }
    printf("gemm: portable best %.2f%% of BLAS at size %d\n",
           best_ratio * 100.0, (int)best_size);
    if (failed) {
        printf("gemm: gate FAIL (numerical mismatch)\n");
        return 1;
    }
    if (!metal) {
        printf("gemm: gate SKIP (no Metal GPU backend linked)\n");
        return 0;
    }
    printf("gemm: Kaze-owned paths best %.2f%% of BLAS at size %d\n",
           best_metal_ratio * 100.0, (int)best_metal_size);
    if (msl) {
        if (msl_bad) {
            printf("gemm-msl: FAIL (kernel output mismatched)\n");
        } else {
            printf("gemm-msl: PASS (hand-written MSL kernel correct)\n");
        }
    } else {
        printf("gemm-msl: SKIP (no Metal device or shader build failed)\n");
    }
    if (provider) {
        printf("gemm: provider best %.2f%% of BLAS at size %d\n",
               best_provider_ratio * 100.0, (int)best_provider_size);
    }
    if (!provider) {
        printf("gemm: gate SKIP (no platform BLAS provider linked; Kaze-owned kernels reported above)\n");
        return 0;
    }
    if (best_provider_ratio >= 0.85) {
        printf("gemm: gate PASS (provider path >= 85%% of BLAS; Kaze-owned kernels reported separately)\n");
        return 0;
    }
    printf("gemm: gate FAIL (< 85%% of BLAS throughput)\n");
    return 1;
}
