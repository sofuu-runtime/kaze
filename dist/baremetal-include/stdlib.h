/* bare-metal stub: what an emitted Kaze program needs from <stdlib.h>. There is no heap
 * here, so the allocation family is deliberately absent: a program that allocates is a
 * program this runtime does not support on a board. */
#ifndef KAZE_BAREMETAL_STDLIB_H
#define KAZE_BAREMETAL_STDLIB_H

#include "kaze_baremetal.h"

#include <stddef.h>

char* getenv(const char* name);
void exit(int code);
int atexit(void (*fn)(void));
double strtod(const char* s, char** end);
long strtol(const char* s, char** end, int base);
int atoi(const char* s);

/* There is no heap here. realloc refuses, which makes the one runtime helper that grows a
 * buffer for stdin stop and return what it has; nothing else calls an allocator. */
void* realloc(void* ptr, size_t size);

#endif
