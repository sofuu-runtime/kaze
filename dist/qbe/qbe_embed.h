/* qbe_embed.h -- in-process QBE entry point for the Kaze native backend. */
#ifndef KAZE_QBE_EMBED_H
#define KAZE_QBE_EMBED_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Compile QBE IL in `ssa` to assembly for `target`.  On success returns 0 and
 * stores a malloc'd NUL-terminated buffer in *asm_out and its byte length in
 * *asm_len.  Returns 2 for an unknown target.  QBE's parser exits the process
 * on malformed IL; the Kaze generator only emits valid IL. */
int qbe_compile_buffer(const char *ssa, size_t ssa_len, const char *target,
                       char **asm_out, size_t *asm_len);

#ifdef __cplusplus
}
#endif

#endif
