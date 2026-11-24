/* kaze_profile.c -- opt-in call profiler for Kaze programs compiled to C.
 *
 * The compiler emits ordinary C, so the profiler does not need a compiler
 * change: compile the emitted file with `-finstrument-functions` and link this
 * shim. Every function entry/exit is recorded by address; `scripts/kaze-profile.sh`
 * resolves the addresses with `atos` and prints the table.
 *
 * Limits, stated rather than hidden:
 *   * counts and times are per generated C function, which is one Kaze routine;
 *   * recursion is counted per call, not collapsed;
 *   * the table is fixed at 4096 distinct functions and the call stack at 4096
 *     frames; anything beyond that is ignored and reported at exit;
 *   * timing is CLOCK_MONOTONIC and includes instrumentation overhead, so a
 *     function that runs for nanoseconds is mostly overhead.
 *
 * Marked `no_instrument_function` so the shim does not profile itself.
 */
#if defined(__APPLE__)
#define _DARWIN_C_SOURCE 1
#endif
#define _POSIX_C_SOURCE 200809L
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <dlfcn.h>

#define KZ_PROFILE_SLOTS 4096
#define KZ_PROFILE_DEPTH 4096

typedef struct {
    void* addr;
    unsigned long long calls;
    unsigned long long incl_ns;
    unsigned long long excl_ns;
} kz_profile_entry;

typedef struct {
    void* addr;
    unsigned long long start_ns;
    unsigned long long child_ns;
} kz_profile_frame;

static kz_profile_entry kz_entries[KZ_PROFILE_SLOTS];
static size_t kz_entry_count;
static kz_profile_frame kz_stack[KZ_PROFILE_DEPTH];
static size_t kz_depth;
static unsigned long long kz_dropped_entries;
static unsigned long long kz_dropped_frames;
static int kz_registered;

__attribute__((no_instrument_function))
static unsigned long long kz_now_ns(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) return 0;
    return (unsigned long long)ts.tv_sec * 1000000000ull + (unsigned long long)ts.tv_nsec;
}

__attribute__((no_instrument_function))
static kz_profile_entry* kz_find(void* addr) {
    size_t start = ((uintptr_t)addr >> 4) % KZ_PROFILE_SLOTS;
    size_t i;
    for (i = 0; i < KZ_PROFILE_SLOTS; i++) {
        size_t at = (start + i) % KZ_PROFILE_SLOTS;
        if (kz_entries[at].addr == addr) return &kz_entries[at];
        if (kz_entries[at].addr == NULL) {
            kz_entries[at].addr = addr;
            kz_entry_count++;
            return &kz_entries[at];
        }
    }
    kz_dropped_entries++;
    return NULL;
}

__attribute__((no_instrument_function))
static void kz_profile_dump(void) {
    const char* path = getenv("KAZE_PROFILE_OUT");
    FILE* out = (path != NULL && path[0] != 0) ? fopen(path, "w") : stderr;
    size_t i;
    if (out == NULL) return;
    fprintf(out, "kaze-profile 1\n");
    fprintf(out, "functions %zu\n", kz_entry_count);
    fprintf(out, "dropped-functions %llu\n", kz_dropped_entries);
    fprintf(out, "dropped-frames %llu\n", kz_dropped_frames);
    {
        Dl_info info;
        if (dladdr((void*)&kz_profile_dump, &info) != 0 && info.dli_fbase != NULL)
            fprintf(out, "base %p\n", info.dli_fbase);
        else
            fprintf(out, "base 0x0\n");
    }
    for (i = 0; i < KZ_PROFILE_SLOTS; i++) {
        if (kz_entries[i].addr == NULL) continue;
        fprintf(out, "%llu %llu %llu %p\n",
                kz_entries[i].calls,
                kz_entries[i].incl_ns,
                kz_entries[i].excl_ns,
                kz_entries[i].addr);
    }
    if (out != stderr) fclose(out);
}

__attribute__((no_instrument_function))
void __cyg_profile_func_enter(void* this_fn, void* call_site) {
    (void)call_site;
    if (kz_depth >= KZ_PROFILE_DEPTH) {
        kz_dropped_frames++;
        return;
    }
    kz_stack[kz_depth].addr = this_fn;
    kz_stack[kz_depth].start_ns = kz_now_ns();
    kz_stack[kz_depth].child_ns = 0;
    kz_depth++;
    if (!kz_registered) {
        kz_registered = 1;
        atexit(kz_profile_dump);
    }
}

__attribute__((no_instrument_function))
void __cyg_profile_func_exit(void* this_fn, void* call_site) {
    unsigned long long stop;
    unsigned long long total;
    kz_profile_entry* e;
    (void)call_site;
    if (kz_depth == 0) return;
    kz_depth--;
    if (kz_stack[kz_depth].addr != this_fn) {
        /* The stack was disturbed (a longjmp, a signal, or a dropped frame);
           report the outermost frame rather than invent a time. */
        return;
    }
    stop = kz_now_ns();
    total = stop - kz_stack[kz_depth].start_ns;
    e = kz_find(this_fn);
    if (e != NULL) {
        e->calls++;
        e->incl_ns += total;
        e->excl_ns += (total >= kz_stack[kz_depth].child_ns) ? total - kz_stack[kz_depth].child_ns : 0;
    }
    if (kz_depth > 0) kz_stack[kz_depth - 1].child_ns += total;
}
