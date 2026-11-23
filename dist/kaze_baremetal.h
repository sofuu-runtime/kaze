/* kaze_baremetal.h -- the libc pieces the Kaze runtime needs on bare metal.
 *
 * The runtime uses the mem and str families, a strtod for the number formatter,
 * snprintf and fprintf for trap messages, and floor and fabs for the
 * shortest-round-trip rule. A hosted target gets all of that from libc; on
 * Cortex-M or RISC-V nothing does, so this file provides it. There are no files
 * and no OS here: trap text goes to kaze_board_write, which a board layer points
 * at a UART. Leave it alone and the runtime still runs, silently.
 */
#ifndef KAZE_BAREMETAL_H
#define KAZE_BAREMETAL_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <stdarg.h>

void kaze_board_write(const char* text);

void*  memcpy(void* dst, const void* src, size_t n);
void*  memset(void* dst, int byte, size_t n);
int    memcmp(const void* a, const void* b, size_t n);
size_t strlen(const char* s);
int    strcmp(const char* a, const char* b);
int    strncmp(const char* a, const char* b, size_t n);
double strtod(const char* s, char** end);
int    snprintf(char* dst, size_t cap, const char* fmt, ...);
int    vsnprintf(char* dst, size_t cap, const char* fmt, va_list ap);
double floor(double x);
double fabs(double x);

typedef struct kaze_file FILE;
extern FILE* kaze_stderr_obj;
#define stderr kaze_stderr_obj
int fprintf(FILE* stream, const char* fmt, ...);

/* The deadline checks use clock(); a board can override this weak default with a real
 * cycle counter, which is what makes a deadline meaningful there. */
typedef long clock_t;
#define CLOCKS_PER_SEC 1000000L
clock_t clock(void);

#endif
