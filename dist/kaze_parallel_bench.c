/* kaze_parallel_bench.c -- Phase 7 feasibility evidence for dense matmul.
 *
 * This is not a new Kaze language feature. It measures the shape a future
 * automatic-parallelism pass would have to preserve: each output element
 * accumulates over k in the same order, so a row-partitioned pthread run is
 * bit-for-bit equal to the serial run. If that equality did not hold, the
 * candidate would not be eligible for the sequential fallback contract.
 *
 * Build/run through scripts/phase7-bench.sh. No model weights, no network.
 */
#if defined(__APPLE__)
#define _DARWIN_C_SOURCE 1
#endif
#define _POSIX_C_SOURCE 200809L
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static int g_n;
static int g_threads;
static float* a;
static float* b;
static float* out_serial;
static float* out_parallel;

typedef struct { int row0; int row1; } kz_bench_slice;

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1000.0 + (double)ts.tv_nsec / 1000000.0;
}

static void matmul_rows(int row0, int row1, float* dst) {
    int n = g_n;
    int i, j, k;
    for (i = row0; i < row1; i++) {
        for (j = 0; j < n; j++) {
            float sum = 0.0f;
            for (k = 0; k < n; k++) sum += a[i * n + k] * b[k * n + j];
            dst[i * n + j] = sum;
        }
    }
}

static void* worker(void* arg) {
    kz_bench_slice* s = (kz_bench_slice*)arg;
    matmul_rows(s->row0, s->row1, out_parallel);
    return NULL;
}

int main(int argc, char** argv) {
    int n = argc > 1 ? atoi(argv[1]) : 192;
    int threads = argc > 2 ? atoi(argv[2]) : (int)sysconf(_SC_NPROCESSORS_ONLN);
    int repeat = 3;
    int i, j, r;
    double best_serial = 1e30, best_parallel = 1e30;
    pthread_t* tids;
    kz_bench_slice* slices;
    if (n < 16) n = 16;
    if (n > 1024) n = 1024;
    if (threads < 1) threads = 1;
    if (threads > 16) threads = 16;
    g_n = n;
    g_threads = threads;
    a = (float*)malloc((size_t)n * n * sizeof(float));
    b = (float*)malloc((size_t)n * n * sizeof(float));
    out_serial = (float*)malloc((size_t)n * n * sizeof(float));
    out_parallel = (float*)malloc((size_t)n * n * sizeof(float));
    tids = (pthread_t*)malloc((size_t)threads * sizeof(pthread_t));
    slices = (kz_bench_slice*)malloc((size_t)threads * sizeof(kz_bench_slice));
    if (!a || !b || !out_serial || !out_parallel || !tids || !slices) return 2;
    for (i = 0; i < n * n; i++) {
        a[i] = (float)((i * 13 + 7) % 101) / 101.0f;
        b[i] = (float)((i * 17 + 11) % 97) / 97.0f;
    }

    for (r = 0; r < repeat; r++) {
        double t0 = now_ms();
        matmul_rows(0, n, out_serial);
        double t1 = now_ms();
        if (t1 - t0 < best_serial) best_serial = t1 - t0;
    }

    for (r = 0; r < repeat; r++) {
        int per = (n + threads - 1) / threads;
        double t0 = now_ms();
        for (i = 0; i < threads; i++) {
            int row0 = i * per;
            int row1 = row0 + per;
            if (row0 > n) row0 = n;
            if (row1 > n) row1 = n;
            slices[i].row0 = row0;
            slices[i].row1 = row1;
            pthread_create(&tids[i], NULL, worker, &slices[i]);
        }
        for (i = 0; i < threads; i++) pthread_join(tids[i], NULL);
        double t1 = now_ms();
        if (t1 - t0 < best_parallel) best_parallel = t1 - t0;
    }

    int exact = memcmp(out_serial, out_parallel, (size_t)n * n * sizeof(float)) == 0;
    printf("kaze-parallel-bench 1\n");
    printf("size %d\nthreads %d\n", n, threads);
    printf("serial-ms %.3f\nparallel-ms %.3f\n", best_serial, best_parallel);
    printf("speedup %.2f\nexact %s\n", best_parallel > 0 ? best_serial / best_parallel : 0.0,
           exact ? "yes" : "no");
    free(a); free(b); free(out_serial); free(out_parallel); free(tids); free(slices);
    return exact ? 0 : 1;
}
