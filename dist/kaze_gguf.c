/* kaze_gguf.c — zero-copy GGUF v2/v3 and SafeTensors ingestion (Phase 12).
 *
 * Weights are mmap'd once; the tensor table points straight into the mapping and
 * no weight byte is ever copied. Metadata (names, tokenizer vocabulary, model
 * hyperparameters) is copied into the caller's metadata arena because it is
 * small and NUL-terminated access is worth the copy.
 */
#include "kaze_tensor.h"
#include "kaze_quant.h"

#include <errno.h>
#include <string.h>

#if defined(_WIN32)
#define KAZE_GGUF_UNAVAILABLE 1
#else
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#define KAZE_GGUF_UNAVAILABLE 0
#endif

#define KAZE_GGUF_MAGIC 0x46554747u /* "GGUF" little-endian */
#define KAZE_GGUF_DEFAULT_ALIGNMENT 32u

/* GGUF metadata value types (the on-disk enum). */
enum {
    KAZE_GGUF_UINT8 = 0,
    KAZE_GGUF_INT8 = 1,
    KAZE_GGUF_UINT16 = 2,
    KAZE_GGUF_INT16 = 3,
    KAZE_GGUF_UINT32 = 4,
    KAZE_GGUF_INT32 = 5,
    KAZE_GGUF_FLOAT32 = 6,
    KAZE_GGUF_BOOL = 7,
    KAZE_GGUF_STRING = 8,
    KAZE_GGUF_ARRAY = 9,
    KAZE_GGUF_UINT64 = 10,
    KAZE_GGUF_INT64 = 11,
    KAZE_GGUF_FLOAT64 = 12
};

typedef struct kaze_gguf_meta_t {
    char *key;
    uint32_t type;
    int64_t i;
    double f;
    char *s;
    uint32_t elem_type;
    size_t count;
    char **strs;
    int32_t *i32s;
    float *f32s;
} kaze_gguf_meta_t;

struct kaze_gguf_t {
    int fd;
    const uint8_t *map;
    size_t size;
    kaze_arena_t *arena;
    uint32_t version;
    uint32_t alignment;
    uint64_t n_kv;
    uint64_t n_tensors;
    kaze_gguf_meta_t *metas;
    kaze_tensor_t *tensors;
    uint64_t data_offset;
};

typedef struct gguf_reader_t {
    const uint8_t *p;
    size_t size;
    size_t off;
} gguf_reader_t;

static void gguf_fail(const char *msg)
{
    (void)kaze_trap(KAZE_ERR_FORMAT, msg);
}

static bool rd_need(gguf_reader_t *r, size_t n)
{
    return r->off <= r->size && n <= r->size - r->off;
}

static bool rd_u8(gguf_reader_t *r, uint8_t *out)
{
    if (!rd_need(r, 1)) {
        return false;
    }
    *out = r->p[r->off];
    r->off += 1;
    return true;
}

static bool rd_u16(gguf_reader_t *r, uint16_t *out)
{
    if (!rd_need(r, 2)) {
        return false;
    }
    memcpy(out, r->p + r->off, 2);
    r->off += 2;
    return true;
}

static bool rd_u32(gguf_reader_t *r, uint32_t *out)
{
    if (!rd_need(r, 4)) {
        return false;
    }
    memcpy(out, r->p + r->off, 4);
    r->off += 4;
    return true;
}

static bool rd_u64(gguf_reader_t *r, uint64_t *out)
{
    if (!rd_need(r, 8)) {
        return false;
    }
    memcpy(out, r->p + r->off, 8);
    r->off += 8;
    return true;
}

static bool rd_f32(gguf_reader_t *r, float *out)
{
    uint32_t bits;
    if (!rd_u32(r, &bits)) {
        return false;
    }
    memcpy(out, &bits, 4);
    return true;
}

static bool rd_f64(gguf_reader_t *r, double *out)
{
    uint64_t bits;
    if (!rd_u64(r, &bits)) {
        return false;
    }
    memcpy(out, &bits, 8);
    return true;
}

/* GGUF strings are a u64 length followed by bytes with no terminator. */
static bool rd_str(gguf_reader_t *r, kaze_arena_t *arena, char **out)
{
    uint64_t len;
    char *s;

    if (!rd_u64(r, &len) || len > (uint64_t)(r->size - r->off)) {
        return false;
    }
    s = (char *)kmalloc(arena, (size_t)len + 1u);
    if (s == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "GGUF string did not fit the metadata arena");
        return false;
    }
    if (len > 0) {
        memcpy(s, r->p + r->off, (size_t)len);
    }
    s[len] = '\0';
    r->off += (size_t)len;
    *out = s;
    return true;
}

/* Skip a value of a known type without materialising it. */
static bool skip_value(gguf_reader_t *r, uint32_t type)
{
    uint64_t count, i;

    switch (type) {
    case KAZE_GGUF_UINT8:
    case KAZE_GGUF_INT8:
    case KAZE_GGUF_BOOL:
        return rd_need(r, 1) && ((r->off += 1), true);
    case KAZE_GGUF_UINT16:
    case KAZE_GGUF_INT16:
        return rd_need(r, 2) && ((r->off += 2), true);
    case KAZE_GGUF_UINT32:
    case KAZE_GGUF_INT32:
    case KAZE_GGUF_FLOAT32:
        return rd_need(r, 4) && ((r->off += 4), true);
    case KAZE_GGUF_UINT64:
    case KAZE_GGUF_INT64:
    case KAZE_GGUF_FLOAT64:
        return rd_need(r, 8) && ((r->off += 8), true);
    case KAZE_GGUF_STRING: {
        uint64_t len;
        if (!rd_u64(r, &len) || len > (uint64_t)(r->size - r->off)) {
            return false;
        }
        r->off += (size_t)len;
        return true;
    }
    case KAZE_GGUF_ARRAY: {
        uint32_t elem;
        if (!rd_u32(r, &elem) || !rd_u64(r, &count)) {
            return false;
        }
        for (i = 0; i < count; i++) {
            if (!skip_value(r, elem)) {
                return false;
            }
        }
        return true;
    }
    default:
        return false;
    }
}

static bool parse_meta_value(gguf_reader_t *r, kaze_arena_t *arena,
                             uint32_t type, kaze_gguf_meta_t *m)
{
    uint8_t b8;
    uint16_t b16;
    uint32_t b32;
    uint64_t b64;
    float f32v;
    double f64v;

    m->type = type;
    switch (type) {
    case KAZE_GGUF_UINT8:
        if (!rd_u8(r, &b8)) return false;
        m->i = (int64_t)b8;
        return true;
    case KAZE_GGUF_INT8:
        if (!rd_u8(r, &b8)) return false;
        m->i = (int64_t)(int8_t)b8;
        return true;
    case KAZE_GGUF_UINT16:
        if (!rd_u16(r, &b16)) return false;
        m->i = (int64_t)b16;
        return true;
    case KAZE_GGUF_INT16:
        if (!rd_u16(r, &b16)) return false;
        m->i = (int64_t)(int16_t)b16;
        return true;
    case KAZE_GGUF_UINT32:
        if (!rd_u32(r, &b32)) return false;
        m->i = (int64_t)b32;
        return true;
    case KAZE_GGUF_INT32:
        if (!rd_u32(r, &b32)) return false;
        m->i = (int64_t)(int32_t)b32;
        return true;
    case KAZE_GGUF_FLOAT32:
        if (!rd_f32(r, &f32v)) return false;
        m->f = (double)f32v;
        m->i = (int64_t)f32v;
        return true;
    case KAZE_GGUF_BOOL:
        if (!rd_u8(r, &b8)) return false;
        m->i = b8 ? 1 : 0;
        return true;
    case KAZE_GGUF_UINT64:
    case KAZE_GGUF_INT64:
        if (!rd_u64(r, &b64)) return false;
        m->i = (int64_t)b64;
        return true;
    case KAZE_GGUF_FLOAT64:
        if (!rd_f64(r, &f64v)) return false;
        m->f = f64v;
        m->i = (int64_t)f64v;
        return true;
    case KAZE_GGUF_STRING:
        return rd_str(r, arena, &m->s);
    case KAZE_GGUF_ARRAY: {
        uint64_t count, i;
        if (!rd_u32(r, &m->elem_type) || !rd_u64(r, &count)) {
            return false;
        }
        m->count = (size_t)count;
        if (count > (uint64_t)(r->size)) {
            return false;
        }
        if (m->elem_type == KAZE_GGUF_STRING) {
            m->strs = (char **)kmalloc(arena, (size_t)count * sizeof(char *));
            if (m->strs == NULL && count > 0) {
                (void)kaze_trap(KAZE_ERR_OOM, "GGUF string array did not fit the metadata arena");
                return false;
            }
            for (i = 0; i < count; i++) {
                if (!rd_str(r, arena, &m->strs[i])) {
                    return false;
                }
            }
            return true;
        }
        if (m->elem_type >= KAZE_GGUF_UINT8 && m->elem_type <= KAZE_GGUF_INT32) {
            m->i32s = (int32_t *)kmalloc(arena, (size_t)count * sizeof(int32_t));
            if (m->i32s == NULL && count > 0) {
                (void)kaze_trap(KAZE_ERR_OOM, "GGUF integer array did not fit the metadata arena");
                return false;
            }
            for (i = 0; i < count; i++) {
                uint8_t u8v;
                uint16_t u16v;
                uint32_t u32v;
                switch (m->elem_type) {
                case KAZE_GGUF_UINT8:
                    if (!rd_u8(r, &u8v)) return false;
                    m->i32s[i] = (int32_t)u8v;
                    break;
                case KAZE_GGUF_INT8:
                    if (!rd_u8(r, &u8v)) return false;
                    m->i32s[i] = (int32_t)(int8_t)u8v;
                    break;
                case KAZE_GGUF_UINT16:
                    if (!rd_u16(r, &u16v)) return false;
                    m->i32s[i] = (int32_t)u16v;
                    break;
                case KAZE_GGUF_INT16:
                    if (!rd_u16(r, &u16v)) return false;
                    m->i32s[i] = (int32_t)(int16_t)u16v;
                    break;
                case KAZE_GGUF_UINT32:
                case KAZE_GGUF_INT32:
                    if (!rd_u32(r, &u32v)) return false;
                    m->i32s[i] = (int32_t)u32v;
                    break;
                default:
                    return false;
                }
            }
            return true;
        }
        if (m->elem_type == KAZE_GGUF_FLOAT32) {
            m->f32s = (float *)kmalloc(arena, (size_t)count * sizeof(float));
            if (m->f32s == NULL && count > 0) {
                (void)kaze_trap(KAZE_ERR_OOM, "GGUF float array did not fit the metadata arena");
                return false;
            }
            for (i = 0; i < count; i++) {
                if (!rd_f32(r, &m->f32s[i])) {
                    return false;
                }
            }
            return true;
        }
        for (i = 0; i < count; i++) {
            if (!skip_value(r, m->elem_type)) {
                return false;
            }
        }
        return true;
    }
    default:
        return false;
    }
}

static uint64_t align_up_u64(uint64_t v, uint64_t a)
{
    if (a == 0 || a > 65536u) {
        a = KAZE_GGUF_DEFAULT_ALIGNMENT;
    }
    return (v + a - 1u) / a * a;
}

static bool parse_gguf(kaze_gguf_t *g, kaze_arena_t *arena)
{
    gguf_reader_t r;
    uint32_t magic, version, i;
    uint64_t n_tensors, n_kv, k;
    uint64_t *rel_offsets;
    uint64_t pos, data_offset;
    int64_t alignment = KAZE_GGUF_DEFAULT_ALIGNMENT;

    r.p = g->map;
    r.size = g->size;
    r.off = 0;
    if (!rd_u32(&r, &magic) || magic != KAZE_GGUF_MAGIC) {
        gguf_fail("not a GGUF file (bad magic)");
        return false;
    }
    if (!rd_u32(&r, &version) || version < 2 || version > 3) {
        gguf_fail("unsupported GGUF version");
        return false;
    }
    if (!rd_u64(&r, &n_tensors) || !rd_u64(&r, &n_kv)) {
        gguf_fail("truncated GGUF header");
        return false;
    }
    if (n_kv > 100000u || n_tensors > 100000u) {
        gguf_fail("GGUF header claims an implausible table size");
        return false;
    }
    g->version = version;
    g->n_tensors = n_tensors;
    g->n_kv = n_kv;
    g->metas = (kaze_gguf_meta_t *)kmalloc(arena, (size_t)n_kv * sizeof(kaze_gguf_meta_t));
    g->tensors = (kaze_tensor_t *)kmalloc(arena, (size_t)n_tensors * sizeof(kaze_tensor_t));
    rel_offsets = (uint64_t *)kmalloc(arena, (size_t)n_tensors * sizeof(uint64_t));
    if ((g->metas == NULL && n_kv > 0) || (g->tensors == NULL && n_tensors > 0) ||
        (rel_offsets == NULL && n_tensors > 0)) {
        (void)kaze_trap(KAZE_ERR_OOM, "GGUF tables did not fit the metadata arena");
        return false;
    }
    if (n_kv > 0) {
        memset(g->metas, 0, (size_t)n_kv * sizeof(kaze_gguf_meta_t));
    }
    if (n_tensors > 0) {
        memset(g->tensors, 0, (size_t)n_tensors * sizeof(kaze_tensor_t));
    }
    for (k = 0; k < n_kv; k++) {
        uint32_t type;
        if (!rd_str(&r, arena, &g->metas[k].key) || !rd_u32(&r, &type) ||
            !parse_meta_value(&r, arena, type, &g->metas[k])) {
            gguf_fail("malformed GGUF metadata entry");
            return false;
        }
    }
    for (k = 0; k < n_tensors; k++) {
        kaze_tensor_t *t = &g->tensors[k];
        char *name = NULL;
        uint32_t ndims = 0, type = 0;
        uint64_t dims[KAZE_TENSOR_MAX_DIMS];
        uint64_t offset = 0;
        kaze_dtype_t dtype;
        int32_t d;

        if (!rd_str(&r, arena, &name) || !rd_u32(&r, &ndims) ||
            ndims < 1 || ndims > KAZE_TENSOR_MAX_DIMS) {
            gguf_fail("malformed GGUF tensor header");
            return false;
        }
        for (i = 0; i < ndims; i++) {
            if (!rd_u64(&r, &dims[i]) || dims[i] == 0 || dims[i] > (uint64_t)INT64_MAX) {
                gguf_fail("malformed GGUF tensor dimensions");
                return false;
            }
        }
        if (!rd_u32(&r, &type) || !rd_u64(&r, &offset)) {
            gguf_fail("truncated GGUF tensor header");
            return false;
        }
        if (!kaze_dtype_from_gguf(type, &dtype)) {
            gguf_fail("unsupported GGUF tensor type in this build");
            return false;
        }
        t->dtype = dtype;
        t->rank = (int32_t)ndims;
        for (d = 0; d < (int32_t)ndims; d++) {
            t->shape[d] = (int64_t)dims[ndims - 1 - (uint32_t)d];
        }
        {
            int64_t step = 1;
            for (d = (int32_t)ndims - 1; d >= 0; d--) {
                t->strides[d] = step;
                step *= t->shape[d];
            }
        }
        t->nbytes = kaze_tensor_bytes(t);
        t->flags = KAZE_TENSOR_FLAG_MMAP;
        t->name = name;
        rel_offsets[k] = offset;
    }
    for (k = 0; k < n_kv; k++) {
        if (strcmp(g->metas[k].key, "general.alignment") == 0 && g->metas[k].i > 0) {
            alignment = g->metas[k].i;
        }
    }
    g->alignment = (uint32_t)alignment;
    pos = (uint64_t)r.off;
    data_offset = align_up_u64(pos, (uint64_t)alignment);
    g->data_offset = data_offset;
    for (k = 0; k < n_tensors; k++) {
        kaze_tensor_t *t = &g->tensors[k];
        if (data_offset + rel_offsets[k] + (uint64_t)t->nbytes > (uint64_t)g->size ||
            rel_offsets[k] > (uint64_t)g->size) {
            gguf_fail("GGUF tensor data lies outside the mapped file");
            return false;
        }
        t->data = (void *)(g->map + data_offset + rel_offsets[k]);
    }
    return true;
}

kaze_gguf_t *kaze_gguf_open(kaze_arena_t *meta, const char *path)
{
    kaze_gguf_t *g;

    if (path == NULL || meta == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "kaze_gguf_open needs a path and a metadata arena");
        return NULL;
    }
#if KAZE_GGUF_UNAVAILABLE
    (void)kaze_trap(KAZE_ERR_IO, "GGUF mmap ingestion is unavailable on this platform");
    return NULL;
#else
    g = (kaze_gguf_t *)kmalloc(meta, sizeof(kaze_gguf_t));
    if (g == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "GGUF handle did not fit the metadata arena");
        return NULL;
    }
    memset(g, 0, sizeof *g);
    g->arena = meta;
    g->fd = -1;
    g->fd = open(path, O_RDONLY);
    if (g->fd < 0) {
        (void)kaze_trap(KAZE_ERR_IO, "cannot open the model file");
        return NULL;
    }
    {
        struct stat st;
        if (fstat(g->fd, &st) != 0 || st.st_size <= 0) {
            (void)kaze_trap(KAZE_ERR_IO, "cannot stat the model file");
            (void)close(g->fd);
            g->fd = -1;
            return NULL;
        }
        g->size = (size_t)st.st_size;
    }
    g->map = (const uint8_t *)mmap(NULL, g->size, PROT_READ, MAP_PRIVATE, g->fd, 0);
    if (g->map == MAP_FAILED) {
        g->map = NULL;
        (void)kaze_trap(KAZE_ERR_IO, "cannot mmap the model file");
        (void)close(g->fd);
        g->fd = -1;
        return NULL;
    }
    if (!parse_gguf(g, meta)) {
        (void)munmap((void *)g->map, g->size);
        g->map = NULL;
        (void)close(g->fd);
        g->fd = -1;
        return NULL;
    }
    return g;
#endif
}

void kaze_gguf_close(kaze_gguf_t *g)
{
    if (g == NULL) {
        return;
    }
#if !KAZE_GGUF_UNAVAILABLE
    if (g->map != NULL) {
        (void)munmap((void *)g->map, g->size);
        g->map = NULL;
    }
    if (g->fd >= 0) {
        (void)close(g->fd);
        g->fd = -1;
    }
#endif
}

size_t kaze_gguf_tensor_count(const kaze_gguf_t *g)
{
    return (g == NULL) ? 0u : (size_t)g->n_tensors;
}

const kaze_tensor_t *kaze_gguf_tensor_at(const kaze_gguf_t *g, size_t index)
{
    if (g == NULL || index >= (size_t)g->n_tensors) {
        return NULL;
    }
    return &g->tensors[index];
}

const kaze_tensor_t *kaze_gguf_tensor(const kaze_gguf_t *g, const char *name)
{
    size_t i;

    if (g == NULL || name == NULL) {
        return NULL;
    }
    for (i = 0; i < (size_t)g->n_tensors; i++) {
        if (g->tensors[i].name != NULL && strcmp(g->tensors[i].name, name) == 0) {
            return &g->tensors[i];
        }
    }
    return NULL;
}

size_t kaze_gguf_meta_count(const kaze_gguf_t *g)
{
    return (g == NULL) ? 0u : (size_t)g->n_kv;
}

const char *kaze_gguf_meta_key(const kaze_gguf_t *g, size_t index)
{
    if (g == NULL || index >= (size_t)g->n_kv) {
        return NULL;
    }
    return g->metas[index].key;
}

static const kaze_gguf_meta_t *find_meta(const kaze_gguf_t *g, const char *key)
{
    size_t i;

    if (g == NULL || key == NULL) {
        return NULL;
    }
    for (i = 0; i < (size_t)g->n_kv; i++) {
        if (strcmp(g->metas[i].key, key) == 0) {
            return &g->metas[i];
        }
    }
    return NULL;
}

bool kaze_gguf_meta_i64(const kaze_gguf_t *g, const char *key, int64_t *out)
{
    const kaze_gguf_meta_t *m = find_meta(g, key);
    if (m == NULL || out == NULL || m->type == KAZE_GGUF_STRING || m->type == KAZE_GGUF_ARRAY) {
        return false;
    }
    *out = (m->type == KAZE_GGUF_FLOAT32 || m->type == KAZE_GGUF_FLOAT64)
               ? (int64_t)m->f : m->i;
    return true;
}

bool kaze_gguf_meta_f32(const kaze_gguf_t *g, const char *key, float *out)
{
    const kaze_gguf_meta_t *m = find_meta(g, key);
    if (m == NULL || out == NULL || m->type == KAZE_GGUF_STRING || m->type == KAZE_GGUF_ARRAY) {
        return false;
    }
    *out = (m->type == KAZE_GGUF_FLOAT32 || m->type == KAZE_GGUF_FLOAT64)
               ? (float)m->f : (float)m->i;
    return true;
}

bool kaze_gguf_meta_str(const kaze_gguf_t *g, const char *key, const char **out)
{
    const kaze_gguf_meta_t *m = find_meta(g, key);
    if (m == NULL || out == NULL || m->type != KAZE_GGUF_STRING || m->s == NULL) {
        return false;
    }
    *out = m->s;
    return true;
}

bool kaze_gguf_meta_bool(const kaze_gguf_t *g, const char *key, bool *out)
{
    const kaze_gguf_meta_t *m = find_meta(g, key);
    if (m == NULL || out == NULL || m->type != KAZE_GGUF_BOOL) {
        return false;
    }
    *out = m->i != 0;
    return true;
}

bool kaze_gguf_meta_str_array(const kaze_gguf_t *g, const char *key,
                              const char ***items, size_t *count)
{
    const kaze_gguf_meta_t *m = find_meta(g, key);
    if (m == NULL || items == NULL || count == NULL || m->type != KAZE_GGUF_ARRAY ||
        m->elem_type != KAZE_GGUF_STRING || m->strs == NULL) {
        return false;
    }
    *items = (const char **)m->strs;
    *count = m->count;
    return true;
}

bool kaze_gguf_meta_i32_array(const kaze_gguf_t *g, const char *key,
                              const int32_t **items, size_t *count)
{
    const kaze_gguf_meta_t *m = find_meta(g, key);
    if (m == NULL || items == NULL || count == NULL || m->type != KAZE_GGUF_ARRAY ||
        m->i32s == NULL) {
        return false;
    }
    *items = (const int32_t *)m->i32s;
    *count = m->count;
    return true;
}

/* ------------------------------------------------------------------------- */
/* SafeTensors: 8-byte header length + JSON header + raw tensor data          */
/* ------------------------------------------------------------------------- */

typedef struct st_entry_t {
    char *name;
    char *dtype;
    int32_t rank;
    int64_t shape[KAZE_TENSOR_MAX_DIMS];
    uint64_t begin;
    uint64_t end;
    struct st_entry_t *next;
} st_entry_t;

typedef struct json_reader_t {
    const char *p;
    size_t n;
    size_t i;
} json_reader_t;

static void j_ws(json_reader_t *r)
{
    while (r->i < r->n) {
        char c = r->p[r->i];
        if (c == ' ' || c == '\t' || c == '\n' || c == '\r') {
            r->i++;
        } else {
            break;
        }
    }
}

static bool j_expect(json_reader_t *r, char want)
{
    j_ws(r);
    if (r->i >= r->n || r->p[r->i] != want) {
        return false;
    }
    r->i++;
    return true;
}

static bool j_string(json_reader_t *r, kaze_arena_t *arena, char **out)
{
    size_t start;
    char *s;
    size_t len = 0;

    j_ws(r);
    if (r->i >= r->n || r->p[r->i] != '"') {
        return false;
    }
    r->i++;
    start = r->i;
    while (r->i < r->n && r->p[r->i] != '"') {
        if (r->p[r->i] == '\\' && r->i + 1 < r->n) {
            r->i++;
        }
        r->i++;
    }
    if (r->i >= r->n) {
        return false;
    }
    s = (char *)kmalloc(arena, (r->i - start) + 1u);
    if (s == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "SafeTensors header string did not fit the metadata arena");
        return false;
    }
    {
        size_t k = start;
        while (k < r->i) {
            if (r->p[k] == '\\' && k + 1 < r->i) {
                char esc = r->p[k + 1];
                k += 2;
                switch (esc) {
                case 'n': s[len++] = '\n'; break;
                case 't': s[len++] = '\t'; break;
                case 'r': s[len++] = '\r'; break;
                case '"': s[len++] = '"'; break;
                case '\\': s[len++] = '\\'; break;
                case '/': s[len++] = '/'; break;
                default: s[len++] = '?'; break;
                }
            } else {
                s[len++] = r->p[k++];
            }
        }
    }
    s[len] = '\0';
    r->i++;
    *out = s;
    return true;
}

static bool j_number(json_reader_t *r, double *out)
{
    char buf[64];
    size_t k = 0;

    j_ws(r);
    if (r->i < r->n && r->p[r->i] == '-') {
        buf[k++] = r->p[r->i++];
    }
    while (r->i < r->n && r->p[r->i] >= '0' && r->p[r->i] <= '9' && k + 1 < sizeof buf) {
        buf[k++] = r->p[r->i++];
    }
    if (r->i < r->n && r->p[r->i] == '.') {
        buf[k++] = r->p[r->i++];
        while (r->i < r->n && r->p[r->i] >= '0' && r->p[r->i] <= '9' && k + 1 < sizeof buf) {
            buf[k++] = r->p[r->i++];
        }
    }
    if (r->i < r->n && (r->p[r->i] == 'e' || r->p[r->i] == 'E')) {
        buf[k++] = r->p[r->i++];
        if (r->i < r->n && (r->p[r->i] == '+' || r->p[r->i] == '-')) {
            buf[k++] = r->p[r->i++];
        }
        while (r->i < r->n && r->p[r->i] >= '0' && r->p[r->i] <= '9' && k + 1 < sizeof buf) {
            buf[k++] = r->p[r->i++];
        }
    }
    if (k == 0) {
        return false;
    }
    buf[k] = '\0';
    *out = strtod(buf, NULL);
    return true;
}

static bool j_skip(json_reader_t *r)
{
    j_ws(r);
    if (r->i >= r->n) {
        return false;
    }
    if (r->p[r->i] == '"') {
        char *tmp;
        kaze_arena_t *scratch = NULL;
        (void)scratch;
        /* A string must still be consumed; use a bounded manual scan. */
        r->i++;
        while (r->i < r->n && r->p[r->i] != '"') {
            if (r->p[r->i] == '\\') {
                r->i++;
            }
            r->i++;
        }
        if (r->i >= r->n) {
            return false;
        }
        r->i++;
        tmp = NULL;
        (void)tmp;
        return true;
    }
    if (r->p[r->i] == '{' || r->p[r->i] == '[') {
        char open = r->p[r->i];
        char close = open == '{' ? '}' : ']';
        int depth = 0;
        while (r->i < r->n) {
            char c = r->p[r->i];
            if (c == '"') {
                r->i++;
                while (r->i < r->n && r->p[r->i] != '"') {
                    if (r->p[r->i] == '\\') {
                        r->i++;
                    }
                    r->i++;
                }
                if (r->i < r->n) {
                    r->i++;
                }
                continue;
            }
            if (c == open) {
                depth++;
            } else if (c == close) {
                depth--;
                if (depth == 0) {
                    r->i++;
                    return true;
                }
            }
            r->i++;
        }
        return false;
    }
    if (r->n - r->i >= 4 && strncmp(r->p + r->i, "true", 4) == 0) {
        r->i += 4;
        return true;
    }
    if (r->n - r->i >= 5 && strncmp(r->p + r->i, "false", 5) == 0) {
        r->i += 5;
        return true;
    }
    if (r->n - r->i >= 4 && strncmp(r->p + r->i, "null", 4) == 0) {
        r->i += 4;
        return true;
    }
    {
        double tmp;
        return j_number(r, &tmp);
    }
}

static bool st_dtype(const char *s, kaze_dtype_t *out)
{
    if (strcmp(s, "F32") == 0 || strcmp(s, "f32") == 0) { *out = KAZE_DTYPE_F32; return true; }
    if (strcmp(s, "F16") == 0 || strcmp(s, "f16") == 0) { *out = KAZE_DTYPE_F16; return true; }
    if (strcmp(s, "BF16") == 0 || strcmp(s, "bf16") == 0) { *out = KAZE_DTYPE_BF16; return true; }
    if (strcmp(s, "I8") == 0) { *out = KAZE_DTYPE_I8; return true; }
    if (strcmp(s, "BOOL") == 0) { *out = KAZE_DTYPE_BOOL; return true; }
    return false;
}

static bool parse_st_entry(json_reader_t *r, kaze_arena_t *arena, st_entry_t *e)
{
    bool saw_dtype = false, saw_shape = false, saw_offsets = false;

    if (!j_expect(r, '{')) {
        return false;
    }
    for (;;) {
        char *key = NULL;
        if (!j_string(r, arena, &key) || !j_expect(r, ':')) {
            return false;
        }
        if (strcmp(key, "dtype") == 0) {
            if (!j_string(r, arena, &e->dtype)) {
                return false;
            }
            saw_dtype = true;
        } else if (strcmp(key, "shape") == 0) {
            int32_t rank = 0;
            if (!j_expect(r, '[')) {
                return false;
            }
            for (;;) {
                double v;
                j_ws(r);
                if (r->i < r->n && r->p[r->i] == ']') {
                    r->i++;
                    break;
                }
                if (rank >= KAZE_TENSOR_MAX_DIMS || !j_number(r, &v) || v < 1.0 ||
                    v > 9.2e18) {
                    return false;
                }
                e->shape[rank++] = (int64_t)v;
                j_ws(r);
                if (r->i < r->n && r->p[r->i] == ',') {
                    r->i++;
                    continue;
                }
                if (r->i < r->n && r->p[r->i] == ']') {
                    r->i++;
                    break;
                }
                return false;
            }
            e->rank = rank;
            saw_shape = true;
        } else if (strcmp(key, "data_offsets") == 0) {
            double a = 0.0, b = 0.0;
            if (!j_expect(r, '[') || !j_number(r, &a) || !j_expect(r, ',') ||
                !j_number(r, &b) || !j_expect(r, ']')) {
                return false;
            }
            if (a < 0.0 || b < a) {
                return false;
            }
            e->begin = (uint64_t)a;
            e->end = (uint64_t)b;
            saw_offsets = true;
        } else if (!j_skip(r)) {
            return false;
        }
        j_ws(r);
        if (r->i < r->n && r->p[r->i] == ',') {
            r->i++;
            continue;
        }
        if (r->i < r->n && r->p[r->i] == '}') {
            r->i++;
            break;
        }
        return false;
    }
    return saw_dtype && saw_shape && saw_offsets;
}

static bool parse_safetensors(kaze_gguf_t *g, kaze_arena_t *arena)
{
    uint64_t header_len = 0;
    json_reader_t r;
    st_entry_t *head = NULL;
    st_entry_t **tail = &head;
    size_t count = 0, index = 0;

    if (g->size < 8 || g->map == NULL) {
        return false;
    }
    memcpy(&header_len, g->map, 8);
    if (header_len > (uint64_t)g->size - 8u) {
        return false;
    }
    r.p = (const char *)(g->map + 8);
    r.n = (size_t)header_len;
    r.i = 0;
    if (!j_expect(&r, '{')) {
        return false;
    }
    for (;;) {
        char *name = NULL;
        j_ws(&r);
        if (r.i < r.n && r.p[r.i] == '}') {
            r.i++;
            break;
        }
        if (!j_string(&r, arena, &name) || !j_expect(&r, ':')) {
            return false;
        }
        if (strcmp(name, "__metadata__") == 0) {
            if (!j_skip(&r)) {
                return false;
            }
        } else {
            st_entry_t *e = (st_entry_t *)kmalloc(arena, sizeof(st_entry_t));
            if (e == NULL) {
                (void)kaze_trap(KAZE_ERR_OOM, "SafeTensors entry did not fit the metadata arena");
                return false;
            }
            memset(e, 0, sizeof *e);
            e->name = name;
            if (!parse_st_entry(&r, arena, e)) {
                return false;
            }
            *tail = e;
            tail = &e->next;
            count++;
        }
        j_ws(&r);
        if (r.i < r.n && r.p[r.i] == ',') {
            r.i++;
            continue;
        }
        if (r.i < r.n && r.p[r.i] == '}') {
            r.i++;
            break;
        }
        return false;
    }
    g->version = 0;
    g->n_kv = 0;
    g->metas = NULL;
    g->n_tensors = count;
    g->data_offset = 8u + header_len;
    g->tensors = (kaze_tensor_t *)kmalloc(arena, count * sizeof(kaze_tensor_t));
    if (g->tensors == NULL && count > 0) {
        (void)kaze_trap(KAZE_ERR_OOM, "SafeTensors table did not fit the metadata arena");
        return false;
    }
    {
        st_entry_t *e = head;
        while (e != NULL) {
            kaze_tensor_t *t = &g->tensors[index];
            kaze_dtype_t dtype;
            int64_t expected;
            int32_t d;
            if (!st_dtype(e->dtype, &dtype) || e->rank < 1) {
                (void)kaze_trap(KAZE_ERR_FORMAT, "unsupported SafeTensors dtype");
                return false;
            }
            memset(t, 0, sizeof *t);
            t->dtype = dtype;
            t->rank = e->rank;
            for (d = 0; d < e->rank; d++) {
                t->shape[d] = e->shape[d];
            }
            {
                int64_t step = 1;
                for (d = e->rank - 1; d >= 0; d--) {
                    t->strides[d] = step;
                    step *= t->shape[d];
                }
            }
            expected = kaze_tensor_numel(t) > 0 ? (int64_t)kaze_tensor_bytes(t) : 0;
            if (e->end - e->begin != (uint64_t)expected ||
                g->data_offset + e->end > (uint64_t)g->size) {
                (void)kaze_trap(KAZE_ERR_FORMAT, "SafeTensors data offset does not match its shape");
                return false;
            }
            t->nbytes = (size_t)expected;
            t->data = (void *)(g->map + g->data_offset + e->begin);
            t->flags = KAZE_TENSOR_FLAG_MMAP;
            t->name = e->name;
            e = e->next;
            index++;
        }
    }
    return true;
}

kaze_gguf_t *kaze_safetensors_open(kaze_arena_t *meta, const char *path)
{
    kaze_gguf_t *g;

    if (path == NULL || meta == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "kaze_safetensors_open needs a path and a metadata arena");
        return NULL;
    }
#if KAZE_GGUF_UNAVAILABLE
    (void)kaze_trap(KAZE_ERR_IO, "SafeTensors mmap ingestion is unavailable on this platform");
    return NULL;
#else
    g = (kaze_gguf_t *)kmalloc(meta, sizeof(kaze_gguf_t));
    if (g == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "SafeTensors handle did not fit the metadata arena");
        return NULL;
    }
    memset(g, 0, sizeof *g);
    g->arena = meta;
    g->fd = open(path, O_RDONLY);
    if (g->fd < 0) {
        (void)kaze_trap(KAZE_ERR_IO, "cannot open the SafeTensors file");
        return NULL;
    }
    {
        struct stat st;
        if (fstat(g->fd, &st) != 0 || st.st_size <= 0) {
            (void)kaze_trap(KAZE_ERR_IO, "cannot stat the SafeTensors file");
            (void)close(g->fd);
            g->fd = -1;
            return NULL;
        }
        g->size = (size_t)st.st_size;
    }
    g->map = (const uint8_t *)mmap(NULL, g->size, PROT_READ, MAP_PRIVATE, g->fd, 0);
    if (g->map == MAP_FAILED) {
        g->map = NULL;
        (void)kaze_trap(KAZE_ERR_IO, "cannot mmap the SafeTensors file");
        (void)close(g->fd);
        g->fd = -1;
        return NULL;
    }
    if (!parse_safetensors(g, meta)) {
        (void)kaze_trap(KAZE_ERR_FORMAT, "malformed SafeTensors header");
        (void)munmap((void *)g->map, g->size);
        g->map = NULL;
        (void)close(g->fd);
        g->fd = -1;
        return NULL;
    }
    return g;
#endif
}


