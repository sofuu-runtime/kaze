/* client_probe.c -- exercise dist/kaze_sitl_client.c without a Kaze program.
 * Used while developing the bridge; the gate uses a real emitted mission. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <unistd.h>

typedef struct { const char* data; size_t len; } KzStr;
typedef struct { size_t len; size_t cap; struct KzValue* items; } KzList;
typedef struct { double x, y, z; } KzPoint;
typedef struct KzValue {
    int kind;
    union { bool boolean; double number; KzStr str; KzList* list; KzPoint point; void* ptr; } as;
} KzValue;
enum { KZ_VAL_NIL = 0, KZ_VAL_BOOL, KZ_VAL_NUM, KZ_VAL_STR, KZ_VAL_LIST, KZ_VAL_RECORD, KZ_VAL_POINT, KZ_VAL_CHAN, KZ_VAL_FIBER };
static KzValue kz_nil(void) { KzValue v; v.kind = KZ_VAL_NIL; v.as.number = 0; return v; }
static KzValue kz_num(double n) { KzValue v; v.kind = KZ_VAL_NUM; v.as.number = n; return v; }
static KzValue kz_bool(bool b) { KzValue v; v.kind = KZ_VAL_BOOL; v.as.boolean = b; return v; }
static KzValue kz_str(const char* s) { KzValue v; v.kind = KZ_VAL_STR; v.as.str.data = s; v.as.str.len = s ? strlen(s) : 0; return v; }
static KzValue kz_point(KzValue x, KzValue y, KzValue z) { KzValue v; v.kind = KZ_VAL_POINT; v.as.point.x = x.as.number; v.as.point.y = y.as.number; v.as.point.z = z.as.number; return v; }

#if defined(__GNUC__) || defined(__clang__)
#define KZ_MAYBE_UNUSED __attribute__((unused))
#else
#define KZ_MAYBE_UNUSED
#endif

#include "kaze_sitl_client.c"

int main(void) {
    int i;
    for (i = 0; i < 300; i++) {
        KzValue v = kz_sitl_field("drone", "altitude");
        KzValue b = kz_sitl_field("drone", "battery");
        KzValue a = kz_sitl_field("drone", "armed");
        printf("alt=%.2f batt=%.0f armed=%d\n", v.as.number, b.as.number, a.as.boolean ? 1 : 0);
        usleep(20000);
        if (i > 4 && a.as.boolean) break;
    }
    return 0;
}
