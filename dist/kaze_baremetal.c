/* kaze_baremetal.c -- see kaze_baremetal.h. Deliberately small: this is what a
 * board port has to supply, and nothing more. */
#include "kaze_baremetal.h"
#include <math.h>

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

/* ---- %f and %g, because the runtime renders numbers through this ------------------
 *
 * kz_format_number asks for "%.0f" for integral values and "%.*g" for the rest, and every
 * '"" + n' in a program goes through it. Without these a board would print the format text
 * instead of the number - which is exactly what it did until this was written.
 */
static int bm_put(char* dst, size_t cap, size_t* o, char c) {
    if (*o + 1 < cap) dst[(*o)++] = c;
    return 0;
}
static int bm_puts(char* dst, size_t cap, size_t* o, const char* s) {
    while (*s != 0) bm_put(dst, cap, o, *s++);
    return 0;
}
/* Unsigned fixed-point digits of frac scaled by 10^digits, one at a time. */
static void bm_fixed(char* dst, size_t cap, size_t* o, double v, int digits) {
    int neg = v < 0;
    double a = neg ? -v : v;
    double ip = floor(a);
    if (a >= 1.8446744073709552e19) {          /* beyond 64-bit: stop pretending */
        bm_puts(dst, cap, o, neg ? "-inf-scale" : "inf-scale");
        return;
    }
    unsigned long long whole = (unsigned long long)ip;
    char tmp[24]; int k = 0;
    do { tmp[k++] = (char)('0' + (int)(whole % 10u)); whole /= 10u; } while (whole != 0);
    if (neg) bm_put(dst, cap, o, '-');
    while (k > 0) bm_put(dst, cap, o, tmp[--k]);
    if (digits > 0) {
        bm_put(dst, cap, o, '.');
        double frac = a - ip;
        for (int i = 0; i < digits; i++) {
            frac *= 10.0;
            int d = (int)frac;
            if (d < 0) d = 0;
            if (d > 9) d = 9;
            bm_put(dst, cap, o, (char)('0' + d));
            frac -= (double)d;
        }
    }
}
/* Strip trailing zeros after a '.', and the point itself if nothing is left. */
static void bm_trim(char* dst, size_t start, size_t* o) {
    size_t end = *o;
    size_t point = end;
    for (size_t i = start; i < end; i++) if (dst[i] == '.') point = i;
    if (point == end) return;
    while (*o > point + 1 && dst[*o - 1] == '0') (*o)--;
    if (*o == point + 1) (*o)--;
}
static void bm_float(char* dst, size_t cap, size_t* o, double v, int prec, int use_g) {
    if (v != v) { bm_puts(dst, cap, o, "NaN"); return; }
    if (v == 1.0 / 0.0) { bm_puts(dst, cap, o, "inf"); return; }
    if (v == -1.0 / 0.0) { bm_puts(dst, cap, o, "-inf"); return; }
    if (prec < 0) prec = 6;
    if (!use_g) { bm_fixed(dst, cap, o, v, prec); return; }

    /* %g: exponent decides fixed or scientific, then trailing zeros go. */
    size_t start = *o;
    double a = v < 0 ? -v : v;
    int exp10 = 0;
    if (a != 0.0) {
        double e = log(a) / 2.30258509299404568402;   /* log10 */
        exp10 = (int)floor(e + 0.5);
        /* one correction step, because log10 from log is not exact at powers of ten */
        double scaled = a / exp(2.30258509299404568402 * (double)exp10);
        while (scaled >= 10.0) { exp10++; scaled /= 10.0; }
        while (scaled < 1.0 && exp10 > -320) { exp10--; scaled *= 10.0; }
    }
    if (prec == 0) prec = 1;
    if (exp10 < -4 || exp10 >= prec) {
        int keep = prec - 1;
        double m = a / exp(2.30258509299404568402 * (double)exp10);
        bm_fixed(dst, cap, o, v < 0 ? -m : m, keep);
        bm_trim(dst, start, o);
        bm_put(dst, cap, o, 'e');
        if (exp10 < 0) { bm_put(dst, cap, o, '-'); exp10 = -exp10; } else bm_put(dst, cap, o, '+');
        if (exp10 < 10) bm_put(dst, cap, o, '0');
        char t[8]; int k = 0;
        do { t[k++] = (char)('0' + (exp10 % 10)); exp10 /= 10; } while (exp10 != 0);
        while (k > 0) bm_put(dst, cap, o, t[--k]);
    } else {
        int digits = prec - 1 - exp10;
        if (digits < 0) digits = 0;
        bm_fixed(dst, cap, o, v, digits);
        bm_trim(dst, start, o);
    }
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
        } else if (*p == 'f' || *p == 'F') {
            double v = va_arg(ap, double);
            bm_float(dst, cap, &o, v, 6, 0);
        } else if (*p == '.') {
            int prec = 0;
            p++;
            if (*p == '*') { prec = va_arg(ap, int); p++; }
            else { while (*p >= '0' && *p <= '9') { prec = prec * 10 + (*p - '0'); p++; } }
            if (*p == 'f' || *p == 'F') bm_float(dst, cap, &o, va_arg(ap, double), prec, 0);
            else if (*p == 'g' || *p == 'G') bm_float(dst, cap, &o, va_arg(ap, double), prec, 1);
            else { bm_put(dst, cap, &o, '%'); bm_put(dst, cap, &o, '.'); }
        } else if (*p == 'g' || *p == 'G') {
            bm_float(dst, cap, &o, va_arg(ap, double), 6, 1);
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

/* -- what an emitted Kaze program needs, and a device does not provide --------------
 *
 * The runtime header is included here, and only here, because these stand-ins have to
 * name kaze_arena_t; the libc pieces above deliberately depend on nothing.
 *
 * The generated C calls printf for its result, atexit for two report hooks, and
 * kaze_arena_init_heap for its arena. On a board there is no heap and no process exit, so
 * these stand in: the arena is a static buffer (KAZE_NO_HEAP removes the malloc path from
 * the runtime entirely), printf goes to the board text hook, atexit has nothing to flush,
 * and exit reaches an attached debugger through semihosting.
 */
#include "kaze_rt.h"

/* -- the maths a board does not ship ------------------------------------------------
 *
 * An emitted Kaze program may call sqrt, sin, cos, atan2, fmod and floor. None is a
 * compiler intrinsic on Cortex-M and a freestanding image has no libm, so they are here.
 * fabs and floor are exact; sqrt is Newton from a bit-trick seed; fmod reduces by whole
 * multiples of the divisor; the trigonometric pair reduces the argument to [-pi/4, pi/4]
 * and sums a Taylor series. Accuracy is checked on the target against the host libm for the
 * values the gates use (scripts/phase14-gates.sh runs that comparison).
 */
double fmod(double x, double y) {
    if (y == 0.0 || x != x || y != y) return 0.0 / 0.0;
    double q = x / y;
    if (q >= 9007199254740992.0 || q <= -9007199254740992.0) return x;
    double whole = (double)(long long)q;
    double r = x - whole * y;
    if (r != 0.0 && ((r < 0.0) != (y < 0.0))) r += y;
    return r;
}

double sqrt(double x) {
    if (x != x || x < 0.0) return 0.0 / 0.0;
    if (x == 0.0) return 0.0;
    union { double d; long long i; } u;
    u.d = x;
    u.i = (u.i >> 1) + 0x1FF8000000000000LL;   /* seed within a few ulp of the answer */
    double g = u.d;
    for (int k = 0; k < 6; k++) g = 0.5 * (g + x / g);
    return g;
}

#define KAZE_HALF_PI 1.57079632679489661923
#define KAZE_PI      3.14159265358979323846

static double kaze_sin_taylor(double r) {          /* |r| <= pi/4 */
    double r2 = r * r, term = r, sum = r;
    for (int n = 1; n < 10; n++) {
        term = -term * r2 / (double)((2 * n) * (2 * n + 1));
        sum += term;
    }
    return sum;
}

static double kaze_cos_taylor(double r) {          /* |r| <= pi/4 */
    double r2 = r * r, term = 1.0, sum = 1.0;
    for (int n = 1; n < 10; n++) {
        term = -term * r2 / (double)((2 * n - 1) * (2 * n));
        sum += term;
    }
    return sum;
}

/* x = k*pi/2 + r with |r| <= pi/4; k mod 4 goes back through quadrant. */
static double kaze_reduce(double x, int* quadrant) {
    double q = x / KAZE_HALF_PI;
    double n = floor(q + 0.5);
    int k = (int)((long long)n & 3LL);
    if (k < 0) k += 4;
    *quadrant = k;
    return x - n * KAZE_HALF_PI;
}

double sin(double x) {
    if (x != x || x == 1.0 / 0.0 || x == -1.0 / 0.0) return 0.0 / 0.0;
    int k = 0;
    double r = kaze_reduce(x, &k);
    if (k == 0) return kaze_sin_taylor(r);
    if (k == 1) return kaze_cos_taylor(r);
    if (k == 2) return -kaze_sin_taylor(r);
    return -kaze_cos_taylor(r);
}

double cos(double x) {
    if (x != x || x == 1.0 / 0.0 || x == -1.0 / 0.0) return 0.0 / 0.0;
    int k = 0;
    double r = kaze_reduce(x, &k);
    if (k == 0) return kaze_cos_taylor(r);
    if (k == 1) return -kaze_sin_taylor(r);
    if (k == 2) return -kaze_cos_taylor(r);
    return kaze_sin_taylor(r);
}

/* atan on 0..1, with the tan(pi/8) identity step so the series argument is at most 1/3. */
static double kaze_atan_unit(double a) {
    int invert = a > 0.41421356237309503;
    if (invert) a = (a - 1.0) / (a + 1.0);
    double a2 = a * a, term = a, sum = a;
    for (int n = 1; n < 20; n++) {
        term = -term * a2;
        sum += term / (double)(2 * n + 1);
    }
    if (invert) sum += KAZE_HALF_PI / 2.0;
    return sum;
}

static double kaze_atan_any(double t) {
    int neg = t < 0.0;
    double a = neg ? -t : t;
    double r = (a <= 1.0) ? kaze_atan_unit(a) : (KAZE_HALF_PI - kaze_atan_unit(1.0 / a));
    return neg ? -r : r;
}

double atan2(double y, double x) {
    if (x == 0.0 && y == 0.0) return 0.0;
    if (x == 0.0) return y > 0.0 ? KAZE_HALF_PI : -KAZE_HALF_PI;
    double r = kaze_atan_any(y / x);
    if (x > 0.0) return r;
    return y >= 0.0 ? r + KAZE_PI : r - KAZE_PI;
}


double asin(double x) {
    if (x > 1.0 || x < -1.0) return 0.0 / 0.0;
    return atan2(x, sqrt(1.0 - x * x));
}
double acos(double x) {
    if (x > 1.0 || x < -1.0) return 0.0 / 0.0;
    return atan2(sqrt(1.0 - x * x), x);
}
double tan(double x) { double c = cos(x); return sin(x) / c; }
double exp(double x) {
    /* e^x = 2^(x/ln2), with the integer and fractional halves separated. */
    double y = x * 1.44269504088896340736;
    double whole = floor(y + 0.5);
    double frac = y - whole;
    double p = 1.0, term = 1.0;
    for (int n = 1; n < 16; n++) { term = term * (frac * 0.69314718055994530942) / (double)n; p += term; }
    double scale = 1.0, base = 2.0;
    long long k = (long long)whole;
    int neg = k < 0;
    if (neg) k = -k;
    while (k > 0) { if (k & 1) scale *= base; base *= base; k >>= 1; }
    return neg ? p / scale : p * scale;
}
double log(double x) {
    if (x <= 0.0) return 0.0 / 0.0;
    /* Reduce to [1, 2) by halving, then the atanh series. */
    int halvings = 0;
    while (x >= 2.0) { x *= 0.5; halvings++; }
    while (x < 1.0) { x *= 2.0; halvings--; }
    double t = (x - 1.0) / (x + 1.0), t2 = t * t, term = t, sum = 0.0;
    for (int n = 0; n < 24; n++) { sum += term / (double)(2 * n + 1); term *= t2; }
    return 2.0 * sum + (double)halvings * 0.69314718055994530942;
}
double pow(double b, double e) {
    if (b == 0.0) return 0.0;
    if (b < 0.0) { double whole = floor(e + 0.5); if (whole != e) return 0.0 / 0.0; double r = exp(e * log(-b)); return ((long long)whole & 1LL) ? -r : r; }
    return exp(e * log(b));
}

void* realloc(void* ptr, size_t size) { (void)ptr; (void)size; return 0; }

/* Monotonic from boot: the board has no calendar (see baremetal-include/time.h). */
time_t time(time_t* out) {
    static time_t ticks;
    ticks += 1;
    if (out != 0) *out = ticks;
    return ticks;
}

int atoi(const char* s) { return (int)strtol(s, 0, 10); }

long strtol(const char* s, char** end, int base) {
    long v = 0;
    int neg = 0;
    const char* p = s;
    while (*p == 32 || (*p >= 9 && *p <= 13)) p++;
    if (*p == 45) { neg = 1; p++; } else if (*p == 43) { p++; }
    if (base == 16 || base == 0) {
        if (p[0] == 48 && (p[1] == 120 || p[1] == 88)) { p += 2; base = 16; }
    }
    if (base == 0) base = 10;
    for (;;) {
        int d;
        char c = *p;
        if (c >= 48 && c <= 57) d = c - 48;
        else if (c >= 97 && c <= 122) d = c - 97 + 10;
        else if (c >= 65 && c <= 90) d = c - 65 + 10;
        else break;
        if (d >= base) break;
        v = v * base + d;
        p++;
    }
    if (end != 0) *end = (char*)p;
    return neg ? -v : v;
}

/* The rest of what an emitted program may reach for, all defined in terms of the
 * routines above so there is one implementation of each idea on the board. */
double ceil(double x) { return -floor(-x); }
double trunc(double x) { return x < 0.0 ? -floor(-x) : floor(x); }
double round(double x) {
    if (x != x) return x;
    return x < 0.0 ? -floor(-x + 0.5) : floor(x + 0.5);
}
double log10(double x) { return log(x) / 2.30258509299404568402; }
double atan(double x) { return atan2(x, 1.0); }
double fmin(double a, double b) { return (a < b || a != a) ? a : b; }
double fmax(double a, double b) { return (a > b || a != a) ? a : b; }

char* getenv(const char* name) { (void)name; return 0; }
int puts(const char* s) { kaze_board_write(s); kaze_board_write("\n"); return 0; }

#define KAZE_STATIC_ARENA_BYTES (16u * 1024u)
static unsigned char kaze_static_arena[KAZE_STATIC_ARENA_BYTES];

bool kaze_arena_init_heap(kaze_arena_t* arena, size_t capacity) {
    if (capacity > KAZE_STATIC_ARENA_BYTES) capacity = KAZE_STATIC_ARENA_BYTES;
    kaze_arena_init(arena, kaze_static_arena, capacity);
    return true;
}

void kaze_arena_free(kaze_arena_t* arena) { (void)arena; }

int atexit(void (*fn)(void)) { (void)fn; return 0; }

int printf(const char* fmt, ...) {
    char buf[192];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    kaze_board_write(buf);
    return n;
}

void exit(int code) {
    unsigned int block[2];
    block[0] = 0x20026u;                       /* ADP_Stopped_ApplicationExit */
    block[1] = (unsigned int)code;
#if defined(__arm__) || defined(__thumb__)
    register unsigned int r0 __asm__("r0") = 0x18;                       /* SYS_EXIT */
    register void* r1 __asm__("r1") = (void*)block;
    __asm__ volatile("bkpt 0xAB" : : "r"(r0), "r"(r1) : "memory");
#elif defined(__riscv)
    register long a0 __asm__("a0") = 0x18;
    register void* a1 __asm__("a1") = (void*)block;
    __asm__ volatile("ebreak" : : "r"(a0), "r"(a1) : "memory");
#else
    (void)block;
#endif
    for (;;) { }
}


#include <signal.h>

/* -- the file and signal surface: refused, deliberately ---------------------------------
 *
 * A board has no filesystem and no signals. fopen returns null, which is exactly how the
 * runtime's read-file-or-string helper falls back to treating its argument as a string;
 * isatty says 1 so the CLI resolver never tries to slurp a stdin that is not there.
 */
/* FILE is opaque here: these are non-null addresses that no routine ever dereferences,
   because every stdio stand-in ignores its stream argument. */
static unsigned char kaze_file_dummy[3][8];
FILE* kaze_stdin_obj = (FILE*)(void*)kaze_file_dummy[0];
FILE* kaze_stdout_obj = (FILE*)(void*)kaze_file_dummy[1];
FILE* kaze_stderr_obj = (FILE*)(void*)kaze_file_dummy[2];

FILE* fopen(const char* path, const char* mode) { (void)path; (void)mode; return 0; }
size_t fread(void* dst, size_t size, size_t count, FILE* stream) { (void)dst; (void)size; (void)count; (void)stream; return 0; }
size_t fwrite(const void* src, size_t size, size_t count, FILE* stream) { (void)src; (void)size; (void)count; (void)stream; return 0; }
int fclose(FILE* stream) { (void)stream; return 0; }
int fflush(FILE* stream) { (void)stream; return 0; }
int fileno(FILE* stream) { (void)stream; return 0; }
int isatty(int fd) { (void)fd; return 1; }
int fseek(FILE* stream, long offset, int whence) { (void)stream; (void)offset; (void)whence; return -1; }
long ftell(FILE* stream) { (void)stream; return -1; }

kaze_sighandler_t signal(int sig, kaze_sighandler_t handler) { (void)sig; (void)handler; return 0; }
int raise(int sig) { (void)sig; return 0; }
