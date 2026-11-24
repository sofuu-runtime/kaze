/* bare-metal stub: the output surface an emitted Kaze program uses, and the file surface
 * it is refused. There is no filesystem on a board, so fopen returns null and the runtime
 * treats the argument as a string, which is the documented behaviour of that helper. */
#ifndef KAZE_BAREMETAL_STDIO_H
#define KAZE_BAREMETAL_STDIO_H

#include "kaze_baremetal.h"

#include <stddef.h>

typedef struct kaze_file FILE;
extern FILE* kaze_stdin_obj;
extern FILE* kaze_stdout_obj;
extern FILE* kaze_stderr_obj;
#define stdin kaze_stdin_obj
#define stdout kaze_stdout_obj
#define stderr kaze_stderr_obj

#define EOF (-1)

int printf(const char* fmt, ...);
int fprintf(FILE* stream, const char* fmt, ...);
int snprintf(char* dst, size_t cap, const char* fmt, ...);
int vsnprintf(char* dst, size_t cap, const char* fmt, va_list ap);
int puts(const char* s);

FILE* fopen(const char* path, const char* mode);
size_t fread(void* dst, size_t size, size_t count, FILE* stream);
size_t fwrite(const void* src, size_t size, size_t count, FILE* stream);
int fclose(FILE* stream);
int fileno(FILE* stream);
int isatty(int fd);
int fseek(FILE* stream, long offset, int whence);
long ftell(FILE* stream);
int fflush(FILE* stream);

#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

#endif
