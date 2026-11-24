/* kaze_metal.m — MetalPerformanceShaders GEMM for the Kaze tensor runtime.
 *
 * Build (only for hosts that want the optional GPU path):
 *   cc -O3 -fobjc-arc -I dist kaze_metal.m -framework Metal \
 *      -framework MetalPerformanceShaders -framework Foundation -o app ...
 *
 * MPSMatrixMultiplication is Apple's tuned GPU GEMM. It is the Metal backend the
 * Phase 12 plan asks for; a hand-written MSL kernel can replace this later
 * without changing the kaze_tensor API.
 */
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <MetalPerformanceShaders/MetalPerformanceShaders.h>

#include <stdbool.h>
#include <stdint.h>
#include <string.h>

#include "kaze_metal.h"

bool kaze_tensor_has_metal(void);

static id<MTLDevice> g_device = nil;
static id<MTLCommandQueue> g_queue = nil;

static void kaze_metal_init_once(void)
{
    if (g_device == nil) {
        g_device = MTLCreateSystemDefaultDevice();
    }
    if (g_device != nil && g_queue == nil) {
        g_queue = [g_device newCommandQueue];
    }
}

bool kaze_metal_available(void)
{
    @autoreleasepool {
        kaze_metal_init_once();
        return g_device != nil;
    }
}

bool kaze_tensor_has_metal(void)
{
    return kaze_metal_available();
}

bool kaze_metal_matmul(const float *a, const float *b, float *c,
                       int64_t m, int64_t n, int64_t k)
{
    @autoreleasepool {
        id<MTLDevice> device;
        id<MTLCommandQueue> queue;
        id<MTLBuffer> buf_a;
        id<MTLBuffer> buf_b;
        id<MTLBuffer> buf_c;
        MPSMatrixDescriptor *desc_a;
        MPSMatrixDescriptor *desc_b;
        MPSMatrixDescriptor *desc_c;
        MPSMatrix *mat_a;
        MPSMatrix *mat_b;
        MPSMatrix *mat_c;
        MPSMatrixMultiplication *mul;
        id<MTLCommandBuffer> cmd;

        if (m <= 0 || n <= 0 || k <= 0 || a == NULL || b == NULL || c == NULL) {
            return false;
        }
        kaze_metal_init_once();
        device = g_device;
        queue = g_queue;
        if (device == nil || queue == nil) {
            return false;
        }
        buf_a = [device newBufferWithBytes:a
                                   length:(NSUInteger)((size_t)m * (size_t)k * sizeof(float))
                                  options:MTLResourceStorageModeShared];
        buf_b = [device newBufferWithBytes:b
                                   length:(NSUInteger)((size_t)k * (size_t)n * sizeof(float))
                                  options:MTLResourceStorageModeShared];
        buf_c = [device newBufferWithLength:(NSUInteger)((size_t)m * (size_t)n * sizeof(float))
                                    options:MTLResourceStorageModeShared];
        if (buf_a == nil || buf_b == nil || buf_c == nil) {
            return false;
        }
        desc_a = [MPSMatrixDescriptor matrixDescriptorWithRows:(NSUInteger)m
                                                       columns:(NSUInteger)k
                                                      rowBytes:(NSUInteger)((size_t)k * sizeof(float))
                                                      dataType:MPSDataTypeFloat32];
        desc_b = [MPSMatrixDescriptor matrixDescriptorWithRows:(NSUInteger)k
                                                       columns:(NSUInteger)n
                                                      rowBytes:(NSUInteger)((size_t)n * sizeof(float))
                                                      dataType:MPSDataTypeFloat32];
        desc_c = [MPSMatrixDescriptor matrixDescriptorWithRows:(NSUInteger)m
                                                       columns:(NSUInteger)n
                                                      rowBytes:(NSUInteger)((size_t)n * sizeof(float))
                                                      dataType:MPSDataTypeFloat32];
        mat_a = [[MPSMatrix alloc] initWithBuffer:buf_a descriptor:desc_a];
        mat_b = [[MPSMatrix alloc] initWithBuffer:buf_b descriptor:desc_b];
        mat_c = [[MPSMatrix alloc] initWithBuffer:buf_c descriptor:desc_c];
        mul = [[MPSMatrixMultiplication alloc]
                  initWithDevice:device
                     resultRows:(NSUInteger)m
                  resultColumns:(NSUInteger)n
                  interiorColumns:(NSUInteger)k];
        cmd = [queue commandBuffer];
        [mul encodeToCommandBuffer:cmd leftMatrix:mat_a rightMatrix:mat_b resultMatrix:mat_c];
        [cmd commit];
        [cmd waitUntilCompleted];
        if (cmd.status != MTLCommandBufferStatusCompleted) {
            return false;
        }
        memcpy(c, buf_c.contents, (size_t)m * (size_t)n * sizeof(float));
        return true;
    }
}

/* ------------------------------------------------------------------------- */
/* Hand-written Metal Shading Language GEMM                                   */
/* ------------------------------------------------------------------------- */

static const char *kaze_gemm_msl_source =
    "#include <metal_stdlib>\n"
    "using namespace metal;\n"
    "#define KAZE_TS 64\n"
    "#define KAZE_TK 16\n"
    "kernel void kaze_sgemm(const device float* A [[buffer(0)]],\n"
    "                       const device float* B [[buffer(1)]],\n"
    "                       device float* C [[buffer(2)]],\n"
    "                       constant int& M [[buffer(3)]],\n"
    "                       constant int& N [[buffer(4)]],\n"
    "                       constant int& K [[buffer(5)]],\n"
    "                       uint2 tgid [[threadgroup_position_in_grid]],\n"
    "                       uint2 tid [[thread_position_in_threadgroup]]) {\n"
    "    threadgroup float As[KAZE_TK][KAZE_TS];\n"
    "    threadgroup float Bs[KAZE_TK][KAZE_TS];\n"
    "    const uint tx = tid.x; const uint ty = tid.y;\n"
    "    const uint lane = ty * 16 + tx;\n"
    "    const int row0 = (int)(tgid.y * KAZE_TS) + (int)ty * 4;\n"
    "    const int col0 = (int)(tgid.x * KAZE_TS) + (int)tx * 4;\n"
    "    float a00 = 0.0f, a01 = 0.0f, a02 = 0.0f, a03 = 0.0f;\n"
    "    float a10 = 0.0f, a11 = 0.0f, a12 = 0.0f, a13 = 0.0f;\n"
    "    float a20 = 0.0f, a21 = 0.0f, a22 = 0.0f, a23 = 0.0f;\n"
    "    float a30 = 0.0f, a31 = 0.0f, a32 = 0.0f, a33 = 0.0f;\n"
    "    for (int k0 = 0; k0 < K; k0 += KAZE_TK) {\n"
    "        for (int i = (int)lane; i < KAZE_TS * KAZE_TK; i += 256) {\n"
    "            int r = i / KAZE_TK; int c = i - r * KAZE_TK;\n"
    "            int gr = (int)(tgid.y * KAZE_TS) + r; int gc = k0 + c;\n"
    "            As[c][r] = (gr < M && gc < K) ? A[gr * K + gc] : 0.0f;\n"
    "        }\n"
    "        for (int i = (int)lane; i < KAZE_TK * KAZE_TS; i += 256) {\n"
    "            int r = i / KAZE_TS; int c = i - r * KAZE_TS;\n"
    "            int gr = k0 + r; int gc = (int)(tgid.x * KAZE_TS) + c;\n"
    "            Bs[r][c] = (gr < K && gc < N) ? B[gr * N + gc] : 0.0f;\n"
    "        }\n"
    "        threadgroup_barrier(mem_flags::mem_threadgroup);\n"
    "        for (int kk = 0; kk < KAZE_TK; kk++) {\n"
    "            float x0 = As[kk][ty * 4 + 0]; float x1 = As[kk][ty * 4 + 1];\n"
    "            float x2 = As[kk][ty * 4 + 2]; float x3 = As[kk][ty * 4 + 3];\n"
    "            float y0 = Bs[kk][tx * 4 + 0]; float y1 = Bs[kk][tx * 4 + 1];\n"
    "            float y2 = Bs[kk][tx * 4 + 2]; float y3 = Bs[kk][tx * 4 + 3];\n"
    "            a00 += x0 * y0; a01 += x0 * y1; a02 += x0 * y2; a03 += x0 * y3;\n"
    "            a10 += x1 * y0; a11 += x1 * y1; a12 += x1 * y2; a13 += x1 * y3;\n"
    "            a20 += x2 * y0; a21 += x2 * y1; a22 += x2 * y2; a23 += x2 * y3;\n"
    "            a30 += x3 * y0; a31 += x3 * y1; a32 += x3 * y2; a33 += x3 * y3;\n"
    "        }\n"
    "        threadgroup_barrier(mem_flags::mem_threadgroup);\n"
    "    }\n"
    "    if (row0 + 0 < M && col0 + 0 < N) C[(row0 + 0) * N + col0 + 0] = a00;\n"
    "    if (row0 + 0 < M && col0 + 1 < N) C[(row0 + 0) * N + col0 + 1] = a01;\n"
    "    if (row0 + 0 < M && col0 + 2 < N) C[(row0 + 0) * N + col0 + 2] = a02;\n"
    "    if (row0 + 0 < M && col0 + 3 < N) C[(row0 + 0) * N + col0 + 3] = a03;\n"
    "    if (row0 + 1 < M && col0 + 0 < N) C[(row0 + 1) * N + col0 + 0] = a10;\n"
    "    if (row0 + 1 < M && col0 + 1 < N) C[(row0 + 1) * N + col0 + 1] = a11;\n"
    "    if (row0 + 1 < M && col0 + 2 < N) C[(row0 + 1) * N + col0 + 2] = a12;\n"
    "    if (row0 + 1 < M && col0 + 3 < N) C[(row0 + 1) * N + col0 + 3] = a13;\n"
    "    if (row0 + 2 < M && col0 + 0 < N) C[(row0 + 2) * N + col0 + 0] = a20;\n"
    "    if (row0 + 2 < M && col0 + 1 < N) C[(row0 + 2) * N + col0 + 1] = a21;\n"
    "    if (row0 + 2 < M && col0 + 2 < N) C[(row0 + 2) * N + col0 + 2] = a22;\n"
    "    if (row0 + 2 < M && col0 + 3 < N) C[(row0 + 2) * N + col0 + 3] = a23;\n"
    "    if (row0 + 3 < M && col0 + 0 < N) C[(row0 + 3) * N + col0 + 0] = a30;\n"
    "    if (row0 + 3 < M && col0 + 1 < N) C[(row0 + 3) * N + col0 + 1] = a31;\n"
    "    if (row0 + 3 < M && col0 + 2 < N) C[(row0 + 3) * N + col0 + 2] = a32;\n"
    "    if (row0 + 3 < M && col0 + 3 < N) C[(row0 + 3) * N + col0 + 3] = a33;\n"
    "}\n";

static id<MTLComputePipelineState> g_msl_pipeline = nil;
static int g_msl_state = 0; /* 0 untried, 1 ready, -1 unavailable */

static bool kaze_metal_msl_init(void)
{
    NSError *error = nil;
    id<MTLLibrary> library;
    id<MTLFunction> function;

    if (g_msl_state != 0) {
        return g_msl_state == 1;
    }
    kaze_metal_init_once();
    if (g_device == nil) {
        g_msl_state = -1;
        return false;
    }
    library = [g_device newLibraryWithSource:[NSString stringWithUTF8String:kaze_gemm_msl_source]
                                     options:nil
                                       error:&error];
    if (library == nil) {
        g_msl_state = -1;
        return false;
    }
    function = [library newFunctionWithName:@"kaze_sgemm"];
    if (function == nil) {
        g_msl_state = -1;
        return false;
    }
    g_msl_pipeline = [g_device newComputePipelineStateWithFunction:function error:&error];
    if (g_msl_pipeline == nil) {
        g_msl_state = -1;
        return false;
    }
    g_msl_state = 1;
    return true;
}

bool kaze_metal_msl_available(void)
{
    @autoreleasepool {
        return kaze_metal_msl_init();
    }
}

static id<MTLBuffer> kaze_msl_buffer(id<MTLDevice> device, const void *bytes, size_t n)
{
    if (bytes != NULL) {
        return [device newBufferWithBytes:bytes length:n options:MTLResourceStorageModeShared];
    }
    return [device newBufferWithLength:n options:MTLResourceStorageModeShared];
}

bool kaze_metal_matmul_msl(const float *a, const float *b, float *c,
                           int64_t m, int64_t n, int64_t k)
{
    @autoreleasepool {
        id<MTLBuffer> buf_a;
        id<MTLBuffer> buf_b;
        id<MTLBuffer> buf_c;
        id<MTLCommandBuffer> cmd;
        id<MTLComputeCommandEncoder> enc;
        int dim_m = (int)m, dim_n = (int)n, dim_k = (int)k;
        MTLSize groups;
        MTLSize threads;

        if (m <= 0 || n <= 0 || k <= 0 || a == NULL || b == NULL || c == NULL) {
            return false;
        }
        if (!kaze_metal_msl_init()) {
            return false;
        }
        buf_a = kaze_msl_buffer(g_device, a, (size_t)m * (size_t)k * sizeof(float));
        buf_b = kaze_msl_buffer(g_device, b, (size_t)k * (size_t)n * sizeof(float));
        buf_c = kaze_msl_buffer(g_device, NULL, (size_t)m * (size_t)n * sizeof(float));
        if (buf_a == nil || buf_b == nil || buf_c == nil) {
            return false;
        }
        cmd = [g_queue commandBuffer];
        enc = [cmd computeCommandEncoder];
        [enc setComputePipelineState:g_msl_pipeline];
        [enc setBuffer:buf_a offset:0 atIndex:0];
        [enc setBuffer:buf_b offset:0 atIndex:1];
        [enc setBuffer:buf_c offset:0 atIndex:2];
        [enc setBytes:&dim_m length:sizeof dim_m atIndex:3];
        [enc setBytes:&dim_n length:sizeof dim_n atIndex:4];
        [enc setBytes:&dim_k length:sizeof dim_k atIndex:5];
        groups = MTLSizeMake((NSUInteger)((n + 63) / 64), (NSUInteger)((m + 63) / 64), 1);
        threads = MTLSizeMake(16, 16, 1);
        [enc dispatchThreadgroups:groups threadsPerThreadgroup:threads];
        [enc endEncoding];
        [cmd commit];
        [cmd waitUntilCompleted];
        if (cmd.status != MTLCommandBufferStatusCompleted) {
            return false;
        }
        memcpy(c, buf_c.contents, (size_t)m * (size_t)n * sizeof(float));
        return true;
    }
}
