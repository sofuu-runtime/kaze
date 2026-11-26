/* kaze_embed_api.c — appended to generated compiler C by `kaze emit-lib-c`.
 *
 * This file is the stateful door in front of the self-hosted compiler. The
 * generated compiler keeps one file-scope arena and a handful of scratch
 * globals; each call swaps in the handle's own arena, so two handles never
 * observe one another, and a trap becomes a status instead of exit(70).
 *
 * Build this file only as part of the emitted translation unit: it calls the
 * generated static routines kaze_fn_api_* directly.
 */
#include "kaze_embed.h"

struct kz_compiler_t {
    kaze_arena_t arena;
    int arena_ready;
    char *out;
    size_t out_cap;
    size_t out_len;
    int trap_code;
};

/* The generated compiler's file-scope state. Kept in one struct so a call is
 * save/restore, not a list of forgotten globals. */
typedef struct kz_embed_saved_t {
    kaze_arena_t arena;
    const char *site_name[KZ_ALLOC_SITES];
    size_t site_bytes[KZ_ALLOC_SITES];
    size_t site_calls[KZ_ALLOC_SITES];
    kz_fiber *tasks_head;
    kz_fiber *tasks_tail;
    kz_fiber *rr_cursor;
    kz_fiber *current;
    long switch_count;
    long tasks_declared;
    char source_path_buf[4096];
    int argc_g;
    char **argv_g;
    int exit_code_g;
} kz_embed_saved_t;

static kz_embed_result_t kz_embed_empty(const char *code)
{
    kz_embed_result_t r;
    r.ok = false;
    r.text = "";
    r.len = 0;
    r.err_code = code;
    r.err_line = 0;
    return r;
}

static char *kz_embed_source_copy(const char *src, size_t len)
{
    char *p;
    if (src == NULL) {
        len = 0;
    }
    p = (char *)malloc(len + 1u);
    if (p == NULL) {
        return NULL;
    }
    if (len > 0) {
        memcpy(p, src, len);
    }
    p[len] = 0;
    return p;
}

static int kz_embed_reserve(kz_compiler_t *c, size_t need)
{
    size_t cap;
    char *grown;
    if (c->out_cap >= need) {
        return 1;
    }
    cap = c->out_cap ? c->out_cap : 256u;
    while (cap < need) {
        if (cap > ((size_t)-1) / 2u) {
            cap = need;
            break;
        }
        cap *= 2u;
    }
    grown = (char *)realloc(c->out, cap);
    if (grown == NULL) {
        return 0;
    }
    c->out = grown;
    c->out_cap = cap;
    return 1;
}

static int kz_embed_copy(kz_compiler_t *c, const char *p, size_t n)
{
    if (!kz_embed_reserve(c, n + 1u)) {
        return 0;
    }
    if (n > 0) {
        memcpy(c->out, p, n);
    }
    c->out[n] = 0;
    c->out_len = n;
    return 1;
}

static int kz_embed_copy_value(kz_compiler_t *c, KzValue v)
{
    if (v.kind == KZ_VAL_STR && v.as.str.data != NULL) {
        return kz_embed_copy(c, v.as.str.data, v.as.str.len);
    }
    {
        const char *s = kz_to_string(v);
        if (s == NULL) {
            s = "";
        }
        return kz_embed_copy(c, s, strlen(s));
    }
}

static void kz_embed_enter(kz_compiler_t *c, kz_embed_saved_t *s)
{
    s->arena = kz_arena;
    memcpy(s->site_name, kz_site_name, sizeof kz_site_name);
    memcpy(s->site_bytes, kz_site_bytes, sizeof kz_site_bytes);
    memcpy(s->site_calls, kz_site_calls, sizeof kz_site_calls);
    s->tasks_head = kz_tasks_head;
    s->tasks_tail = kz_tasks_tail;
    s->rr_cursor = kz_rr_cursor;
    s->current = kz_current;
    s->switch_count = kz_switch_count;
    s->tasks_declared = kz_tasks_declared;
    memcpy(s->source_path_buf, kz_source_path_buf, sizeof kz_source_path_buf);
    s->argc_g = kz_argc_g;
    s->argv_g = kz_argv_g;
    s->exit_code_g = kz_exit_code_g;

    kz_arena = c->arena;
    kaze_arena_reset(&kz_arena);
    memset(kz_site_name, 0, sizeof kz_site_name);
    memset(kz_site_bytes, 0, sizeof kz_site_bytes);
    memset(kz_site_calls, 0, sizeof kz_site_calls);
    kz_tasks_head = NULL;
    kz_tasks_tail = NULL;
    kz_rr_cursor = NULL;
    kz_current = NULL;
    kz_switch_count = 0;
    kz_tasks_declared = 0;
    kz_source_path_buf[0] = 0;
    kz_argc_g = 0;
    kz_argv_g = NULL;
    kz_exit_code_g = 0;
    c->trap_code = 0;
}

static void kz_embed_leave(kz_compiler_t *c, kz_embed_saved_t *s)
{
    c->arena = kz_arena;
    c->trap_code = kz_exit_code_g;
    kz_arena = s->arena;
    memcpy(kz_site_name, s->site_name, sizeof kz_site_name);
    memcpy(kz_site_bytes, s->site_bytes, sizeof kz_site_bytes);
    memcpy(kz_site_calls, s->site_calls, sizeof kz_site_calls);
    kz_tasks_head = s->tasks_head;
    kz_tasks_tail = s->tasks_tail;
    kz_rr_cursor = s->rr_cursor;
    kz_current = s->current;
    kz_switch_count = s->switch_count;
    kz_tasks_declared = s->tasks_declared;
    memcpy(kz_source_path_buf, s->source_path_buf, sizeof kz_source_path_buf);
    kz_argc_g = s->argc_g;
    kz_argv_g = s->argv_g;
    kz_exit_code_g = s->exit_code_g;
}

static kz_embed_result_t kz_embed_answer(kz_compiler_t *c, int ok, const char *code)
{
    kz_embed_result_t r;
    r.ok = ok != 0;
    r.text = c->out ? c->out : "";
    r.len = c->out_len;
    r.err_code = ok ? NULL : code;
    r.err_line = 0;
    return r;
}

kz_compiler_t *kz_embed_compiler_new(size_t arena_bytes)
{
    kz_compiler_t *c = (kz_compiler_t *)calloc(1u, sizeof(kz_compiler_t));
    if (c == NULL) {
        return NULL;
    }
    if (arena_bytes == 0) {
        arena_bytes = (size_t)256 * 1024u * 1024u;
    }
    if (!kaze_arena_init_heap(&c->arena, arena_bytes)) {
        free(c);
        return NULL;
    }
    c->arena_ready = 1;
    return c;
}

void kz_embed_compiler_free(kz_compiler_t *c)
{
    if (c == NULL) {
        return;
    }
    if (c->arena_ready) {
        kaze_arena_free(&c->arena);
    }
    free(c->out);
    free(c);
}

kz_embed_result_t kz_embed_check(kz_compiler_t *c, const char *src, size_t len)
{
    kz_embed_saved_t saved;
    char *copy;
    KzValue v;
    int ok;
    if (c == NULL) {
        return kz_embed_empty("E_KAZE_STATE");
    }
    copy = kz_embed_source_copy(src, len);
    if (copy == NULL) {
        return kz_embed_empty("E_KAZE_MEMORY");
    }
    kz_embed_enter(c, &saved);
    v = kaze_fn_api_check(kz_str(copy));
    ok = (v.kind == KZ_VAL_STR && v.as.str.len >= 9u &&
          memcmp(v.as.str.data, "check: ok", 9u) == 0);
    (void)kz_embed_copy_value(c, v);
    kz_embed_leave(c, &saved);
    free(copy);
    if (c->trap_code != 0) {
        ok = 0;
    }
    return kz_embed_answer(c, ok, "E_KAZE_CHECK");
}

kz_embed_result_t kz_embed_compile(kz_compiler_t *c, const char *src, size_t len)
{
    kz_embed_saved_t saved;
    char *copy;
    KzValue v;
    int ok;
    if (c == NULL) {
        return kz_embed_empty("E_KAZE_STATE");
    }
    copy = kz_embed_source_copy(src, len);
    if (copy == NULL) {
        return kz_embed_empty("E_KAZE_MEMORY");
    }
    kz_embed_enter(c, &saved);
    v = kaze_fn_api_compile(kz_str(copy));
    (void)kz_embed_copy_value(c, v);
    ok = c->out_len > 0u && !(c->out_len >= 8u && memcmp(c->out, "compile:", 8u) == 0) &&
         !(c->out_len >= 6u && memcmp(c->out, "check:", 6u) == 0);
    kz_embed_leave(c, &saved);
    free(copy);
    if (c->trap_code != 0) {
        ok = 0;
    }
    return kz_embed_answer(c, ok, "E_KAZE_COMPILE");
}

kz_embed_result_t kz_embed_emit_c(kz_compiler_t *c, const char *src, size_t len, bool standalone)
{
    kz_embed_saved_t saved;
    char *copy;
    KzValue v;
    int ok;
    if (c == NULL) {
        return kz_embed_empty("E_KAZE_STATE");
    }
    copy = kz_embed_source_copy(src, len);
    if (copy == NULL) {
        return kz_embed_empty("E_KAZE_MEMORY");
    }
    kz_embed_enter(c, &saved);
    v = kaze_fn_api_emit_c(kz_str(copy), kz_bool(standalone ? 1 : 0));
    (void)kz_embed_copy_value(c, v);
    ok = c->out_len > 0u && strstr(c->out, "Kaze module resolution failed") == NULL &&
         strstr(c->out, "#error") == NULL;
    kz_embed_leave(c, &saved);
    free(copy);
    if (c->trap_code != 0) {
        ok = 0;
    }
    return kz_embed_answer(c, ok, "E_KAZE_EMIT");
}

kz_embed_result_t kz_embed_to_machine(kz_compiler_t *c, const char *src, size_t len)
{
    kz_embed_saved_t saved;
    char *copy;
    KzValue v;
    if (c == NULL) {
        return kz_embed_empty("E_KAZE_STATE");
    }
    copy = kz_embed_source_copy(src, len);
    if (copy == NULL) {
        return kz_embed_empty("E_KAZE_MEMORY");
    }
    kz_embed_enter(c, &saved);
    v = kaze_fn_api_to_machine(kz_str(copy));
    (void)kz_embed_copy_value(c, v);
    kz_embed_leave(c, &saved);
    free(copy);
    return kz_embed_answer(c, c->trap_code == 0, "E_KAZE_MACHINE");
}

kz_embed_result_t kz_embed_to_human(kz_compiler_t *c, const char *src, size_t len)
{
    kz_embed_saved_t saved;
    char *copy;
    KzValue v;
    if (c == NULL) {
        return kz_embed_empty("E_KAZE_STATE");
    }
    copy = kz_embed_source_copy(src, len);
    if (copy == NULL) {
        return kz_embed_empty("E_KAZE_MEMORY");
    }
    kz_embed_enter(c, &saved);
    v = kaze_fn_api_to_human(kz_str(copy));
    (void)kz_embed_copy_value(c, v);
    kz_embed_leave(c, &saved);
    free(copy);
    return kz_embed_answer(c, c->trap_code == 0, "E_KAZE_HUMAN");
}

kz_embed_result_t kz_embed_version(kz_compiler_t *c)
{
    kz_embed_saved_t saved;
    KzValue v;
    if (c == NULL) {
        return kz_embed_empty("E_KAZE_STATE");
    }
    kz_embed_enter(c, &saved);
    v = kaze_fn_api_version();
    (void)kz_embed_copy_value(c, v);
    kz_embed_leave(c, &saved);
    return kz_embed_answer(c, c->trap_code == 0, "E_KAZE_VERSION");
}
