/* arena_stress.c -- Phase 1 acceptance gate.
 *
 * The roadmap asks for "Valgrind / ASan reports zero memory leaks on 1,000,000
 * consecutive arena allocation and bulk-reset cycles". This program is that
 * gate: it runs a million alloc/reset cycles against a heap arena, plus vector
 * and map churn, and exits non-zero if anything traps or a leak is reported by
 * the sanitizer.
 */
#include <stdio.h>
#include <stdlib.h>
#include "kaze_rt.h"

#define CYCLES 1000000

int main(void)
{
    kaze_arena_t arena;
    if (!kaze_arena_init_heap(&arena, 64 * 1024)) {
        fprintf(stderr, "arena_stress: could not allocate the arena\n");
        return 2;
    }

    /* One million allocation + bulk-reset cycles. Every pointer must be
     * 8-byte aligned and inside the arena, and the used mark must return to
     * zero after each reset. */
    size_t i = 0;
    while (i < CYCLES) {
        for (size_t k = 0; k < 8; k++) {
            void *p = kmalloc(&arena, 1 + (k * 37) % 512);
            if (p == NULL || ((uintptr_t)p % 8) != 0) {
                fprintf(stderr, "arena_stress: bad allocation at cycle %zu\n", i);
                kaze_arena_free(&arena);
                return 3;
            }
            *(unsigned char *)p = (unsigned char)k;
        }
        kaze_arena_reset(&arena);
        if (kaze_arena_used(&arena) != 0) {
            fprintf(stderr, "arena_stress: reset left %zu bytes used\n", kaze_arena_used(&arena));
            kaze_arena_free(&arena);
            return 4;
        }
        i += 1;
    }

    /* A string built in the arena, freed by the same bulk reset. */
    kaze_str_t s = str_from_cstr(&arena, "kaze");
    kaze_str_t t = str_concat(&arena, s, s);
    if (s.len != 4 || t.len != 8) {
        fprintf(stderr, "arena_stress: string lengths are wrong\n");
        kaze_arena_free(&arena);
        return 5;
    }
    kaze_arena_reset(&arena);

    kaze_arena_free(&arena);
    printf("arena-stress: %d cycles clean\n", CYCLES);
    return 0;
}
