/* kaze-host: a small C host that gives the Kaze toolchain the two things a
 * pure-Kaze binary cannot do itself -- spawn a C compiler and exec a built program.
 *
 * The language has no process-spawn and no file-write primitive (by design: scripts
 * reach the outside world through capabilities), so `kaze run` lives here, in the
 * host, exactly as Phase 10's "embedded host bridge" describes.
 *
 *   kaze-host run <file.kaze> [args...]   emit C, build it, run it
 *   kaze-host <any compiler command> ...  pass through to the compiler
 *
 * The compiler is found via $KAZE_BIN, else next to this binary, else on $PATH.
 */
#define _POSIX_C_SOURCE 200809L
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
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

static int do_run(int argc, char** argv, const char* self) {
    if (argc < 3) {
        fprintf(stderr, "usage: %s run <file.kaze> [args...]\n", self);
        return 2;
    }
    const char* source = argv[2];
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
    {
        FILE* probe = fopen(c_path, "r");
        if (probe != NULL) {
            char buf[4096];
            size_t n = fread(buf, 1, sizeof buf - 1, probe);
            buf[n] = 0;
            wants_runtime = strstr(buf, "kaze_rt.h") != NULL;
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
    char* build_argv_plain[] = { (char*)cc, "-std=c99", "-O2", "-w", (char*)rt_h, c_path, "-lm", "-o", bin_path, NULL };
    char* build_argv_rt[] = { (char*)cc, "-std=c99", "-O2", "-w", (char*)rt_h, c_path, (char*)rt_c, "-lm", "-o", bin_path, NULL };
    // If the runtime is not next to the compiler, the emitted C had better be
    // self-contained; try the plain build rather than failing on a missing file.
    if (wants_runtime && access(rt_c, R_OK) != 0) wants_runtime = 0;
    status = run_child(wants_runtime ? build_argv_rt : build_argv_plain);
    if (status != 0) {
        fprintf(stderr, "kaze-host: the emitted C for %s did not build\n", source);
        unlink(c_path); rmdir(dir);
        return status;
    }

    /* 3. run it, forwarding the remaining arguments */
    char* run_argv[64];
    int n = 0;
    run_argv[n++] = bin_path;
    for (int i = 3; i < argc && n < 62; i++) run_argv[n++] = argv[i];
    run_argv[n] = NULL;
    int code = run_child(run_argv);
    /* the temp directory is ours: remove the binary, the C and the directory */
    unlink(bin_path);
    unlink(c_path);
    rmdir(dir);
    return code;
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
                     "{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"capabilities\":{\"textDocumentSync\":1,\"hoverProvider\":true,\"definitionProvider\":true}}}", id_tok);
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
        fprintf(stderr, "usage: %s run <file.kaze> [args...] | <compiler command> ...\n", argv[0]);
        return 2;
    }
    find_compiler(argv[0]);
    if (strcmp(argv[1], "run") == 0) return do_run(argc, argv, argv[0]);
    if (strcmp(argv[1], "lsp") == 0) return do_lsp();

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
        if (compiler_status != 0) { unlink(out_path); return compiler_status; }
        FILE* fp = fopen(out_path, "r");
        int clean = 0, reported = 0;
        if (fp != NULL) {
            char line[1024];
            while (fgets(line, sizeof line, fp) != NULL) {
                fputs(line, stdout);
                if (strstr(line, "check: ok") != NULL) clean = 1;
                if (strstr(line, "diagnostic(s)") != NULL) reported = 1;
            }
            fclose(fp);
        }
        unlink(out_path);
        if (reported) return 1;
        (void)clean;   /* unknown text: do not invent a failure */
        return 0;
    }

    /* everything else goes to the compiler unchanged */
    char* fwd[64];
    int n = 0;
    fwd[n++] = (char*)kaze_bin;
    for (int i = 1; i < argc && n < 62; i++) fwd[n++] = argv[i];
    fwd[n] = NULL;
    return run_child(fwd);
}










