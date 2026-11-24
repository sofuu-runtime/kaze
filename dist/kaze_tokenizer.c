/* kaze_tokenizer.c — GGUF byte-level BPE tokenizer (Phase 12).
 *
 * SmolLM2 / GPT-2 style: the vocabulary and merges are read from GGUF metadata,
 * tokens are stored in the byte-level "unicode" alphabet, and encoding is the
 * usual pre-tokenize -> merge -> vocabulary lookup. The pre-tokenizer is the
 * SMOLLM rule set llama.cpp uses for this family: each digit is its own word,
 * an optional single leading space binds to the next word, punctuation runs
 * stand alone, and a whitespace run keeps all but its last character before a
 * word (the last one binds to the word).
 *
 * Non-ASCII input is treated as letters at the UTF-8 byte level. That matches
 * the reference for Latin text; arbitrary scripts (CJK, emoji) are not a claim
 * of this implementation and are reported as a documented limitation.
 */
#include "kaze_tensor.h"

#include <string.h>

typedef struct kt_str_t {
    const uint8_t *data;
    uint32_t len;
} kt_str_t;

typedef struct kt_vocab_entry_t {
    uint64_t hash;
    const uint8_t *key;
    uint32_t len;
    int32_t id;
} kt_vocab_entry_t;

typedef struct kt_vocab_t {
    kt_vocab_entry_t *entries;
    size_t cap;  /* power of two */
    size_t len;
} kt_vocab_t;

typedef struct kt_merge_entry_t {
    uint64_t hash;
    const uint8_t *left;
    uint32_t llen;
    const uint8_t *right;
    uint32_t rlen;
    int32_t rank;
    const uint8_t *result;
    uint32_t result_len;
} kt_merge_entry_t;

typedef struct kt_merges_t {
    kt_merge_entry_t *entries;
    size_t cap;
    size_t len;
} kt_merges_t;

struct kaze_tokenizer_t {
    kaze_arena_t *arena;
    int32_t vocab_size;
    int32_t bos;
    int32_t eos;
    int32_t unk;
    char **token_text;
    uint8_t *token_type;
    kt_vocab_t vocab;
    kt_merges_t merges;
};

/* GPT-2 byte <-> unicode alphabet (bytes_to_unicode). c2b[cp] is the original
 * byte for a mapped codepoint, or -1 when the codepoint is not from the table. */
static int32_t kt_c2b[512];
static bool kt_c2b_ready = false;

static void kt_init_c2b(void)
{
    int32_t b, n = 0;

    if (kt_c2b_ready) {
        return;
    }
    for (b = 0; b < 512; b++) {
        kt_c2b[b] = -1;
    }
    for (b = 33; b <= 126; b++) {
        kt_c2b[b] = b;
    }
    for (b = 161; b <= 172; b++) {
        kt_c2b[b] = b;
    }
    for (b = 174; b <= 255; b++) {
        kt_c2b[b] = b;
    }
    for (b = 0; b < 256; b++) {
        if (kt_c2b[b] < 0) {
            kt_c2b[256 + n] = b;
            n++;
        }
    }
    kt_c2b_ready = true;
}

static uint64_t kt_hash(const uint8_t *data, size_t len)
{
    uint64_t h = 1469598103934665603ull;
    size_t i;
    for (i = 0; i < len; i++) {
        h ^= (uint64_t)data[i];
        h *= 1099511628211ull;
    }
    return h;
}

static uint64_t kt_hash_pair(const uint8_t *a, size_t alen,
                             const uint8_t *b, size_t blen)
{
    uint64_t h = 1469598103934665603ull;
    size_t i;
    for (i = 0; i < alen; i++) {
        h ^= (uint64_t)a[i];
        h *= 1099511628211ull;
    }
    h ^= 0xFFu;
    h *= 1099511628211ull;
    for (i = 0; i < blen; i++) {
        h ^= (uint64_t)b[i];
        h *= 1099511628211ull;
    }
    h ^= (uint64_t)alen << 32 | (uint64_t)blen;
    h *= 1099511628211ull;
    return h;
}

static uint8_t *kt_alloc(kaze_arena_t *arena, size_t n)
{
    uint8_t *p = (uint8_t *)kmalloc(arena, n == 0 ? 1u : n);
    if (p == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "tokenizer allocation did not fit the metadata arena");
    }
    return p;
}

/* Decode a UTF-8 token/merge string into the original byte alphabet. */
static bool kt_mapped_to_bytes(kaze_arena_t *arena, const char *s, size_t slen,
                               kt_str_t *out)
{
    uint8_t *buf = kt_alloc(arena, slen == 0 ? 1u : slen);
    size_t i = 0, o = 0;

    if (buf == NULL) {
        return false;
    }
    while (i < slen) {
        uint8_t c = (uint8_t)s[i];
        uint32_t cp;
        int extra, k;

        if (c < 0x80) {
            cp = c;
            extra = 0;
        } else if ((c & 0xE0) == 0xC0) {
            cp = (uint32_t)(c & 0x1F);
            extra = 1;
        } else if ((c & 0xF0) == 0xE0) {
            cp = (uint32_t)(c & 0x0F);
            extra = 2;
        } else if ((c & 0xF8) == 0xF0) {
            cp = (uint32_t)(c & 0x07);
            extra = 3;
        } else {
            return false;
        }
        if (i + (size_t)extra >= slen + 1 && extra > 0) {
            return false;
        }
        for (k = 1; k <= extra; k++) {
            if (i + (size_t)k >= slen || ((uint8_t)s[i + (size_t)k] & 0xC0) != 0x80) {
                return false;
            }
            cp = (cp << 6) | (uint32_t)((uint8_t)s[i + (size_t)k] & 0x3F);
        }
        i += (size_t)extra + 1;
        if (cp >= 512 || kt_c2b[cp] < 0) {
            return false;
        }
        buf[o++] = (uint8_t)kt_c2b[cp];
    }
    out->data = buf;
    out->len = (uint32_t)o;
    return true;
}

static void kt_vocab_init(kaze_arena_t *arena, kt_vocab_t *v, size_t expected)
{
    size_t cap = 8;
    while (cap < expected * 2 + 8) {
        cap <<= 1;
    }
    v->cap = cap;
    v->len = 0;
    v->entries = (kt_vocab_entry_t *)kmalloc(arena, cap * sizeof(kt_vocab_entry_t));
    if (v->entries == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "tokenizer vocabulary table did not fit");
        return;
    }
    memset(v->entries, 0, cap * sizeof(kt_vocab_entry_t));
}

static void kt_vocab_put(kt_vocab_t *v, const uint8_t *key, uint32_t len, int32_t id)
{
    uint64_t h = kt_hash(key, len);
    size_t mask = v->cap - 1;
    size_t i = (size_t)h & mask;
    size_t probes = 0;

    while (v->entries[i].key != NULL) {
        if (v->entries[i].hash == h && v->entries[i].len == len &&
            memcmp(v->entries[i].key, key, len) == 0) {
            v->entries[i].id = id;
            return;
        }
        i = (i + 1) & mask;
        probes++;
        if (probes > v->cap) {
            return;
        }
    }
    v->entries[i].hash = h;
    v->entries[i].key = key;
    v->entries[i].len = len;
    v->entries[i].id = id;
    v->len++;
}

static int32_t kt_vocab_get(const kt_vocab_t *v, const uint8_t *key, uint32_t len)
{
    uint64_t h = kt_hash(key, len);
    size_t mask = v->cap - 1;
    size_t i = (size_t)h & mask;
    size_t probes = 0;

    while (v->entries[i].key != NULL) {
        if (v->entries[i].hash == h && v->entries[i].len == len &&
            memcmp(v->entries[i].key, key, len) == 0) {
            return v->entries[i].id;
        }
        i = (i + 1) & mask;
        probes++;
        if (probes > v->cap) {
            break;
        }
    }
    return -1;
}

static void kt_merges_init(kaze_arena_t *arena, kt_merges_t *m, size_t expected)
{
    size_t cap = 8;
    while (cap < expected * 2 + 8) {
        cap <<= 1;
    }
    m->cap = cap;
    m->len = 0;
    m->entries = (kt_merge_entry_t *)kmalloc(arena, cap * sizeof(kt_merge_entry_t));
    if (m->entries == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "tokenizer merge table did not fit");
        return;
    }
    memset(m->entries, 0, cap * sizeof(kt_merge_entry_t));
}

static void kt_merges_put(kt_merges_t *m, const uint8_t *left, uint32_t llen,
                          const uint8_t *right, uint32_t rlen, int32_t rank,
                          const uint8_t *result, uint32_t result_len)
{
    uint64_t h = kt_hash_pair(left, llen, right, rlen);
    size_t mask = m->cap - 1;
    size_t i = (size_t)h & mask;
    size_t probes = 0;

    while (m->entries[i].left != NULL) {
        if (m->entries[i].hash == h && m->entries[i].llen == llen &&
            m->entries[i].rlen == rlen &&
            memcmp(m->entries[i].left, left, llen) == 0 &&
            memcmp(m->entries[i].right, right, rlen) == 0) {
            return;
        }
        i = (i + 1) & mask;
        probes++;
        if (probes > m->cap) {
            return;
        }
    }
    m->entries[i].hash = h;
    m->entries[i].left = left;
    m->entries[i].llen = llen;
    m->entries[i].right = right;
    m->entries[i].rlen = rlen;
    m->entries[i].rank = rank;
    m->entries[i].result = result;
    m->entries[i].result_len = result_len;
    m->len++;
}

static const kt_merge_entry_t *kt_merges_get(const kt_merges_t *m,
                                             const uint8_t *left, uint32_t llen,
                                             const uint8_t *right, uint32_t rlen)
{
    uint64_t h = kt_hash_pair(left, llen, right, rlen);
    size_t mask = m->cap - 1;
    size_t i = (size_t)h & mask;
    size_t probes = 0;

    while (m->entries[i].left != NULL) {
        if (m->entries[i].hash == h && m->entries[i].llen == llen &&
            m->entries[i].rlen == rlen &&
            memcmp(m->entries[i].left, left, llen) == 0 &&
            memcmp(m->entries[i].right, right, rlen) == 0) {
            return &m->entries[i];
        }
        i = (i + 1) & mask;
        probes++;
        if (probes > m->cap) {
            break;
        }
    }
    return NULL;
}

/* ------------------------------------------------------------------------- */
/* Construction from GGUF                                                     */
/* ------------------------------------------------------------------------- */

kaze_tokenizer_t *kaze_tokenizer_from_gguf(kaze_arena_t *arena, const kaze_gguf_t *g)
{
    kaze_tokenizer_t *t;
    const char **tokens = NULL;
    const char **merges = NULL;
    const int32_t *types = NULL;
    size_t n_tokens = 0, n_merges = 0, i;
    const char *model = NULL;
    int64_t special;

    if (arena == NULL || g == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "tokenizer needs a metadata arena and a GGUF handle");
        return NULL;
    }
    if (!kaze_gguf_meta_str(g, "tokenizer.ggml.model", &model) ||
        strcmp(model, "gpt2") != 0) {
        (void)kaze_trap(KAZE_ERR_FORMAT, "Phase 12 tokenizer supports the gpt2 byte-level BPE model only");
        return NULL;
    }
    if (!kaze_gguf_meta_str_array(g, "tokenizer.ggml.tokens", &tokens, &n_tokens) ||
        !kaze_gguf_meta_str_array(g, "tokenizer.ggml.merges", &merges, &n_merges) ||
        n_tokens == 0) {
        (void)kaze_trap(KAZE_ERR_FORMAT, "GGUF tokenizer metadata is missing tokens or merges");
        return NULL;
    }
    (void)kaze_gguf_meta_i32_array(g, "tokenizer.ggml.token_type", &types, &n_tokens);
    kt_init_c2b();
    t = (kaze_tokenizer_t *)kmalloc(arena, sizeof(kaze_tokenizer_t));
    if (t == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "tokenizer handle did not fit the metadata arena");
        return NULL;
    }
    memset(t, 0, sizeof *t);
    t->arena = arena;
    t->vocab_size = (int32_t)n_tokens;
    t->token_text = (char **)kmalloc(arena, n_tokens * sizeof(char *));
    t->token_type = (uint8_t *)kmalloc(arena, n_tokens);
    if (t->token_text == NULL || t->token_type == NULL) {
        (void)kaze_trap(KAZE_ERR_OOM, "tokenizer token tables did not fit");
        return NULL;
    }
    kt_vocab_init(arena, &t->vocab, n_tokens);
    for (i = 0; i < n_tokens; i++) {
        kt_str_t key;
        t->token_text[i] = (char *)tokens[i];
        t->token_type[i] = (types != NULL) ? (uint8_t)types[i] : 1u;
        if (kt_mapped_to_bytes(arena, tokens[i], strlen(tokens[i]), &key)) {
            kt_vocab_put(&t->vocab, key.data, key.len, (int32_t)i);
        }
    }
    kt_merges_init(arena, &t->merges, n_merges);
    for (i = 0; i < n_merges; i++) {
        const char *merge = merges[i];
        const char *space = strchr(merge, ' ');
        kt_str_t left, right;
        uint8_t *result;
        if (space == NULL) {
            continue;
        }
        if (!kt_mapped_to_bytes(arena, merge, (size_t)(space - merge), &left) ||
            !kt_mapped_to_bytes(arena, space + 1, strlen(space + 1), &right)) {
            continue;
        }
        result = kt_alloc(arena, (size_t)left.len + right.len);
        if (result == NULL) {
            return NULL;
        }
        if (left.len > 0) memcpy(result, left.data, left.len);
        if (right.len > 0) memcpy(result + left.len, right.data, right.len);
        kt_merges_put(&t->merges, left.data, left.len, right.data, right.len,
                      (int32_t)i, result, left.len + right.len);
    }
    t->bos = kaze_gguf_meta_i64(g, "tokenizer.ggml.bos_token_id", &special)
                 ? (int32_t)special : 1;
    t->eos = kaze_gguf_meta_i64(g, "tokenizer.ggml.eos_token_id", &special)
                 ? (int32_t)special : 2;
    t->unk = kaze_gguf_meta_i64(g, "tokenizer.ggml.unknown_token_id", &special)
                 ? (int32_t)special : 0;
    return t;
}

/* ------------------------------------------------------------------------- */
/* Pre-tokenization (the SMOLLM rule set llama.cpp uses for this family)      */
/* ------------------------------------------------------------------------- */

typedef struct kt_piece_t {
    size_t start;
    size_t len;
} kt_piece_t;

static bool kt_is_space(uint8_t c)
{
    return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\v' || c == '\f';
}

static bool kt_is_digit(uint8_t c)
{
    return c >= '0' && c <= '9';
}

static bool kt_is_letter(uint8_t c)
{
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c >= 0x80;
}

static bool kt_is_punct(uint8_t c)
{
    return !kt_is_space(c) && !kt_is_digit(c) && !kt_is_letter(c);
}

static bool kt_contraction(const uint8_t *s, size_t n, size_t i, size_t *len)
{
    static const char *const parts[] = { "'s", "'t", "'re", "'ve", "'m", "'ll", "'d" };
    size_t k;
    if (i >= n || s[i] != '\'') {
        return false;
    }
    for (k = 0; k < sizeof(parts) / sizeof(parts[0]); k++) {
        size_t plen = strlen(parts[k]);
        if (i + plen <= n && memcmp(s + i, parts[k], plen) == 0) {
            *len = plen;
            return true;
        }
    }
    return false;
}

static size_t kt_pretokenize(const uint8_t *s, size_t n, kt_piece_t *out, size_t cap)
{
    size_t i = 0, count = 0;

    while (i < n && count < cap) {
        size_t start = i;
        size_t clen = 0;

        if (kt_contraction(s, n, i, &clen)) {
            out[count].start = i;
            out[count].len = clen;
            count++;
            i += clen;
            continue;
        }
        if (kt_is_digit(s[i])) {
            out[count].start = i;
            out[count].len = 1;
            count++;
            i++;
            continue;
        }
        if (s[i] == ' ' && i + 1 < n && kt_is_letter(s[i + 1])) {
            i++;
            start = i - 1;
            while (i < n && kt_is_letter(s[i])) {
                i++;
            }
            out[count].start = start;
            out[count].len = i - start;
            count++;
            continue;
        }
        if (kt_is_letter(s[i])) {
            while (i < n && kt_is_letter(s[i])) {
                i++;
            }
            out[count].start = start;
            out[count].len = i - start;
            count++;
            continue;
        }
        if (s[i] == ' ' && i + 1 < n && kt_is_punct(s[i + 1])) {
            i++;
            start = i - 1;
            while (i < n && kt_is_punct(s[i])) {
                i++;
            }
            out[count].start = start;
            out[count].len = i - start;
            count++;
            continue;
        }
        if (kt_is_punct(s[i])) {
            while (i < n && kt_is_punct(s[i])) {
                i++;
            }
            out[count].start = start;
            out[count].len = i - start;
            count++;
            continue;
        }
        if (kt_is_space(s[i])) {
            size_t run = 0;
            while (i + run < n && kt_is_space(s[i + run])) {
                run++;
            }
            if (i + run == n) {
                out[count].start = i;
                out[count].len = run;
                count++;
                i += run;
                continue;
            }
            if (run >= 2) {
                out[count].start = i;
                out[count].len = run - 1;
                count++;
                i += run - 1;
                continue;
            }
        }
        out[count].start = i;
        out[count].len = 1;
        count++;
        i++;
    }
    return count;
}

/* ------------------------------------------------------------------------- */
/* BPE encode                                                                 */
/* ------------------------------------------------------------------------- */

static int32_t *kt_bpe_word(kaze_arena_t *arena, const kaze_tokenizer_t *t,
                            const uint8_t *s, size_t n, size_t *out_n)
{
    kt_str_t *syms;
    size_t count = n, i;
    int32_t *ids;
    size_t out_count = 0;

    syms = (kt_str_t *)kt_alloc(arena, (n == 0 ? 1 : n) * sizeof(kt_str_t));
    if (syms == NULL) {
        *out_n = 0;
        return NULL;
    }
    for (i = 0; i < n; i++) {
        syms[i].data = s + i;
        syms[i].len = 1;
    }
    for (;;) {
        const kt_merge_entry_t *best = NULL;
        size_t best_at = 0;
        size_t k;
        kt_str_t *next;
        size_t next_count = 0;

        for (k = 0; k + 1 < count; k++) {
            const kt_merge_entry_t *m = kt_merges_get(&t->merges, syms[k].data,
                                                      syms[k].len, syms[k + 1].data,
                                                      syms[k + 1].len);
            if (m != NULL && (best == NULL || m->rank < best->rank)) {
                best = m;
                best_at = k;
            }
        }
        if (best == NULL) {
            break;
        }
        next = (kt_str_t *)kt_alloc(arena, count * sizeof(kt_str_t));
        if (next == NULL) {
            break;
        }
        k = 0;
        while (k < count) {
            if (k + 1 < count && k >= best_at &&
                syms[k].len == best->llen && syms[k + 1].len == best->rlen &&
                memcmp(syms[k].data, best->left, best->llen) == 0 &&
                memcmp(syms[k + 1].data, best->right, best->rlen) == 0) {
                next[next_count].data = best->result;
                next[next_count].len = best->result_len;
                next_count++;
                k += 2;
            } else {
                next[next_count++] = syms[k];
                k++;
            }
        }
        syms = next;
        count = next_count;
    }
    ids = (int32_t *)kt_alloc(arena, (count == 0 ? 1 : count) * sizeof(int32_t));
    if (ids == NULL) {
        *out_n = 0;
        return NULL;
    }
    for (i = 0; i < count; i++) {
        int32_t id = kt_vocab_get(&t->vocab, syms[i].data, syms[i].len);
        if (id >= 0) {
            ids[out_count++] = id;
        } else {
            uint32_t b;
            for (b = 0; b < syms[i].len; b++) {
                int32_t single = kt_vocab_get(&t->vocab, syms[i].data + b, 1);
                if (single >= 0) {
                    ids[out_count++] = single;
                }
            }
        }
    }
    *out_n = out_count;
    return ids;
}

const int32_t *kaze_tokenize(kaze_arena_t *arena, const kaze_tokenizer_t *t,
                             const char *text, size_t len, size_t *n_out)
{
    kt_piece_t *pieces;
    size_t n_pieces, p, total = 0;
    int32_t *ids;
    kaze_vec_t *vec;

    if (n_out == NULL) {
        return NULL;
    }
    *n_out = 0;
    if (arena == NULL || t == NULL || text == NULL) {
        (void)kaze_trap(KAZE_ERR_BADARG, "kaze_tokenize needs an arena, a tokenizer and text");
        return NULL;
    }
    pieces = (kt_piece_t *)kt_alloc(arena, (len == 0 ? 1 : len) * sizeof(kt_piece_t));
    if (pieces == NULL) {
        return NULL;
    }
    n_pieces = kt_pretokenize((const uint8_t *)text, len, pieces, len == 0 ? 1 : len);
    vec = vec_new(arena, sizeof(int32_t), 16);
    if (vec == NULL) {
        return NULL;
    }
    for (p = 0; p < n_pieces; p++) {
        size_t got = 0;
        int32_t *part = kt_bpe_word(arena, t, (const uint8_t *)text + pieces[p].start,
                                    pieces[p].len, &got);
        size_t k;
        if (part == NULL) {
            continue;
        }
        for (k = 0; k < got; k++) {
            if (!vec_push(arena, vec, &part[k])) {
                return NULL;
            }
        }
        total += got;
    }
    ids = (int32_t *)kt_alloc(arena, (total == 0 ? 1 : total) * sizeof(int32_t));
    if (ids == NULL) {
        return NULL;
    }
    if (total > 0) {
        memcpy(ids, vec->data, total * sizeof(int32_t));
    }
    *n_out = total;
    return ids;
}

/* ------------------------------------------------------------------------- */
/* Decode                                                                     */
/* ------------------------------------------------------------------------- */

static bool kt_push_u8(kaze_arena_t *arena, kaze_vec_t *v, uint8_t b)
{
    return vec_push(arena, v, &b);
}

static bool kt_push_utf8(kaze_arena_t *arena, kaze_vec_t *v, uint32_t cp)
{
    if (cp < 0x80) {
        return kt_push_u8(arena, v, (uint8_t)cp);
    }
    if (cp < 0x800) {
        return kt_push_u8(arena, v, (uint8_t)(0xC0 | (cp >> 6))) &&
               kt_push_u8(arena, v, (uint8_t)(0x80 | (cp & 0x3F)));
    }
    if (cp < 0x10000) {
        return kt_push_u8(arena, v, (uint8_t)(0xE0 | (cp >> 12))) &&
               kt_push_u8(arena, v, (uint8_t)(0x80 | ((cp >> 6) & 0x3F))) &&
               kt_push_u8(arena, v, (uint8_t)(0x80 | (cp & 0x3F)));
    }
    return kt_push_u8(arena, v, (uint8_t)(0xF0 | (cp >> 18))) &&
           kt_push_u8(arena, v, (uint8_t)(0x80 | ((cp >> 12) & 0x3F))) &&
           kt_push_u8(arena, v, (uint8_t)(0x80 | ((cp >> 6) & 0x3F))) &&
           kt_push_u8(arena, v, (uint8_t)(0x80 | (cp & 0x3F)));
}

char *kaze_tokenizer_decode(kaze_arena_t *arena, const kaze_tokenizer_t *t,
                            const int32_t *ids, size_t n)
{
    kaze_vec_t *bytes;
    size_t i;
    uint8_t zero = 0;
    char *out;

    if (arena == NULL || t == NULL || (ids == NULL && n > 0)) {
        (void)kaze_trap(KAZE_ERR_BADARG, "kaze_tokenizer_decode needs an arena, a tokenizer and ids");
        return NULL;
    }
    bytes = vec_new(arena, 1, 64);
    if (bytes == NULL) {
        return NULL;
    }
    for (i = 0; i < n; i++) {
        const char *text;
        size_t clen, k = 0;
        if (ids[i] < 0 || ids[i] >= t->vocab_size) {
            continue;
        }
        text = t->token_text[ids[i]];
        if (text == NULL) {
            continue;
        }
        clen = strlen(text);
        while (k < clen) {
            uint8_t c = (uint8_t)text[k];
            uint32_t cp;
            int extra, e;
            if (c < 0x80) {
                cp = c;
                extra = 0;
            } else if ((c & 0xE0) == 0xC0) {
                cp = c & 0x1Fu;
                extra = 1;
            } else if ((c & 0xF0) == 0xE0) {
                cp = c & 0x0Fu;
                extra = 2;
            } else if ((c & 0xF8) == 0xF0) {
                cp = c & 0x07u;
                extra = 3;
            } else {
                k++;
                continue;
            }
            if (k + (size_t)extra >= clen) {
                break;
            }
            for (e = 1; e <= extra; e++) {
                cp = (cp << 6) | (uint32_t)((uint8_t)text[k + (size_t)e] & 0x3Fu);
            }
            k += (size_t)extra + 1;
            if (cp < 512 && kt_c2b[cp] >= 0) {
                if (!kt_push_u8(arena, bytes, (uint8_t)kt_c2b[cp])) {
                    return NULL;
                }
            } else if (!kt_push_utf8(arena, bytes, cp)) {
                return NULL;
            }
        }
    }
    if (!kt_push_u8(arena, bytes, zero)) {
        return NULL;
    }
    out = (char *)kt_alloc(arena, bytes->len);
    if (out == NULL) {
        return NULL;
    }
    memcpy(out, bytes->data, bytes->len);
    return out;
}

int32_t kaze_tokenizer_bos(const kaze_tokenizer_t *t)
{
    return t == NULL ? -1 : t->bos;
}

int32_t kaze_tokenizer_eos(const kaze_tokenizer_t *t)
{
    return t == NULL ? -1 : t->eos;
}

size_t kaze_tokenizer_vocab_size(const kaze_tokenizer_t *t)
{
    return t == NULL ? 0u : (size_t)t->vocab_size;
}


