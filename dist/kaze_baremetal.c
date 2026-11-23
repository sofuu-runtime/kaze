/* kaze_baremetal.c -- see kaze_baremetal.h. Deliberately small: this is what a
 * board port has to supply, and nothing more. */
#include "kaze_baremetal.h"

void __attribute__((weak)) kaze_board_write(const char* text) { (void)text; }

void* memcpy(void* dst, const void* src, size_t n) {
    unsigned char* d = (unsigned char*)dst;
    const unsigned char* s = (const unsigned char*)src;
    while (n--) *d++ = *s++;
    return dst;
}
void* memset(void* dst, int byte, size_t n) {
    unsigned char* d = (unsigned char*)dst;
    while (n--) *d++ = (unsigned char)byte;
    return dst;
}
int memcmp(const void* a, const void* b, size_t n) {
    const unsigned char* x = (const unsigned char*)a;
    const unsigned char* y = (const unsigned char*)b;
    for (size_t i = 0; i < n; i++) if (x[i] != y[i]) return x[i] < y[i] ? -1 : 1;
    return 0;
}
size_t strlen(const char* s) { size_t n = 0; while (s[n] != 0) n++; return n; }
int strcmp(const char* a, const char* b) {
    while (*a != 0 && *a == *b) { a++; b++; }
    return (unsigned char)*a - (unsigned char)*b;
}
int strncmp(const char* a, const char* b, size_t n) {
    for (size_t i = 0; i < n; i++) {
        if (a[i] != b[i]) return (unsigned char)a[i] - (unsigned char)b[i];
        if (a[i] == 0) return 0;
    }
    return 0;
}

/* 64-bit divide-by-ten without a division instruction. On a Thumb-only core the
 * compiler lowers `v / 10` on a 64-bit value to __aeabi_uldivmod, which pulls in a
 * compiler-rt object; on this machine that object faulted at run time inside the
 * runtime's OOM message. Shifts and compares cost nothing and keep the port free of
 * that dependency. */
static unsigned long long bm_udiv10(unsigned long long v, unsigned long long* rem) {
    unsigned long long q = 0;
    unsigned long long r = 0;
    for (int i = 63; i >= 0; i--) {
        r = (r << 1) | ((v >> i) & 1ULL);
        q <<= 1;
        if (r >= 10ULL) { r -= 10ULL; q |= 1ULL; }
    }
    *rem = r;
    return q;
}

int vsnprintf(char* dst, size_t cap, const char* fmt, va_list ap) {
    size_t o = 0;
    for (const char* p = fmt; *p != 0; p++) {
        if (*p != '%') { if (o + 1 < cap) dst[o++] = *p; continue; }
        p++;
        if (*p == 's') {
            const char* s = va_arg(ap, const char*);
            while (s != 0 && *s != 0) { if (o + 1 < cap) dst[o++] = *s; s++; }
        } else if (*p == 'c') {
            char ch = (char)va_arg(ap, int);
            if (o + 1 < cap) dst[o++] = ch;
        } else if (*p == '%') {
            if (o + 1 < cap) dst[o++] = '%';
        } else if (*p == 'd' || *p == 'i') {
            int v = va_arg(ap, int);
            char tmp[24]; int k = 0;
            if (v < 0) { if (o + 1 < cap) dst[o++] = '-'; v = -v; }
            do { tmp[k++] = (char)('0' + (v % 10)); v /= 10; } while (v != 0);
            while (k > 0) { if (o + 1 < cap) dst[o++] = tmp[--k]; }
        } else if (*p == 'l' && p[1] == 'l') {
            unsigned long long v = (unsigned long long)va_arg(ap, long long);
            char tmp[24]; int k = 0;
            do {
                unsigned long long rem = 0;
                unsigned long long q = bm_udiv10(v, &rem);
                tmp[k++] = (char)('0' + (int)rem);
                v = q;
            } while (v != 0);
            while (k > 0) { if (o + 1 < cap) dst[o++] = tmp[--k]; }
            if (p[2] != 0) p++;
            if (p[2] != 0) p++;
        } else {
            if (o + 1 < cap) dst[o++] = '%';
            if (o + 1 < cap) dst[o++] = *p;
        }
    }
    if (cap > 0) dst[o < cap ? o : cap - 1] = 0;
    return (int)o;
}
int snprintf(char* dst, size_t cap, const char* fmt, ...) {
    va_list ap; va_start(ap, fmt);
    int n = vsnprintf(dst, cap, fmt, ap);
    va_end(ap);
    return n;
}

double floor(double x) { long long i = (long long)x; if ((double)i > x) i--; return (double)i; }
double fabs(double x) { return x < 0 ? -x : x; }

FILE* kaze_stderr_obj = (FILE*)0;
int fprintf(FILE* stream, const char* fmt, ...) {
    (void)stream;
    char buf[256];
    va_list ap; va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    kaze_board_write(buf);
    return n;
}

/* No decimal parser: the runtime only round-trips numbers it printed itself, and on a
 * board the formatter is not the hot path. Returning 0 makes kz_format_number fall
 * through to 17 significant digits - the widest and always correct choice. */
double strtod(const char* s, char** end) {
    (void)s;
    if (end != 0) *end = 0;
    return 0.0;
}

__attribute__((weak)) clock_t clock(void) { return 0; }
