/* kaze-native -- the Stage A native host.
 *
 * The pure-Kaze compiler emits QBE IL; this host compiles that IL with the
 * vendored in-process QBE library, then uses the system assembler and linker
 * only.  No C compiler is involved in the hot path.
 *
 *   kaze-native emit  <file.kaze> [-o file.ssa] [--target T]
 *   kaze-native asm   <file.kaze> [-o file.s]   [--target T]
 *   kaze-native build <file.kaze> [-o program]  [--target T]
 *   kaze-native run   <file.kaze> [args...]     [--target T]
 *   kaze-native version
 */
#define _POSIX_C_SOURCE 200809L

#include "qbe/qbe_embed.h"

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef PATH_MAX
#define PATH_MAX 4096
#endif

#if defined(__APPLE__)
/* _POSIX_C_SOURCE alone does not expose mkdtemp on Darwin. */
char *mkdtemp(char *);
#endif

static const char *kaze_bin = NULL;
static const char *self_dir = NULL;

static void fail(const char *what)
{
    fprintf(stderr, "kaze-native: %s: %s\n", what, strerror(errno));
    exit(70);
}

static int run_child(char *const argv[])
{
    pid_t pid = fork();
    if (pid < 0) fail("fork");
    if (pid == 0) {
        execvp(argv[0], argv);
        fprintf(stderr, "kaze-native: cannot run %s: %s\n", argv[0], strerror(errno));
        _exit(127);
    }
    int status = 0;
    while (waitpid(pid, &status, 0) < 0) {
        if (errno != EINTR) fail("waitpid");
    }
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return 70;
}

static void find_tools(const char *self)
{
    const char *env = getenv("KAZE_BIN");
    static char buf[PATH_MAX];
    if (env != NULL && env[0] != '\0') {
        kaze_bin = env;
    } else {
        const char *slash = strrchr(self, '/');
        if (slash != NULL) {
            size_t n = (size_t)(slash - self);
            if (n + 6 < sizeof buf) {
                memcpy(buf, self, n);
                snprintf(buf + n, sizeof buf - n, "/kaze");
                if (access(buf, X_OK) == 0) kaze_bin = buf;
            }
        }
        if (kaze_bin == NULL) kaze_bin = "kaze";
    }
    self_dir = getenv("KAZE_NATIVE_DIR");
    if (self_dir == NULL || self_dir[0] == '\0') {
        const char *slash = strrchr(self, '/');
        if (slash != NULL) {
            size_t n = (size_t)(slash - self);
            static char dir[PATH_MAX];
            if (n < sizeof dir) {
                memcpy(dir, self, n);
                dir[n] = '\0';
                self_dir = dir;
            }
        }
        if (self_dir == NULL) self_dir = ".";
    }
}

static int file_exists(const char *p)
{
    return access(p, R_OK) == 0;
}

/* The native runtime object and the QBE archive live next to the driver in a
 * build tree, or in <root>/dist when bin/ sits beside dist/. */
static void find_dist(const char *name, char *out, size_t cap)
{
    const char *env = getenv("KAZE_RT_DIR");
    static const char *candidates[] = { NULL, NULL, NULL, NULL, NULL };
    static char a[PATH_MAX], b[PATH_MAX], c[PATH_MAX], d[PATH_MAX];
    if (env != NULL && env[0] != '\0') {
        snprintf(out, cap, "%s/%s", env, name);
        if (file_exists(out)) return;
    }
    snprintf(a, sizeof a, "%s/%s", self_dir, name);
    candidates[0] = a;
    snprintf(b, sizeof b, "%s/../dist/%s", self_dir, name);
    candidates[1] = b;
    snprintf(c, sizeof c, "%s/../lib/kaze/%s", self_dir, name);
    candidates[2] = c;
    snprintf(d, sizeof d, "%s/../lib/%s", self_dir, name);
    candidates[3] = d;
    candidates[4] = NULL;
    for (int i = 0; candidates[i] != NULL; i++) {
        snprintf(out, cap, "%s", candidates[i]);
        if (file_exists(out)) return;
    }
    snprintf(out, cap, "%s/%s", self_dir, name);
}

static int write_text(const char *path, const char *text, size_t len)
{
    FILE *f = fopen(path, "wb");
    if (f == NULL) return -1;
    size_t n = fwrite(text, 1, len, f);
    int rc = fclose(f);
    return (n == len && rc == 0) ? 0 : -1;
}

static char *read_text(const char *path, size_t *len)
{
    FILE *f = fopen(path, "rb");
    if (f == NULL) return NULL;
    if (fseek(f, 0, SEEK_END) != 0) { fclose(f); return NULL; }
    long n = ftell(f);
    if (n < 0) { fclose(f); return NULL; }
    rewind(f);
    char *buf = (char *)malloc((size_t)n + 1);
    if (buf == NULL) { fclose(f); return NULL; }
    size_t got = fread(buf, 1, (size_t)n, f);
    fclose(f);
    buf[got] = '\0';
    *len = got;
    return buf;
}

/* Ask the compiler for QBE IL.  The compiler writes its value to stdout, so a
 * pipe is the interface; a leading E_KAZE_ token is a diagnostic, not IL. */
static char *compiler_ssa(const char *source, size_t *len)
{
    int fds[2];
    if (pipe(fds) != 0) fail("pipe");
    pid_t pid = fork();
    if (pid < 0) fail("fork");
    if (pid == 0) {
        if (dup2(fds[1], STDOUT_FILENO) < 0) _exit(70);
        close(fds[0]);
        close(fds[1]);
        char *argv[] = { (char *)kaze_bin, (char *)"emit-qbe", (char *)source, NULL };
        execvp(argv[0], argv);
        _exit(127);
    }
    close(fds[1]);
    size_t cap = 8192, used = 0;
    char *buf = (char *)malloc(cap);
    if (buf == NULL) fail("malloc");
    for (;;) {
        if (used + 2048 >= cap) {
            cap *= 2;
            char *nb = (char *)realloc(buf, cap);
            if (nb == NULL) fail("realloc");
            buf = nb;
        }
        ssize_t n = read(fds[0], buf + used, cap - used - 1);
        if (n < 0) {
            if (errno == EINTR) continue;
            fail("read");
        }
        if (n == 0) break;
        used += (size_t)n;
    }
    close(fds[0]);
    int status = 0;
    while (waitpid(pid, &status, 0) < 0) {
        if (errno != EINTR) fail("waitpid");
    }
    buf[used] = '\0';
    if (used == 0 || strncmp(buf, "E_KAZE_", 7) == 0) {
        fprintf(stderr, "%s", buf);
        free(buf);
        return NULL;
    }
    if (WIFEXITED(status) && WEXITSTATUS(status) != 0) {
        free(buf);
        return NULL;
    }
    *len = used;
    return buf;
}

/* The checker is part of compilation. The C99 driver refuses a program with a
 * hard diagnostic before it emits C; the native driver has to do the same, or
 * `run --backend=native` would execute a program that `check` rejects. The
 * compiler's check mode writes raw diagnostics to stdout (empty on success);
 * the unified CLI adds "check: ok" on success, which is not a diagnostic. */
static int compiler_check(const char *source)
{
    int fds[2];
    if (pipe(fds) != 0) fail("pipe");
    pid_t pid = fork();
    if (pid < 0) fail("fork");
    if (pid == 0) {
        if (dup2(fds[1], STDOUT_FILENO) < 0) _exit(70);
        close(fds[0]);
        close(fds[1]);
        char *argv[] = { (char *)kaze_bin, (char *)"check", (char *)source, NULL };
        execvp(argv[0], argv);
        _exit(127);
    }
    close(fds[1]);
    size_t cap = 8192, used = 0;
    char *buf = (char *)malloc(cap);
    if (buf == NULL) fail("malloc");
    for (;;) {
        if (used + 2048 >= cap) {
            cap *= 2;
            char *nb = (char *)realloc(buf, cap);
            if (nb == NULL) fail("realloc");
            buf = nb;
        }
        ssize_t n = read(fds[0], buf + used, cap - used - 1);
        if (n < 0) {
            if (errno == EINTR) continue;
            fail("read");
        }
        if (n == 0) break;
        used += (size_t)n;
    }
    close(fds[0]);
    buf[used] = '\0';
    int status = 0;
    while (waitpid(pid, &status, 0) < 0) {
        if (errno != EINTR) fail("waitpid");
    }
    int exited_ok = WIFEXITED(status) && WEXITSTATUS(status) == 0;
    int diagnostic = strstr(buf, "E_KAZE_") != NULL;
    if (!exited_ok || diagnostic) {
        fwrite(buf, 1, used, stdout);
        free(buf);
        return 1;
    }
    free(buf);
    return 0;
}

static const char *default_target(void)
{
    static struct utsname u;
    if (uname(&u) == 0) {
        if (strcmp(u.sysname, "Darwin") == 0) {
            if (strcmp(u.machine, "arm64") == 0) return "arm64_apple";
            return "amd64_apple";
        }
        if (strcmp(u.machine, "aarch64") == 0 || strcmp(u.machine, "arm64") == 0) return "arm64";
    }
    return "amd64_sysv";
}

static int target_is_local(const char *target)
{
    const char *host = default_target();
    return strcmp(target, host) == 0;
}

static int target_arch(const char *target, char *arch, size_t cap, int *apple)
{
    *apple = 0;
    if (strcmp(target, "arm64_apple") == 0) {
        snprintf(arch, cap, "arm64");
        *apple = 1;
        return 0;
    }
    if (strcmp(target, "amd64_apple") == 0) {
        snprintf(arch, cap, "x86_64");
        *apple = 1;
        return 0;
    }
    if (strcmp(target, "arm64") == 0) {
        snprintf(arch, cap, "aarch64");
        return 0;
    }
    if (strcmp(target, "amd64_sysv") == 0) {
        snprintf(arch, cap, "x86_64");
        return 0;
    }
    if (strcmp(target, "rv64") == 0) {
        snprintf(arch, cap, "riscv64");
        return 0;
    }
    return 1;
}

static void ensure_runtime_object(char *path, size_t cap)
{
    find_dist("kaze_native_rt.o", path, cap);
    if (file_exists(path)) return;
    /* One-time convenience build.  The hot path still never invokes a C
     * compiler; scripts/build-native.sh normally creates this object. */
    const char *cc = getenv("KAZE_CC");
    if (cc == NULL || cc[0] == '\0') cc = "cc";
    char src[PATH_MAX], obj[PATH_MAX];
    find_dist("kaze_native_rt.c", src, sizeof src);
    if (!file_exists(src)) {
        fprintf(stderr, "E_KAZE_BACKEND_UNSUPPORTED: kaze_native_rt.c not found\n");
        exit(2);
    }
    snprintf(obj, sizeof obj, "%s", path);
    char *argv[] = { (char *)cc, (char *)"-O2", (char *)"-std=c99", (char *)"-w",
                     (char *)"-c", src, (char *)"-o", obj, NULL };
    if (run_child(argv) != 0) {
        fprintf(stderr, "E_KAZE_BACKEND_UNSUPPORTED: cannot build kaze_native_rt.o\n");
        exit(2);
    }
}

static int do_emit(const char *source, const char *out_path, const char *target, int stop_at_asm)
{
    size_t ssa_len = 0;
    char *ssa = compiler_ssa(source, &ssa_len);
    if (ssa == NULL) return 1;

    if (!stop_at_asm) {
        if (out_path != NULL) {
            if (write_text(out_path, ssa, ssa_len) != 0) fail(out_path);
        } else {
            fwrite(ssa, 1, ssa_len, stdout);
        }
        free(ssa);
        return 0;
    }

    char *asm_buf = NULL;
    size_t asm_len = 0;
    int rc = qbe_compile_buffer(ssa, ssa_len, target, &asm_buf, &asm_len);
    free(ssa);
    if (rc == 2) {
        fprintf(stderr, "E_KAZE_BACKEND_UNSUPPORTED: QBE has no target '%s'\n", target);
        return 2;
    }
    if (rc != 0 || asm_buf == NULL) {
        fprintf(stderr, "E_KAZE_BACKEND_UNSUPPORTED: QBE could not compile the IR\n");
        return 1;
    }
    if (out_path != NULL) {
        if (write_text(out_path, asm_buf, asm_len) != 0) fail(out_path);
    } else {
        fwrite(asm_buf, 1, asm_len, stdout);
    }
    free(asm_buf);
    return 0;
}

/* xcrun is the only subprocess that can add ~200 ms to a build.  Its answer
 * changes only when Xcode moves, so cache it next to the user's other caches
 * and let SDKROOT win when a builder has already chosen an SDK. */
static int sdk_path(char *out, size_t cap)
{
    const char *env = getenv("SDKROOT");
    if (env != NULL && env[0] != '\0') {
        snprintf(out, cap, "%s", env);
        return 0;
    }
    const char *home = getenv("HOME");
    char cache[PATH_MAX];
    if (home != NULL && home[0] != '\0') {
        snprintf(cache, sizeof cache, "%s/.cache/kaze/sdk-path", home);
        FILE *cf = fopen(cache, "r");
        if (cf != NULL) {
            if (fgets(out, (int)cap, cf) != NULL) {
                size_t n = strlen(out);
                while (n > 0 && (out[n - 1] == '\n' || out[n - 1] == '\r')) out[--n] = '\0';
                fclose(cf);
                if (n > 0 && access(out, R_OK) == 0) return 0;
            } else {
                fclose(cf);
            }
        }
    }
    FILE *f = popen("xcrun --show-sdk-path 2>/dev/null", "r");
    if (f == NULL) return -1;
    if (fgets(out, (int)cap, f) == NULL) { pclose(f); return -1; }
    pclose(f);
    size_t n = strlen(out);
    while (n > 0 && (out[n - 1] == '\n' || out[n - 1] == '\r')) out[--n] = '\0';
    if (n == 0) return -1;
    if (home != NULL && home[0] != '\0') {
        char dir[PATH_MAX];
        snprintf(dir, sizeof dir, "%s/.cache/kaze", home);
        mkdir(dir, 0700);
        snprintf(dir, sizeof dir, "%s/.cache", home);
        mkdir(dir, 0700);
        FILE *wf = fopen(cache, "w");
        if (wf != NULL) {
            fprintf(wf, "%s\n", out);
            fclose(wf);
        }
    }
    return 0;
}

static int build_native(const char *source, const char *out_path, const char *target)
{
    int apple = 0;
    char arch[64];
    if (target_arch(target, arch, sizeof arch, &apple) != 0) {
        fprintf(stderr, "E_KAZE_BACKEND_UNSUPPORTED: target '%s'\n", target);
        return 2;
    }
    if (!target_is_local(target)) {
        fprintf(stderr, "E_KAZE_ABI_MISMATCH: cannot link '%s' on this host; pass --backend=c99 or choose %s\n",
                target, default_target());
        return 2;
    }

    char dir[] = "/tmp/kaze-native-XXXXXX";
    if (mkdtemp(dir) == NULL) fail("mkdtemp");
    char ssa_path[PATH_MAX], asm_path[PATH_MAX], obj_path[PATH_MAX];
    snprintf(ssa_path, sizeof ssa_path, "%s/program.ssa", dir);
    snprintf(asm_path, sizeof asm_path, "%s/program.s", dir);
    snprintf(obj_path, sizeof obj_path, "%s/program.o", dir);

    int rc = do_emit(source, ssa_path, target, 0);
    if (rc != 0) { unlink(ssa_path); rmdir(dir); return rc; }
    size_t ssa_len = 0;
    char *ssa = read_text(ssa_path, &ssa_len);
    if (ssa == NULL) { unlink(ssa_path); rmdir(dir); fail("read SSA"); }
    char *asm_buf = NULL;
    size_t asm_len = 0;
    rc = qbe_compile_buffer(ssa, ssa_len, target, &asm_buf, &asm_len);
    free(ssa);
    if (rc != 0 || asm_buf == NULL) {
        unlink(ssa_path);
        rmdir(dir);
        fprintf(stderr, "E_KAZE_BACKEND_UNSUPPORTED: QBE could not compile the IR\n");
        return 1;
    }
    if (write_text(asm_path, asm_buf, asm_len) != 0) fail(asm_path);
    free(asm_buf);

    char *as_argv[8];
    int n = 0;
    as_argv[n++] = (char *)"as";
    if (apple) { as_argv[n++] = (char *)"-arch"; as_argv[n++] = arch; }
    as_argv[n++] = (char *)"-o";
    as_argv[n++] = obj_path;
    as_argv[n++] = asm_path;
    as_argv[n] = NULL;
    rc = run_child(as_argv);
    if (rc != 0) {
        fprintf(stderr, "E_KAZE_BACKEND_UNSUPPORTED: the system assembler rejected QBE output\n");
        unlink(ssa_path); unlink(asm_path); rmdir(dir);
        return rc;
    }

    char rt_path[PATH_MAX];
    ensure_runtime_object(rt_path, sizeof rt_path);

    char *ld_argv[20];
    n = 0;
    ld_argv[n++] = (char *)"ld";
    ld_argv[n++] = (char *)"-w";
    ld_argv[n++] = (char *)"-o";
    ld_argv[n++] = (char *)out_path;
    ld_argv[n++] = obj_path;
    ld_argv[n++] = rt_path;
    if (apple) {
        static char sdk[PATH_MAX];
        if (sdk_path(sdk, sizeof sdk) != 0) {
            fprintf(stderr, "E_KAZE_BACKEND_UNSUPPORTED: xcrun could not find the macOS SDK\n");
            unlink(ssa_path); unlink(asm_path); rmdir(dir);
            return 2;
        }
        static char syslib[PATH_MAX + 32];
        snprintf(syslib, sizeof syslib, "-syslibroot");
        ld_argv[n++] = syslib;
        ld_argv[n++] = sdk;
        ld_argv[n++] = (char *)"-lSystem";
        ld_argv[n++] = (char *)"-arch";
        ld_argv[n++] = arch;
        ld_argv[n++] = (char *)"-e";
        ld_argv[n++] = (char *)"_main";
        ld_argv[n++] = (char *)"-platform_version";
        ld_argv[n++] = (char *)"macos";
        ld_argv[n++] = (char *)"11.0";
        ld_argv[n++] = (char *)"11.0";
    } else {
        ld_argv[n++] = (char *)"-lc";
        ld_argv[n++] = (char *)"-lm";
        if (strcmp(arch, "x86_64") == 0) {
            ld_argv[n++] = (char *)"-dynamic-linker";
            ld_argv[n++] = (char *)"/lib64/ld-linux-x86-64.so.2";
        } else if (strcmp(arch, "aarch64") == 0) {
            ld_argv[n++] = (char *)"-dynamic-linker";
            ld_argv[n++] = (char *)"/lib/ld-linux-aarch64.so.1";
        }
        ld_argv[n++] = (char *)"-e";
        ld_argv[n++] = (char *)"main";
    }
    ld_argv[n] = NULL;
    if (getenv("KAZE_NATIVE_DEBUG") != NULL) {
        fputs("kaze-native ld:", stderr);
        for (int z = 0; ld_argv[z] != NULL; z++) fprintf(stderr, " %s", ld_argv[z]);
        fputc('\n', stderr);
    }
    rc = run_child(ld_argv);
    if (rc != 0 && !apple) {
        /* Some Linux distributions keep libc outside ld's default search path;
         * the C compiler's driver knows where it is.  This is a link fallback
         * only, never a C compilation of Kaze source. */
        char *cc_argv[12];
        int m = 0;
        const char *cc = getenv("KAZE_CC");
        if (cc == NULL || cc[0] == '\0') cc = "cc";
        cc_argv[m++] = (char *)cc;
        cc_argv[m++] = (char *)"-o";
        cc_argv[m++] = (char *)out_path;
        cc_argv[m++] = obj_path;
        cc_argv[m++] = rt_path;
        cc_argv[m++] = (char *)"-lm";
        cc_argv[m] = NULL;
        rc = run_child(cc_argv);
    }
    if (rc != 0) {
        fprintf(stderr, "E_KAZE_BACKEND_UNSUPPORTED: the system linker rejected the object\n");
    }
    unlink(ssa_path);
    unlink(asm_path);
    unlink(obj_path);
    rmdir(dir);
    return rc;
}

static int do_run(const char *source, int argc, char **argv, const char *target)
{
    char dir[] = "/tmp/kaze-native-XXXXXX";
    if (mkdtemp(dir) == NULL) fail("mkdtemp");
    char bin[PATH_MAX];
    snprintf(bin, sizeof bin, "%s/program", dir);
    int rc = build_native(source, bin, target);
    if (rc == 0) {
        char *run_argv[64];
        int n = 0;
        run_argv[n++] = bin;
        for (int i = 0; i < argc && n < 62; i++) run_argv[n++] = argv[i];
        run_argv[n] = NULL;
        rc = run_child(run_argv);
    }
    if (getenv("KAZE_NATIVE_KEEP") == NULL) {
        unlink(bin);
        rmdir(dir);
    } else {
        fprintf(stderr, "kaze-native: kept %s\n", bin);
    }
    return rc;
}

static void usage(const char *self)
{
    fprintf(stderr,
            "usage: %s emit  <file.kaze> [-o file.ssa] [--target T]\n"
            "       %s asm   <file.kaze> [-o file.s]   [--target T]\n"
            "       %s build <file.kaze> [-o program]  [--target T]\n"
            "       %s run   <file.kaze> [args...]     [--target T]\n"
            "       %s version\n"
            "targets: arm64_apple arm64 amd64_sysv amd64_apple rv64\n",
            self, self, self, self, self);
}

int main(int argc, char **argv)
{
    if (argc < 2) { usage(argv[0]); return 2; }
    find_tools(argv[0]);
    if (strcmp(argv[1], "version") == 0 || strcmp(argv[1], "--version") == 0) {
        printf("kaze-native 1.0 (QBE 1.2, Stage A)\n");
        return 0;
    }
    if (argc < 3) { usage(argv[0]); return 2; }

    const char *mode = argv[1];
    const char *source = argv[2];
    const char *out_path = NULL;
    const char *target = default_target();
    int arg_start = 3;
    for (int i = 3; i < argc; i++) {
        if (strcmp(argv[i], "-o") == 0 && i + 1 < argc) {
            out_path = argv[++i];
            arg_start = i + 1;
        } else if (strncmp(argv[i], "--target=", 9) == 0) {
            target = argv[i] + 9;
            arg_start = i + 1;
        } else if (strcmp(argv[i], "--target") == 0 && i + 1 < argc) {
            target = argv[++i];
            arg_start = i + 1;
        } else if (strncmp(argv[i], "--", 2) == 0) {
            /* An unknown option belongs to the program (--arg, --ticks, ...). */
            arg_start = i;
            break;
        } else {
            arg_start = i;
            break;
        }
    }

    if (strcmp(mode, "emit") == 0) return do_emit(source, out_path, target, 0);
    if (strcmp(mode, "asm") == 0) return do_emit(source, out_path, target, 1);
    if (strcmp(mode, "build") == 0 || strcmp(mode, "run") == 0) {
        /* The differential gate compiles emitted C directly, so it opts out:
         * it compares what the backend does after a program has been checked.
         * Ordinary users get the same refusal the C99 driver gives. */
        if (getenv("KAZE_NATIVE_NO_CHECK") == NULL) {
            int check = compiler_check(source);
            if (check != 0) return check;
        }
    }
    if (strcmp(mode, "build") == 0) return build_native(source, out_path != NULL ? out_path : "a.out", target);
    if (strcmp(mode, "run") == 0) return do_run(source, argc - arg_start, argv + arg_start, target);
    usage(argv[0]);
    return 2;
}
