/* kaze_cli.c -- the unified Kaze command-line driver.
 *
 * This is the user-facing `kaze` executable.  It owns process creation, the
 * multi-file module loader, automatic subsystem linking, and the command
 * surface; the pure-Kaze compiler is linked into the same binary and called
 * in a forked child for each compilation.  A child is used for two reasons:
 * the compiler's generated entry point owns a single arena, and a compiler
 * trap must not take the driver (or an LSP session) down with it.  The child
 * is created with fork(), never with exec() of a sibling `kaze` binary, so a
 * copied executable works with no ./bin directory beside it.
 *
 * Exit codes:
 *   0   success
 *   1   compile/check/verify diagnostic
 *   2   CLI usage or setup error
 *   70  runtime trap (a program's own exit status is passed through)
 *
 * The runtime sources are embedded at build time (dist/kaze_embedded_rt.h).
 * Pre-built static archives in <prefix>/lib/kaze, when present, are preferred
 * for speed; otherwise the needed C files are materialised from the embedded
 * copies in a temporary directory.
 */
#define _POSIX_C_SOURCE 200809L

#include <ctype.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#if defined(__APPLE__)
#include <mach-o/dyld.h>
/* _POSIX_C_SOURCE alone does not expose mkdtemp on Darwin. */
char *mkdtemp(char *);
#endif

#include "kaze_embedded_rt.h"

#ifndef PATH_MAX
#define PATH_MAX 4096
#endif

#ifndef KAZE_VERSION
#define KAZE_VERSION "1.0"
#endif

/* The two library entry points.  They are generated C `main` functions whose
 * names are remapped with -Dmain=... by scripts/build-cli.sh. */
extern int kaze_compiler_main(int argc, char **argv);
#if KAZE_HAVE_VERIFIER
extern int kaze_verify_main(int argc, char **argv);
#endif

#define KAZE_EXIT_OK 0
#define KAZE_EXIT_DIAG 1
#define KAZE_EXIT_USAGE 2
#define KAZE_EXIT_TRAP 70

/* ------------------------------------------------------------------ */
/* small utilities                                                     */
/* ------------------------------------------------------------------ */

static char *native_tool_path(void);
static int g_verbose = 0;
static int g_keep_temps = 0;
static int g_in_child = 0;

static void die_oom(void) {
    fputs("kaze: out of memory\n", stderr);
    _exit(KAZE_EXIT_TRAP);
}

static void *xmalloc(size_t n) {
    void *p = malloc(n ? n : 1);
    if (p == NULL) die_oom();
    return p;
}

static void *xrealloc(void *p, size_t n) {
    void *q = realloc(p, n ? n : 1);
    if (q == NULL) die_oom();
    return q;
}

static char *xstrdup(const char *s) {
    size_t n = s ? strlen(s) : 0;
    char *p = (char *)xmalloc(n + 1);
    if (n) memcpy(p, s, n);
    p[n] = 0;
    return p;
}

static char *xasprintf(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(NULL, 0, fmt, ap);
    va_end(ap);
    if (n < 0) die_oom();
    char *p = (char *)xmalloc((size_t)n + 1);
    va_start(ap, fmt);
    vsnprintf(p, (size_t)n + 1, fmt, ap);
    va_end(ap);
    return p;
}

typedef struct {
    char *data;
    size_t len;
    size_t cap;
} Buf;

static void buf_init(Buf *b) {
    b->data = NULL;
    b->len = 0;
    b->cap = 0;
}

static void buf_reserve(Buf *b, size_t extra) {
    size_t need = b->len + extra + 1;
    if (need <= b->cap) return;
    size_t cap = b->cap ? b->cap : 256;
    while (cap < need) cap *= 2;
    b->data = (char *)xrealloc(b->data, cap);
    b->cap = cap;
}

static void buf_append(Buf *b, const void *p, size_t n) {
    buf_reserve(b, n);
    if (n) memcpy(b->data + b->len, p, n);
    b->len += n;
    b->data[b->len] = 0;
}

static void buf_appends(Buf *b, const char *s) {
    if (s != NULL) buf_append(b, s, strlen(s));
}

static void buf_appendc(Buf *b, char c) {
    buf_append(b, &c, 1);
}

static void buf_free(Buf *b) {
    free(b->data);
    b->data = NULL;
    b->len = 0;
    b->cap = 0;
}

typedef struct {
    char **items;
    size_t len;
    size_t cap;
} StrVec;

static void vec_init(StrVec *v) {
    v->items = NULL;
    v->len = 0;
    v->cap = 0;
}

static void vec_push(StrVec *v, const char *s) {
    if (v->len + 1 > v->cap) {
        size_t cap = v->cap ? v->cap * 2 : 8;
        v->items = (char **)xrealloc(v->items, cap * sizeof(char *));
        v->cap = cap;
    }
    v->items[v->len++] = xstrdup(s);
}

static void vec_push_owned(StrVec *v, char *s) {
    if (v->len + 1 > v->cap) {
        size_t cap = v->cap ? v->cap * 2 : 8;
        v->items = (char **)xrealloc(v->items, cap * sizeof(char *));
        v->cap = cap;
    }
    v->items[v->len++] = s;
}

static int vec_has(const StrVec *v, const char *s) {
    size_t i;
    for (i = 0; i < v->len; i++) {
        if (strcmp(v->items[i], s) == 0) return 1;
    }
    return 0;
}

static void vec_free(StrVec *v) {
    size_t i;
    for (i = 0; i < v->len; i++) free(v->items[i]);
    free(v->items);
    v->items = NULL;
    v->len = 0;
    v->cap = 0;
}

/* ------------------------------------------------------------------ */
/* temporary files and cleanup                                         */
/* ------------------------------------------------------------------ */

#define MAX_TRACKED 8192
static char *g_tracked_paths[MAX_TRACKED];
static int g_tracked_path_count = 0;
static char *g_tracked_dirs[MAX_TRACKED];
static int g_tracked_dir_count = 0;

static void track_path(const char *p) {
    if (p == NULL || p[0] == 0) return;
    if (g_tracked_path_count < MAX_TRACKED) {
        g_tracked_paths[g_tracked_path_count++] = xstrdup(p);
    }
}

static void track_dir(const char *p) {
    if (p == NULL || p[0] == 0) return;
    if (g_tracked_dir_count < MAX_TRACKED) {
        g_tracked_dirs[g_tracked_dir_count++] = xstrdup(p);
    }
}

static char *path_join(const char *a, const char *b);

/* Remove a directory tree without assuming the host has `rm -r`.  Build
 * scratch directories contain an `include/`, a `src/`, generated C and the
 * program itself, so a bare rmdir is not enough. */
static void remove_tree(const char *path) {
    struct stat st;
    DIR *dir;
    struct dirent *ent;
    if (path == NULL || lstat(path, &st) != 0) return;
    if (!S_ISDIR(st.st_mode)) {
        unlink(path);
        return;
    }
    dir = opendir(path);
    if (dir != NULL) {
        while ((ent = readdir(dir)) != NULL) {
            char *child;
            if (strcmp(ent->d_name, ".") == 0 || strcmp(ent->d_name, "..") == 0) continue;
            child = path_join(path, ent->d_name);
            remove_tree(child);
            free(child);
        }
        closedir(dir);
    }
    rmdir(path);
}

static void cleanup_temps(void) {
    int i;
    if (g_in_child || g_keep_temps) return;
    for (i = 0; i < g_tracked_path_count; i++) {
        unlink(g_tracked_paths[i]);
        free(g_tracked_paths[i]);
    }
    g_tracked_path_count = 0;
    for (i = g_tracked_dir_count - 1; i >= 0; i--) {
        remove_tree(g_tracked_dirs[i]);
        free(g_tracked_dirs[i]);
    }
    g_tracked_dir_count = 0;
}

static void cleanup_signal(int sig) {
    int i;
    g_keep_temps = 0;
    g_in_child = 0;
    for (i = 0; i < g_tracked_path_count; i++) unlink(g_tracked_paths[i]);
    for (i = g_tracked_dir_count - 1; i >= 0; i--) remove_tree(g_tracked_dirs[i]);
    _exit(128 + sig);
}

static void install_cleanup(void) {
    struct sigaction sa;
    atexit(cleanup_temps);
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = cleanup_signal;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGINT, &sa, NULL);
    sigaction(SIGTERM, &sa, NULL);
    sigaction(SIGHUP, &sa, NULL);
}

static const char *temp_root(void) {
    const char *t = getenv("TMPDIR");
    if (t != NULL && t[0] != 0) return t;
    return "/tmp";
}

static char *make_temp_dir(const char *prefix) {
    const char *root = temp_root();
    size_t n = strlen(root) + strlen(prefix) + 16;
    char *tpl = (char *)xmalloc(n);
    snprintf(tpl, n, "%s/%sXXXXXX", root, prefix);
    if (mkdtemp(tpl) == NULL) {
        fprintf(stderr, "kaze: cannot create a temporary directory under %s: %s\n",
                root, strerror(errno));
        free(tpl);
        return NULL;
    }
    track_dir(tpl);
    return tpl;
}

static int write_file_raw(const char *path, const void *data, size_t len, int mode) {
    FILE *fp = fopen(path, "wb");
    if (fp == NULL) return -1;
    if (len && fwrite(data, 1, len, fp) != len) {
        fclose(fp);
        return -1;
    }
    if (fclose(fp) != 0) return -1;
    chmod(path, (mode_t)mode);
    return 0;
}

static int write_tracked(const char *path, const void *data, size_t len, int mode) {
    int rc = write_file_raw(path, data, len, mode);
    if (rc == 0) track_path(path);
    return rc;
}

static int read_file(const char *path, Buf *out) {
    FILE *fp = fopen(path, "rb");
    char chunk[8192];
    size_t n;
    if (fp == NULL) return -1;
    while ((n = fread(chunk, 1, sizeof chunk, fp)) > 0) buf_append(out, chunk, n);
    if (ferror(fp)) {
        fclose(fp);
        return -1;
    }
    fclose(fp);
    return 0;
}

static int path_exists(const char *path) {
    struct stat st;
    return stat(path, &st) == 0;
}

static int is_dir(const char *path) {
    struct stat st;
    return stat(path, &st) == 0 && S_ISDIR(st.st_mode);
}

static int is_regular(const char *path) {
    struct stat st;
    return stat(path, &st) == 0 && S_ISREG(st.st_mode);
}

static char *path_join(const char *a, const char *b) {
    if (a == NULL || a[0] == 0) return xstrdup(b);
    size_t na = strlen(a);
    size_t nb = strlen(b);
    int slash = (a[na - 1] != '/');
    char *p = (char *)xmalloc(na + nb + 2);
    memcpy(p, a, na);
    size_t o = na;
    if (slash) p[o++] = '/';
    memcpy(p + o, b, nb);
    p[o + nb] = 0;
    return p;
}

static char *path_dirname(const char *p) {
    const char *slash = strrchr(p, '/');
    if (slash == NULL) return xstrdup(".");
    if (slash == p) return xstrdup("/");
    size_t n = (size_t)(slash - p);
    char *out = (char *)xmalloc(n + 1);
    memcpy(out, p, n);
    out[n] = 0;
    return out;
}

static const char *path_basename(const char *p) {
    const char *slash = strrchr(p, '/');
    return slash ? slash + 1 : p;
}

static char *canonical_path(const char *p) {
    char buf[PATH_MAX];
    if (realpath(p, buf) != NULL) return xstrdup(buf);
    return NULL;
}

static int mkdir_p(const char *path) {
    char tmp[PATH_MAX];
    size_t n;
    size_t i;
    if (path == NULL || path[0] == 0) return -1;
    n = strlen(path);
    if (n >= sizeof tmp) return -1;
    memcpy(tmp, path, n + 1);
    while (n > 1 && tmp[n - 1] == '/') tmp[--n] = 0;
    for (i = 1; i < n; i++) {
        if (tmp[i] == '/') {
            char save = tmp[i];
            tmp[i] = 0;
            if (mkdir(tmp, 0777) != 0 && errno != EEXIST) return -1;
            tmp[i] = save;
        }
    }
    if (mkdir(tmp, 0777) != 0 && errno != EEXIST) return -1;
    return 0;
}

/* ------------------------------------------------------------------ */
/* process helpers                                                     */
/* ------------------------------------------------------------------ */

static int wait_status(pid_t pid) {
    int status = 0;
    while (waitpid(pid, &status, 0) < 0) {
        if (errno != EINTR) return KAZE_EXIT_TRAP;
    }
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return KAZE_EXIT_TRAP;
}

static int run_child(char *const argv[]) {
    pid_t pid;
    fflush(NULL);
    pid = fork();
    if (pid < 0) return KAZE_EXIT_TRAP;
    if (pid == 0) {
        execvp(argv[0], argv);
        fprintf(stderr, "kaze: cannot run %s: %s\n", argv[0], strerror(errno));
        _exit(127);
    }
    return wait_status(pid);
}

/* Read a child's stdout+stderr into out.  Only the verifier-less build
 * uses it, so keep strict compilers quiet when the verifier is embedded. */
#if defined(__GNUC__) || defined(__clang__)
__attribute__((unused))
#endif
static int capture_child(char *const argv[], const char *cwd, Buf *out) {
    int fds[2];
    pid_t pid;
    if (pipe(fds) != 0) return KAZE_EXIT_TRAP;
    fflush(NULL);
    pid = fork();
    if (pid < 0) {
        close(fds[0]);
        close(fds[1]);
        return KAZE_EXIT_TRAP;
    }
    if (pid == 0) {
        if (cwd != NULL && chdir(cwd) != 0) _exit(127);
        close(fds[0]);
        if (dup2(fds[1], STDOUT_FILENO) < 0 || dup2(fds[1], STDERR_FILENO) < 0) _exit(70);
        close(fds[1]);
        execvp(argv[0], argv);
        _exit(127);
    }
    close(fds[1]);
    for (;;) {
        char chunk[8192];
        ssize_t n = read(fds[0], chunk, sizeof chunk);
        if (n < 0) {
            if (errno == EINTR) continue;
            break;
        }
        if (n == 0) break;
        buf_append(out, chunk, (size_t)n);
    }
    close(fds[0]);
    return wait_status(pid);
}

typedef enum { ENTRY_COMPILER = 0, ENTRY_VERIFIER = 1 } EntryKind;

static int capture_entry(EntryKind kind, char *const argv[], Buf *out) {
    int fds[2];
    pid_t pid;
    if (pipe(fds) != 0) return KAZE_EXIT_TRAP;
    fflush(NULL);
    pid = fork();
    if (pid < 0) {
        close(fds[0]);
        close(fds[1]);
        return KAZE_EXIT_TRAP;
    }
    if (pid == 0) {
        int rc;
        g_in_child = 1;
        close(fds[0]);
        if (dup2(fds[1], STDOUT_FILENO) < 0 || dup2(fds[1], STDERR_FILENO) < 0) _exit(70);
        close(fds[1]);
        if (kind == ENTRY_VERIFIER) {
#if KAZE_HAVE_VERIFIER
            int argc = 0;
            while (argv[argc] != NULL) argc++;
            rc = kaze_verify_main(argc, (char **)argv);
#else
            rc = 2;
#endif
        } else {
            int argc = 0;
            while (argv[argc] != NULL) argc++;
            rc = kaze_compiler_main(argc, (char **)argv);
        }
        fflush(NULL);
        _exit(rc);
    }
    close(fds[1]);
    for (;;) {
        char chunk[8192];
        ssize_t n = read(fds[0], chunk, sizeof chunk);
        if (n < 0) {
            if (errno == EINTR) continue;
            break;
        }
        if (n == 0) break;
        buf_append(out, chunk, (size_t)n);
    }
    close(fds[0]);
    return wait_status(pid);
}

static int run_compiler(char *const argv[], Buf *out) {
    return capture_entry(ENTRY_COMPILER, argv, out);
}

static int run_program_capture(const char *path, char *const argv[], const char *cwd, Buf *out) {
    int fds[2];
    pid_t pid;
    if (pipe(fds) != 0) return KAZE_EXIT_TRAP;
    fflush(NULL);
    pid = fork();
    if (pid < 0) {
        close(fds[0]);
        close(fds[1]);
        return KAZE_EXIT_TRAP;
    }
    if (pid == 0) {
        if (cwd != NULL && chdir(cwd) != 0) _exit(127);
        close(fds[0]);
        if (dup2(fds[1], STDOUT_FILENO) < 0 || dup2(fds[1], STDERR_FILENO) < 0) _exit(70);
        close(fds[1]);
        execv(path, argv);
        _exit(127);
    }
    close(fds[1]);
    for (;;) {
        char chunk[8192];
        ssize_t n = read(fds[0], chunk, sizeof chunk);
        if (n < 0) {
            if (errno == EINTR) continue;
            break;
        }
        if (n == 0) break;
        buf_append(out, chunk, (size_t)n);
    }
    close(fds[0]);
    return wait_status(pid);
}

/* ------------------------------------------------------------------ */
/* installation layout                                                 */
/* ------------------------------------------------------------------ */

static char *g_self_path = NULL;
static char *g_exe_dir = NULL;
static char *g_prefix_dir = NULL;

static char *find_self_path(const char *argv0) {
#if defined(__APPLE__)
    char small[PATH_MAX];
    uint32_t size = (uint32_t)sizeof small;
    if (_NSGetExecutablePath(small, &size) == 0) {
        char *r = canonical_path(small);
        if (r != NULL) return r;
    }
#elif defined(__linux__)
    char buf[PATH_MAX];
    ssize_t n = readlink("/proc/self/exe", buf, sizeof buf - 1);
    if (n > 0) {
        buf[n] = 0;
        return xstrdup(buf);
    }
#endif
    if (argv0 != NULL && strchr(argv0, '/') != NULL) {
        char *r = canonical_path(argv0);
        if (r != NULL) return r;
    }
    {
        const char *path = getenv("PATH");
        if (path != NULL && argv0 != NULL) {
            const char *p = path;
            while (*p) {
                const char *colon = strchr(p, ':');
                size_t n = colon ? (size_t)(colon - p) : strlen(p);
                if (n > 0) {
                    char *cand = (char *)xmalloc(n + 1 + strlen(argv0) + 1);
                    memcpy(cand, p, n);
                    cand[n] = '/';
                    strcpy(cand + n + 1, argv0);
                    if (access(cand, X_OK) == 0) {
                        char *r = canonical_path(cand);
                        free(cand);
                        if (r != NULL) return r;
                    }
                    free(cand);
                }
                if (!colon) break;
                p = colon + 1;
            }
        }
    }
    return xstrdup(argv0 ? argv0 : "kaze");
}

static void init_paths(const char *argv0) {
    g_self_path = find_self_path(argv0);
    g_exe_dir = path_dirname(g_self_path);
    g_prefix_dir = path_dirname(g_exe_dir);
}

/* The stdlib directory.  Search order is explicit, then installed, then a
 * source checkout (bin/kaze -> ../lib). */
static char *find_std_dir(void) {
    const char *env = getenv("KAZE_STD_DIR");
    char *cand;
    if (env != NULL && env[0] != 0 && is_dir(env)) return xstrdup(env);
    cand = path_join(g_prefix_dir, "share/kaze/std");
    if (is_dir(cand)) return cand;
    free(cand);
    cand = path_join(g_exe_dir, "../share/kaze/std");
    if (is_dir(cand)) return cand;
    free(cand);
    cand = path_join(g_exe_dir, "../lib");
    if (is_dir(cand)) return cand;
    free(cand);
    cand = path_join(g_prefix_dir, "lib");
    if (is_dir(cand)) return cand;
    free(cand);
    return NULL;
}

/* The directory holding libkaze_*.a, when the tree was installed. */
static char *find_lib_dir(void) {
    const char *env = getenv("KAZE_LIB_DIR");
    char *cand;
    if (env != NULL && env[0] != 0) return xstrdup(env);
    cand = path_join(g_prefix_dir, "lib/kaze");
    if (is_dir(cand)) return cand;
    free(cand);
    cand = path_join(g_exe_dir, "../lib/kaze");
    if (is_dir(cand)) return cand;
    free(cand);
    cand = path_join(g_exe_dir, "../dist/lib");
    if (is_dir(cand)) return cand;
    free(cand);
    return NULL;
}

/* ------------------------------------------------------------------ */
/* embedded runtime                                                    */
/* ------------------------------------------------------------------ */

static const struct kaze_embedded_blob *embedded_find(const char *name) {
    int i;
    for (i = 0; i < KAZE_EMBEDDED_BLOB_COUNT; i++) {
        if (strcmp(kaze_embedded_blobs[i].name, name) == 0) return &kaze_embedded_blobs[i];
    }
    return NULL;
}

static int embedded_write(const char *dir, const char *name) {
    const struct kaze_embedded_blob *b = embedded_find(name);
    char *path;
    int rc;
    if (b == NULL) return -1;
    path = path_join(dir, name);
    rc = write_tracked(path, b->data, (size_t)b->size, 0644);
    free(path);
    return rc;
}

static int embedded_write_suffix(const char *dir, const char *suffix) {
    int i;
    for (i = 0; i < KAZE_EMBEDDED_BLOB_COUNT; i++) {
        const char *name = kaze_embedded_blobs[i].name;
        size_t nl = strlen(name);
        size_t sl = strlen(suffix);
        if (nl >= sl && strcmp(name + nl - sl, suffix) == 0) {
            if (embedded_write(dir, name) != 0) return -1;
        }
    }
    return 0;
}

/* ------------------------------------------------------------------ */
/* multi-file module loader                                            */
/* ------------------------------------------------------------------ */

typedef struct {
    char *file;
    int line;
} LineOrigin;

typedef struct {
    Buf text;
    LineOrigin *map;
    size_t map_len;
    size_t map_cap;
} BundlePart;

typedef struct {
    BundlePart *parts;
    size_t parts_len;
    size_t parts_cap;
    StrVec needs;
    StrVec seen;
    StrVec stack;
    int used_imports;
    int failed;
    char error[2048];
    const char *std_dir;
} Bundler;

typedef struct {
    Buf text;
    LineOrigin *map;
    size_t map_len;
    char *path;
    int bundled;
} BundledSource;

static void bundle_part_init(BundlePart *p) {
    buf_init(&p->text);
    p->map = NULL;
    p->map_len = 0;
    p->map_cap = 0;
}

static void bundle_part_free(BundlePart *p) {
    buf_free(&p->text);
    free(p->map);
    p->map = NULL;
    p->map_len = 0;
    p->map_cap = 0;
}

static void bundle_part_line(BundlePart *p, const char *file, int line, const char *text, size_t len) {
    if (p->map_len + 1 > p->map_cap) {
        size_t cap = p->map_cap ? p->map_cap * 2 : 64;
        p->map = (LineOrigin *)xrealloc(p->map, cap * sizeof(LineOrigin));
        p->map_cap = cap;
    }
    p->map[p->map_len].file = xstrdup(file);
    p->map[p->map_len].line = line;
    p->map_len++;
    buf_append(&p->text, text, len);
}

static void bundle_error(Bundler *b, const char *fmt, ...) {
    va_list ap;
    if (b->failed) return;
    b->failed = 1;
    va_start(ap, fmt);
    vsnprintf(b->error, sizeof b->error, fmt, ap);
    va_end(ap);
}

static int bundle_add_part(Bundler *b, BundlePart *p) {
    if (b->parts_len + 1 > b->parts_cap) {
        size_t cap = b->parts_cap ? b->parts_cap * 2 : 4;
        b->parts = (BundlePart *)xrealloc(b->parts, cap * sizeof(BundlePart));
        b->parts_cap = cap;
    }
    b->parts[b->parts_len++] = *p;
    bundle_part_init(p);
    return 0;
}

/* Strip a trailing CR and surrounding whitespace for header inspection. */
static void trim_span(const char *s, size_t n, size_t *a, size_t *b) {
    size_t x = 0;
    size_t y = n;
    while (x < y && (s[x] == ' ' || s[x] == '\t')) x++;
    while (y > x && (s[y - 1] == ' ' || s[y - 1] == '\t' || s[y - 1] == '\r')) y--;
    *a = x;
    *b = y;
}

static int parse_use_line(const char *line, size_t n, char **target) {
    size_t a, z, i;
    trim_span(line, n, &a, &z);
    if (z < a + 4) return 0;
    if (strncmp(line + a, "use", 3) != 0) return 0;
    if (!(line[a + 3] == ' ' || line[a + 3] == '\t')) return 0;
    i = a + 3;
    while (i < z && (line[i] == ' ' || line[i] == '\t')) i++;
    if (i >= z || line[i] != '"') return 0;
    i++;
    size_t start = i;
    while (i < z && line[i] != '"') i++;
    if (i >= z) return 0;
    *target = (char *)xmalloc(i - start + 1);
    memcpy(*target, line + start, i - start);
    (*target)[i - start] = 0;
    return 1;
}

/* Parse "needs: a, b" and append names.  Returns 1 when the line is a needs
 * declaration, even when it names nothing. */
static int parse_needs_line(const char *line, size_t n, StrVec *needs) {
    size_t a, z, i;
    trim_span(line, n, &a, &z);
    if (z < a + 6) return 0;
    if (strncmp(line + a, "needs:", 6) != 0) return 0;
    i = a + 6;
    while (i < z) {
        while (i < z && (line[i] == ' ' || line[i] == '\t' || line[i] == ',')) i++;
        size_t start = i;
        while (i < z && line[i] != ',' && line[i] != '#' && line[i] != ' ' && line[i] != '\t') i++;
        if (i > start) {
            char *name = (char *)xmalloc(i - start + 1);
            memcpy(name, line + start, i - start);
            name[i - start] = 0;
            if (!vec_has(needs, name)) vec_push_owned(needs, name);
            else free(name);
        }
        if (i < z && line[i] == '#') break;
    }
    return 1;
}

static int has_kaze_suffix(const char *p) {
    size_t n = strlen(p);
    return n >= 5 && strcmp(p + n - 5, ".kaze") == 0;
}

static int try_use_candidate(const char *importer_dir, const char *cwd, const char *std_dir,
                             const char *target, char **out) {
    char *cand = NULL;
    if (target[0] == '/') {
        cand = xstrdup(target);
    } else if (strncmp(target, "std/", 4) == 0 && std_dir != NULL) {
        cand = path_join(std_dir, target + 4);
    } else if (strncmp(target, "std/", 4) == 0) {
        return 0;
    } else {
        cand = path_join(importer_dir, target);
    }
    if (cand != NULL) {
        if (is_regular(cand)) {
            *out = cand;
            return 1;
        }
        if (!has_kaze_suffix(cand)) {
            char *with_ext = xasprintf("%s.kaze", cand);
            if (is_regular(with_ext)) {
                free(cand);
                *out = with_ext;
                return 1;
            }
            free(with_ext);
        }
        free(cand);
    }
    if (target[0] != '/' && strncmp(target, "std/", 4) != 0) {
        cand = path_join(cwd, target);
        if (is_regular(cand)) {
            *out = cand;
            return 1;
        }
        if (!has_kaze_suffix(cand)) {
            char *with_ext = xasprintf("%s.kaze", cand);
            if (is_regular(with_ext)) {
                free(cand);
                *out = with_ext;
                return 1;
            }
            free(with_ext);
        }
        free(cand);
        if (std_dir != NULL) {
            cand = path_join(std_dir, target);
            if (is_regular(cand)) {
                *out = cand;
                return 1;
            }
            if (!has_kaze_suffix(cand)) {
                char *with_ext = xasprintf("%s.kaze", cand);
                if (is_regular(with_ext)) {
                    free(cand);
                    *out = with_ext;
                    return 1;
                }
                free(with_ext);
            }
            free(cand);
        }
    }
    return 0;
}

static int bundle_collect_file(Bundler *b, const char *path, const char *cwd, int depth);

static int bundle_collect_entry(Bundler *b, const char *path, const char *cwd) {
    return bundle_collect_file(b, path, cwd, 0);
}

static int bundle_collect_file(Bundler *b, const char *path, const char *cwd, int depth) {
    char *real = canonical_path(path);
    char *dir;
    Buf src;
    size_t pos = 0;
    int lineno = 0;
    BundlePart part;
    if (real == NULL) {
        bundle_error(b, "E_KAZE_IO: cannot read '%s': %s", path, strerror(errno));
        return -1;
    }
    if (depth > 128) {
        bundle_error(b, "E_KAZE_IMPORT: import nesting is deeper than 128 files at '%s'", real);
        free(real);
        return -1;
    }
    if (vec_has(&b->stack, real)) {
        Buf chain;
        size_t i;
        buf_init(&chain);
        for (i = 0; i < b->stack.len; i++) {
            buf_appends(&chain, b->stack.items[i]);
            buf_appends(&chain, " -> ");
        }
        buf_appends(&chain, real);
        bundle_error(b, "E_KAZE_IMPORT: import cycle: %s", chain.data ? chain.data : real);
        buf_free(&chain);
        free(real);
        return -1;
    }
    if (vec_has(&b->seen, real)) {
        free(real);
        return 0;
    }
    vec_push(&b->seen, real);
    vec_push(&b->stack, real);
    dir = path_dirname(real);
    buf_init(&src);
    if (read_file(real, &src) != 0) {
        bundle_error(b, "E_KAZE_IO: cannot read '%s': %s", real, strerror(errno));
        buf_free(&src);
        free(dir);
        free(real);
        return -1;
    }
    bundle_part_init(&part);
    while (pos < src.len) {
        size_t start = pos;
        size_t end;
        char *target = NULL;
        while (pos < src.len && src.data[pos] != '\n') pos++;
        end = pos;
        if (pos < src.len) pos++;
        lineno++;
        if (parse_use_line(src.data + start, end - start, &target)) {
            char *resolved = NULL;
            if (!try_use_candidate(dir, cwd, b->std_dir, target, &resolved)) {
                bundle_error(b, "E_KAZE_IO: %s:%d: cannot resolve '%s'", real, lineno, target);
                free(target);
                break;
            }
            b->used_imports = 1;
            free(target);
            if (bundle_collect_file(b, resolved, cwd, depth + 1) != 0) {
                free(resolved);
                break;
            }
            free(resolved);
            continue;
        }
        if (parse_needs_line(src.data + start, end - start, &b->needs)) {
            continue;
        }
        bundle_part_line(&part, real, lineno, src.data + start, end - start);
        buf_appendc(&part.text, '\n');
    }
    bundle_add_part(b, &part);
    /* pop the stack */
    if (b->stack.len > 0) {
        free(b->stack.items[b->stack.len - 1]);
        b->stack.len--;
    }
    free(dir);
    buf_free(&src);
    free(real);
    return b->failed ? -1 : 0;
}

static void bundled_source_init(BundledSource *bs) {
    buf_init(&bs->text);
    bs->map = NULL;
    bs->map_len = 0;
    bs->path = NULL;
    bs->bundled = 0;
}

static void bundled_source_free(BundledSource *bs) {
    buf_free(&bs->text);
    free(bs->map);
    bs->map = NULL;
    bs->map_len = 0;
    free(bs->path);
    bs->path = NULL;
    bs->bundled = 0;
}

static void bundled_add_line(BundledSource *bs, const char *file, int line,
                             const char *text, size_t len) {
    bs->map = (LineOrigin *)xrealloc(bs->map, (bs->map_len + 1) * sizeof(LineOrigin));
    bs->map[bs->map_len].file = xstrdup(file);
    bs->map[bs->map_len].line = line;
    bs->map_len++;
    buf_append(&bs->text, text, len);
}

/* Build a single source file for a program that uses modules.  When the file
 * has no `use` line, *bundled is 0 and the caller passes the original path to
 * the compiler (preserving line numbers exactly). */
static int bundle_source(const char *path, BundledSource *out, const char *std_dir, char *err, size_t errcap) {
    Bundler b;
    Buf src;
    size_t pos = 0;
    int lineno = 0;
    int has_use = 0;
    BundledSource result;
    bundled_source_init(&result);
    memset(&b, 0, sizeof b);
    vec_init(&b.needs);
    vec_init(&b.seen);
    vec_init(&b.stack);
    b.std_dir = std_dir;
    buf_init(&src);
    if (read_file(path, &src) != 0) {
        snprintf(err, errcap, "cannot read '%s': %s", path, strerror(errno));
        buf_free(&src);
        return -1;
    }
    /* A quick scan decides whether the loader is needed at all. */
    while (pos < src.len) {
        size_t start = pos;
        size_t end;
        char *target = NULL;
        while (pos < src.len && src.data[pos] != '\n') pos++;
        end = pos;
        if (pos < src.len) pos++;
        lineno++;
        if (parse_use_line(src.data + start, end - start, &target)) {
            has_use = 1;
            free(target);
            break;
        }
        (void)lineno;
    }
    buf_free(&src);
    if (!has_use) {
        out->path = xstrdup(path);
        out->bundled = 0;
        return 0;
    }
    {
        char *cwd = getcwd(NULL, 0);
        if (cwd == NULL) cwd = xstrdup(".");
        if (bundle_collect_entry(&b, path, cwd) != 0) {
            snprintf(err, errcap, "%s", b.error[0] ? b.error : "module loader failed");
            free(cwd);
            vec_free(&b.needs);
            vec_free(&b.seen);
            vec_free(&b.stack);
            return -1;
        }
        free(cwd);
    }
    if (b.failed) {
        snprintf(err, errcap, "%s", b.error[0] ? b.error : "module loader failed");
        vec_free(&b.needs);
        vec_free(&b.seen);
        vec_free(&b.stack);
        return -1;
    }
    /* needs first, mapped to the root file */
    if (b.needs.len > 0) {
        Buf line;
        size_t i;
        buf_init(&line);
        buf_appends(&line, "needs: ");
        for (i = 0; i < b.needs.len; i++) {
            if (i > 0) buf_appends(&line, ", ");
            buf_appends(&line, b.needs.items[i]);
        }
        buf_appendc(&line, '\n');
        bundled_add_line(&result, path, 1, line.data, line.len);
        buf_free(&line);
        bundled_add_line(&result, path, 1, "\n", 1);
    }
    {
        size_t i;
        for (i = 0; i < b.parts_len; i++) {
            size_t j;
            BundlePart *p = &b.parts[i];
            size_t line_start = 0;
            size_t map_i = 0;
            buf_append(&result.text, p->text.data ? p->text.data : "", p->text.len);
            for (j = 0; j < p->map_len; j++) {
                result.map = (LineOrigin *)xrealloc(result.map, (result.map_len + 1) * sizeof(LineOrigin));
                result.map[result.map_len].file = xstrdup(p->map[j].file);
                result.map[result.map_len].line = p->map[j].line;
                result.map_len++;
            }
            (void)line_start;
            (void)map_i;
            if (p->text.len == 0 || p->text.data[p->text.len - 1] != '\n') {
                buf_appendc(&result.text, '\n');
                result.map = (LineOrigin *)xrealloc(result.map, (result.map_len + 1) * sizeof(LineOrigin));
                result.map[result.map_len].file = xstrdup(path);
                result.map[result.map_len].line = 1;
                result.map_len++;
            }
            buf_appendc(&result.text, '\n');
            result.map = (LineOrigin *)xrealloc(result.map, (result.map_len + 1) * sizeof(LineOrigin));
            result.map[result.map_len].file = xstrdup(path);
            result.map[result.map_len].line = 1;
            result.map_len++;
        }
    }
    result.bundled = 1;
    *out = result;
    for (size_t i = 0; i < b.parts_len; i++) bundle_part_free(&b.parts[i]);
    free(b.parts);
    vec_free(&b.needs);
    vec_free(&b.seen);
    vec_free(&b.stack);
    return 0;
}

/* Write the bundled source to a tracked temporary file and return it. */
static char *materialize_bundle(BundledSource *bs, const char *dir) {
    char *path;
    if (bs->bundled == 0) return xstrdup(bs->path);
    path = xasprintf("%s/bundle-%ld.kaze", dir, (long)getpid());
    if (write_tracked(path, bs->text.data ? bs->text.data : "", bs->text.len, 0644) != 0) {
        free(path);
        return NULL;
    }
    return path;
}

/* ------------------------------------------------------------------ */
/* diagnostics                                                         */
/* ------------------------------------------------------------------ */

typedef struct {
    int line;
    char code[96];
    char *message;
    char *file;
} Diag;

typedef struct {
    Diag *items;
    size_t len;
    size_t cap;
} DiagVec;

static void diagvec_free(DiagVec *d) {
    size_t i;
    for (i = 0; i < d->len; i++) {
        free(d->items[i].message);
        free(d->items[i].file);
    }
    free(d->items);
    d->items = NULL;
    d->len = 0;
    d->cap = 0;
}

static void diagvec_push(DiagVec *d, int line, const char *code, const char *msg) {
    if (d->len + 1 > d->cap) {
        size_t cap = d->cap ? d->cap * 2 : 8;
        d->items = (Diag *)xrealloc(d->items, cap * sizeof(Diag));
        d->cap = cap;
    }
    d->items[d->len].line = line;
    snprintf(d->items[d->len].code, sizeof d->items[d->len].code, "%s", code);
    d->items[d->len].message = xstrdup(msg);
    d->items[d->len].file = NULL;
    d->len++;
}

static int has_diagnostics(const char *text) {
    return text != NULL && strstr(text, "E_KAZE_") != NULL;
}

/* Parse the compiler's two stable one-line diagnostic forms:
 *     E_KAZE_... at line N: message          (the legacy one-file spelling)
 *     path.kaze:N: E_KAZE_...: message       (the module loader's spelling)
 * Keeping both in one parser is what lets `check --json` report a diagnostic
 * on an imported module without the driver knowing anything about modules.
 */
static void parse_diagnostics(const char *text, DiagVec *out) {
    const char *p = text;
    while (p != NULL && *p != 0) {
        const char *end = strchr(p, '\n');
        size_t n = end ? (size_t)(end - p) : strlen(p);
        const char *s = p;
        const char *line_at = NULL;
        const char *code_at = NULL;
        char *file = NULL;
        char code[96];
        int line = 0;
        const char *msg = NULL;

        if (n > 2 && (s[0] == 'E' || s[0] == 'W') && s[1] == '_') {
            line_at = strstr(s, " at line ");
            if (line_at != NULL && line_at < p + n) {
                size_t cn = (size_t)(line_at - s);
                if (cn < sizeof code) {
                    memcpy(code, s, cn);
                    code[cn] = 0;
                    code_at = s;
                    line = atoi(line_at + 9);
                    msg = strchr(line_at + 9, ':');
                    if (msg != NULL) {
                        msg++;
                        while (*msg == ':' || *msg == ' ') msg++;
                    } else {
                        msg = line_at + 9;
                    }
                }
            }
        } else {
            /* Locate the `: E_CODE:` / `: W_CODE:` marker in the line. */
            const char *q;
            const char *marker = NULL;
            for (q = s; q + 4 < p + n; q++) {
                if (q[0] == ':' && q[1] == ' ' && (q[2] == 'E' || q[2] == 'W') && q[3] == '_') {
                    marker = q;
                    code_at = q + 2;
                    break;
                }
            }
            if (code_at != NULL) {
                const char *prev = NULL;
                const char *code_end = code_at;
                size_t cn;
                for (q = s; q < marker; q++) {
                    if (*q == ':') prev = q;
                }
                if (prev != NULL) {
                    line = atoi(prev + 1);
                    file = (char *)xmalloc((size_t)(prev - s) + 1);
                    memcpy(file, s, (size_t)(prev - s));
                    file[prev - s] = 0;
                }
                while (code_end < p + n && *code_end != ':' && *code_end != ' ') code_end++;
                cn = (size_t)(code_end - code_at);
                if (cn < sizeof code) {
                    memcpy(code, code_at, cn);
                    code[cn] = 0;
                    msg = code_end;
                    if (*msg == ':') msg++;
                    while (*msg == ':' || *msg == ' ') msg++;
                } else {
                    code_at = NULL;
                }
            }
        }

        if (code_at != NULL && msg != NULL) {
            size_t mn = (size_t)(p + n - msg);
            char *copy = (char *)xmalloc(mn + 1);
            memcpy(copy, msg, mn);
            copy[mn] = 0;
            diagvec_push(out, line, code, copy);
            free(copy);
            if (file != NULL) {
                out->items[out->len - 1].file = file;
                file = NULL;
            }
        }
        free(file);
        if (end == NULL) break;
        p = end + 1;
    }
}

static void json_escape_fp(FILE *fp, const char *s) {
    const unsigned char *p = (const unsigned char *)(s ? s : "");
    fputc('"', fp);
    for (; *p != 0; p++) {
        unsigned char c = *p;
        switch (c) {
            case '"': fputs("\\\"", fp); break;
            case '\\': fputs("\\\\", fp); break;
            case '\n': fputs("\\n", fp); break;
            case '\r': fputs("\\r", fp); break;
            case '\t': fputs("\\t", fp); break;
            default:
                if (c < 0x20) fprintf(fp, "\\u%04x", c);
                else fputc(c, fp);
                break;
        }
    }
    fputc('"', fp);
}

/* ------------------------------------------------------------------ */
/* compiler invocations                                                */
/* ------------------------------------------------------------------ */

static int compiler_emit_c(const char *source, Buf *out) {
    char *argv[5];
    int rc;
    argv[0] = (char *)"kaze";
    argv[1] = (char *)"emit-c";
    argv[2] = (char *)source;
    argv[3] = NULL;
    buf_init(out);
    rc = run_compiler(argv, out);
    return rc;
}

static int compiler_check(const char *source, Buf *out) {
    char *argv[5];
    int rc;
    argv[0] = (char *)"kaze";
    argv[1] = (char *)"check";
    argv[2] = (char *)source;
    argv[3] = NULL;
    buf_init(out);
    rc = run_compiler(argv, out);
    return rc;
}

/* ------------------------------------------------------------------ */
/* feature scan and build pipeline                                     */
/* ------------------------------------------------------------------ */

typedef struct {
    int tensor;
    int http;
    int parallel;
    int sitl;
    int metal;
    int blas;
} Features;

static void detect_features(const char *c, Features *f) {
    memset(f, 0, sizeof *f);
    if (c == NULL) return;
    if (strstr(c, "kaze_tensor.h") != NULL) f->tensor = 1;
    if (strstr(c, "kaze_http.h") != NULL) f->http = 1;
    if (strstr(c, "Multicore Parallel Runtime") != NULL ||
        strstr(c, "kz_parallel_map") != NULL ||
        strstr(c, "kz_num_workers") != NULL) f->parallel = 1;
    if (strstr(c, "kaze_metal.h") != NULL) f->metal = 1;
    if (strstr(c, "kaze_blas.h") != NULL) f->blas = 1;
}

typedef struct {
    int release;
    int debug;
    int asan;
    int strict;
    int sitl;
    int metal;
    int blas;
    const char *backend;
    const char *target;
    const char *gpu;
    const char *cc;
    const char *output;
} BuildOpts;

static void build_opts_init(BuildOpts *o) {
    memset(o, 0, sizeof *o);
    o->backend = "c99";
    o->target = "native";
    o->gpu = NULL;
    o->cc = NULL;
}

static const char *compiler_cc(const BuildOpts *o) {
    const char *cc = o->cc;
    if (cc != NULL && cc[0] != 0) return cc;
    cc = getenv("KAZE_CC");
    if (cc != NULL && cc[0] != 0) return cc;
    return "cc";
}

static char *default_output_name(const char *source) {
    const char *base = path_basename(source);
    size_t n = strlen(base);
    if (n > 5 && strcmp(base + n - 5, ".kaze") == 0) n -= 5;
    if (n == 0) n = 1;
    char *out = (char *)xmalloc(n + 1);
    memcpy(out, base, n);
    out[n] = 0;
    if (strcmp(out, ".") == 0 || strcmp(out, "..") == 0) {
        free(out);
        out = xstrdup("a.out");
    }
    return out;
}

static void argv_push(StrVec *v, const char *s) {
    vec_push(v, s);
}

static void argv_push_owned(StrVec *v, char *s) {
    vec_push_owned(v, s);
}

static int archives_ready(const char *libdir, Features *f, int want_sitl) {
    char *rt;
    char *tensor;
    char *sitl;
    int ok = 1;
    if (libdir == NULL) return 0;
    rt = path_join(libdir, "libkaze_rt.a");
    if (!is_regular(rt)) ok = 0;
    free(rt);
    if (f->tensor) {
        tensor = path_join(libdir, "libkaze_tensor.a");
        if (!is_regular(tensor)) ok = 0;
        free(tensor);
    }
    if (want_sitl) {
        sitl = path_join(libdir, "libkaze_sitl.a");
        if (!is_regular(sitl)) ok = 0;
        free(sitl);
    }
    if (f->blas) {
        char *blas = path_join(libdir, "libkaze_blas.a");
        if (!is_regular(blas)) ok = 0;
        free(blas);
    }
    return ok;
}

/* Write every runtime source named by the features into src_dir. */
static int materialize_sources(const char *src_dir, Features *f, int want_sitl) {
    static const char *rt_src[] = { "kaze_rt.c", NULL };
    static const char *tensor_src[] = {
        "kaze_tensor.c", "kaze_quant.c", "kaze_gguf.c", "kaze_llama.c",
        "kaze_tokenizer.c", "kaze_autograd.c", NULL
    };
    static const char *sitl_src[] = { "kaze_mavlink.c", NULL };
    static const char *blas_src[] = { "kaze_blas.c", NULL };
    const char **lists[4];
    int nlists = 0;
    int i, j;
    lists[nlists++] = rt_src;
    if (f->tensor) lists[nlists++] = tensor_src;
    if (want_sitl) lists[nlists++] = sitl_src;
    if (f->blas) lists[nlists++] = blas_src;
    for (i = 0; i < nlists; i++) {
        for (j = 0; lists[i][j] != NULL; j++) {
            if (embedded_write(src_dir, lists[i][j]) != 0) return -1;
        }
    }
    return 0;
}

static void print_argv(const StrVec *v) {
    size_t i;
    fputs("kaze: command:", stderr);
    for (i = 0; i < v->len; i++) fprintf(stderr, " %s", v->items[i]);
    fputc('\n', stderr);
}

/* Compile emitted C into an executable.  Returns 0 on success. */
static int link_program(const char *program_c, const char *tmpdir, const BuildOpts *opts,
                        Features *features, const char *output) {
    char *inc_dir = path_join(tmpdir, "include");
    char *src_dir = path_join(tmpdir, "src");
    char *libdir = find_lib_dir();
    int use_archives;
    int want_sitl = opts->sitl || getenv("KAZE_SITL") != NULL;
    const char *cc = compiler_cc(opts);
    StrVec args;
    int rc;
    int i;

    if (mkdir_p(inc_dir) != 0) {
        fprintf(stderr, "kaze: cannot create %s\n", inc_dir);
        free(inc_dir);
        free(src_dir);
        free(libdir);
        return KAZE_EXIT_USAGE;
    }
    if (embedded_write_suffix(inc_dir, ".h") != 0) {
        fprintf(stderr, "kaze: cannot materialise the embedded runtime headers\n");
        free(inc_dir);
        free(src_dir);
        free(libdir);
        return KAZE_EXIT_USAGE;
    }
    if (features->metal) {
        if (embedded_write(inc_dir, "kaze_metal.h") != 0 ||
            embedded_write(tmpdir, "kaze_metal.m") != 0) {
            fprintf(stderr, "kaze: cannot materialise the Metal runtime\n");
            free(inc_dir);
            free(src_dir);
            free(libdir);
            return KAZE_EXIT_USAGE;
        }
    }
    /* The emitted program textually includes the SITL host when
     * -DKAZE_SITL_LINK is set; keep the fragment beside the headers. */
    if (want_sitl && embedded_write(inc_dir, "kaze_sitl_client.c") != 0) {
        fprintf(stderr, "kaze: cannot materialise the SITL client\n");
        free(inc_dir);
        free(src_dir);
        free(libdir);
        return KAZE_EXIT_USAGE;
    }
    use_archives = archives_ready(libdir, features, want_sitl);
    if (!use_archives) {
        if (mkdir_p(src_dir) != 0 || materialize_sources(src_dir, features, want_sitl) != 0) {
            fprintf(stderr, "kaze: cannot materialise the embedded runtime sources\n");
            free(inc_dir);
            free(src_dir);
            free(libdir);
            return KAZE_EXIT_USAGE;
        }
    }

    vec_init(&args);
    argv_push(&args, cc);
    argv_push(&args, "-std=c99");
    if (opts->asan) {
        argv_push(&args, "-O1");
        argv_push(&args, "-g");
        argv_push(&args, "-fsanitize=address,undefined");
        argv_push(&args, "-fno-omit-frame-pointer");
    } else if (opts->debug) {
        argv_push(&args, "-O0");
        argv_push(&args, "-g");
    } else if (opts->release) {
        argv_push(&args, "-O3");
    } else {
        argv_push(&args, "-O2");
    }
    argv_push(&args, opts->strict ? "-Wall" : "-w");
    if (opts->strict) {
        argv_push(&args, "-Wextra");
        argv_push(&args, "-Werror");
        argv_push(&args, "-pedantic");
    }
    {
        char *inc_flag = xasprintf("-I%s", inc_dir);
        argv_push_owned(&args, inc_flag);
    }
    if (opts->debug || opts->asan) argv_push(&args, "-g");
    if (want_sitl) argv_push(&args, "-DKAZE_SITL_LINK");
    if (features->metal || opts->metal) argv_push(&args, "-DKAZE_GPU_METAL");
    argv_push(&args, program_c);
    if (use_archives) {
        char *lflag = xasprintf("-L%s", libdir);
        argv_push_owned(&args, lflag);
        if (features->tensor) argv_push(&args, "-lkaze_tensor");
        if (want_sitl) argv_push(&args, "-lkaze_sitl");
        argv_push(&args, "-lkaze_rt");
    } else {
        static const char *rt_src[] = { "kaze_rt.c", NULL };
        static const char *tensor_src[] = {
            "kaze_tensor.c", "kaze_quant.c", "kaze_gguf.c", "kaze_llama.c",
            "kaze_tokenizer.c", "kaze_autograd.c", NULL
        };
        if (features->tensor) {
            for (i = 0; tensor_src[i] != NULL; i++) {
                char *p = path_join(src_dir, tensor_src[i]);
                argv_push_owned(&args, p);
            }
        }
        if (want_sitl) {
            char *p = path_join(src_dir, "kaze_mavlink.c");
            argv_push_owned(&args, p);
        }
        if (features->blas) {
            char *p = path_join(src_dir, "kaze_blas.c");
            argv_push_owned(&args, p);
        }
        for (i = 0; rt_src[i] != NULL; i++) {
            char *p = path_join(src_dir, rt_src[i]);
            argv_push_owned(&args, p);
        }
    }
    argv_push(&args, "-lm");
    if (features->http) argv_push(&args, "-lcurl");
    if (features->parallel || features->tensor) argv_push(&args, "-pthread");
    if (features->metal || opts->metal) {
        argv_push(&args, "-framework");
        argv_push(&args, "Metal");
        argv_push(&args, "-framework");
        argv_push(&args, "Foundation");
    }
    if (features->blas && use_archives) argv_push(&args, "-lkaze_blas");
    if (features->blas) {
#if defined(__APPLE__)
        argv_push(&args, "-framework");
        argv_push(&args, "Accelerate");
#endif
    }
    argv_push(&args, "-o");
    argv_push(&args, output);
    argv_push(&args, NULL);

    if (g_verbose) print_argv(&args);
    rc = run_child(args.items);
    if (rc != 0 && use_archives) {
        /* A stale archive must not stop a source checkout from building. */
        fprintf(stderr, "kaze: prebuilt runtime archive did not link; retrying from embedded sources\n");
        vec_free(&args);
        if (mkdir_p(src_dir) != 0 || materialize_sources(src_dir, features, want_sitl) != 0) {
            fprintf(stderr, "kaze: cannot materialise the embedded runtime sources\n");
            free(inc_dir); free(src_dir); free(libdir);
            return KAZE_EXIT_USAGE;
        }
        vec_init(&args);
        argv_push(&args, cc);
        argv_push(&args, "-std=c99");
        argv_push(&args, "-O2");
        argv_push(&args, "-w");
        {
            char *inc_flag = xasprintf("-I%s", inc_dir);
            argv_push_owned(&args, inc_flag);
        }
        if (want_sitl) argv_push(&args, "-DKAZE_SITL_LINK");
        argv_push(&args, program_c);
        if (features->tensor) {
            static const char *tensor_src[] = {
                "kaze_tensor.c", "kaze_quant.c", "kaze_gguf.c", "kaze_llama.c",
                "kaze_tokenizer.c", "kaze_autograd.c", NULL
            };
            for (i = 0; tensor_src[i] != NULL; i++) {
                char *p = path_join(src_dir, tensor_src[i]);
                argv_push_owned(&args, p);
            }
        }
        {
            char *p = path_join(src_dir, "kaze_rt.c");
            argv_push_owned(&args, p);
        }
        argv_push(&args, "-lm");
        if (features->http) argv_push(&args, "-lcurl");
        if (features->parallel || features->tensor) argv_push(&args, "-pthread");
        if (features->blas) {
#if defined(__APPLE__)
            argv_push(&args, "-framework");
            argv_push(&args, "Accelerate");
#endif
        }
        argv_push(&args, "-o");
        argv_push(&args, output);
        argv_push(&args, NULL);
        if (g_verbose) print_argv(&args);
        rc = run_child(args.items);
    }
    vec_free(&args);
    free(inc_dir);
    free(src_dir);
    free(libdir);
    if (rc == 127) {
        fprintf(stderr, "kaze: cannot run the C compiler '%s'.\n", cc);
        fprintf(stderr, "  install a C99 compiler or set KAZE_CC=/path/to/cc\n");
        return KAZE_EXIT_USAGE;
    }
    return rc;
}

/* Build source -> output.  Returns 0, 1 (diagnostic) or 2 (setup). */
static int build_source(const char *source, const BuildOpts *opts, const char *tmpdir,
                        const char *output) {
    Buf c;
    char *program_c = NULL;
    Features features;
    int check_rc;
    int rc;

    /* The checker is part of compilation: refuse a program with a hard
     * diagnostic rather than compiling whatever the parser recovered. The
     * compiler resolves imports itself, so the entry path goes in unchanged. */
    {
        Buf diag;
        buf_init(&diag);
        check_rc = compiler_check(source, &diag);
        if (check_rc != 0 || has_diagnostics(diag.data)) {
            fwrite(diag.data, 1, diag.len, stdout);
            buf_free(&diag);
            return KAZE_EXIT_DIAG;
        }
        buf_free(&diag);
    }
    if (compiler_emit_c(source, &c) != 0) {
        fputs("kaze: the compiler did not emit C\n", stderr);
        buf_free(&c);
        return KAZE_EXIT_DIAG;
    }
    if (c.len == 0) {
        fputs("kaze: the compiler produced no output\n", stderr);
        buf_free(&c);
        return KAZE_EXIT_DIAG;
    }
    {
        const char *base = path_basename(source);
        size_t n = strlen(base);
        if (n > 5 && strcmp(base + n - 5, ".kaze") == 0) n -= 5;
        (void)n;
    }
    program_c = xasprintf("%s/program-%ld.c", tmpdir, (long)getpid());
    if (write_tracked(program_c, c.data, c.len, 0644) != 0) {
        fprintf(stderr, "kaze: cannot write %s: %s\n", program_c, strerror(errno));
        buf_free(&c);
        free(program_c);
        return KAZE_EXIT_USAGE;
    }
    detect_features(c.data, &features);
    if (opts->sitl || getenv("KAZE_SITL") != NULL) features.sitl = 1;
    if (opts->metal) features.metal = 1;
    if (opts->blas || getenv("KAZE_BLAS") != NULL) features.blas = 1;
    buf_free(&c);
    rc = link_program(program_c, tmpdir, opts, &features, output);
    free(program_c);
    if (rc != 0) {
        fprintf(stderr, "kaze: the emitted C did not build (backend %s)\n", opts->backend ? opts->backend : "c99");
        if (!g_keep_temps) fprintf(stderr, "kaze: re-run with --keep-temps to inspect the C and the compiler command\n");
        return rc == KAZE_EXIT_USAGE ? KAZE_EXIT_USAGE : KAZE_EXIT_DIAG;
    }
    return 0;
}

/* ------------------------------------------------------------------ */
/* commands                                                            */
/* ------------------------------------------------------------------ */

static void usage(FILE *fp) {
    fputs(
        "USAGE: kaze <command> [options] [args...]\n"
        "\n"
        "PROJECT & EXECUTION:\n"
        "  init [name]              Scaffold a new project (kaze.toml + src/main.kaze + tests/)\n"
        "  run <file.kaze> [-- ...] Compile and run immediately\n"
        "  build <file.kaze>        Build an executable artifact\n"
        "    -o <path>                Output binary path (default: stripped file stem)\n"
        "    --release                Optimise (-O3)\n"
        "    --backend=<c99|native>   Select the emitter (c99 is the portable backend)\n"
        "    --target=<triple>        Cross-target (native)\n"
        "    --asan                   Build with AddressSanitizer + UBSan\n"
        "    --debug, -g              Debug symbols, -O0\n"
        "  test [filter]            Discover and execute tests/**/*_test.kaze suites\n"
        "\n"
        "STATIC ANALYSIS & FORMATTING:\n"
        "  check <file.kaze>        Verify syntax, types, units, contracts, capabilities (exit 0/1)\n"
        "    --json                   Emit machine-readable JSON diagnostics\n"
        "  verify <file.kaze>       Run the Phase 16 formal proof & obligation verifier\n"
        "  fmt [options] <paths...> Format .kaze files deterministically\n"
        "    -w, --write              Modify files in place\n"
        "    --check                  Exit 1 when a file is not formatted\n"
        "\n"
        "TOOLCHAIN & INTEROP:\n"
        "  emit-c <file.kaze>       Print generated ANSI C99 source\n"
        "  emit-ir <file.kaze>      Print the shared unboxed scalar IR\n"
        "  emit-qbe <file.kaze>     Print QBE SSA for --backend=native\n"
        "  compile <file.kaze>      Emit a compact Kaze bytecode module (hex)\n"
        "  to-machine <file.kaze>   Convert human syntax -> canonical S-expression form\n"
        "  to-human <file.kaze>     Convert machine S-expression -> canonical human syntax\n"
        "  lsp [--dap]              Start the Language Server Protocol daemon\n"
        "  repl                     Start the interactive expression REPL\n"
        "  pkg <subcmd>             Package manager (init, verify, add, lock)\n"
        "  version                  Print the Kaze version, target and backend status\n"
        "\n"
        "GLOBAL:\n"
        "  --verbose                Show the C compiler command line\n"
        "  --keep-temps             Keep temporary build directories\n"
        "  -h, --help               This message\n",
        fp);
}

static int is_known_command(const char *s) {
    static const char *cmds[] = {
        "init", "run", "build", "test", "check", "verify", "fmt",
        "emit-c", "emit-ir", "emit-qbe", "compile", "to-machine", "to-human",
        "module-manifest", "emit-c-unit", "emit-ir-c",
        "lsp", "repl", "pkg", "version", "surface", "help", NULL
    };
    int i;
    for (i = 0; cmds[i] != NULL; i++) if (strcmp(s, cmds[i]) == 0) return 1;
    return 0;
}

static int cmd_version(void) {
    const char *triple = "unknown";
#if defined(__APPLE__) && defined(__aarch64__)
    triple = "aarch64-apple-darwin";
#elif defined(__APPLE__) && defined(__x86_64__)
    triple = "x86_64-apple-darwin";
#elif defined(__linux__) && defined(__aarch64__)
    triple = "aarch64-unknown-linux-gnu";
#elif defined(__linux__) && defined(__x86_64__)
    triple = "x86_64-unknown-linux-gnu";
#elif defined(__linux__) && defined(__arm__)
    triple = "armv7-unknown-linux-gnueabihf";
#endif
    printf("kaze %s\n", KAZE_VERSION);
    printf("target: %s\n", triple);
    {
        char *native = native_tool_path();
        int have_native = access(native, X_OK) == 0;
        printf("backend: c99 (%s)%s\n", compiler_cc(&(BuildOpts){0}),
               have_native ? "; native (QBE 1.2)" : "");
        free(native);
    }
    printf("runtime: embedded\n");
#if KAZE_HAVE_VERIFIER
    printf("verifier: embedded\n");
#else
    printf("verifier: external\n");
#endif
    return KAZE_EXIT_OK;
}

static int cmd_emit_c(int argc, char **argv) {
    const char *source = NULL;
    const char *output = NULL;
    int i;
    Buf c;
    int rc = KAZE_EXIT_OK;
    for (i = 0; i < argc; i++) {
        if (strcmp(argv[i], "-o") == 0 && i + 1 < argc) output = argv[++i];
        else if (argv[i][0] == '-' && argv[i][1] != 0) {
            fprintf(stderr, "kaze emit-c: unknown option %s\n", argv[i]);
            return KAZE_EXIT_USAGE;
        } else if (source == NULL) source = argv[i];
        else {
            fprintf(stderr, "kaze emit-c: unexpected argument %s\n", argv[i]);
            return KAZE_EXIT_USAGE;
        }
    }
    if (source == NULL) {
        fputs("usage: kaze emit-c <file.kaze> [-o out.c]\n", stderr);
        return KAZE_EXIT_USAGE;
    }
    if (!is_regular(source)) {
        fprintf(stderr, "kaze: no such file: %s\n", source);
        return KAZE_EXIT_USAGE;
    }
    /* Emit from the entry file directly: the compiler resolves the module DAG
     * and mangles every imported symbol. Text-bundling first would hide that
     * machinery from the only command that is supposed to expose it. */
    if (compiler_emit_c(source, &c) != 0) {
        fputs("kaze: the compiler did not emit C\n", stderr);
        buf_free(&c);
        return KAZE_EXIT_DIAG;
    }
    if (output != NULL) {
        if (write_file_raw(output, c.data, c.len, 0644) != 0) {
            fprintf(stderr, "kaze: cannot write %s: %s\n", output, strerror(errno));
            rc = KAZE_EXIT_USAGE;
        }
    } else {
        fwrite(c.data, 1, c.len, stdout);
    }
    buf_free(&c);
    return rc;
}

/* Forward a command to the compiler entry unchanged. The module manifest and
 * per-unit emitter are compiler features that make up the host build's public
 * surface; a unified binary that hides them breaks `kaze-host build`. */
static int cmd_passthrough(const char *command, int argc, char **argv) {
    char **av = (char **)xmalloc(((size_t)argc + 3) * sizeof *av);
    Buf out;
    int i;
    int rc;
    av[0] = (char *)"kaze";
    av[1] = (char *)command;
    for (i = 0; i < argc; i++) av[i + 2] = argv[i];
    av[argc + 2] = NULL;
    buf_init(&out);
    rc = run_compiler(av, &out);
    if (out.len > 0) fwrite(out.data, 1, out.len, stdout);
    buf_free(&out);
    free(av);
    return rc;
}

static int cmd_compile_like(const char *command, int argc, char **argv) {
    const char *source = NULL;
    int i;
    Buf out;
    int compiler_rc;
    if (argc < 1) {
        fprintf(stderr, "usage: kaze %s <file.kaze>\n", command);
        return KAZE_EXIT_USAGE;
    }
    for (i = 0; i < argc; i++) {
        if (argv[i][0] == '-' && argv[i][1] != 0) {
            fprintf(stderr, "kaze %s: unknown option %s\n", command, argv[i]);
            return KAZE_EXIT_USAGE;
        }
        if (source == NULL) source = argv[i];
        else {
            fprintf(stderr, "kaze %s: unexpected argument %s\n", command, argv[i]);
            return KAZE_EXIT_USAGE;
        }
    }
    if (!is_regular(source)) {
        fprintf(stderr, "kaze: no such file: %s\n", source);
        return KAZE_EXIT_USAGE;
    }
    /* Every compile-like command, including IR and QBE, resolves imports in
     * the compiler now. Passing the source path keeps the entry location (and
     * therefore per-file diagnostics and mangled module names) intact. */
    {
        char *av[5];
        av[0] = (char *)"kaze";
        av[1] = (char *)command;
        av[2] = (char *)source;
        av[3] = NULL;
        buf_init(&out);
        compiler_rc = run_compiler(av, &out);
    }
    fwrite(out.data ? out.data : "", 1, out.len, stdout);
    if (compiler_rc != 0 && (out.data == NULL || out.len == 0)) {
        buf_free(&out);
        return compiler_rc == KAZE_EXIT_USAGE ? KAZE_EXIT_USAGE : KAZE_EXIT_DIAG;
    }
    /* emit-qbe carries lowering diagnostics in the output stream (there is no
     * separate stderr channel through the embedded compiler). A caller that
     * treats any output as IL must also see a failing status. */
    if (strcmp(command, "emit-qbe") == 0 && compiler_rc != 0) {
        buf_free(&out);
        return compiler_rc == KAZE_EXIT_USAGE ? KAZE_EXIT_USAGE : KAZE_EXIT_DIAG;
    }
    if (strcmp(command, "compile") == 0) {
        if (out.data != NULL && strncmp(out.data, "compile:", 8) == 0 &&
            (strstr(out.data, "diagnostic") != NULL || strstr(out.data, "E_KAZE") != NULL ||
             strstr(out.data, "could not") != NULL || strstr(out.data, "not defined") != NULL)) {
            buf_free(&out);
            return KAZE_EXIT_DIAG;
        }
    }
    buf_free(&out);
    return KAZE_EXIT_OK;
}

static int cmd_check(int argc, char **argv) {
    int json = 0;
    StrVec files;
    int i;
    int any_bad = 0;
    int first = 1;
    vec_init(&files);
    for (i = 0; i < argc; i++) {
        if (strcmp(argv[i], "--json") == 0) json = 1;
        else if (argv[i][0] == '-' && argv[i][1] != 0) {
            fprintf(stderr, "kaze check: unknown option %s\n", argv[i]);
            vec_free(&files);
            return KAZE_EXIT_USAGE;
        } else {
            vec_push(&files, argv[i]);
        }
    }
    if (files.len == 0) {
        fputs("usage: kaze check [--json] <file.kaze>...\n", stderr);
        vec_free(&files);
        return KAZE_EXIT_USAGE;
    }
    /* A missing path is a setup error, not a clean program.  Without this the
     * compiler's own check command reports "check: ok" for a file it never
     * opened, which hides typos in scripts and editor integrations. */
    for (i = 0; i < (int)files.len; i++) {
        if (!is_regular(files.items[i])) {
            fprintf(stderr, "kaze: no such file: %s\n", files.items[i]);
            vec_free(&files);
            return KAZE_EXIT_USAGE;
        }
    }
    if (json) fputs("{\"files\":[", stdout);
    for (i = 0; i < (int)files.len; i++) {
        Buf out;
        DiagVec diags;
        int bad;
        memset(&diags, 0, sizeof diags);
        buf_init(&out);
        /* The compiler loads imports itself now, so checking the original file
         * keeps every diagnostic on its real path and line. Bundling here would
         * flatten `as`/`from` imports and mangle the line numbers again. */
        if (compiler_check(files.items[i], &out) != 0) bad = 1;
        else bad = has_diagnostics(out.data) ? 1 : 0;
        if (bad) parse_diagnostics(out.data, &diags);
        if (!json) {
            if (out.len) fwrite(out.data, 1, out.len, stdout);
        }
        if (json) {
            size_t d;
            if (!first) fputs(",", stdout);
            first = 0;
            fputs("{\"file\":", stdout);
            json_escape_fp(stdout, files.items[i]);
            fputs(",\"diagnostics\":[", stdout);
            for (d = 0; d < diags.len; d++) {
                const char *file = diags.items[d].file != NULL
                                       ? diags.items[d].file
                                       : files.items[i];
                int line = diags.items[d].line;
                if (d > 0) fputc(',', stdout);
                fputs("{\"file\":", stdout);
                json_escape_fp(stdout, file);
                printf(",\"line\":%d,\"severity\":\"error\",\"code\":", line);
                json_escape_fp(stdout, diags.items[d].code);
                fputs(",\"message\":", stdout);
                json_escape_fp(stdout, diags.items[d].message);
                fputc('}', stdout);
            }
            fputs("]}", stdout);
        }
        if (bad) any_bad = 1;
        diagvec_free(&diags);
        buf_free(&out);
    }
    if (json) puts(any_bad ? "],\"ok\":false}" : "],\"ok\":true}");
    vec_free(&files);
    return any_bad ? KAZE_EXIT_DIAG : KAZE_EXIT_OK;
}

/* Directory walk for fmt/test. */
static void collect_kaze_files(const char *path, StrVec *out, int skip_hw) {
    struct dirent *ent;
    DIR *dir;
    if (is_regular(path)) {
        /* An explicit path is a file the caller named, even when it has no
         * .kaze suffix (the formatter round-trip gate feeds one back in). */
        vec_push(out, path);
        return;
    }
    if (!is_dir(path)) return;
    dir = opendir(path);
    if (dir == NULL) return;
    while ((ent = readdir(dir)) != NULL) {
        char *child;
        if (strcmp(ent->d_name, ".") == 0 || strcmp(ent->d_name, "..") == 0) continue;
        if (strcmp(ent->d_name, ".git") == 0) continue;
        if (skip_hw && (strcmp(ent->d_name, "hw") == 0 || strcmp(ent->d_name, "http") == 0 ||
                        strcmp(ent->d_name, "fixtures") == 0 || strcmp(ent->d_name, "parity") == 0))
            continue;
        child = path_join(path, ent->d_name);
        if (!is_dir(child) && !has_kaze_suffix(child)) {
            free(child);
            continue;
        }
        collect_kaze_files(child, out, skip_hw);
        free(child);
    }
    closedir(dir);
}

static int cmd_fmt(int argc, char **argv) {
    int write_in_place = 0;
    int check = 0;
    StrVec paths;
    StrVec files;
    int i;
    int bad = 0;
    vec_init(&paths);
    vec_init(&files);
    for (i = 0; i < argc; i++) {
        if (strcmp(argv[i], "-w") == 0 || strcmp(argv[i], "--write") == 0) write_in_place = 1;
        else if (strcmp(argv[i], "--check") == 0) check = 1;
        else if (argv[i][0] == '-' && argv[i][1] != 0) {
            fprintf(stderr, "kaze fmt: unknown option %s\n", argv[i]);
            vec_free(&paths);
            vec_free(&files);
            return KAZE_EXIT_USAGE;
        } else vec_push(&paths, argv[i]);
    }
    if (paths.len == 0) {
        fputs("usage: kaze fmt [-w|--check] <paths...>\n", stderr);
        vec_free(&paths);
        vec_free(&files);
        return KAZE_EXIT_USAGE;
    }
    for (i = 0; i < (int)paths.len; i++) {
        if (!is_regular(paths.items[i]) && !is_dir(paths.items[i])) {
            fprintf(stderr, "kaze fmt: no such file or directory: %s\n", paths.items[i]);
            vec_free(&paths);
            vec_free(&files);
            return KAZE_EXIT_USAGE;
        }
        collect_kaze_files(paths.items[i], &files, 0);
    }
    if (files.len == 0) {
        fprintf(stderr, "kaze fmt: no .kaze files under %s\n", paths.items[0]);
        vec_free(&paths);
        vec_free(&files);
        return KAZE_EXIT_USAGE;
    }
    for (i = 0; i < (int)files.len; i++) {
        char *av[5];
        Buf out;
        Buf original;
        av[0] = (char *)"kaze";
        av[1] = (char *)"fmt";
        av[2] = files.items[i];
        av[3] = NULL;
        buf_init(&out);
        if (run_compiler(av, &out) != 0) {
            fputs("kaze fmt: the compiler failed\n", stderr);
            buf_free(&out);
            bad = 1;
            continue;
        }
        buf_init(&original);
        read_file(files.items[i], &original);
        if (write_in_place) {
            if (out.len != original.len || memcmp(out.data, original.data, out.len) != 0) {
                if (write_file_raw(files.items[i], out.data, out.len, 0644) != 0) {
                    fprintf(stderr, "kaze fmt: cannot write %s: %s\n", files.items[i], strerror(errno));
                    bad = 1;
                }
            }
        } else if (check) {
            if (out.len != original.len || memcmp(out.data, original.data, out.len) != 0) {
                fprintf(stderr, "%s: not formatted\n", files.items[i]);
                bad = 1;
            }
        } else {
            fwrite(out.data, 1, out.len, stdout);
        }
        buf_free(&out);
        buf_free(&original);
    }
    vec_free(&paths);
    vec_free(&files);
    return bad ? KAZE_EXIT_DIAG : KAZE_EXIT_OK;
}

static int cmd_verify(int argc, char **argv) {
    const char *source = NULL;
    const char *mode = "report";
    int i;
    BundledSource bs;
    char err[2048];
    char *std_dir;
    char *tmpdir;
    char *materialized;
    Buf out;
    int rc;
    for (i = 0; i < argc; i++) {
        if (strcmp(argv[i], "--json") == 0) mode = "json";
        else if (strcmp(argv[i], "--smt2") == 0) mode = "smt2";
        else if (strcmp(argv[i], "--horn") == 0) mode = "horn";
        else if (strcmp(argv[i], "--proof") == 0) mode = "proof";
        else if (strncmp(argv[i], "--mode=", 7) == 0) mode = argv[i] + 7;
        else if (argv[i][0] == '-' && argv[i][1] != 0) {
            fprintf(stderr, "kaze verify: unknown option %s\n", argv[i]);
            return KAZE_EXIT_USAGE;
        } else if (source == NULL) source = argv[i];
        else {
            fprintf(stderr, "kaze verify: unexpected argument %s\n", argv[i]);
            return KAZE_EXIT_USAGE;
        }
    }
    if (source == NULL) {
        fputs("usage: kaze verify [--json|--smt2|--horn|--proof] <file.kaze>\n", stderr);
        return KAZE_EXIT_USAGE;
    }
    if (!is_regular(source)) {
        fprintf(stderr, "kaze: no such file: %s\n", source);
        return KAZE_EXIT_USAGE;
    }
#if !KAZE_HAVE_VERIFIER
    {
        char *sib = path_join(g_exe_dir, "kaze-verify");
        if (!is_regular(sib)) {
            fprintf(stderr, "kaze: this build has no embedded verifier and no %s\n", sib);
            free(sib);
            return KAZE_EXIT_USAGE;
        }
        {
            Buf out2;
            char *av[7];
            int n = 0;
            av[n++] = sib;
            av[n++] = (char *)"--arg";
            av[n++] = xasprintf("source=%s", source);
            av[n++] = (char *)"--arg";
            av[n++] = xasprintf("mode=%s", mode);
            av[n] = NULL;
            buf_init(&out2);
            rc = capture_child(av, NULL, &out2);
            fwrite(out2.data, 1, out2.len, stdout);
            buf_free(&out2);
            free(sib);
            return rc == 0 ? KAZE_EXIT_OK : KAZE_EXIT_DIAG;
        }
    }
#endif
    tmpdir = make_temp_dir("kaze-verify-");
    if (tmpdir == NULL) return KAZE_EXIT_USAGE;
    std_dir = find_std_dir();
    bundled_source_init(&bs);
    if (bundle_source(source, &bs, std_dir, err, sizeof err) != 0) {
        fprintf(stderr, "kaze: %s\n", err);
        bundled_source_free(&bs);
        free(std_dir);
        return KAZE_EXIT_DIAG;
    }
    materialized = (bs.bundled == 1) ? materialize_bundle(&bs, tmpdir) : xstrdup(bs.path);
    bundled_source_free(&bs);
    free(std_dir);
    if (materialized == NULL) return KAZE_EXIT_USAGE;
    {
        char *argv2[7];
        char *src_arg = xasprintf("source=%s", materialized);
        char *mode_arg = xasprintf("mode=%s", mode);
        buf_init(&out);
        argv2[0] = (char *)"kaze-verify";
        argv2[1] = (char *)"--arg";
        argv2[2] = src_arg;
        argv2[3] = (char *)"--arg";
        argv2[4] = mode_arg;
        argv2[5] = NULL;
#if KAZE_HAVE_VERIFIER
        rc = capture_entry(ENTRY_VERIFIER, argv2, &out);
#else
        rc = KAZE_EXIT_USAGE;
#endif
        fwrite(out.data ? out.data : "", 1, out.len, stdout);
        free(src_arg);
        free(mode_arg);
    }
    free(materialized);
    if (rc != 0) {
        buf_free(&out);
        return KAZE_EXIT_DIAG;
    }
    if (strcmp(mode, "report") == 0) {
        if (strstr(out.data ? out.data : "", "unproven: 0") == NULL &&
            strstr(out.data ? out.data : "", "unproven:") != NULL) {
            /* A non-zero unproven count is a failed proof obligation. */
            const char *u = strstr(out.data, "unproven:");
            if (u != NULL && strncmp(u + 9, " 0", 2) != 0) {
                buf_free(&out);
                return KAZE_EXIT_DIAG;
            }
        }
        if (strstr(out.data ? out.data : "", "certified routines:") != NULL) {
            const char *c = strstr(out.data, "certified routines:");
            int x = 0, y = 0;
            if (c != NULL && sscanf(c + 19, " %d/%d", &x, &y) == 2 && x < y) {
                buf_free(&out);
                return KAZE_EXIT_DIAG;
            }
        }
    } else if (strcmp(mode, "json") == 0) {
        const char *u = strstr(out.data ? out.data : "", "\"unproven\":");
        if (u != NULL) {
            int n = atoi(u + 11);
            if (n > 0) {
                buf_free(&out);
                return KAZE_EXIT_DIAG;
            }
        }
    }
    buf_free(&out);
    return KAZE_EXIT_OK;
}

/* -- run / build option parsing ------------------------------------ */

static int parse_build_opts(int argc, char **argv, BuildOpts *opts, const char **source,
                            StrVec *rest, int allow_rest) {
    int i;
    int seen_source = 0;
    for (i = 0; i < argc; i++) {
        const char *a = argv[i];
        if (strcmp(a, "--") == 0) {
            i++;
            while (i < argc) {
                if (allow_rest) vec_push(rest, argv[i]);
                else {
                    fprintf(stderr, "kaze: unexpected argument %s\n", argv[i]);
                    return KAZE_EXIT_USAGE;
                }
                i++;
            }
            return KAZE_EXIT_OK;
        }
        if (strcmp(a, "--release") == 0) opts->release = 1;
        else if (strcmp(a, "--debug") == 0 || strcmp(a, "-g") == 0) opts->debug = 1;
        else if (strcmp(a, "--asan") == 0) opts->asan = 1;
        else if (strcmp(a, "--strict") == 0) opts->strict = 1;
        else if (strcmp(a, "--sitl") == 0) opts->sitl = 1;
        else if (strcmp(a, "--gpu=metal") == 0) opts->metal = 1;
        else if (strcmp(a, "--metal") == 0) opts->metal = 1;
        else if (strcmp(a, "--blas") == 0) opts->blas = 1;
        else if (strcmp(a, "--keep-temps") == 0) g_keep_temps = 1;
        else if (strcmp(a, "--verbose") == 0) g_verbose = 1;
        else if (strncmp(a, "--backend=", 10) == 0) opts->backend = a + 10;
        else if (strncmp(a, "--target=", 9) == 0) opts->target = a + 9;
        else if (strncmp(a, "--cc=", 5) == 0) opts->cc = a + 5;
        else if (strncmp(a, "--gpu=", 6) == 0) opts->gpu = a + 6;
        else if (strcmp(a, "-o") == 0 && i + 1 < argc) opts->output = argv[++i];
        else if (strncmp(a, "-O", 2) == 0 && a[2] >= '0' && a[2] <= '3' && a[3] == 0) {
            opts->release = (a[2] == '3');
            opts->debug = (a[2] == '0');
        } else if (a[0] == '-' && a[1] != 0) {
            if (seen_source && allow_rest) {
                vec_push(rest, a);
            } else {
                fprintf(stderr, "kaze: unknown option %s\n", a);
                return KAZE_EXIT_USAGE;
            }
        } else if (!seen_source) {
            *source = a;
            seen_source = 1;
        } else if (allow_rest) {
            vec_push(rest, a);
        } else {
            fprintf(stderr, "kaze: unexpected argument %s\n", a);
            return KAZE_EXIT_USAGE;
        }
    }
    return *source == NULL ? KAZE_EXIT_USAGE : KAZE_EXIT_OK;
}

static int validate_backend(const char *backend) {
    if (backend == NULL || backend[0] == 0 || strcmp(backend, "c99") == 0 ||
        strcmp(backend, "native") == 0) return 1;
    fprintf(stderr, "kaze: unknown backend '%s' (expected c99 or native)\n", backend);
    return 0;
}

static int validate_target(const char *target) {
    if (target == NULL || strcmp(target, "native") == 0) return 1;
    fprintf(stderr, "kaze: --target=%s needs the cross/back-end toolchain; this binary supports --target=native\n", target);
    return 0;
}

/* The native host is a sibling of the unified CLI (or KAZE_NATIVE_BIN).
 * It owns QBE, `as` and `ld`; this file only forwards the command. */
static char *native_tool_path(void) {
    const char *env = getenv("KAZE_NATIVE_BIN");
    if (env != NULL && env[0] != 0) return xstrdup(env);
    if (g_exe_dir != NULL) {
        char *cand = path_join(g_exe_dir, "kaze-native");
        if (access(cand, X_OK) == 0) return cand;
        free(cand);
    }
    return xstrdup("kaze-native");
}

static int run_native_tool(const char *mode, const char *source, const StrVec *rest,
                           const BuildOpts *opts, const char *output) {
    char *tool = native_tool_path();
    size_t cap = 8 + (rest != NULL ? rest->len : 0);
    char **av = (char **)xmalloc(cap * sizeof *av);
    size_t n = 0;
    const char *target = opts->target;
    int rc;
    av[n++] = tool;
    av[n++] = (char *)mode;
    av[n++] = (char *)source;
    if (output != NULL) {
        av[n++] = (char *)"-o";
        av[n++] = (char *)output;
    }
    if (target != NULL && target[0] != 0 && strcmp(target, "native") != 0) {
        av[n++] = xasprintf("--target=%s", target);
    }
    if (rest != NULL) {
        size_t i;
        for (i = 0; i < rest->len && n + 2 < cap; i++) av[n++] = rest->items[i];
    }
    av[n] = NULL;
    if (g_verbose) {
        size_t i;
        fputs("kaze: command:", stderr);
        for (i = 0; av[i] != NULL; i++) fprintf(stderr, " %s", av[i]);
        fputc('\n', stderr);
    }
    rc = run_child(av);
    if (rc == 127) {
        fprintf(stderr, "E_KAZE_BACKEND_UNSUPPORTED: cannot find %s\n", tool);
        rc = KAZE_EXIT_DIAG;
    }
    free(tool);
    return rc;
}

static int cmd_run(int argc, char **argv) {
    BuildOpts opts;
    const char *source = NULL;
    StrVec rest;
    char *tmpdir;
    char *output;
    int rc;
    build_opts_init(&opts);
    vec_init(&rest);
    if (parse_build_opts(argc, argv, &opts, &source, &rest, 1) != KAZE_EXIT_OK) {
        vec_free(&rest);
        return KAZE_EXIT_USAGE;
    }
    if (!is_regular(source)) {
        fprintf(stderr, "kaze: no such file: %s\n", source);
        vec_free(&rest);
        return KAZE_EXIT_USAGE;
    }
    if (!validate_backend(opts.backend) || !validate_target(opts.target)) {
        vec_free(&rest);
        return KAZE_EXIT_USAGE;
    }
    if (opts.backend != NULL && strcmp(opts.backend, "native") == 0) {
        rc = run_native_tool("run", source, &rest, &opts, NULL);
        vec_free(&rest);
        return rc;
    }
    tmpdir = make_temp_dir("kaze-run-");
    if (tmpdir == NULL) {
        vec_free(&rest);
        return KAZE_EXIT_USAGE;
    }
    output = xasprintf("%s/program", tmpdir);
    track_path(output);
    rc = build_source(source, &opts, tmpdir, output);
    if (rc != 0) {
        free(output);
        vec_free(&rest);
        if (g_keep_temps) fprintf(stderr, "kaze: temporary files: %s\n", tmpdir);
        return rc;
    }
    {
        StrVec run;
        size_t i;
        vec_init(&run);
        vec_push(&run, output);
        for (i = 0; i < rest.len; i++) vec_push(&run, rest.items[i]);
        vec_push_owned(&run, NULL);
        if (g_verbose) {
            size_t k;
            fputs("kaze: exec:", stderr);
            for (k = 0; run.items[k] != NULL; k++) fprintf(stderr, " [%s]", run.items[k]);
            fputc('\n', stderr);
        }
        rc = run_child(run.items);
        vec_free(&run);
    }
    if (g_keep_temps) fprintf(stderr, "kaze: temporary files: %s\n", tmpdir);
    free(output);
    vec_free(&rest);
    return rc;
}

static int cmd_build(int argc, char **argv) {
    BuildOpts opts;
    const char *source = NULL;
    StrVec rest;
    char *tmpdir;
    char *output;
    int rc;
    build_opts_init(&opts);
    vec_init(&rest);
    if (parse_build_opts(argc, argv, &opts, &source, &rest, 0) != KAZE_EXIT_OK) {
        vec_free(&rest);
        return KAZE_EXIT_USAGE;
    }
    if (!is_regular(source)) {
        fprintf(stderr, "kaze: no such file: %s\n", source);
        vec_free(&rest);
        return KAZE_EXIT_USAGE;
    }
    if (!validate_backend(opts.backend) || !validate_target(opts.target)) {
        vec_free(&rest);
        return KAZE_EXIT_USAGE;
    }
    tmpdir = make_temp_dir("kaze-build-");
    if (tmpdir == NULL) {
        vec_free(&rest);
        return KAZE_EXIT_USAGE;
    }
    output = opts.output ? xstrdup(opts.output) : default_output_name(source);
    {
        char *dir = path_dirname(output);
        if (mkdir_p(dir) != 0) {
            fprintf(stderr, "kaze: cannot create output directory %s\n", dir);
            free(dir);
            free(output);
            vec_free(&rest);
            return KAZE_EXIT_USAGE;
        }
        free(dir);
    }
    if (opts.backend != NULL && strcmp(opts.backend, "native") == 0) {
        rc = run_native_tool("build", source, NULL, &opts, output);
        if (rc == 0) printf("kaze: built %s\n", output);
        if (g_keep_temps) fprintf(stderr, "kaze: temporary files: %s\n", tmpdir);
        free(output);
        vec_free(&rest);
        return rc;
    }
    rc = build_source(source, &opts, tmpdir, output);
    if (rc == 0) printf("kaze: built %s\n", output);
    if (g_keep_temps) fprintf(stderr, "kaze: temporary files: %s\n", tmpdir);
    free(output);
    vec_free(&rest);
    return rc;
}

static int cmd_test(int argc, char **argv) {
    const char *filter = NULL;
    StrVec files;
    int i;
    char *tmpdir;
    char *policy_dir;
    int passed = 0, failed = 0;
    int rc = KAZE_EXIT_OK;
    vec_init(&files);
    for (i = 0; i < argc; i++) {
        if (argv[i][0] == '-' && argv[i][1] != 0) {
            if (strcmp(argv[i], "--verbose") == 0) g_verbose = 1;
            else if (strcmp(argv[i], "--keep-temps") == 0) g_keep_temps = 1;
            else {
                fprintf(stderr, "kaze test: unknown option %s\n", argv[i]);
                vec_free(&files);
                return KAZE_EXIT_USAGE;
            }
        } else if (filter == NULL) filter = argv[i];
        else {
            fprintf(stderr, "kaze test: unexpected argument %s\n", argv[i]);
            vec_free(&files);
            return KAZE_EXIT_USAGE;
        }
    }
    if (is_dir("tests/stdlib")) collect_kaze_files("tests/stdlib", &files, 0);
    else if (is_dir("tests")) collect_kaze_files("tests", &files, 1);
    if (files.len == 0) {
        fprintf(stderr, "kaze test: no tests/stdlib/*_test.kaze suites found\n");
        vec_free(&files);
        return KAZE_EXIT_USAGE;
    }
    tmpdir = make_temp_dir("kaze-test-");
    if (tmpdir == NULL) {
        vec_free(&files);
        return KAZE_EXIT_USAGE;
    }
    policy_dir = xasprintf("%s/policy", tmpdir);
    {
        char *policy_path = xasprintf("%s/policy.json", policy_dir);
        int policy_ok = mkdir_p(policy_dir) == 0 &&
                        write_tracked(policy_path, "{\"deny\":[]}\n", 12, 0644) == 0;
        free(policy_path);
        if (!policy_ok) {
            fprintf(stderr, "kaze: cannot create the test policy directory\n");
            free(tmpdir);
            free(policy_dir);
            vec_free(&files);
            return KAZE_EXIT_USAGE;
        }
    }
    for (i = 0; i < (int)files.len; i++) {
        const char *file = files.items[i];
        const char *base = path_basename(file);
        size_t n = strlen(base);
        if (n > 10 && strcmp(base + n - 10, "_test.kaze") == 0) n -= 10;
        else if (n > 5) n -= 5;
        {
            char *name = (char *)xmalloc(n + 1);
            memcpy(name, base, n);
            name[n] = 0;
            if (filter != NULL && strstr(name, filter) == NULL) {
                free(name);
                continue;
            }
            {
                char *out = xasprintf("%s/test-%d", tmpdir, passed + failed);
                BuildOpts opts;
                int brc;
                build_opts_init(&opts);
                opts.asan = getenv("KAZE_TEST_ASAN") != NULL;
                brc = build_source(file, &opts, tmpdir, out);
                if (brc != 0) {
                    printf("FAIL  %-24s build\n", name);
                    failed++;
                    rc = KAZE_EXIT_DIAG;
                } else {
                    char *run_argv[4];
                    Buf outbuf;
                    int rrc;
                    run_argv[0] = out;
                    run_argv[1] = (char *)"--ticks";
                    run_argv[2] = (char *)"4";
                    run_argv[3] = NULL;
                    buf_init(&outbuf);
                    rrc = run_program_capture(out, run_argv, policy_dir, &outbuf);
                    if (rrc != 0 || (outbuf.data != NULL && strstr(outbuf.data, "FAIL") != NULL)) {
                        printf("FAIL  %-24s exit %d\n", name, rrc);
                        if (outbuf.len) {
                            const char *p = outbuf.data;
                            int lines = 0;
                            while (p != NULL && *p != 0 && lines < 12) {
                                const char *e = strchr(p, '\n');
                                size_t ln = e ? (size_t)(e - p) : strlen(p);
                                fprintf(stderr, "    %.*s\n", (int)ln, p);
                                lines++;
                                p = e ? e + 1 : NULL;
                            }
                        }
                        failed++;
                        rc = KAZE_EXIT_DIAG;
                    } else {
                        printf("PASS  %-24s\n", name);
                        passed++;
                    }
                    buf_free(&outbuf);
                }
                free(out);
            }
            free(name);
        }
    }
    printf("\nkaze test: %d passed, %d failed\n", passed, failed);
    if (g_keep_temps) fprintf(stderr, "kaze: temporary files: %s\n", tmpdir);
    free(tmpdir);
    free(policy_dir);
    vec_free(&files);
    if (passed + failed == 0) {
        fprintf(stderr, "kaze test: filter '%s' matched no tests\n", filter ? filter : "");
        return KAZE_EXIT_USAGE;
    }
    return rc;
}

/* ------------------------------------------------------------------ */
/* init / pkg / repl                                                   */
/* ------------------------------------------------------------------ */

static int safe_project_name(const char *s) {
    const unsigned char *p = (const unsigned char *)s;
    if (s == NULL || s[0] == 0) return 0;
    for (; *p != 0; p++) {
        if (!(isalnum(*p) || *p == '_' || *p == '-' || *p == '.')) return 0;
    }
    return 1;
}

static int cmd_init(int argc, char **argv) {
    const char *name = NULL;
    int force = 0;
    int i;
    char *cwd;
    char *root;
    char *src;
    char *tests;
    char *path;
    for (i = 0; i < argc; i++) {
        if (strcmp(argv[i], "--force") == 0) force = 1;
        else if (argv[i][0] == '-' && argv[i][1] != 0) {
            fprintf(stderr, "kaze init: unknown option %s\n", argv[i]);
            return KAZE_EXIT_USAGE;
        } else if (name == NULL) name = argv[i];
        else {
            fprintf(stderr, "kaze init: unexpected argument %s\n", argv[i]);
            return KAZE_EXIT_USAGE;
        }
    }
    if (name == NULL) name = ".";
    cwd = getcwd(NULL, 0);
    if (cwd == NULL) cwd = xstrdup(".");
    if (strcmp(name, ".") == 0) {
        root = xstrdup(cwd);
        name = path_basename(cwd);
    } else {
        root = path_join(cwd, name);
    }
    if (!safe_project_name(name)) {
        fprintf(stderr, "kaze init: project name '%s' may only contain letters, digits, '_', '-' and '.'\n", name);
        free(cwd);
        free(root);
        return KAZE_EXIT_USAGE;
    }
    if (path_exists(root) && !is_dir(root)) {
        fprintf(stderr, "kaze init: %s exists and is not a directory\n", root);
        free(cwd);
        free(root);
        return KAZE_EXIT_USAGE;
    }
    if (is_dir(root) && !force) {
        DIR *d = opendir(root);
        int nonempty = 0;
        if (d != NULL) {
            struct dirent *e;
            while ((e = readdir(d)) != NULL) {
                if (strcmp(e->d_name, ".") != 0 && strcmp(e->d_name, "..") != 0) {
                    nonempty = 1;
                    break;
                }
            }
            closedir(d);
        }
        if (nonempty) {
            fprintf(stderr, "kaze init: %s is not empty (use --force to scaffold anyway)\n", root);
            free(cwd);
            free(root);
            return KAZE_EXIT_USAGE;
        }
    }
    src = path_join(root, "src");
    tests = path_join(root, "tests");
    if (mkdir_p(src) != 0 || mkdir_p(tests) != 0) {
        fprintf(stderr, "kaze init: cannot create the project directories under %s\n", root);
        free(cwd); free(root); free(src); free(tests);
        return KAZE_EXIT_USAGE;
    }
    path = path_join(root, "kaze.toml");
    {
        char *body = xasprintf("[package]\nname = \"%s\"\nversion = \"0.1.0\"\nentry = \"src/main.kaze\"\n", name);
        if (write_file_raw(path, body, strlen(body), 0644) != 0) {
            fprintf(stderr, "kaze init: cannot write %s\n", path);
            free(body); free(path); free(cwd); free(root); free(src); free(tests);
            return KAZE_EXIT_USAGE;
        }
        free(body);
    }
    free(path);
    path = path_join(src, "main.kaze");
    {
        const char *body =
            "# A Kaze 1.0 program.\n"
            "action main() -> num:\n"
            "    return 2 + 3\n";
        if (write_file_raw(path, body, strlen(body), 0644) != 0) {
            fprintf(stderr, "kaze init: cannot write %s\n", path);
            free(path); free(cwd); free(root); free(src); free(tests);
            return KAZE_EXIT_USAGE;
        }
    }
    free(path);
    path = path_join(tests, "smoke_test.kaze");
    {
        const char *body =
            "# Run by `kaze test`: a suite passes when it exits 0 and prints no FAIL.\n"
            "action main() -> num:\n"
            "    return 0\n";
        if (write_file_raw(path, body, strlen(body), 0644) != 0) {
            fprintf(stderr, "kaze init: cannot write %s\n", path);
            free(path); free(cwd); free(root); free(src); free(tests);
            return KAZE_EXIT_USAGE;
        }
    }
    free(path);
    path = path_join(root, ".gitignore");
    write_file_raw(path, "kaze-build/\n", 12, 0644);
    free(path);
    printf("kaze: created project '%s' in %s\n", name, root);
    printf("  cd %s\n", root);
    printf("  kaze check src/main.kaze\n");
    printf("  kaze run src/main.kaze\n");
    free(cwd);
    free(root);
    free(src);
    free(tests);
    return KAZE_EXIT_OK;
}

static int cmd_pkg(int argc, char **argv) {
    if (argc < 1) {
        fputs("usage: kaze pkg <init|verify|add|lock>\n", stderr);
        return KAZE_EXIT_USAGE;
    }
    if (strcmp(argv[0], "init") == 0) return cmd_init(argc - 1, argv + 1);
    if (strcmp(argv[0], "verify") == 0) {
        FILE *fp;
        char line[1024];
        char entry[PATH_MAX] = "";
        if (!is_regular("kaze.toml")) {
            fputs("kaze pkg verify: no kaze.toml in this directory\n", stderr);
            return KAZE_EXIT_USAGE;
        }
        fp = fopen("kaze.toml", "r");
        if (fp == NULL) return KAZE_EXIT_USAGE;
        while (fgets(line, sizeof line, fp) != NULL) {
            char *eq = strchr(line, '=');
            if (eq == NULL) continue;
            if (strncmp(line, "entry", 5) == 0) {
                char *q1 = strchr(eq, '"');
                char *q2 = q1 ? strchr(q1 + 1, '"') : NULL;
                if (q1 && q2 && (size_t)(q2 - q1 - 1) < sizeof entry) {
                    memcpy(entry, q1 + 1, (size_t)(q2 - q1 - 1));
                    entry[q2 - q1 - 1] = 0;
                }
            }
        }
        fclose(fp);
        if (entry[0] == 0) {
            fputs("kaze pkg verify: kaze.toml has no entry\n", stderr);
            return KAZE_EXIT_USAGE;
        }
        if (!is_regular(entry)) {
            fprintf(stderr, "kaze pkg verify: entry %s does not exist\n", entry);
            return KAZE_EXIT_DIAG;
        }
        printf("kaze pkg verify: %s\n", entry);
        return KAZE_EXIT_OK;
    }
    if (strcmp(argv[0], "add") == 0 || strcmp(argv[0], "lock") == 0) {
        fputs("kaze pkg: registry support is planned for Phase 20; kaze.toml is already authoritative\n", stderr);
        return KAZE_EXIT_USAGE;
    }
    fprintf(stderr, "kaze pkg: unknown subcommand %s\n", argv[0]);
    return KAZE_EXIT_USAGE;
}

static int repl_has_needs(const Buf *session) {
    return session->len > 0 && strstr(session->data, "needs:") != NULL;
}

static int repl_has_main(const Buf *session) {
    if (session->len == 0) return 0;
    return strstr(session->data, "action main(") != NULL ||
           strstr(session->data, "check main(") != NULL ||
           strstr(session->data, "filter main(") != NULL ||
           strstr(session->data, "fiber main(") != NULL;
}

static void repl_help(void) {
    fputs(
        "Kaze REPL -- definitions accumulate; effects run in a fresh process.\n"
        "\n"
        "  <kaze source>   append a line (a blank line commits the block)\n"
        "  = <expression>  evaluate and print a value\n"
        "  :run            compile and run the buffer\n"
        "  :check          compile-check the buffer\n"
        "  :show           print the buffer\n"
        "  :clear          empty the buffer\n"
        "  :load FILE      replace the buffer with FILE\n"
        "  :save FILE      write the buffer to FILE\n"
        "  :history        numbered input history\n"
        "  :help           this text\n"
        "  :quit           leave\n"
        "\n"
        "A run does not preserve values, only source.  `=` is unavailable once\n"
        "the buffer defines its own main.\n",
        stdout);
}

static void repl_compose(const Buf *session, const char *expr, Buf *source) {
    buf_init(source);
    if (!repl_has_needs(session)) buf_appends(source, "needs: log\n");
    if (session->len > 0) {
        buf_append(source, session->data, session->len);
        if (session->data[session->len - 1] != '\n') buf_appendc(source, '\n');
    }
    if (expr != NULL) {
        buf_appends(source, "\naction main() -> text:\n    return \"\" + (");
        buf_appends(source, expr);
        buf_appends(source, ")\n");
    } else if (!repl_has_main(session)) {
        buf_appends(source, "\naction main() -> num:\n    return 0\n");
    }
}

static int repl_compile(const Buf *source, const char *tmpdir, int serial, int check_only) {
    char *src = xasprintf("%s/repl-%d.kaze", tmpdir, serial);
    int rc;
    if (write_tracked(src, source->data, source->len, 0644) != 0) {
        fprintf(stderr, "kaze repl: cannot write %s\n", src);
        free(src);
        return KAZE_EXIT_USAGE;
    }
    if (check_only) {
        Buf out;
        buf_init(&out);
        rc = compiler_check(src, &out);
        if (out.len > 0) fwrite(out.data, 1, out.len, stdout);
        else if (rc == 0) fputs("check: ok\n", stdout);
        buf_free(&out);
    } else {
        char *bin = xasprintf("%s/repl-bin-%d", tmpdir, serial);
        BuildOpts opts;
        build_opts_init(&opts);
        track_path(bin);
        rc = build_source(src, &opts, tmpdir, bin);
        if (rc == 0) {
            char *av[2];
            Buf out;
            av[0] = bin;
            av[1] = NULL;
            buf_init(&out);
            rc = run_program_capture(bin, av, NULL, &out);
            if (out.len > 0) fwrite(out.data, 1, out.len, stdout);
            buf_free(&out);
        }
        free(bin);
    }
    free(src);
    return rc;
}

static char *repl_argument(const char *line, size_t prefix) {
    const char *p = line + prefix;
    char *out;
    size_t n;
    while (*p == ' ' || *p == '\t') p++;
    n = strlen(p);
    while (n > 0 && (p[n - 1] == ' ' || p[n - 1] == '\t')) n--;
    if (n >= 2 && p[0] == '"' && p[n - 1] == '"') {
        p++;
        n -= 2;
    }
    out = (char *)xmalloc(n + 1);
    memcpy(out, p, n);
    out[n] = 0;
    return out;
}

static int cmd_repl(int argc, char **argv) {
    char line[4096];
    char *tmpdir = make_temp_dir("kaze-repl-");
    Buf session;
    Buf pending;
    StrVec history;
    int serial = 0;
    int interactive = isatty(fileno(stdin));
    size_t h;
    if (tmpdir == NULL) return KAZE_EXIT_USAGE;
    buf_init(&session);
    buf_init(&pending);
    vec_init(&history);
    if (argc > 1) {
        fputs("usage: kaze repl [file.kaze]\n", stderr);
        return KAZE_EXIT_USAGE;
    }
    if (argc == 1) {
        Buf preload;
        buf_init(&preload);
        if (read_file(argv[0], &preload) != 0) {
            fprintf(stderr, "kaze repl: cannot read %s\n", argv[0]);
            buf_free(&preload);
            return KAZE_EXIT_USAGE;
        }
        buf_append(&session, preload.data, preload.len);
        if (preload.len > 0 && preload.data[preload.len - 1] != '\n') buf_appendc(&session, '\n');
        buf_free(&preload);
        if (interactive) fprintf(stderr, "loaded %s\n", argv[0]);
    }
    if (interactive) fputs("Kaze REPL. :help for commands, :quit to leave.\n", stderr);
    for (;;) {
        if (interactive) {
            fputs(pending.len > 0 ? "...> " : "kaze> ", stderr);
            fflush(stderr);
        }
        if (fgets(line, sizeof line, stdin) == NULL) {
            if (interactive) fputc('\n', stderr);
            break;
        }
        {
            size_t n = strlen(line);
            while (n > 0 && (line[n - 1] == '\n' || line[n - 1] == '\r')) line[--n] = 0;
        }
        if (pending.len == 0) {
            if (strcmp(line, ":quit") == 0 || strcmp(line, ":q") == 0 ||
                strcmp(line, "quit") == 0 || strcmp(line, "exit") == 0) {
                break;
            }
            if (strcmp(line, ":help") == 0 || strcmp(line, "help") == 0 || strcmp(line, "?") == 0) {
                repl_help();
                continue;
            }
            if (strcmp(line, ":show") == 0) {
                if (session.len > 0) fwrite(session.data, 1, session.len, stdout);
                continue;
            }
            if (strcmp(line, ":clear") == 0) {
                session.len = 0;
                pending.len = 0;
                fputs("buffer cleared\n", stdout);
                continue;
            }
            if (strcmp(line, ":history") == 0) {
                for (h = 0; h < history.len; h++) {
                    printf("%4lu  %s\n", (unsigned long)(h + 1), history.items[h]);
                }
                continue;
            }
            if (strncmp(line, ":load ", 6) == 0) {
                char *path = repl_argument(line, 6);
                Buf loaded;
                buf_init(&loaded);
                if (read_file(path, &loaded) != 0) {
                    fprintf(stderr, "kaze repl: no such file: %s\n", path);
                } else {
                    session.len = 0;
                    buf_append(&session, loaded.data, loaded.len);
                    if (loaded.len > 0 && loaded.data[loaded.len - 1] != '\n') {
                        buf_appendc(&session, '\n');
                    }
                    pending.len = 0;
                    printf("loaded %s\n", path);
                }
                buf_free(&loaded);
                free(path);
                continue;
            }
            if (strncmp(line, ":save ", 6) == 0) {
                char *path = repl_argument(line, 6);
                Buf saved;
                repl_compose(&session, NULL, &saved);
                if (write_file_raw(path, saved.data, saved.len, 0644) != 0) {
                    fprintf(stderr, "kaze repl: cannot write %s\n", path);
                } else {
                    printf("wrote %s\n", path);
                }
                buf_free(&saved);
                free(path);
                continue;
            }
            if (strcmp(line, ":check") == 0) {
                Buf source;
                repl_compose(&session, NULL, &source);
                repl_compile(&source, tmpdir, serial++, 1);
                buf_free(&source);
                continue;
            }
            if (strcmp(line, ":run") == 0) {
                Buf source;
                repl_compose(&session, NULL, &source);
                repl_compile(&source, tmpdir, serial++, 0);
                buf_free(&source);
                continue;
            }
            if (line[0] == '=') {
                const char *expr = line + 1;
                while (*expr == ' ' || *expr == '\t') expr++;
                if (*expr != 0) {
                    Buf source;
                    vec_push_owned(&history, xstrdup(line));
                    if (repl_has_main(&session)) {
                        fputs("eval: the buffer already defines main; use :run or :clear\n", stderr);
                    } else {
                        repl_compose(&session, expr, &source);
                        repl_compile(&source, tmpdir, serial++, 0);
                        buf_free(&source);
                    }
                }
                continue;
            }
            if (line[0] == ':') {
                fprintf(stderr, "kaze repl: unknown command %s\n", line);
                continue;
            }
        }
        if (line[0] != 0) {
            buf_appends(&pending, line);
            buf_appendc(&pending, '\n');
            vec_push_owned(&history, xstrdup(line));
        } else if (pending.len > 0) {
            if (session.len > 0 && session.data[session.len - 1] != '\n') buf_appendc(&session, '\n');
            buf_append(&session, pending.data, pending.len);
            pending.len = 0;
        }
    }
    vec_free(&history);
    buf_free(&pending);
    buf_free(&session);
    if (interactive) fputc('\n', stderr);
    return KAZE_EXIT_OK;
}

/* ------------------------------------------------------------------ */
/* LSP                                                                 */
/* ------------------------------------------------------------------ */

static char lsp_msg[1 << 20];
static char lsp_doc[1 << 20];
static size_t lsp_doc_len = 0;
static char lsp_src_path[PATH_MAX];

static int lsp_write(const char *body) {
    size_t n = strlen(body);
    printf("Content-Length: %lu\r\n\r\n%s", (unsigned long)n, body);
    fflush(stdout);
    return 0;
}

static int json_string(const char *src, const char *key, char *out, size_t cap) {
    const char *p = strstr(src, key);
    size_t n = 0;
    if (p == NULL) return 0;
    p = strchr(p + strlen(key), ':');
    if (p == NULL) return 0;
    p++;
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
    if (*p != '"') return 0;
    p++;
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

static int json_id_token(const char *src, char *out, size_t cap) {
    int depth = 0;
    int in_str = 0;
    const char *p;
    if (cap < 2) return 0;
    out[0] = 0;
    for (p = src; *p != 0; p++) {
        if (in_str) {
            if (*p == '\\' && p[1] != 0) { p++; continue; }
            if (*p == '"') in_str = 0;
            continue;
        }
        if (*p == '"') {
            const char *q = p + 1;
            const char *r = q;
            while (*r != 0 && *r != '"') { if (*r == '\\' && r[1] != 0) r++; r++; }
            if (depth <= 1 && (r - q) == 2 && q[0] == 'i' && q[1] == 'd') {
                const char *v = (r != 0 && *r == '"') ? r + 1 : r;
                size_t n = 0;
                while (*v == ' ' || *v == '\t' || *v == '\n' || *v == '\r' || *v == ':') v++;
                if (*v == '"') {
                    out[n++] = *v++;
                    while (*v != 0 && *v != '"' && n + 2 < cap) out[n++] = *v++;
                    if (*v == '"') out[n++] = *v++;
                } else {
                    while (*v == '-' || *v == '+' || *v == '.' || *v == 'e' || *v == 'E' ||
                           (*v >= '0' && *v <= '9')) {
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

static void json_escape_buf(char *dst, size_t cap, const char *src) {
    size_t o = 0;
    const unsigned char *p = (const unsigned char *)(src ? src : "");
    for (; *p != 0; p++) {
        unsigned char ch = *p;
        if (o + 8 >= cap) break;
        if (ch == '"' || ch == '\\') { dst[o++] = '\\'; dst[o++] = (char)ch; }
        else if (ch == '\n') { dst[o++] = '\\'; dst[o++] = 'n'; }
        else if (ch == '\r') { dst[o++] = '\\'; dst[o++] = 'r'; }
        else if (ch == '\t') { dst[o++] = '\\'; dst[o++] = 't'; }
        else if (ch < 0x20) {
            static const char hex[] = "0123456789abcdef";
            dst[o++] = '\\'; dst[o++] = 'u'; dst[o++] = '0'; dst[o++] = '0';
            dst[o++] = hex[(ch >> 4) & 0xF]; dst[o++] = hex[ch & 0xF];
        } else dst[o++] = (char)ch;
    }
    while (o > 0 && ((unsigned char)dst[o - 1] & 0xC0) == 0x80) o--;
    dst[o] = 0;
}

static void lsp_cleanup_file(void) {
    if (lsp_src_path[0] != 0) unlink(lsp_src_path);
}

static void lsp_publish(const char *uri) {
    char *av[5];
    Buf out;
    char body[16384];
    size_t used;
    char safe_uri[1024];
    const char *p;
    int count = 0;
    if (lsp_src_path[0] == 0) {
        char tpl[PATH_MAX];
        snprintf(tpl, sizeof tpl, "%s/kaze-lsp-XXXXXX", temp_root());
        snprintf(lsp_src_path, sizeof lsp_src_path, "%s", tpl);
        {
            int fd = mkstemp(lsp_src_path);
            if (fd < 0) { lsp_src_path[0] = 0; return; }
            close(fd);
        }
        atexit(lsp_cleanup_file);
    }
    if (write_file_raw(lsp_src_path, lsp_doc, lsp_doc_len, 0644) != 0) return;
    av[0] = (char *)"kaze";
    av[1] = (char *)"check";
    av[2] = lsp_src_path;
    av[3] = NULL;
    buf_init(&out);
    run_compiler(av, &out);
    json_escape_buf(safe_uri, sizeof safe_uri, uri);
    {
        int n = snprintf(body, sizeof body,
                         "{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\","
                         "\"params\":{\"uri\":\"%s\",\"diagnostics\":[", safe_uri);
        if (n < 0 || (size_t)n >= sizeof body) { buf_free(&out); return; }
        used = (size_t)n;
    }
    p = out.data;
    while (p != NULL && *p != 0) {
        const char *e = strchr(p, '\n');
        size_t n = e ? (size_t)(e - p) : strlen(p);
        if (n > 2 && (p[0] == 'E' || p[0] == 'W') && p[1] == '_') {
            const char *at = strstr(p, " at line ");
            if (at != NULL && at < p + n) {
                int lineno = atoi(at + 9) - 1;
                char msg[2048];
                char *colon;
                size_t room;
                int n2;
                if (lineno < 0) lineno = 0;
                json_escape_buf(msg, sizeof msg, p);
                colon = NULL;
                (void)colon;
                room = sizeof body - used;
                n2 = snprintf(body + used, room,
                              "%s{\"range\":{\"start\":{\"line\":%d,\"character\":0},"
                              "\"end\":{\"line\":%d,\"character\":0}},\"severity\":1,\"message\":\"%s\"}",
                              count > 0 ? "," : "", lineno, lineno, msg);
                if (n2 < 0 || (size_t)n2 >= room) break;
                used += (size_t)n2;
                count++;
            }
        }
        if (e == NULL) break;
        p = e + 1;
    }
    if (used + 4 > sizeof body) used = sizeof body - 4;
    snprintf(body + used, sizeof body - used, "]}}");
    buf_free(&out);
    lsp_write(body);
}

static int lsp_word_at(int want_line, int want_char, char *word, size_t cap, int *decl_line) {
    int line = 0;
    size_t i = 0;
    while (i < lsp_doc_len) {
        size_t start = i;
        while (i < lsp_doc_len && lsp_doc[i] != '\n') i++;
        if (line == want_line) {
            size_t len = i - start;
            size_t c = (size_t)want_char < len ? (size_t)want_char : (len ? len - 1 : 0);
            size_t a = start + c, b = a;
            size_t n;
            while (a > start && (isalnum((unsigned char)lsp_doc[a - 1]) || lsp_doc[a - 1] == '_')) a--;
            while (b < start + len && (isalnum((unsigned char)lsp_doc[b]) || lsp_doc[b] == '_')) b++;
            n = b - a;
            if (n == 0 || n + 1 > cap) return 0;
            memcpy(word, lsp_doc + a, n);
            word[n] = 0;
            *decl_line = -1;
            {
                int ln = 0;
                size_t j = 0;
                while (j < lsp_doc_len) {
                    size_t s2 = j;
                    while (j < lsp_doc_len && lsp_doc[j] != '\n') j++;
                    {
                        const char *text = lsp_doc + s2;
                        size_t tl = j - s2;
                        if ((strncmp(text, "action ", 7) == 0 || strncmp(text, "record ", 7) == 0 ||
                             strncmp(text, "routine ", 8) == 0) &&
                            strstr(text, word) != NULL && strstr(text, word) < text + tl) {
                            *decl_line = ln;
                            break;
                        }
                    }
                    ln++;
                    j++;
                }
            }
            return 1;
        }
        line++;
        i++;
    }
    return 0;
}

static int lsp_position(const char *msg, int *line, int *ch) {
    const char *p = strstr(msg, "\"position\"");
    const char *l;
    const char *c;
    const char *lc;
    const char *cc;
    if (p == NULL) return 0;
    l = strstr(p, "\"line\"");
    c = strstr(p, "\"character\"");
    if (l == NULL || c == NULL) return 0;
    lc = strchr(l, ':');
    cc = strchr(c, ':');
    if (lc == NULL || cc == NULL) return 0;
    *line = atoi(lc + 1);
    *ch = atoi(cc + 1);
    return 1;
}

static int read_frame(void) {
    char header[512];
    for (;;) {
        if (fgets(header, sizeof header, stdin) == NULL) return 1;
        if (strncasecmp(header, "Content-Length:", 15) == 0) break;
    }
    {
        char *end = NULL;
        long len = strtol(header + 15, &end, 10);
        size_t got = 0;
        if (end == header + 15 || (*end != 0 && *end != '\r' && *end != '\n' && *end != ' ')) return 2;
        if (len < 0 || len > (long)sizeof lsp_msg - 1) return 2;
        while (fgets(header, sizeof header, stdin) != NULL) {
            if (strcmp(header, "\r\n") == 0 || strcmp(header, "\n") == 0) break;
        }
        if (len == 0) return 3;
        while (got < (size_t)len) {
            size_t n = fread(lsp_msg + got, 1, (size_t)len - got, stdin);
            if (n == 0) return 1;
            got += n;
        }
        lsp_msg[got] = 0;
    }
    return 0;
}

static int more_input(int ms) {
    struct pollfd pfd;
    pfd.fd = STDIN_FILENO;
    pfd.events = POLLIN;
    pfd.revents = 0;
    return poll(&pfd, 1, ms) > 0;
}

static int cmd_lsp(int argc, char **argv) {
    int saw_shutdown = 0;
    int pending = 0;
    char pending_uri[512];
    int i;
    pending_uri[0] = 0;
    for (i = 0; i < argc; i++) {
        if (strcmp(argv[i], "--dap") == 0) {
            fprintf(stderr, "kaze lsp: --dap is accepted; the DAP transport is planned for Phase 20\n");
        } else {
            fprintf(stderr, "kaze lsp: unknown option %s\n", argv[i]);
            return KAZE_EXIT_USAGE;
        }
    }
    for (;;) {
        if (pending && !more_input(0)) {
            lsp_publish(pending_uri);
            pending = 0;
        }
        {
            int rc = read_frame();
            char method[128];
            char id_tok[64];
            int has_id;
            if (rc == 1 || rc == 2) {
                if (pending) lsp_publish(pending_uri);
                return rc == 1 ? 0 : 1;
            }
            if (rc == 3) continue;
            if (!json_string(lsp_msg, "\"method\"", method, sizeof method)) continue;
            has_id = json_id_token(lsp_msg, id_tok, sizeof id_tok);
            if (strcmp(method, "initialize") == 0) {
                char body[512];
                snprintf(body, sizeof body,
                         "{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"capabilities\":{\"textDocumentSync\":1,\"hoverProvider\":true,\"definitionProvider\":true},\"serverInfo\":{\"name\":\"kaze-lsp\",\"version\":\"%s\"}}}",
                         id_tok, KAZE_VERSION);
                lsp_write(body);
            } else if (strcmp(method, "initialized") == 0) {
                /* notification */
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
                if (lsp_position(lsp_msg, &line, &ch) &&
                    lsp_word_at(line, ch, word, sizeof word, &decl) && decl >= 0) {
                    snprintf(body, sizeof body,
                             "{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"contents\":{\"kind\":\"markdown\",\"value\":\"`%s` is declared in this document\"}}}",
                             id_tok, word);
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
                if (lsp_position(lsp_msg, &line, &ch) &&
                    lsp_word_at(line, ch, word, sizeof word, &decl) && decl >= 0) {
                    snprintf(body, sizeof body,
                             "{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"uri\":\"%s\",\"range\":{\"start\":{\"line\":%d,\"character\":0},\"end\":{\"line\":%d,\"character\":0}}}}",
                             id_tok, uri, decl, decl);
                } else {
                    snprintf(body, sizeof body, "{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":null}", id_tok);
                }
                lsp_write(body);
            } else if (strcmp(method, "shutdown") == 0) {
                char body[256];
                saw_shutdown = 1;
                snprintf(body, sizeof body, "{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":null}", id_tok);
                lsp_write(body);
            } else if (strcmp(method, "exit") == 0) {
                if (pending) lsp_publish(pending_uri);
                return saw_shutdown ? 0 : 1;
            } else if (has_id) {
                char body[256];
                snprintf(body, sizeof body,
                         "{\"jsonrpc\":\"2.0\",\"id\":%s,\"error\":{\"code\":-32601,\"message\":\"method not found\"}}",
                         id_tok);
                lsp_write(body);
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/* main                                                                */
/* ------------------------------------------------------------------ */

int main(int argc, char **argv) {
    const char *cmd;
    int rest_argc;
    char **rest_argv;
    int rc;
    init_paths(argv[0]);
    install_cleanup();
    /* The compiler resolves `std/` from KAZE_STD_DIR and a few conventional
     * prefixes.  An arbitrary installation prefix (the isolated gate uses
     * /tmp/...) is not conventional, so publish the directory discovered from
     * the executable before the compiler child runs.  An explicit user value
     * always wins. */
    {
        const char *explicit_std = getenv("KAZE_STD_DIR");
        if (explicit_std == NULL || explicit_std[0] == 0) {
            char *std_dir = find_std_dir();
            if (std_dir != NULL) {
                setenv("KAZE_STD_DIR", std_dir, 1);
                free(std_dir);
            }
        }
    }
    {
        int argi = 1;
        while (argi < argc) {
            if (strcmp(argv[argi], "--verbose") == 0) {
                g_verbose = 1;
                argi++;
            } else if (strcmp(argv[argi], "--keep-temps") == 0) {
                g_keep_temps = 1;
                argi++;
            } else {
                break;
            }
        }
        if (argi >= argc) {
            usage(stderr);
            return KAZE_EXIT_USAGE;
        }
        cmd = argv[argi];
        rest_argc = argc - argi - 1;
        rest_argv = argv + argi + 1;
    }
    if (strcmp(cmd, "-h") == 0 || strcmp(cmd, "--help") == 0 || strcmp(cmd, "help") == 0) {
        usage(stdout);
        return KAZE_EXIT_OK;
    }
    if (strcmp(cmd, "--version") == 0 || strcmp(cmd, "-V") == 0 || strcmp(cmd, "version") == 0) {
        return cmd_version();
    }
    if (strcmp(cmd, "check") == 0) rc = cmd_check(rest_argc, rest_argv);
    else if (strcmp(cmd, "fmt") == 0) rc = cmd_fmt(rest_argc, rest_argv);
    else if (strcmp(cmd, "emit-c") == 0) rc = cmd_emit_c(rest_argc, rest_argv);
    else if (strcmp(cmd, "compile") == 0) rc = cmd_compile_like("compile", rest_argc, rest_argv);
    else if (strcmp(cmd, "to-machine") == 0) rc = cmd_compile_like("to-machine", rest_argc, rest_argv);
    else if (strcmp(cmd, "to-human") == 0) rc = cmd_compile_like("to-human", rest_argc, rest_argv);
    else if (strcmp(cmd, "emit-ir") == 0) rc = cmd_compile_like("emit-ir", rest_argc, rest_argv);
    else if (strcmp(cmd, "emit-qbe") == 0) rc = cmd_compile_like("emit-qbe", rest_argc, rest_argv);
    else if (strcmp(cmd, "module-manifest") == 0) rc = cmd_passthrough("module-manifest", rest_argc, rest_argv);
    else if (strcmp(cmd, "emit-c-unit") == 0) rc = cmd_passthrough("emit-c-unit", rest_argc, rest_argv);
    else if (strcmp(cmd, "emit-ir-c") == 0) rc = cmd_passthrough("emit-ir-c", rest_argc, rest_argv);
    else if (strcmp(cmd, "surface") == 0) {
        char *av[3];
        Buf out;
        av[0] = (char *)"kaze";
        av[1] = (char *)"surface";
        av[2] = NULL;
        buf_init(&out);
        run_compiler(av, &out);
        fwrite(out.data ? out.data : "", 1, out.len, stdout);
        buf_free(&out);
        rc = KAZE_EXIT_OK;
    }
    else if (strcmp(cmd, "verify") == 0) rc = cmd_verify(rest_argc, rest_argv);
    else if (strcmp(cmd, "run") == 0) rc = cmd_run(rest_argc, rest_argv);
    else if (strcmp(cmd, "build") == 0) rc = cmd_build(rest_argc, rest_argv);
    else if (strcmp(cmd, "test") == 0) rc = cmd_test(rest_argc, rest_argv);
    else if (strcmp(cmd, "init") == 0) rc = cmd_init(rest_argc, rest_argv);
    else if (strcmp(cmd, "pkg") == 0) rc = cmd_pkg(rest_argc, rest_argv);
    else if (strcmp(cmd, "repl") == 0) rc = cmd_repl(rest_argc, rest_argv);
    else if (strcmp(cmd, "lsp") == 0) rc = cmd_lsp(rest_argc, rest_argv);
    else if (is_known_command(cmd)) {
        usage(stderr);
        rc = KAZE_EXIT_USAGE;
    } else if (is_regular(cmd)) {
        /* Legacy: `kaze file.kaze` emits C. */
        char *av[4];
        Buf out;
        av[0] = (char *)"kaze";
        av[1] = (char *)"emit-c";
        av[2] = (char *)cmd;
        av[3] = NULL;
        buf_init(&out);
        run_compiler(av, &out);
        fwrite(out.data ? out.data : "", 1, out.len, stdout);
        buf_free(&out);
        rc = KAZE_EXIT_OK;
    } else {
        fprintf(stderr, "kaze: unknown command or file: %s\n", cmd);
        usage(stderr);
        rc = KAZE_EXIT_USAGE;
    }
    return rc;
}
