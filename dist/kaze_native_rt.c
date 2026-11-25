/* kaze_native_rt.c -- the small libc bridge a QBE-generated Kaze program uses.
 *
 * Nothing here is Kaze semantics: QBE emits the program, and these routines
 * only print the value `main` returned and stop on a trap.  The number
 * formatting deliberately mirrors kz_format_number in compiler/codegen.kaze
 * so `--backend=native` and `--backend=c99` agree on stdout byte-for-byte.
 */
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static void kaze_native_format_f64(char *buf, size_t cap, double v)
{
    if (isnan(v)) {
        snprintf(buf, cap, "NaN");
    } else if (isinf(v)) {
        snprintf(buf, cap, v > 0 ? "inf" : "-inf");
    } else if (floor(v) == v && fabs(v) < 1e17) {
        snprintf(buf, cap, "%.0f", v);
    } else {
        int prec = 15;
        for (; prec <= 17; prec++) {
            snprintf(buf, cap, "%.*g", prec, v);
            if (strtod(buf, NULL) == v) break;
        }
        if (prec > 17) snprintf(buf, cap, "%.17g", v);
    }
}

void kaze_native_print_f64(double v)
{
    char buf[64];
    kaze_native_format_f64(buf, sizeof buf, v);
    printf("%s\n", buf);
}

void kaze_native_print_str(const char *s)
{
    printf("%s\n", s != NULL ? s : "");
}

void kaze_native_log_str(const char *s)
{
    printf("[info] %s\n", s != NULL ? s : "");
}

void kaze_native_alert_str(const char *s)
{
    fprintf(stderr, "[alert] %s\n", s != NULL ? s : "");
}

void kaze_native_log_f64(double v)
{
    char buf[64];
    kaze_native_format_f64(buf, sizeof buf, v);
    printf("[info] %s\n", buf);
}

void kaze_native_alert_f64(double v)
{
    char buf[64];
    kaze_native_format_f64(buf, sizeof buf, v);
    fprintf(stderr, "[alert] %s\n", buf);
}

void kaze_native_log_bool(int v)
{
    printf("[info] %s\n", v ? "true" : "false");
}

void kaze_native_alert_bool(int v)
{
    fprintf(stderr, "[alert] %s\n", v ? "true" : "false");
}

void kaze_native_print_i64(int64_t v)
{
    kaze_native_print_f64((double)v);
}

void kaze_native_print_bool(int v)
{
    printf("%s\n", v ? "true" : "false");
}

void kaze_native_trap(const char *message)
{
    fprintf(stderr, "%s\n", message != NULL ? message : "E_KAZE_BADARG: trap");
    exit(70);
}

/* ── dynamic values ──────────────────────────────────────────────────────
 * The scalar IR unboxes numbers; the language still has dynamic entry points
 * (unannotated parameters) and dynamic expressions (lists, indexing, fields).
 * A native value is a heap pointer, so QBE carries it in one `l` register and
 * every operation is a call.  The observable rules mirror the C99 preamble:
 * arithmetic reads a non-number as 0, `+` concatenates when either side is
 * text, `length of` refuses a number, and an out-of-range or negative index is
 * the fallback only when the source wrote `otherwise`. */

typedef struct KzNv KzNv;
struct KzNv {
    int kind; /* 0 nil, 1 bool, 2 num, 3 str, 4 list, 5 record */
    double num;
    char *str;
    size_t len;
    size_t cap;
    KzNv **items;
    const char *type_name;
    size_t count;
    const char **keys;
    KzNv **vals;
};

static KzNv kaze_native_nil_value = { 0 };

/* A number or boolean only reads kind and num (both inside the first 16
 * bytes). Allocating the whole 72-byte record for every arithmetic temporary
 * is what made a one-million-byte SHA-256 need almost 2 GB. */
#define KZ_NV_SCALAR_BYTES (offsetof(KzNv, str))

/* Scalar boxes are never freed; the whole run shares their lifetime. A bump
 * arena removes a malloc header from each one, which is most of what remains
 * of that high-water mark. Chunks are kept because their values stay live. */
#define KZ_NV_ARENA_CHUNK (1024u * 1024u)
static char *kaze_native_nv_arena;
static size_t kaze_native_nv_used;
static size_t kaze_native_nv_cap;
static size_t kaze_native_nv_count;
static size_t kaze_native_nv_bytes;
static size_t kaze_native_str_count;
static size_t kaze_native_str_bytes;
static size_t kaze_native_list_count;
static size_t kaze_native_list_bytes;

static void kaze_native_nv_stats(void)
{
    if (getenv("KAZE_NV_STATS") != NULL)
        fprintf(stderr,
                "kaze: %zu scalar boxes (%.1f MB), %zu strings (%.1f MB), "
                "%zu lists (%.1f MB)\n",
                kaze_native_nv_count,
                (double)kaze_native_nv_bytes / (1024.0 * 1024.0),
                kaze_native_str_count,
                (double)kaze_native_str_bytes / (1024.0 * 1024.0),
                kaze_native_list_count,
                (double)kaze_native_list_bytes / (1024.0 * 1024.0));
}

static KzNv *kaze_native_scalar_alloc(void)
{
    size_t need = (KZ_NV_SCALAR_BYTES + 15u) & ~(size_t)15u;
    char *p;
    if (kaze_native_nv_count == 0) atexit(kaze_native_nv_stats);
    kaze_native_nv_count += 1;
    kaze_native_nv_bytes += need;
    if (kaze_native_nv_arena == NULL || kaze_native_nv_used + need > kaze_native_nv_cap) {
        size_t cap = need > KZ_NV_ARENA_CHUNK ? need : KZ_NV_ARENA_CHUNK;
        char *chunk = (char *)malloc(cap);
        if (chunk == NULL) kaze_native_trap("E_KAZE_OOM: cannot allocate a value");
        kaze_native_nv_arena = chunk;
        kaze_native_nv_used = 0;
        kaze_native_nv_cap = cap;
    }
    p = kaze_native_nv_arena + kaze_native_nv_used;
    kaze_native_nv_used += need;
    return (KzNv *)p;
}

const char *kaze_native_value_text(KzNv *v);

KzNv *kz_nv_nil(void)
{
    return &kaze_native_nil_value;
}

static KzNv *kaze_native_small_nums[256];

KzNv *kz_nv_num(double v)
{
    KzNv *out;
    if (v >= 0.0 && v <= 255.0 && (double)(int)v == v &&
        !(v == 0.0 && signbit(v))) {
        int i = (int)v;
        if (kaze_native_small_nums[i] != NULL) return kaze_native_small_nums[i];
        out = kaze_native_scalar_alloc();
        if (out == NULL) kaze_native_trap("E_KAZE_OOM: cannot allocate a value");
        out->kind = 2;
        out->num = v;
        kaze_native_small_nums[i] = out;
        return out;
    }
    out = kaze_native_scalar_alloc();
    if (out == NULL) kaze_native_trap("E_KAZE_OOM: cannot allocate a value");
    out->kind = 2;
    out->num = v;
    return out;
}

KzNv *kz_nv_bool(int v)
{
    KzNv *out = kaze_native_scalar_alloc();
    if (out == NULL) kaze_native_trap("E_KAZE_OOM: cannot allocate a value");
    out->kind = 1;
    out->num = v ? 1.0 : 0.0;
    return out;
}

static KzNv *kaze_native_one_byte[256];

KzNv *kz_nv_str_n(const char *s, size_t n)
{
    KzNv *out;
    if (n == 1) {
        unsigned char c = (unsigned char)s[0];
        if (kaze_native_one_byte[c] != NULL) return kaze_native_one_byte[c];
    }
    out = (KzNv *)calloc(1, sizeof(KzNv));
    if (out == NULL) kaze_native_trap("E_KAZE_OOM: cannot allocate a value");
    kaze_native_str_count += 1;
    kaze_native_str_bytes += n + 1;
    out->kind = 3;
    out->str = (char *)malloc(n + 1);
    if (out->str == NULL) kaze_native_trap("E_KAZE_OOM: cannot allocate text");
    if (n > 0 && s != NULL) memcpy(out->str, s, n);
    out->str[n] = '\0';
    out->len = n;
    if (n == 1) kaze_native_one_byte[(unsigned char)s[0]] = out;
    return out;
}

KzNv *kz_nv_str(const char *s)
{
    return kz_nv_str_n(s != NULL ? s : "", s != NULL ? strlen(s) : 0);
}

KzNv *kz_nv_list_new(int64_t n)
{
    KzNv *out = (KzNv *)calloc(1, sizeof(KzNv));
    if (out == NULL) kaze_native_trap("E_KAZE_OOM: cannot allocate a list");
    out->kind = 4;
    kaze_native_list_count += 1;
    kaze_native_list_bytes += (n > 0 ? (size_t)n : 0) * sizeof(KzNv *);
    out->cap = n > 0 ? (size_t)n : 0;
    out->items = out->cap > 0 ? (KzNv **)calloc(out->cap, sizeof(KzNv *)) : NULL;
    if (out->cap > 0 && out->items == NULL) kaze_native_trap("E_KAZE_OOM: cannot allocate list items");
    return out;
}

void kz_nv_list_set(KzNv *list, int64_t i, KzNv *item)
{
    size_t need;
    if (list == NULL || list->kind != 4 || i < 0) return;
    need = (size_t)i + 1;
    if (need > list->cap) {
        size_t cap = list->cap > 0 ? list->cap : 4;
        KzNv **items;
        while (cap < need) cap *= 2;
        items = (KzNv **)realloc(list->items, cap * sizeof(KzNv *));
        if (items == NULL) kaze_native_trap("E_KAZE_OOM: cannot grow a list");
        list->items = items;
        list->cap = cap;
    }
    list->items[i] = item != NULL ? item : kz_nv_nil();
    if (need > list->len) list->len = need;
}

double kz_nv_to_f64(KzNv *v)
{
    return (v != NULL && v->kind == 2) ? v->num : 0.0;
}

int kz_nv_truthy(KzNv *v)
{
    if (v == NULL) return 0;
    if (v->kind == 1) return v->num != 0.0;
    if (v->kind == 2) return v->num != 0.0;
    if (v->kind == 3) return v->str != NULL && v->str[0] != '\0';
    if (v->kind == 4) return v->len > 0;
    if (v->kind == 5) return 1;
    return 0;
}

int64_t kz_nv_iter_len(KzNv *v)
{
    return (v != NULL && v->kind == 4) ? (int64_t)v->len : 0;
}

KzNv *kz_nv_iter_at(KzNv *v, int64_t i)
{
    if (v == NULL || v->kind != 4 || i < 0 || (size_t)i >= v->len) return kz_nv_nil();
    return v->items[i] != NULL ? v->items[i] : kz_nv_nil();
}

KzNv *kz_nv_index(KzNv *coll, double idx)
{
    size_t i = (idx >= 0.0) ? (size_t)idx : 0;
    if (coll != NULL && coll->kind == 4) {
        if (i < coll->len) return coll->items[i];
        return kz_nv_nil();
    }
    if (coll != NULL && coll->kind == 3) {
        if (i < coll->len) return kz_nv_str_n(coll->str + i, 1);
        return kz_nv_str("");
    }
    return kz_nv_nil();
}

KzNv *kz_nv_index_fb(KzNv *coll, KzNv *idx, KzNv *fb)
{
    /* The C99 helper takes a dynamic index: a missing or non-numeric index is
     * the fallback, not index 0. Keep the dynamic value until here. */
    double d = (idx != NULL && idx->kind == 2) ? idx->num : -1.0;
    if (!(d >= 0.0)) return fb;
    if (coll != NULL && coll->kind == 4) {
        if ((size_t)d >= coll->len) return fb;
    } else if (coll != NULL && coll->kind == 3) {
        if ((size_t)d >= coll->len) return fb;
    }
    return kz_nv_index(coll, d);
}

KzNv *kz_nv_record_new(const char *type_name, int64_t n)
{
    KzNv *out = (KzNv *)calloc(1, sizeof(KzNv));
    if (out == NULL) kaze_native_trap("E_KAZE_OOM: cannot allocate a record");
    out->kind = 5;
    out->type_name = type_name;
    out->count = n > 0 ? (size_t)n : 0;
    if (out->count > 0) {
        out->keys = (const char **)calloc(out->count, sizeof(const char *));
        out->vals = (KzNv **)calloc(out->count, sizeof(KzNv *));
        if (out->keys == NULL || out->vals == NULL) kaze_native_trap("E_KAZE_OOM: cannot allocate record fields");
    }
    return out;
}

void kz_nv_record_set(KzNv *rec, const char *key, KzNv *value)
{
    size_t i;
    if (rec == NULL || rec->kind != 5 || key == NULL) return;
    for (i = 0; i < rec->count; i++) {
        if (rec->keys[i] != NULL && strcmp(rec->keys[i], key) == 0) {
            rec->vals[i] = value != NULL ? value : kz_nv_nil();
            return;
        }
    }
    /* A positional constructor can name a field the schema did not know
     * about; keep it rather than dropping the value. */
    {
        size_t n = rec->count + 1;
        const char **keys = (const char **)realloc(rec->keys, n * sizeof(const char *));
        KzNv **vals = (KzNv **)realloc(rec->vals, n * sizeof(KzNv *));
        if (keys == NULL || vals == NULL) kaze_native_trap("E_KAZE_OOM: cannot grow a record");
        rec->keys = keys;
        rec->vals = vals;
        rec->keys[rec->count] = key;
        rec->vals[rec->count] = value != NULL ? value : kz_nv_nil();
        rec->count = n;
    }
}

KzNv *kz_nv_field(KzNv *v, const char *name)
{
    size_t i;
    if (v == NULL) return kz_nv_nil();
    if (v->kind == 5) {
        for (i = 0; i < v->count; i++) {
            if (v->keys[i] != NULL && name != NULL && strcmp(v->keys[i], name) == 0) return v->vals[i];
        }
        return kz_nv_nil();
    }
    return kz_nv_nil();
}

double kz_nv_len(KzNv *v)
{
    if (v != NULL && v->kind == 3) return (double)v->len;
    if (v != NULL && v->kind == 4) return (double)v->len;
    kaze_native_trap("E_KAZE_TYPE: length of does not apply to this value");
    return 0.0;
}

double kaze_native_str_len(const char *s)
{
    return (double)(s != NULL ? strlen(s) : 0);
}

static char *kaze_native_concat(const char *a, const char *b)
{
    size_t la = a != NULL ? strlen(a) : 0;
    size_t lb = b != NULL ? strlen(b) : 0;
    char *out = (char *)malloc(la + lb + 1);
    if (out == NULL) kaze_native_trap("E_KAZE_OOM: cannot concatenate text");
    if (la > 0) memcpy(out, a, la);
    if (lb > 0) memcpy(out + la, b, lb);
    out[la + lb] = '\0';
    return out;
}

static int kz_nv_same(KzNv *a, KzNv *b)
{
    if (a == NULL) a = kz_nv_nil();
    if (b == NULL) b = kz_nv_nil();
    if (a->kind != b->kind) return 0;
    if (a->kind == 0) return 1;
    if (a->kind == 1 || a->kind == 2) return a->num == b->num;
    if (a->kind == 3) {
        if (a->str == NULL || b->str == NULL) return a->str == b->str;
        return strcmp(a->str, b->str) == 0;
    }
    return a->items == b->items; /* lists and records: pointer identity */
}

int kz_nv_eq(KzNv *a, KzNv *b) { return kz_nv_same(a, b); }
int kz_nv_ne(KzNv *a, KzNv *b) { return !kz_nv_same(a, b); }

int kz_nv_lt(KzNv *a, KzNv *b)
{
    if (a != NULL && b != NULL && a->kind == 2 && b->kind == 2) return a->num < b->num;
    if (a != NULL && b != NULL && a->kind == 3 && b->kind == 3) return strcmp(a->str, b->str) < 0;
    return 0;
}

int kz_nv_le(KzNv *a, KzNv *b)
{
    if (a != NULL && b != NULL && a->kind == 2 && b->kind == 2) return a->num <= b->num;
    if (a != NULL && b != NULL && a->kind == 3 && b->kind == 3) return strcmp(a->str, b->str) <= 0;
    return 0;
}

int kz_nv_gt(KzNv *a, KzNv *b)
{
    if (a != NULL && b != NULL && a->kind == 2 && b->kind == 2) return a->num > b->num;
    if (a != NULL && b != NULL && a->kind == 3 && b->kind == 3) return strcmp(a->str, b->str) > 0;
    return 0;
}

int kz_nv_ge(KzNv *a, KzNv *b)
{
    if (a != NULL && b != NULL && a->kind == 2 && b->kind == 2) return a->num >= b->num;
    if (a != NULL && b != NULL && a->kind == 3 && b->kind == 3) return strcmp(a->str, b->str) >= 0;
    return 0;
}

KzNv *kz_nv_add(KzNv *a, KzNv *b)
{
    if (a != NULL && b != NULL && a->kind == 2 && b->kind == 2) return kz_nv_num(a->num + b->num);
    if ((a != NULL && a->kind == 3) || (b != NULL && b->kind == 3)) {
        const char *sa = kaze_native_value_text(a);
        const char *sb = kaze_native_value_text(b);
        return kz_nv_str(kaze_native_concat(sa, sb));
    }
    if (a != NULL && b != NULL && a->kind == 4 && b->kind == 4) {
        /* `+` is value semantics: the C99 path returns a new list. Appending
         * in place here aliases every other holder of `a` and loops forever
         * for `a + a` because b->len grows while we iterate. The IR uses
         * kz_nv_add_own only where the C99 ownership pass proved it safe. */
        KzNv *out = kz_nv_list_new((int64_t)(a->len + b->len));
        size_t i;
        for (i = 0; i < a->len; i++) kz_nv_list_set(out, (int64_t)i, a->items[i]);
        for (i = 0; i < b->len; i++) kz_nv_list_set(out, (int64_t)(a->len + i), b->items[i]);
        return out;
    }
    return kz_nv_nil();
}

KzNv *kz_nv_add_own(KzNv *a, KzNv *b)
{
    if (a != NULL && b != NULL && a->kind == 4 && b->kind == 4) {
        size_t n = b->len;
        size_t i;
        for (i = 0; i < n; i++) {
            /* Read before kz_nv_list_set grows the buffer: when a == b the
             * realloc can move it, so b->items[i] must not be cached. */
            KzNv *item = b->items[i];
            kz_nv_list_set(a, (int64_t)a->len, item);
        }
        return a;
    }
    return kz_nv_add(a, b);
}

double kaze_native_clock_monotonic_ns(void)
{
#if defined(CLOCK_MONOTONIC)
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) == 0)
        return (double)ts.tv_sec * 1000000000.0 + (double)ts.tv_nsec;
#endif
    return (double)clock() * (1000000000.0 / (double)CLOCKS_PER_SEC);
}

double kaze_native_clock_utc_seconds(void)
{
    return (double)time(NULL);
}

double kaze_native_code(KzNv *v)
{
    if (v == NULL || v->kind != 3 || v->str == NULL || v->len == 0) return 0.0;
    return (double)(unsigned char)v->str[0];
}

double kaze_native_byte_at(KzNv *coll, KzNv *idx)
{
    double d = (idx != NULL && idx->kind == 2) ? idx->num : -1.0;
    size_t i;
    if (!(d >= 0.0)) return 0.0;
    i = (size_t)d;
    if (coll != NULL && coll->kind == 3) {
        if (i < coll->len) return (double)(unsigned char)coll->str[i];
        return 0.0;
    }
    if (coll != NULL && coll->kind == 4) {
        if (i < coll->len) {
            KzNv *v = coll->items[i];
            if (v != NULL && v->kind == 2) return v->num;
        }
    }
    return 0.0;
}

KzNv *kz_nv_character(double n)
{
    char one[2];
    if (!(n >= 0.0) || n > 255.0) return kz_nv_str("");
    one[0] = (char)(int)n;
    one[1] = '\0';
    return kz_nv_str_n(one, 1);
}

const char *kaze_native_value_text(KzNv *v)
{
    static char buf[64];
    if (v == NULL) return "nil";
    if (v->kind == 3) return v->str != NULL ? v->str : "";
    if (v->kind == 1) return v->num != 0.0 ? "true" : "false";
    if (v->kind == 2) {
        kaze_native_format_f64(buf, sizeof buf, v->num);
        return buf;
    }
    if (v->kind == 4) return "list";
    if (v->kind == 5) return v->type_name != NULL ? v->type_name : "record";
    return "nil";
}

void kaze_native_print_val(KzNv *v)
{
    if (v == NULL) return;
    if (v->kind == 2) {
        kaze_native_print_f64(v->num);
    } else if (v->kind == 1) {
        printf("%s\n", v->num != 0.0 ? "true" : "false");
    } else if (v->kind == 3) {
        printf("%s\n", v->str != NULL ? v->str : "");
    }
}

void kaze_native_log_val(KzNv *v)
{
    printf("[info] %s\n", kaze_native_value_text(v));
}

void kaze_native_alert_val(KzNv *v)
{
    fprintf(stderr, "[alert] %s\n", kaze_native_value_text(v));
}

/* ── parameterized main ──────────────────────────────────────────────────
 * `kaze-native run file.kaze --arg d=4` resolves parameters through the same
 * layers the C99 entry point uses: --arg name=value, --name=value, AGENT_*,
 * SOFUU_*, positional argv, and piped stdin for the first parameter.
 * Only compiler/qbe_codegen.kaze calls these; keep the argument order in sync
 * with its `kaze_native_param_*` call sites. */

static char *kaze_native_read_file(const char *path, int *was_file)
{
    FILE *f = fopen(path, "rb");
    long sz;
    char *buf;
    size_t rd;
    *was_file = 0;
    if (f == NULL) return NULL;
    if (fseek(f, 0, SEEK_END) != 0) { fclose(f); return NULL; }
    sz = ftell(f);
    if (sz < 0) { fclose(f); return NULL; }
    rewind(f);
    buf = (char *)malloc((size_t)sz + 1);
    if (buf == NULL) { fclose(f); return NULL; }
    rd = fread(buf, 1, (size_t)sz, f);
    fclose(f);
    buf[rd] = '\0';
    *was_file = 1;
    return buf;
}

static const char *kaze_native_resolve(int argc, char **argv, const char *name, int index)
{
    size_t nlen = strlen(name);
    int i;
    int pos_idx = 0;
    char env_name[160];

    for (i = 1; i < argc; i++) {
        const char *a = argv[i];
        if (strncmp(a, "--", 2) != 0) continue;
        {
            const char *opt = a + 2;
            int is_file = 0;
            char *file_value;
            if (strncmp(opt, name, nlen) == 0 && opt[nlen] == '=') {
                file_value = kaze_native_read_file(opt + nlen + 1, &is_file);
                return is_file ? file_value : opt + nlen + 1;
            }
            if (strcmp(opt, "arg") == 0 && i + 1 < argc) {
                const char *next = argv[i + 1];
                if (strncmp(next, name, nlen) == 0 && next[nlen] == '=') {
                    file_value = kaze_native_read_file(next + nlen + 1, &is_file);
                    return is_file ? file_value : next + nlen + 1;
                }
            }
            if (strncmp(opt, "arg=", 4) == 0) {
                const char *next = opt + 4;
                if (strncmp(next, name, nlen) == 0 && next[nlen] == '=') {
                    file_value = kaze_native_read_file(next + nlen + 1, &is_file);
                    return is_file ? file_value : next + nlen + 1;
                }
            }
        }
    }

    snprintf(env_name, sizeof env_name, "AGENT_%s", name);
    for (i = 6; env_name[i] != '\0'; i++) {
        if (env_name[i] >= 'a' && env_name[i] <= 'z') env_name[i] = (char)(env_name[i] - 32);
    }
    {
        const char *ev = getenv(env_name);
        if (ev != NULL) {
            int is_file = 0;
            char *file_value = kaze_native_read_file(ev, &is_file);
            return is_file ? file_value : ev;
        }
    }
    snprintf(env_name, sizeof env_name, "SOFUU_%s", name);
    for (i = 6; env_name[i] != '\0'; i++) {
        if (env_name[i] >= 'a' && env_name[i] <= 'z') env_name[i] = (char)(env_name[i] - 32);
    }
    {
        const char *ev = getenv(env_name);
        if (ev != NULL) {
            int is_file = 0;
            char *file_value = kaze_native_read_file(ev, &is_file);
            return is_file ? file_value : ev;
        }
    }

    for (i = 1; i < argc; i++) {
        int is_file = 0;
        char *file_value;
        if (strcmp(argv[i], "--ticks") == 0 || strcmp(argv[i], "--cap") == 0 ||
            strcmp(argv[i], "--config") == 0 || strcmp(argv[i], "--arg") == 0 ||
            strcmp(argv[i], "--entry") == 0 || strcmp(argv[i], "-o") == 0) {
            i++;
            continue;
        }
        if (argv[i][0] == '-') continue;
        if (pos_idx == index) {
            file_value = kaze_native_read_file(argv[i], &is_file);
            return is_file ? file_value : argv[i];
        }
        pos_idx++;
    }

    if (index == 0 && !isatty(fileno(stdin))) {
        size_t cap = 4096, len = 0;
        char *buf = (char *)malloc(cap);
        size_t n;
        if (buf == NULL) return NULL;
        while ((n = fread(buf + len, 1, cap - len - 1, stdin)) > 0) {
            len += n;
            if (len + 1024 >= cap) {
                char *nb = (char *)realloc(buf, cap * 2);
                if (nb == NULL) break;
                buf = nb;
                cap *= 2;
            }
        }
        buf[len] = '\0';
        if (len == 0) { free(buf); return NULL; }
        return buf;
    }
    return NULL;
}

static void kaze_native_new_param(const char *ty, const char *name, const char *routine,
                                  const char *flavor)
{
    fprintf(stderr,
            "E_KAZE_PARAM: parameter '%s' of '%s' is declared as %s and %s; "
            "provide --arg %s=<value>, the environment variable AGENT_%s, "
            "or \"%s\" in the config file\n",
            name, routine, ty, flavor, name, name, name);
    exit(2);
}

double kaze_native_param_f64(int argc, char **argv, const char *name, const char *routine,
                             const char *ty, int index)
{
    const char *value = kaze_native_resolve(argc, argv, name, index);
    char *end = NULL;
    double d;
    if (value == NULL) {
        /* An unannotated parameter is the language's untyped number: the C99
         * entry leaves it nil and arithmetic reads that as zero. */
        if (ty == NULL || ty[0] == '\0') return 0.0;
        kaze_native_new_param(ty, name, routine, "has no value");
    }
    d = strtod(value, &end);
    if (end == value || end == NULL || *end != '\0') {
        if (ty == NULL || ty[0] == '\0') return 0.0;
        fprintf(stderr,
                "E_KAZE_PARAM: parameter '%s' of '%s' is declared as %s but the value given is not one\n",
                name, routine, ty);
        exit(2);
    }
    return d;
}

int64_t kaze_native_param_i64(int argc, char **argv, const char *name, const char *routine,
                              const char *ty, int index)
{
    return (int64_t)kaze_native_param_f64(argc, argv, name, routine, ty, index);
}

int kaze_native_param_bool(int argc, char **argv, const char *name, const char *routine,
                           const char *ty, int index)
{
    const char *value = kaze_native_resolve(argc, argv, name, index);
    if (value == NULL) kaze_native_new_param(ty, name, routine, "has no value");
    if (strcmp(value, "true") == 0 || strcmp(value, "1") == 0) return 1;
    if (strcmp(value, "false") == 0 || strcmp(value, "0") == 0) return 0;
    fprintf(stderr,
            "E_KAZE_PARAM: parameter '%s' of '%s' is declared as %s but the value given is not one\n",
            name, routine, ty);
    exit(2);
}

const char *kaze_native_param_str(int argc, char **argv, const char *name, const char *routine,
                                  const char *ty, int index)
{
    const char *value = kaze_native_resolve(argc, argv, name, index);
    if (value == NULL) kaze_native_new_param(ty, name, routine, "has no value");
    return value;
}

KzNv *kaze_native_param_val(int argc, char **argv, const char *name, const char *routine,
                            const char *ty, int index)
{
    const char *value = kaze_native_resolve(argc, argv, name, index);
    (void)routine;
    (void)ty;
    /* An unannotated parameter is dynamic on the C99 path: absent is nil,
     * present is text. Both are value shapes, so the callee sees exactly what
     * the authoritative backend would hand it. */
    if (value == NULL) return kz_nv_nil();
    return kz_nv_str(value);
}
