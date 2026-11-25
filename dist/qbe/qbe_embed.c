/* qbe_embed.c -- compile QBE IL from memory, without spawning a process.
 *
 * QBE's own main.c reads a file and writes to stdout.  This wrapper keeps the
 * same pipeline (parse -> target ABI -> SSA -> register allocation -> emit) but
 * uses POSIX stream buffers, so the Kaze host can call it in-process.
 *
 * QBE is MIT-licensed; see ../LICENSE.
 */
#define _POSIX_C_SOURCE 200809L

#include "all.h"
#include "qbe_embed.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

Target T;

char debug['Z' + 1];

extern Target T_amd64_sysv;
extern Target T_amd64_apple;
extern Target T_arm64;
extern Target T_arm64_apple;
extern Target T_rv64;

static FILE *qbe_out;

static void qbe_data(Dat *d)
{
    emitdat(d, qbe_out);
    if (d->type == DEnd) {
        fputs("/* end data */\n\n", qbe_out);
        freeall();
    }
}

/* The pipeline is main.c's, unchanged: ABI lowering, SSA construction, the
 * copy/fold/simplify passes, instruction selection and register allocation. */
static void qbe_func(Fn *fn)
{
    uint n;

    T.abi0(fn);
    fillrpo(fn);
    fillpreds(fn);
    filluse(fn);
    promote(fn);
    filluse(fn);
    ssa(fn);
    filluse(fn);
    ssacheck(fn);
    fillalias(fn);
    loadopt(fn);
    filluse(fn);
    fillalias(fn);
    coalesce(fn);
    filluse(fn);
    ssacheck(fn);
    copy(fn);
    filluse(fn);
    fold(fn);
    T.abi1(fn);
    simpl(fn);
    fillpreds(fn);
    filluse(fn);
    T.isel(fn);
    fillrpo(fn);
    filllive(fn);
    fillloop(fn);
    fillcost(fn);
    spill(fn);
    rega(fn);
    fillrpo(fn);
    simpljmp(fn);
    fillpreds(fn);
    fillrpo(fn);
    assert(fn->rpo[0] == fn->start);
    for (n = 0;; n++) {
        if (n == fn->nblk - 1) {
            fn->rpo[n]->link = 0;
            break;
        } else {
            fn->rpo[n]->link = fn->rpo[n + 1];
        }
    }
    T.emitfn(fn, qbe_out);
    fprintf(qbe_out, "/* end function %s */\n\n", fn->name);
    freeall();
}

static void qbe_dbgfile(char *fn)
{
    emitdbgfile(fn, qbe_out);
}

static Target *qbe_target(const char *name)
{
    if (name == NULL || name[0] == '\0' || strcmp(name, "arm64_apple") == 0) return &T_arm64_apple;
    if (strcmp(name, "arm64") == 0) return &T_arm64;
    if (strcmp(name, "amd64_sysv") == 0) return &T_amd64_sysv;
    if (strcmp(name, "amd64_apple") == 0) return &T_amd64_apple;
    if (strcmp(name, "rv64") == 0) return &T_rv64;
    return NULL;
}

int qbe_compile_buffer(const char *ssa, size_t ssa_len, const char *target,
                       char **asm_out, size_t *asm_len)
{
    Target *t;
    FILE *in;
    FILE *out;
    char *buf = NULL;
    size_t len = 0;

    if (asm_out == NULL || asm_len == NULL) return 1;
    *asm_out = NULL;
    *asm_len = 0;
    t = qbe_target(target);
    if (t == NULL) return 2;

    in = fmemopen((void *)ssa, ssa_len, "r");
    if (in == NULL) return 1;
    out = open_memstream(&buf, &len);
    if (out == NULL) {
        fclose(in);
        return 1;
    }

    T = *t;
    qbe_out = out;
    parse(in, (char *)"<kaze>", qbe_dbgfile, qbe_data, qbe_func);
    T.emitfin(out);
    fclose(in);
    fclose(out); /* open_memstream NUL-terminates the buffer and sets len */

    *asm_out = buf;
    *asm_len = len;
    return 0;
}
