/* kaze_quant.h — internal quantization helpers for the Phase 12 tensor runtime.
 *
 * Not part of the public Kaze surface. The block layouts match GGUF (llama.cpp)
 * exactly, because the gate accepts GGUF files produced for llama.cpp.
 */
#ifndef KAZE_QUANT_H
#define KAZE_QUANT_H

#include "kaze_tensor.h"

#ifdef __cplusplus
extern "C" {
#endif

#define KAZE_QK8_0 32
#define KAZE_QK4_0 32
#define KAZE_QK_K 256

/* IEEE half and bfloat16 conversions (round-to-nearest-even for f16). */
float kaze_f16_to_f32(uint16_t h);
uint16_t kaze_f32_to_f16(float f);
float kaze_bf16_to_f32(uint16_t b);
uint16_t kaze_f32_to_bf16(float f);

/* GGUF ggml_type value -> Kaze dtype. Returns false for an unsupported type. */
bool kaze_dtype_from_gguf(uint32_t ggml_type, kaze_dtype_t *out);

/* Dequantize a quantized/dense tensor into a fresh dense F32 arena tensor. */
kaze_tensor_t *kaze_quant_dequant(kaze_arena_t *arena, const kaze_tensor_t *t);
/* Dequantize one row of n elements into out[n]. */
void kaze_quant_dequant_row(const kaze_tensor_t *t, int64_t row,
                            float *out, int64_t n);
/* Dot product of quantized row "row" with dense x[n]. */
float kaze_quant_row_dot(const kaze_tensor_t *t, int64_t row,
                                const float *x, int64_t n);

/* Q8_1 activation quantization, matching llama.cpp's quantize_row_q8_1_ref:
 * per 32-element block, d = amax/127 rounded through fp16, q = round(x/d).
 * Used by the quantized matmul path so the Kaze logits and llama.cpp's logits
 * come from the same integer dot product. */
#define KAZE_Q8_1_BLOCK_BYTES 34u
size_t kaze_q8_1_bytes(int64_t n);
void kaze_quantize_q8_1(const float *x, int64_t n, uint8_t *out);
float kaze_quant_q8_0_q8_1_dot(const kaze_tensor_t *w, int64_t row,
                               const uint8_t *q81, int64_t n);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* KAZE_QUANT_H */
