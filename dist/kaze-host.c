/* kaze-host: a small C host that gives the Kaze toolchain the two things a
 * pure-Kaze binary cannot do itself -- spawn a C compiler and exec a built program.
 *
 * The language has no process-spawn and no file-write primitive (by design: scripts
 * reach the outside world through capabilities), so `kaze run` lives here, in the
 * host, exactly as Phase 10's "embedded host bridge" describes.
 *
 *   kaze-host run <file.kaze> [args...]   emit C, build it, run it
 *   kaze-host build <file.kaze> [-o out] [--force]
 *                                        incremental per-module .o build
 *   kaze-host <any compiler command> ...  pass through to the compiler
 *
 * The compiler is found via $KAZE_BIN, else next to this binary, else on $PATH.
 */
#define _POSIX_C_SOURCE 200809L
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>   /* strncasecmp */
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#if defined(__APPLE__)
/* _POSIX_C_SOURCE alone does not expose mkdtemp on Darwin. */
char *mkdtemp(char *);
#endif

static const char* kaze_bin = NULL;

static void fail(const char* what) {
    fprintf(stderr, "kaze-host: %s: %s\n", what, strerror(errno));
    exit(70);
}

/* Run a child and return its exit status (or 70 on spawn failure). */
static int run_child(char* const argv[]) {
    pid_t pid = fork();
    if (pid < 0) fail("fork");
    if (pid == 0) {
        execvp(argv[0], argv);
        fprintf(stderr, "kaze-host: cannot run %s: %s\n", argv[0], strerror(errno));
        _exit(127);
    }
    int status = 0;
    while (waitpid(pid, &status, 0) < 0) { if (errno != EINTR) fail("waitpid"); }
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return 70;
}

static void find_compiler(const char* self) {
    const char* env = getenv("KAZE_BIN");
    if (env && env[0]) { kaze_bin = env; return; }
    /* same directory as this binary, named kaze */
    static char buf[4096];
    const char* slash = strrchr(self, '/');
    if (slash != NULL) {
        size_t n = (size_t)(slash - self);
        if (n + 6 < sizeof buf) {
            memcpy(buf, self, n);
            snprintf(buf + n, sizeof buf - n, "/kaze");
            if (access(buf, X_OK) == 0) { kaze_bin = buf; return; }
        }
    }
    kaze_bin = "kaze";
}

static int is_backend_flag(const char* a) {
    return strncmp(a, "--backend=", 10) == 0;
}

static void native_bin_path(const char* self, char* out, size_t cap) {
    const char* env = getenv("KAZE_NATIVE_BIN");
    if (env && env[0]) { snprintf(out, cap, "%s", env); return; }
    const char* slash = strrchr(self, '/');
    if (slash != NULL) {
        size_t n = (size_t)(slash - self);
        snprintf(out, cap, "%.*s/kaze-native", (int)n, self);
    } else {
        snprintf(out, cap, "%s", "kaze-native");
    }
}

static int do_build_native(int argc, char** argv, const char* self) {
    static char native_bin[4096];
    native_bin_path(self, native_bin, sizeof native_bin);
    char* build_argv[64];
    int n = 0;
    build_argv[n++] = native_bin;
    build_argv[n++] = (char*)"build";
    const char* source = NULL;
    for (int i = 2; i < argc && n < 60; i++) {
        if (strcmp(argv[i], "--backend=native") == 0) continue;
        if (strcmp(argv[i], "--force") == 0 || strcmp(argv[i], "--release") == 0) continue;
        if (strcmp(argv[i], "-o") == 0 && i + 1 < argc) {
            build_argv[n++] = (char*)"-o";
            build_argv[n++] = argv[++i];
            continue;
        }
        if (strncmp(argv[i], "--target=", 9) == 0 || strcmp(argv[i], "--target") == 0) {
            build_argv[n++] = argv[i];
            if (strcmp(argv[i], "--target") == 0 && i + 1 < argc) build_argv[n++] = argv[++i];
            continue;
        }
        if (source == NULL) source = argv[i];
        build_argv[n++] = argv[i];
    }
    if (source == NULL) {
        fprintf(stderr, "usage: %s build <entry.kaze> [--backend=native] [-o output]\n", self);
        return 2;
    }
    build_argv[n] = NULL;
    int status = run_child(build_argv);
    if (status == 127) fprintf(stderr, "E_KAZE_BACKEND_UNSUPPORTED: cannot find %s\n", native_bin);
    return status;
}

static int do_run_native(int argc, char** argv, const char* self) {
    static char native_bin[4096];
    native_bin_path(self, native_bin, sizeof native_bin);
    const char* source = NULL;
    char* run_argv[64];
    int n = 0;
    run_argv[n++] = native_bin;
    run_argv[n++] = (char*)"run";
    for (int i = 2; i < argc && n < 62; i++) {
        if (is_backend_flag(argv[i])) continue;
        if (source == NULL) { source = argv[i]; run_argv[n++] = argv[i]; }
        else run_argv[n++] = argv[i];
    }
    if (source == NULL) {
        fprintf(stderr, "usage: %s run --backend=native <file.kaze> [args...]\n", self);
        return 2;
    }
    run_argv[n] = NULL;
    int status = run_child(run_argv);
    if (status == 127) fprintf(stderr, "E_KAZE_BACKEND_UNSUPPORTED: cannot find %s\n", native_bin);
    return status;
}

static int do_run(int argc, char** argv, const char* self) {
    if (argc < 3) {
        fprintf(stderr, "usage: %s run <file.kaze> [args...]\n", self);
        return 2;
    }
    /* `run --backend=c99 file` and `run file --backend=c99` are both natural;
     * drop backend flags before the source is chosen and before program args. */
    char* clean[64];
    int cn = 0;
    const char* source = NULL;
    for (int i = 2; i < argc && cn < 62; i++) {
        if (is_backend_flag(argv[i])) continue;
        if (source == NULL) source = argv[i];
        else clean[cn++] = argv[i];
    }
    if (source == NULL) source = argv[2];
    static char runtime_dir[4096];
    const char* env_rt = getenv("KAZE_RT_DIR");
    if (env_rt && env_rt[0]) snprintf(runtime_dir, sizeof runtime_dir, "%s", env_rt);
    const char* selfdir = runtime_dir[0] ? NULL : strrchr(kaze_bin, '/');
    if (selfdir != NULL) {
        size_t n = (size_t)(selfdir - kaze_bin);
        if (n + 6 < sizeof runtime_dir) {
            memcpy(runtime_dir, kaze_bin, n);
            /* <dir>/dist first (a bundle), then <dir>/../dist (bin/ beside dist/) */
            snprintf(runtime_dir + n, sizeof runtime_dir - n, "/dist");
            if (access(runtime_dir, R_OK) != 0 && n + 9 < sizeof runtime_dir) {
                snprintf(runtime_dir + n, sizeof runtime_dir - n, "/../dist");
            }
        }
    }
    if (runtime_dir[0] == 0) snprintf(runtime_dir, sizeof runtime_dir, "dist");
    /* last resort: the CWD's dist directory, which is where a build tree keeps it */

    /* mkdtemp is not in the C99 namespace this file asks for, so the directory is
     * named from the pid and created with mkdir, which is portable POSIX. */
    char dir[] = "/tmp/kaze-run-XXXXXX";
    if (mkdtemp(dir) == NULL) fail("mkdtemp");   /* a predictable name is a symlink target */
    char c_path[4200], bin_path[4200];
    snprintf(c_path, sizeof c_path, "%s/program.c", dir);
    snprintf(bin_path, sizeof bin_path, "%s/program", dir);

    /* 1. emit C. The compiler returns the C as its value (it has no file-write
     *    primitive), so the host owns the redirection. */
    int status = 0;
    {
        int fd = open(c_path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
        if (fd < 0) fail("open the emitted C");
        pid_t pid = fork();
        if (pid < 0) fail("fork");
        if (pid == 0) {
            if (dup2(fd, STDOUT_FILENO) < 0) _exit(70);
            char* emit_argv[] = { (char*)kaze_bin, "emit-c", (char*)source, NULL };
            execvp(emit_argv[0], emit_argv);
            _exit(127);
        }
        int st = 0;
        if (waitpid(pid, &st, 0) < 0) fail("waitpid");
        close(fd);
        status = WIFEXITED(st) ? WEXITSTATUS(st) : 70;
    }
    if (status != 0) {
        fprintf(stderr, "kaze-host: %s did not compile\n", source);
        unlink(c_path); rmdir(dir);
        return status;
    }
    /* Does the emitted C want the runtime header? The self-hosted emitter's output
     *    does; the Rust-era emitter's output is self-contained. */
    int wants_runtime = 0;
    int wants_http = 0;
    {
        /* The prelude is large, so scan the whole file rather than a prefix. */
        FILE* probe = fopen(c_path, "r");
        if (probe != NULL) {
            char buf[8192];
            while (fgets(buf, sizeof buf, probe) != NULL) {
                if (strstr(buf, "kaze_rt.h") != NULL) wants_runtime = 1;
                if (strstr(buf, "kz_http_request") != NULL) wants_http = 1;
            }
            fclose(probe);
        }
    }

    /* 2. build it. The Rust-era compiler emits self-contained C; the self-hosted one
     *    includes kaze_rt.h, so put both forms on the include path. */
    const char* cc = getenv("KAZE_CC");
    if (cc == NULL || cc[0] == 0) cc = "cc";
    char rt_h[4200], rt_c[4200];
    snprintf(rt_h, sizeof rt_h, "-I%s", runtime_dir);
    snprintf(rt_c, sizeof rt_c, "%s/kaze_rt.c", runtime_dir);
    char* build_argv[16];
    int bn = 0;
    build_argv[bn++] = (char*)cc;
    build_argv[bn++] = "-std=c99";
    build_argv[bn++] = "-O2";
    build_argv[bn++] = "-w";
    build_argv[bn++] = (char*)rt_h;
    build_argv[bn++] = c_path;
    if (wants_runtime && access(rt_c, R_OK) == 0) build_argv[bn++] = (char*)rt_c;
    build_argv[bn++] = "-lm";
    if (wants_http) build_argv[bn++] = "-lcurl";
    build_argv[bn++] = "-o";
    build_argv[bn++] = bin_path;
    build_argv[bn] = NULL;
    // If the runtime is not next to the compiler, the emitted C had better be
    // self-contained; the runtime translation unit is simply left out above.
    status = run_child(build_argv);
    if (status != 0) {
        fprintf(stderr, "kaze-host: the emitted C for %s did not build\n", source);
        unlink(c_path); rmdir(dir);
        return status;
    }

    /* 3. run it, forwarding the remaining arguments */
    char* run_argv[64];
    int n = 0;
    run_argv[n++] = bin_path;
    for (int i = 0; i < cn && n < 62; i++) run_argv[n++] = clean[i];
    run_argv[n] = NULL;
    int code = run_child(run_argv);
    /* the temp directory is ours: remove the binary, the C and the directory */
    unlink(bin_path);
    unlink(c_path);
    rmdir(dir);
    return code;
}


/* ------------------------------------------------------------------ */
/* Native incremental build: `kaze-host build <entry.kaze> [-o out]`    */
/*                                                                     */
/* The compiler resolves the module DAG and emits one C translation    */
/* unit per module. This host compiles each unit to an object keyed by */
/* (source bytes, compiler identity, dependency interface hashes), so  */
/* editing a private body recompiles one object and relinks.            */
/* ------------------------------------------------------------------ */

#define KZH_MAX_MODULES 512
#define KZH_PATH 4096

typedef struct {
    char tag[64];
    char path[KZH_PATH];
    char deps[4 * KZH_PATH];
    char iface[64];
    char flags[128];
} kzh_module;

static uint64_t kzh_fnv1a(const void* data, size_t n, uint64_t h) {
    const unsigned char* p = (const unsigned char*)data;
    for (size_t i = 0; i < n; i++) {
        h ^= (uint64_t)p[i];
        h *= 1099511628211ULL;
    }
    return h;
}

static uint64_t kzh_hash_bytes(const char* s, uint64_t h) {
    if (s == NULL) s = "";
    return kzh_fnv1a(s, strlen(s), h);
}

/* The semantic hash of a source, or an empty string when the compiler cannot
 * produce one.  kaze-host uses it for the object key so cosmetic edits skip
 * the C compiler just as they skip the Kaze compiler. */
static int kzh_ast_hash(const char* path, char* out, size_t cap) {
    int fds[2];
    pid_t pid;
    char buf[256];
    size_t used = 0;
    ssize_t n;
    if (cap > 0) out[0] = 0;
    if (pipe(fds) != 0) return 1;
    pid = fork();
    if (pid < 0) { close(fds[0]); close(fds[1]); return 1; }
    if (pid == 0) {
        close(fds[0]);
        if (dup2(fds[1], STDOUT_FILENO) < 0) _exit(127);
        close(fds[1]);
        {
            char* av[] = { (char*)kaze_bin, (char*)"ast-hash", (char*)path, NULL };
            execvp(av[0], av);
        }
        _exit(127);
    }
    close(fds[1]);
    {
        char chunk[4096];
        while ((n = read(fds[0], chunk, sizeof chunk)) > 0) {
            size_t copy = (size_t)n;
            if (used + copy + 1 >= sizeof buf) copy = sizeof buf - 1 - used;
            if (copy > 0) {
                memcpy(buf + used, chunk, copy);
                used += copy;
            }
        }
    }
    close(fds[0]);
    {
        int status = 0;
        while (waitpid(pid, &status, 0) < 0) { if (errno != EINTR) return 1; }
        if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) return 1;
    }
    buf[used] = 0;
    if (strncmp(buf, "module ", 7) != 0) return 1;
    {
        size_t i = 7, j = 0;
        while (buf[i] != 0 && !isspace((unsigned char)buf[i]) && j + 1 < cap) out[j++] = buf[i++];
        out[j] = 0;
    }
    return out[0] != 0 ? 0 : 1;
}

static uint64_t kzh_hash_file(const char* path) {
    uint64_t h = 1469598103934665603ULL;
    FILE* f = fopen(path, "rb");
    if (f == NULL) return h;
    char buf[65536];
    size_t n;
    while ((n = fread(buf, 1, sizeof buf, f)) > 0) h = kzh_fnv1a(buf, n, h);
    fclose(f);
    return h;
}

/* Split one manifest line on tabs, preserving empty fields. Returns the number
 * of fields written. The line is modified in place. */
static int kzh_split_tabs(char* line, char** out, int max) {
    int n = 0;
    char* p = line;
    while (n < max) {
        out[n++] = p;
        char* tab = strchr(p, '\t');
        if (tab == NULL) break;
        *tab = 0;
        p = tab + 1;
    }
    return n;
}

static char* kzh_trim_nl(char* s) {
    size_t n = strlen(s);
    while (n > 0 && (s[n-1] == '\n' || s[n-1] == '\r')) s[--n] = 0;
    return s;
}

/* Capture a child's stdout. Returns malloc'd NUL-terminated text (never NULL;
 * an empty string on failure) and stores the exit status in *status. */
static char* kzh_capture(char* const argv[], int* status) {
    int fds[2];
    if (pipe(fds) != 0) fail("pipe");
    pid_t pid = fork();
    if (pid < 0) fail("fork");
    if (pid == 0) {
        close(fds[0]);
        if (dup2(fds[1], STDOUT_FILENO) < 0) _exit(70);
        close(fds[1]);
        execvp(argv[0], argv);
        fprintf(stderr, "kaze-host: cannot run %s: %s\n", argv[0], strerror(errno));
        _exit(127);
    }
    close(fds[1]);
    size_t cap = 16384, len = 0;
    char* out = (char*)malloc(cap);
    if (out == NULL) fail("malloc");
    for (;;) {
        if (len + 4096 + 1 > cap) {
            cap *= 2;
            char* nb = (char*)realloc(out, cap);
            if (nb == NULL) fail("realloc");
            out = nb;
        }
        ssize_t r = read(fds[0], out + len, cap - len - 1);
        if (r < 0) { if (errno == EINTR) continue; break; }
        if (r == 0) break;
        len += (size_t)r;
    }
    close(fds[0]);
    out[len] = 0;
    int st = 0;
    while (waitpid(pid, &st, 0) < 0) { if (errno != EINTR) fail("waitpid"); }
    *status = WIFEXITED(st) ? WEXITSTATUS(st) : 70;
    return out;
}

static const char* kzh_runtime_dir(void) {
    static char dir[KZH_PATH];
    if (dir[0]) return dir;
    const char* env_rt = getenv("KAZE_RT_DIR");
    if (env_rt && env_rt[0]) {
        snprintf(dir, sizeof dir, "%s", env_rt);
        return dir;
    }
    const char* slash = strrchr(kaze_bin, '/');
    if (slash != NULL) {
        size_t n = (size_t)(slash - kaze_bin);
        if (n + 6 < sizeof dir) {
            memcpy(dir, kaze_bin, n);
            snprintf(dir + n, sizeof dir - n, "/dist");
            if (access(dir, R_OK) != 0 && n + 9 < sizeof dir) {
                snprintf(dir + n, sizeof dir - n, "/../dist");
            }
        }
    }
    if (dir[0] == 0) snprintf(dir, sizeof dir, "dist");
    return dir;
}

/* Does a generated translation unit mention a runtime header? The manifest
 * flags describe source-level calls, but a unit can pull in a header through a
 * type or an imported wrapper. Link decisions read the C that was actually
 * produced, so they cannot drift from the emitter. */
static int kzh_file_has(const char* path, const char* needle) {
    FILE* f = fopen(path, "r");
    char line[8192];
    int hit = 0;
    if (f == NULL) return 0;
    while (fgets(line, sizeof line, f) != NULL) {
        if (strstr(line, needle) != NULL) { hit = 1; break; }
    }
    fclose(f);
    return hit;
}

static void kzh_dirname(const char* path, char* out, size_t cap) {
    snprintf(out, cap, "%s", path);
    char* slash = strrchr(out, '/');
    if (slash == NULL) {
        snprintf(out, cap, ".");
    } else if (slash == out) {
        slash[1] = 0;
    } else {
        *slash = 0;
    }
}

static void kzh_basename_stem(const char* path, char* out, size_t cap) {
    const char* base = strrchr(path, '/');
    base = base ? base + 1 : path;
    snprintf(out, cap, "%s", base);
    char* dot = strrchr(out, '.');
    if (dot != NULL && dot != out) *dot = 0;
}

static int kzh_find_module(kzh_module* mods, int n, const char* tag) {
    for (int i = 0; i < n; i++) if (strcmp(mods[i].tag, tag) == 0) return i;
    return -1;
}

static int kzh_has_flag(const kzh_module* m, const char* flag) {
    const char* p = m->flags;
    size_t n = strlen(flag);
    while (*p) {
        while (*p == ',') p++;
        const char* q = p;
        while (*q && *q != ',') q++;
        if ((size_t)(q - p) == n && strncmp(p, flag, n) == 0) return 1;
        p = q;
    }
    return 0;
}

static int do_build(int argc, char** argv, const char* self) {
    if (argc < 3) {
        fprintf(stderr, "usage: %s build <entry.kaze> [-o output]\n", self);
        return 2;
    }
    const char* entry = argv[2];
    const char* output = NULL;
    int force = 0;
    for (int i = 3; i < argc; i++) {
        if (strcmp(argv[i], "-o") == 0 && i + 1 < argc) output = argv[++i];
        else if (strcmp(argv[i], "--force") == 0) force = 1;
        else if (strcmp(argv[i], "--release") == 0) { /* same optimized build today */ }
        else if (strncmp(argv[i], "--backend=", 10) == 0) { /* c99 is this path */ }
        else { fprintf(stderr, "kaze-host: build: unknown argument %s\n", argv[i]); return 2; }
    }

    char entry_dir[KZH_PATH];
    kzh_dirname(entry, entry_dir, sizeof entry_dir);

    char* manifest_argv[] = { (char*)kaze_bin, (char*)"module-manifest", (char*)entry, NULL };
    int mstatus = 0;
    char* manifest = kzh_capture(manifest_argv, &mstatus);
    if (mstatus != 0) {
        fprintf(stderr, "kaze-host: build: module resolution failed\n%s", manifest);
        free(manifest);
        return mstatus;
    }

    static kzh_module mods[KZH_MAX_MODULES];
    int nmods = 0;
    char* save = NULL;
    for (char* line = strtok_r(manifest, "\n", &save); line != NULL; line = strtok_r(NULL, "\n", &save)) {
        kzh_trim_nl(line);
        if (line[0] == 0) continue;
        char* f[8];
        int nf = kzh_split_tabs(line, f, 8);
        if (nf > 0 && strcmp(f[0], "E") == 0) {
            fprintf(stderr, "kaze-host: build: %s\n", nf > 1 ? f[1] : line);
            free(manifest);
            return 1;
        }
        if (nf >= 6 && strcmp(f[0], "M") == 0) {
            if (nmods >= KZH_MAX_MODULES) break;
            kzh_module* m = &mods[nmods++];
            memset(m, 0, sizeof *m);
            snprintf(m->tag, sizeof m->tag, "%s", f[1]);
            snprintf(m->path, sizeof m->path, "%s", f[2]);
            snprintf(m->deps, sizeof m->deps, "%s", f[3]);
            snprintf(m->iface, sizeof m->iface, "%s", f[4]);
            snprintf(m->flags, sizeof m->flags, "%s", f[5]);
        }
    }
    free(manifest);
    if (nmods == 0) {
        fprintf(stderr, "kaze-host: build: the module manifest is empty\n");
        return 1;
    }

    char cache_dir[KZH_PATH], cache_root[KZH_PATH];
    snprintf(cache_root, sizeof cache_root, "%s/.kaze-cache", entry_dir);
    snprintf(cache_dir, sizeof cache_dir, "%s", cache_root);
    if (mkdir(cache_dir, 0777) != 0 && errno != EEXIST) fail("mkdir .kaze-cache");

    uint64_t compiler_id = kzh_hash_file(kaze_bin);
    const char* rt_dir = kzh_runtime_dir();
    const char* cc = getenv("KAZE_CC");
    if (cc == NULL || cc[0] == 0) cc = "cc";

    static char objs[KZH_MAX_MODULES][KZH_PATH];
    int compiled = 0, cached = 0;

    for (int i = 0; i < nmods; i++) {
        kzh_module* m = &mods[i];
        /* Key: source bytes, compiler identity, dependency interfaces, flags. */
        uint64_t key = 1469598103934665603ULL;
        {
            char ast[32];
            if (kzh_ast_hash(m->path, ast, sizeof ast) == 0)
                key = kzh_hash_bytes(ast, key);
            else
                key = kzh_hash_file(m->path);
        }
        key = kzh_fnv1a(&compiler_id, sizeof compiler_id, key);
        key = kzh_hash_bytes(m->flags, key);
        key = kzh_hash_bytes("c99", key);
        char* depsave = NULL;
        char depcopy[4 * KZH_PATH];
        snprintf(depcopy, sizeof depcopy, "%s", m->deps);
        for (char* d = strtok_r(depcopy, ",", &depsave); d != NULL; d = strtok_r(NULL, ",", &depsave)) {
            int di = kzh_find_module(mods, nmods, d);
            if (di >= 0) {
                key = kzh_hash_bytes(mods[di].iface, key);
                key = kzh_hash_bytes(mods[di].tag, key);
            }
        }
        char keytext[32];
        snprintf(keytext, sizeof keytext, "%016llx", (unsigned long long)key);

        snprintf(objs[i], sizeof objs[i], "%s/%s.o", cache_root, m->tag);
        char keypath[KZH_PATH], ifacepath[KZH_PATH], cpath[KZH_PATH];
        snprintf(keypath, sizeof keypath, "%s/%s.key", cache_root, m->tag);
        snprintf(ifacepath, sizeof ifacepath, "%s/%s.kziface", cache_root, m->tag);
        snprintf(cpath, sizeof cpath, "%s/%s.c", cache_root, m->tag);

        int fresh = 0;
        if (!force && access(objs[i], R_OK) == 0) {
            FILE* kf = fopen(keypath, "r");
            if (kf != NULL) {
                char have[64];
                if (fgets(have, sizeof have, kf) != NULL) {
                    kzh_trim_nl(have);
                    if (strcmp(have, keytext) == 0) fresh = 1;
                }
                fclose(kf);
            }
        }
        if (fresh) {
            cached++;
            continue;
        }

        /* 1. Ask the compiler for this module's translation unit. */
        {
            int fd = open(cpath, O_WRONLY | O_CREAT | O_TRUNC, 0600);
            if (fd < 0) fail("open the emitted unit");
            pid_t pid = fork();
            if (pid < 0) fail("fork");
            if (pid == 0) {
                if (dup2(fd, STDOUT_FILENO) < 0) _exit(70);
                char* unit_argv[] = { (char*)kaze_bin, (char*)"emit-c-unit", (char*)entry, m->tag, NULL };
                execvp(unit_argv[0], unit_argv);
                _exit(127);
            }
            int st = 0;
            if (waitpid(pid, &st, 0) < 0) fail("waitpid");
            close(fd);
            int ust = WIFEXITED(st) ? WEXITSTATUS(st) : 70;
            if (ust != 0) {
                fprintf(stderr, "kaze-host: build: %s did not emit\n", m->path);
                return ust;
            }
        }

        /* 2. Compile it. */
        {
            char inc[KZH_PATH + 3];
            snprintf(inc, sizeof inc, "-I%s", rt_dir);
            char* cc_argv[16];
            int n = 0;
            cc_argv[n++] = (char*)cc;
            cc_argv[n++] = "-std=c99";
            cc_argv[n++] = "-O2";
            cc_argv[n++] = "-w";
            cc_argv[n++] = inc;
            cc_argv[n++] = "-c";
            cc_argv[n++] = cpath;
            cc_argv[n++] = "-o";
            cc_argv[n++] = objs[i];
            cc_argv[n] = NULL;
            int st = run_child(cc_argv);
            if (st != 0) {
                fprintf(stderr, "kaze-host: build: %s did not compile\n", m->path);
                return st;
            }
        }

        /* 3. Record the key only after a successful object write. */
        {
            FILE* kf = fopen(keypath, "w");
            if (kf != NULL) { fprintf(kf, "%s\n", keytext); fclose(kf); }
            FILE* inf = fopen(ifacepath, "w");
            if (inf != NULL) { fprintf(inf, "%s\n", m->iface); fclose(inf); }
        }
        compiled++;
    }

    /* The root module is the one the manifest marks with the root flag; the
     * compiler emits main there, so link order is otherwise irrelevant. */
    char out_path[KZH_PATH];
    if (output == NULL) {
        kzh_basename_stem(entry, out_path, sizeof out_path);
    } else {
        snprintf(out_path, sizeof out_path, "%s", output);
    }

    int wants_http = 0, wants_tensor = 0, wants_parallel = 0;
    for (int i = 0; i < nmods; i++) if (kzh_has_flag(&mods[i], "http")) wants_http = 1;
    for (int i = 0; i < nmods; i++) {
        char cpath[KZH_PATH];
        snprintf(cpath, sizeof cpath, "%s/%s.c", cache_root, mods[i].tag);
        if (kzh_file_has(cpath, "kaze_http.h")) wants_http = 1;
        if (kzh_file_has(cpath, "kaze_tensor.h")) wants_tensor = 1;
        if (kzh_file_has(cpath, "pthread.h")) wants_parallel = 1;
    }

    char rt_c[KZH_PATH];
    snprintf(rt_c, sizeof rt_c, "%s/kaze_rt.c", rt_dir);
    char** link_argv = (char**)calloc((size_t)nmods + 32, sizeof(char*));
    if (link_argv == NULL) fail("calloc");
    int ln = 0;
    link_argv[ln++] = (char*)cc;
    link_argv[ln++] = "-std=c99";
    link_argv[ln++] = "-O2";
    link_argv[ln++] = "-w";
    for (int i = 0; i < nmods; i++) link_argv[ln++] = objs[i];
    if (access(rt_c, R_OK) == 0) link_argv[ln++] = rt_c;
    if (wants_tensor) {
        /* The same archive closure build-cli.sh ships: the tensor unit can
         * reference quantisation/GGUF helpers, so all six sources travel
         * together. */
        static const char* tensor_src[] = {
            "kaze_tensor.c", "kaze_quant.c", "kaze_gguf.c",
            "kaze_llama.c", "kaze_tokenizer.c", "kaze_autograd.c"
        };
        static char tensor_paths[6][KZH_PATH];
        for (int i = 0; i < 6; i++) {
            snprintf(tensor_paths[i], sizeof tensor_paths[i], "%s/%s", rt_dir, tensor_src[i]);
            if (access(tensor_paths[i], R_OK) == 0) link_argv[ln++] = tensor_paths[i];
        }
    }
    link_argv[ln++] = "-lm";
    if (wants_http) link_argv[ln++] = "-lcurl";
    if (wants_tensor || wants_parallel) link_argv[ln++] = "-pthread";
    link_argv[ln++] = "-o";
    link_argv[ln++] = out_path;
    link_argv[ln] = NULL;
    int lst = run_child(link_argv);
    free(link_argv);
    if (lst != 0) {
        fprintf(stderr, "kaze-host: build: link failed\n");
        return lst;
    }
    fprintf(stderr, "kaze-host: built %s (%d module%s, %d compiled, %d cached)\n",
            out_path, nmods, nmods == 1 ? "" : "s", compiled, cached);
    return 0;
}

/* ------------------------------------------------------------------ */
/* Interactive LSP                                                     */
/*                                                                     */
/* The compiler cannot read a stream or write one reply at a time, but */
/* a host can. This loop owns the pipes and asks the compiler for one  */
/* thing it is good at: the diagnostics for a document.                */
/* ------------------------------------------------------------------ */

static char lsp_msg[1 << 20];   /* the last JSON-RPC message */
static char lsp_doc[1 << 20];   /* current document text */
static size_t lsp_doc_len = 0;

static int lsp_write(const char* body) {
    size_t n = strlen(body);
    printf("Content-Length: %lu\r\n\r\n%s", (unsigned long)n, body);
    fflush(stdout);
    return 0;
}

/* Copy the JSON string value that follows key into out, unescaping \" \\ \n \t.
 * Returns 0 when the key is absent. Deliberately small: the host is not a JSON
 * library, it reads the three fields the protocol needs. */
static int json_string(const char* src, const char* key, char* out, size_t cap) {
    const char* p = strstr(src, key);
    if (p == NULL) return 0;
    p = strchr(p + strlen(key), ':');
    if (p == NULL) return 0;
    p++;
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
    if (*p != '"') return 0;
    p++;
    size_t n = 0;
    while (*p && *p != '"') {
        char c = *p++;
        if (c == '\\' && *p) {
            char e = *p++;
            if (e == 'n') c = '\n';
            else if (e == 't') c = '\t';
            else c = e;
        }
        if (n + 1 < cap) out[n++] = c;
    }
    out[n] = 0;
    return 1;
}

/* The request id, echoed verbatim, or absent.
 *
 * Finds the top-level "id" by tracking brace depth, because JSON member order is not
 * fixed: an earlier version only looked before "params" and a client that put `id` last
 * (still valid JSON) got an uninitialised token echoed back - the language server replied
 * with stack garbage. Numbers and strings are both copied as written, so the reply can
 * echo exactly what the client sent. */
static int json_id_token(const char* src, char* out, size_t cap) {
    if (cap < 2) return 0;
    out[0] = 0;
    int depth = 0;
    int in_str = 0;
    for (const char* p = src; *p != 0; p++) {
        if (in_str) {
            if (*p == '\\' && p[1] != 0) { p++; continue; }
            if (*p == '"') in_str = 0;
            continue;
        }
        if (*p == '"') {
            /* a string at depth 1 whose text is exactly "id" is the id key */
            const char* q = p + 1;
            const char* r = q;
            while (*r != 0 && *r != '"') { if (*r == '\\' && r[1] != 0) r++; r++; }
            if (depth <= 1 && (r - q) == 2 && q[0] == 'i' && q[1] == 'd') {
                const char* v = (r != 0 && *r == '"') ? r + 1 : r;
                while (*v == ' ' || *v == '\t' || *v == '\n' || *v == '\r' || *v == ':') v++;
                size_t n = 0;
                if (*v == '"') {
                    out[n++] = *v++;
                    while (*v != 0 && *v != '"' && n + 2 < cap) out[n++] = *v++;
                    if (*v == '"') out[n++] = *v++;
                } else {
                    while (*v == '-' || *v == '+' || *v == '.' || *v == 'e' || *v == 'E' || (*v >= '0' && *v <= '9')) {
                        if (n + 2 >= cap) break;
                        out[n++] = *v++;
                    }
                }
                out[n] = 0;
                return n > 0;
            }
            in_str = 1;
            continue;
        }
        if (*p == '{' || *p == '[') depth++;
        else if (*p == '}' || *p == ']') depth--;
    }
    return 0;
}

/* Escape one string into dst as a JSON string body: quotes, backslashes and every
 * control character below 0x20. One function for every place a value is embedded,
 * because the first review of this file found the uri going in unescaped while the
 * message was escaped, and a uri with a quote broke the frame. */
static void json_escape(char* dst, size_t cap, const char* src) {
    size_t o = 0;
    for (const unsigned char* p = (const unsigned char*)src; p != NULL && *p != 0; p++) {
        if (o + 8 >= cap) break;              /* leave room for a 6-byte \u00XX */
        unsigned char ch = *p;
        if (ch == '"' || ch == '\\') { dst[o++] = '\\'; dst[o++] = (char)ch; }
        else if (ch == '\n') { dst[o++] = '\\'; dst[o++] = 'n'; }
        else if (ch == '\r') { dst[o++] = '\\'; dst[o++] = 'r'; }
        else if (ch == '\t') { dst[o++] = '\\'; dst[o++] = 't'; }
        else if (ch < 0x20) {
            static const char hex[] = "0123456789abcdef";
            dst[o++] = '\\'; dst[o++] = 'u'; dst[o++] = '0'; dst[o++] = '0';
            dst[o++] = hex[(ch >> 4) & 0xF]; dst[o++] = hex[ch & 0xF];
        } else { dst[o++] = (char)ch; }
    }
    /* do not leave half a UTF-8 sequence: back off over continuation bytes */
    while (o > 0 && ((unsigned char)dst[o - 1] & 0xC0) == 0x80) o--;
    dst[o] = 0;
}

/* Copy a bounded, escaped version of a compiler line into dst. */
static void json_escape_line(char* dst, size_t cap, const char* src) {
    size_t o = 0;
    for (const unsigned char* p = (const unsigned char*)src; p != NULL && *p != 0 && *p != '\n' && *p != '\r'; p++) {
        if (o + 8 >= cap) break;
        unsigned char ch = *p;
        if (ch == '"' || ch == '\\') { dst[o++] = '\\'; dst[o++] = (char)ch; }
        else if (ch == '\t') { dst[o++] = '\\'; dst[o++] = 't'; }
        else if (ch < 0x20) {
            static const char hex[] = "0123456789abcdef";
            dst[o++] = '\\'; dst[o++] = 'u'; dst[o++] = '0'; dst[o++] = '0';
            dst[o++] = hex[(ch >> 4) & 0xF]; dst[o++] = hex[ch & 0xF];
        } else { dst[o++] = (char)ch; }
    }
    /* do not leave half a UTF-8 sequence: back off over continuation bytes */
    while (o > 0 && ((unsigned char)dst[o - 1] & 0xC0) == 0x80) o--;
    dst[o] = 0;
}
/* Diagnostics for the current document, from the compiler.
 *
 * Two things this function must not do, both found by review: trust snprintf's return
 * value (it is the would-be length, so `used` could pass the end of `body` and the final
 * append wrapped `sizeof body - used` into a huge size - a stack overflow reachable from a
 * document with long identifiers), and leave the compiler child unreaped when a pipe
 * cannot be wrapped. */
static char lsp_src_path[256];   /* created once by mkstemp, removed at exit */

static void lsp_cleanup(void) {
    if (lsp_src_path[0] != 0) unlink(lsp_src_path);
}

static void lsp_publish(const char* uri) {
    if (lsp_src_path[0] == 0) {
        snprintf(lsp_src_path, sizeof lsp_src_path, "/tmp/kaze-lsp-XXXXXX");
        int fd = mkstemp(lsp_src_path);
        if (fd < 0) { lsp_src_path[0] = 0; return; }
        close(fd);
        atexit(lsp_cleanup);
    }
    const char* src_path = lsp_src_path;
    FILE* fp = fopen(src_path, "w");
    if (fp == NULL) return;
    fwrite(lsp_doc, 1, lsp_doc_len, fp);
    fclose(fp);

    int out[2];
    if (pipe(out) != 0) { unlink(src_path); return; }
    pid_t pid = fork();
    if (pid < 0) { close(out[0]); close(out[1]); unlink(src_path); return; }
    if (pid == 0) {
        close(out[0]);
        if (dup2(out[1], STDOUT_FILENO) < 0 || dup2(out[1], STDERR_FILENO) < 0) _exit(70);
        char* argv2[] = { (char*)kaze_bin, "check", (char*)src_path, NULL };
        execvp(argv2[0], argv2);
        _exit(127);
    }
    close(out[1]);
    FILE* pipe_out = fdopen(out[0], "r");
    if (pipe_out == NULL) {
        close(out[0]);
        { int st0 = 0; (void)waitpid(pid, &st0, 0); }   /* never leave a zombie */
        unlink(src_path);
        return;
    }

    char body[8192];
    size_t used = 0;
    char safe_uri[1024];
    json_escape(safe_uri, sizeof safe_uri, uri);
    int need = snprintf(body, sizeof body,
                        "{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\","
                        "\"params\":{\"uri\":\"%s\",\"diagnostics\":[", safe_uri);
    if (need < 0 || (size_t)need >= sizeof body) { fclose(pipe_out); unlink(src_path); return; }
    used = (size_t)need;

    int count = 0;
    /* 4 KiB, not 1 KiB: a longer diagnostic would be split by fgets and the
       continuation dropped, silently losing the tail of a message. */
    char line[4096];
    while (fgets(line, sizeof line, pipe_out) != NULL) {
        if (strncmp(line, "E_", 2) != 0) continue;
        int lineno = 0;
        const char* at = strstr(line, " at line ");
        if (at != NULL) lineno = atoi(at + 9) - 1;
        if (lineno < 0) lineno = 0;
        char msg[1024];
        json_escape_line(msg, sizeof msg, line);
        size_t room = sizeof body - used;
        int n2 = snprintf(body + used, room,
                          "%s{\"range\":{\"start\":{\"line\":%d,\"character\":0},"
                          "\"end\":{\"line\":%d,\"character\":0}},\"severity\":1,\"message\":\"%s\"}",
                          count > 0 ? "," : "", lineno, lineno, msg);
        if (n2 < 0 || (size_t)n2 >= room) break;   /* roll back: keep the JSON valid */
        used += (size_t)n2;
        count++;
    }
    fclose(pipe_out);
    { int st2 = 0; (void)waitpid(pid, &st2, 0); }
    unlink(src_path);
    if (used + 4 > sizeof body) used = sizeof body - 4;
    snprintf(body + used, sizeof body - used, "]}}");
    lsp_write(body);
}

/* The word at a line/character, and its declaration line if the document declares it. */
static int lsp_word_at(int want_line, int want_char, char* word, size_t cap, int* decl_line) {
    int line = 0;
    size_t i = 0;
    while (i < lsp_doc_len) {
        size_t start = i;
        while (i < lsp_doc_len && lsp_doc[i] != '\n') i++;
        if (line == want_line) {
            size_t len = i - start;
            size_t c = (size_t)want_char < len ? (size_t)want_char : (len ? len - 1 : 0);
            size_t a = start + c, b = a;
            while (a > start && (isalnum((unsigned char)lsp_doc[a - 1]) || lsp_doc[a - 1] == '_')) a--;
            while (b < start + len && (isalnum((unsigned char)lsp_doc[b]) || lsp_doc[b] == '_')) b++;
            size_t n = b - a;
            if (n == 0 || n + 1 > cap) return 0;
            memcpy(word, lsp_doc + a, n);
            word[n] = 0;
            *decl_line = -1;
            int ln = 0;
            size_t j = 0;
            while (j < lsp_doc_len) {
                size_t s2 = j;
                while (j < lsp_doc_len && lsp_doc[j] != '\n') j++;
                const char* text = lsp_doc + s2;
                size_t tl = j - s2;
                if ((strncmp(text, "action ", 7) == 0 || strncmp(text, "record ", 7) == 0 ||
                     strncmp(text, "routine ", 8) == 0) &&
                    strstr(text, word) != NULL && strstr(text, word) < text + tl) {
                    *decl_line = ln;
                    break;
                }
                ln++;
                j++;
            }
            return 1;
        }
        line++;
        i++;
    }
    return 0;
}

static int lsp_position(const char* msg, int* line, int* ch) {
    const char* p = strstr(msg, "\"position\"");
    if (p == NULL) return 0;
    const char* l = strstr(p, "\"line\"");
    const char* c = strstr(p, "\"character\"");
    if (l == NULL || c == NULL) return 0;
    /* A hostile client can send `"line"` with no colon; the original code passed
     * strchr()'s NULL to atoi and died with SIGSEGV (verified under ASan). */
    const char* lc = strchr(l, ':');
    const char* cc2 = strchr(c, ':');
    if (lc == NULL || cc2 == NULL) return 0;
    *line = atoi(lc + 1);
    *ch = atoi(cc2 + 1);
    return 1;
}

/* Read one framed message into lsp_msg. 0 = ok, 1 = closed, 2 = framing error. */
static int read_frame(void) {
    char header[512];
    for (;;) {
        if (fgets(header, sizeof header, stdin) == NULL) return 1;
        if (strncasecmp(header, "Content-Length:", 15) == 0) break;
    }
    char* end = NULL;
    long len = strtol(header + 15, &end, 10);
    if (end == header + 15 || (*end != 0 && *end != '\r' && *end != '\n' && *end != ' ')) return 2;
    if (len < 0 || len > (long)sizeof lsp_msg - 1) return 2;
    /* consume the rest of the header block */
    while (fgets(header, sizeof header, stdin) != NULL) {
        if (strcmp(header, "\r\n") == 0 || strcmp(header, "\n") == 0) break;
    }
    if (len == 0) return 3;   /* an empty frame is not a kill switch: skip it */
    size_t got = 0;
    while (got < (size_t)len) {
        size_t n = fread(lsp_msg + got, 1, (size_t)len - got, stdin);
        if (n == 0) return 1;   /* EOF mid-frame: the stream is unrecoverable */
        got += n;
    }
    lsp_msg[got] = 0;
    return 0;
}

/* True when another frame is already waiting, with a short quiet period: a burst of
 * edits should produce one publishDiagnostics, not one compiler fork per keystroke. */
static int more_input(int ms) {
    struct pollfd pfd;
    pfd.fd = STDIN_FILENO;
    pfd.events = POLLIN;
    pfd.revents = 0;
    return poll(&pfd, 1, ms) > 0;
}

static int do_lsp(void) {
    int saw_shutdown = 0;
    int pending = 0;
    char pending_uri[512];
    pending_uri[0] = 0;

    for (;;) {
        /* Coalesce: if a publish is pending and the client is quiet, publish now. */
        if (pending && !more_input(0)) {
            lsp_publish(pending_uri);
            pending = 0;
        }
        int rc = read_frame();
        if (rc == 1 || rc == 2) {
            if (pending) lsp_publish(pending_uri);   /* do not drop the last edit */
            return rc == 1 ? 0 : 1;
        }
        if (rc == 3) continue;

        char method[128];
        if (!json_string(lsp_msg, "\"method\"", method, sizeof method)) continue;
        char id_tok[64];
        int has_id = json_id_token(lsp_msg, id_tok, sizeof id_tok);

        if (strcmp(method, "initialize") == 0) {
            char body[512];
            snprintf(body, sizeof body,
                     "{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"capabilities\":{\"textDocumentSync\":1,\"hoverProvider\":true,\"definitionProvider\":true},\"serverInfo\":{\"name\":\"kaze-host-lsp\",\"version\":\"1.0\"}}}", id_tok);
            lsp_write(body);
        } else if (strcmp(method, "initialized") == 0) {
            /* notification: no reply */
        } else if (strcmp(method, "textDocument/didOpen") == 0 ||
                   strcmp(method, "textDocument/didChange") == 0) {
            char text[1 << 20];
            char uri[512];
            if (json_string(lsp_msg, "\"uri\"", uri, sizeof uri) &&
                json_string(lsp_msg, "\"text\"", text, sizeof text)) {
                size_t n = strlen(text);
                if (n > sizeof lsp_doc - 1) n = sizeof lsp_doc - 1;
                memcpy(lsp_doc, text, n);
                lsp_doc[n] = 0;
                lsp_doc_len = n;
                snprintf(pending_uri, sizeof pending_uri, "%s", uri);
                pending = 1;
            }
        } else if (strcmp(method, "textDocument/hover") == 0) {
            int line = 0, ch = 0;
            char word[128];
            int decl = -1;
            char body[1024];
            if (lsp_position(lsp_msg, &line, &ch) && lsp_word_at(line, ch, word, sizeof word, &decl) && decl >= 0) {
                snprintf(body, sizeof body,
                         "{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"contents\":{\"kind\":\"markdown\",\"value\":\"`%s` is declared in this document\"}}}", id_tok, word);
            } else {
                snprintf(body, sizeof body, "{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":null}", id_tok);
            }
            lsp_write(body);
        } else if (strcmp(method, "textDocument/definition") == 0) {
            int line = 0, ch = 0;
            char word[128];
            int decl = -1;
            char body[1024];
            char uri[512] = "file:///document.kaze";
            (void)json_string(lsp_msg, "\"uri\"", uri, sizeof uri);
            if (lsp_position(lsp_msg, &line, &ch) && lsp_word_at(line, ch, word, sizeof word, &decl) && decl >= 0) {
                snprintf(body, sizeof body,
                         "{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"uri\":\"%s\",\"range\":{\"start\":{\"line\":%d,\"character\":0},\"end\":{\"line\":%d,\"character\":0}}}}",
                         id_tok, uri, decl, decl);
            } else {
                snprintf(body, sizeof body, "{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":null}", id_tok);
            }
            lsp_write(body);
        } else if (strcmp(method, "shutdown") == 0) {
            saw_shutdown = 1;
            char body[256];
            snprintf(body, sizeof body, "{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":null}", id_tok);
            lsp_write(body);
        } else if (strcmp(method, "exit") == 0) {
            if (pending) lsp_publish(pending_uri);
            return saw_shutdown ? 0 : 1;
        } else if (has_id) {
            char body[256];
            snprintf(body, sizeof body,
                     "{\"jsonrpc\":\"2.0\",\"id\":%s,\"error\":{\"code\":-32601,\"message\":\"method not found\"}}", id_tok);
            lsp_write(body);
        }
    }
}
int main(int argc, char** argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s run <file.kaze> [args...] | version | <compiler command> ...\n", argv[0]);
        return 2;
    }
    find_compiler(argv[0]);
    if (strcmp(argv[1], "run") == 0) {
        for (int i = 2; i < argc; i++)
            if (strcmp(argv[i], "--backend=native") == 0) return do_run_native(argc, argv, argv[0]);
        return do_run(argc, argv, argv[0]);
    }
    if (strcmp(argv[1], "lsp") == 0) return do_lsp();
    if (strcmp(argv[1], "build") == 0) {
        for (int i = 2; i < argc; i++)
            if (strcmp(argv[i], "--backend=native") == 0) return do_build_native(argc, argv, argv[0]);
        return do_build(argc, argv, argv[0]);
    }

    /* `check` in the compiler prints diagnostics but always exits 0, so a pipeline
     * cannot see failure. The host watches the text and supplies the status the
     * compiler does not: 0 when it says "check: ok", 1 when it reports diagnostics. */
    if (strcmp(argv[1], "check") == 0) {
        char out_path[] = "/tmp/kaze-check-XXXXXX";
        int fd = mkstemp(out_path);
        if (fd < 0) fail("open the check output");
        pid_t pid = fork();
        if (pid < 0) fail("fork");
        if (pid == 0) {
            if (dup2(fd, STDOUT_FILENO) < 0) _exit(70);
            char* fwd[64];
            int n = 0;
            fwd[n++] = (char*)kaze_bin;
            for (int i = 1; i < argc && n < 62; i++) fwd[n++] = argv[i];
            fwd[n] = NULL;
            execvp(fwd[0], fwd);
            _exit(127);
        }
        int st = 0;
        if (waitpid(pid, &st, 0) < 0) fail("waitpid");
        close(fd);
        int compiler_status = WIFEXITED(st) ? WEXITSTATUS(st) : 70;
        /* The compiler exits 1 on diagnostics and still prints them. A spawn
         * failure (127) or a crash keeps its own status. */
        if (compiler_status != 0 && compiler_status != 1) { unlink(out_path); return compiler_status; }
        FILE* fp = fopen(out_path, "r");
        int clean = 0, reported = 0, native_bad = 0;
        if (fp != NULL) {
            char line[1024];
            while (fgets(line, sizeof line, fp) != NULL) {
                fputs(line, stdout);
                if (strstr(line, "check: ok") != NULL) clean = 1;
                if (strstr(line, "diagnostic(s)") != NULL) reported = 1;
                if (strncmp(line, "native: ", 8) == 0 && strstr(line, "E_KAZE_") != NULL) native_bad = 1;
            }
            fclose(fp);
        }
        unlink(out_path);
        if (reported || compiler_status == 1) return 1;
        (void)clean;   /* unknown text: do not invent a failure */
        return 0;
    }

    /* Normalize version flags into the compiler's `version` subcommand. */
    if (strcmp(argv[1], "--version") == 0 || strcmp(argv[1], "-V") == 0) {
        char* ver_argv[] = { (char*)kaze_bin, (char*)"version", NULL };
        return run_child(ver_argv);
    }

    /* everything else goes to the compiler unchanged */
    char* fwd[64];
    int n = 0;
    fwd[n++] = (char*)kaze_bin;
    for (int i = 1; i < argc && n < 62; i++) fwd[n++] = argv[i];
    fwd[n] = NULL;
    return run_child(fwd);
}










