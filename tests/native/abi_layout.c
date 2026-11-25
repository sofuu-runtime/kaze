/* The native backend stores f64, i64, bool and text pointers in QBE `alloc8`
 * cells and calls the C99 runtime helpers across the platform ABI.  These
 * compile-time assertions pin the layout the two sides agreed on; a change
 * here is an E_KAZE_ABI_MISMATCH even before a program runs.
 *
 * Keep these typedefs in lockstep with compiler/codegen.kaze's emitted
 * KzValue/KzList/KzStr/KzPoint prelude.  tests/native/abi-layout.sh compiles
 * this file for the host and for x86_64 so both LP64 ABIs are checked. */
#include <stddef.h>
#include <stdint.h>

#include "kaze_rt.h"

typedef enum { KZ_VAL_NIL = 0, KZ_VAL_BOOL, KZ_VAL_NUM, KZ_VAL_STR, KZ_VAL_LIST, KZ_VAL_RECORD, KZ_VAL_POINT, KZ_VAL_CHAN, KZ_VAL_FIBER } KzValKind;
struct KzValue;
typedef struct { size_t len; size_t cap; struct KzValue* items; } KzList;
typedef struct { const char* type_name; size_t field_count; const char** keys; struct KzValue* values; } KzRecord;
typedef struct { const char* data; size_t len; } KzStr;
typedef struct { kaze_float_t x, y, z; } KzPoint;
typedef struct KzValue {
    KzValKind kind;
    union { bool boolean; kaze_float_t number; KzStr str; KzList* list; KzRecord* record; KzPoint point; void* ptr; } as;
} KzValue;
typedef struct { const char* ptr; size_t len; } KzStrView;

#define KZ_ABI_ASSERT(name, cond) typedef char kz_abi_##name[(cond) ? 1 : -1]
#define KZ_ABI_OFF(s, m) offsetof(s, m)

KZ_ABI_ASSERT(float_is_double, sizeof(kaze_float_t) == sizeof(double));
KZ_ABI_ASSERT(point_x, KZ_ABI_OFF(KzPoint, x) == 0);
KZ_ABI_ASSERT(point_y, KZ_ABI_OFF(KzPoint, y) == sizeof(kaze_float_t));
KZ_ABI_ASSERT(point_z, KZ_ABI_OFF(KzPoint, z) == 2 * sizeof(kaze_float_t));
KZ_ABI_ASSERT(str_data, KZ_ABI_OFF(KzStr, data) == 0);
KZ_ABI_ASSERT(str_view_ptr, KZ_ABI_OFF(KzStrView, ptr) == 0);
KZ_ABI_ASSERT(list_len, KZ_ABI_OFF(KzList, len) == 0);
KZ_ABI_ASSERT(list_cap, KZ_ABI_OFF(KzList, cap) == sizeof(size_t));
KZ_ABI_ASSERT(list_items, KZ_ABI_OFF(KzList, items) == 2 * sizeof(size_t));
KZ_ABI_ASSERT(value_kind, KZ_ABI_OFF(KzValue, kind) == 0);
KZ_ABI_ASSERT(value_payload, KZ_ABI_OFF(KzValue, as) == 8);
KZ_ABI_ASSERT(value_size, sizeof(KzValue) == 32);
KZ_ABI_ASSERT(str_len, KZ_ABI_OFF(KzStr, len) == 8);
KZ_ABI_ASSERT(str_view_len, KZ_ABI_OFF(KzStrView, len) == 8);

#if UINTPTR_MAX == 0xFFFFFFFFFFFFFFFFu
KZ_ABI_ASSERT(lp64_pointer, sizeof(void*) == 8);
#else
KZ_ABI_ASSERT(ilp32_pointer, sizeof(void*) == 4);
#endif

int main(void) { return 0; }
