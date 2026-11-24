/* kaze_http.h -- the opt-in HTTP backend for the pure-Kaze C emitter.
 *
 * Included at the end of the emitted program's prelude, so it can use KzValue
 * and the arena helpers. The compiler knows one builtin:
 *
 *   http_fetch(method, url, headers, body, timeout_ms)
 *
 * It returns the five-item list
 *
 *   [ok, status, response_headers, response_body, error]
 *
 * `ok` is false only for a transport or policy failure; an HTTP 404 is a
 * successful exchange with status 404, because the status is data.
 *
 * Authority is explicit and host-side:
 *   KAZE_HTTP=1                 required; without it every call is refused
 *   KAZE_HTTP_ALLOW=host[:port],...  destination allowlist (default: loopback)
 *   KAZE_HTTP_MAX_BYTES=N       response body ceiling (default 1 MiB, max 16 MiB)
 *   KAZE_HTTP_CAINFO=path       extra CA bundle for a private test server
 *
 * TLS uses the platform libcurl trust store with peer and hostname
 * verification on. Redirects are followed at most five times, and each
 * redirect target is checked against the same allowlist before it is
 * requested; a relative Location stays on the current origin. A 200 response
 * with a Location header is not a redirect. There is no post-resolution IP
 * check, so the allowlist matches the URL host as written, not the address DNS
 * returns. That limitation is deliberate: the host owns DNS and the policy is
 * documented rather than guessed.
 *
 * Text is NUL-terminated in this runtime, so a response body containing a NUL
 * byte is truncated at the first NUL. The cap is enforced on the wire bytes.
 *
 * Link with -lcurl when a program calls http_fetch. Programs that never call
 * it do not need the library (the static inline is discarded), and a build
 * without <curl/curl.h> still compiles: the function returns E_HTTP_BACKEND.
 */
#ifndef KAZE_HTTP_H
#define KAZE_HTTP_H

#if defined(KAZE_HTTP_NO_CURL)
/* A forced backend-less build, used by scripts/http-smoke.py to exercise the
 * unavailable-backend path on a machine that does have libcurl. */
#elif defined(__has_include)
#  if __has_include(<curl/curl.h>)
#    include <curl/curl.h>
#    define KZ_HTTP_HAVE_CURL 1
#    include <sys/types.h>
#    include <sys/socket.h>
#    include <netdb.h>
#    include <arpa/inet.h>
#    include <netinet/in.h>
#  endif
#endif

typedef struct {
    char* data;
    size_t len;
    size_t cap;
    size_t limit;
    int overflow;
} kz_http_buf;

KZ_MAYBE_UNUSED static inline int kz_http_buf_add(kz_http_buf* b, const char* p, size_t n) {
    if (b->overflow) return 0;
    if (n > b->limit || b->len + n > b->limit) {
        b->overflow = 1;
        return 0;
    }
    if (b->len + n + 1 > b->cap) {
        size_t cap = b->cap ? b->cap : 256;
        while (cap < b->len + n + 1) cap *= 2;
        if (cap > b->limit + 1) cap = b->limit + 1;
        char* next = (char*)realloc(b->data, cap);
        if (next == NULL) {
            b->overflow = 1;
            return 0;
        }
        b->data = next;
        b->cap = cap;
    }
    memcpy(b->data + b->len, p, n);
    b->len += n;
    b->data[b->len] = 0;
    return 1;
}

KZ_MAYBE_UNUSED static inline KzValue kz_http_result(int ok, long status,
                                                     const char* headers,
                                                     const char* body,
                                                     const char* error) {
    KzValue items[5];
    items[0] = kz_bool(ok != 0);
    items[1] = kz_num((double)status);
    items[2] = kz_str(kz_str_alloc_copy(headers ? headers : ""));
    items[3] = kz_str(kz_str_alloc_copy(body ? body : ""));
    items[4] = kz_str(kz_str_alloc_copy(error ? error : ""));
    return kz_list_from(5, items);
}

KZ_MAYBE_UNUSED static inline const char* kz_http_text(KzValue v) {
    return kz_to_string(v);
}

#ifdef KZ_HTTP_HAVE_CURL

KZ_MAYBE_UNUSED static size_t kz_http_on_body(char* p, size_t size, size_t nmemb, void* user) {
    kz_http_buf* b = (kz_http_buf*)user;
    size_t n = size * nmemb;
    if (!kz_http_buf_add(b, p, n)) return 0;
    return n;
}

KZ_MAYBE_UNUSED static inline int kz_http_allowed(const char* host);
KZ_MAYBE_UNUSED static inline int kz_http_host_of(const char* url, char* out, size_t cap);

KZ_MAYBE_UNUSED static int kz_http_header_name_is(const char* p, size_t n, const char* want) {
    size_t i, wl = strlen(want);
    if (n < wl) return 0;
    for (i = 0; i < wl; i++) {
        char c = p[i];
        if (c >= 'A' && c <= 'Z') c = (char)(c + 32);
        if (c != want[i]) return 0;
    }
    return 1;
}

/* A redirect must stay inside the same grant. This runs inside the header
 * callback, before libcurl follows it, so a denied target is never requested.
 * A relative Location stays on the current origin and is allowed. */
KZ_MAYBE_UNUSED static int kz_http_redirect_ok(const char* value, char* host, size_t cap) {
    char tmp[2048];
    const char* v = value;
    while (*v == ' ' || *v == '\t') v++;
    if (*v == 0) return 1;
    if (strncmp(v, "http://", 7) == 0 || strncmp(v, "https://", 8) == 0) {
        return kz_http_host_of(v, host, cap) && kz_http_allowed(host);
    }
    if (v[0] == '/' && v[1] == '/') {
        if (snprintf(tmp, sizeof tmp, "http:%s", v) >= (int)sizeof tmp) return 0;
        return kz_http_host_of(tmp, host, cap) && kz_http_allowed(host);
    }
    return 1;
}

typedef struct {
    kz_http_buf* buf;
    int denied;
    int status;
    char host[512];
} kz_http_header_ctx;

KZ_MAYBE_UNUSED static size_t kz_http_on_header(char* p, size_t size, size_t nmemb, void* user) {
    kz_http_header_ctx* ctx = (kz_http_header_ctx*)user;
    size_t n = size * nmemb;
    if (!kz_http_buf_add(ctx->buf, p, n)) return 0;
    if (n > 8 && strncmp(p, "HTTP/", 5) == 0) {
        const char* sp = (const char*)memchr(p, ' ', n);
        if (sp != NULL) {
            int code = 0, digits = 0;
            const char* q = sp + 1;
            while (digits < 3 && q + digits < p + n && q[digits] >= '0' && q[digits] <= '9') {
                code = code * 10 + (q[digits] - '0');
                digits++;
            }
            if (digits == 3) ctx->status = code;
        }
    }
    if (ctx->status >= 301 && ctx->status <= 308 && ctx->status != 304 &&
        ctx->status != 305 && ctx->status != 306 &&
        n > 9 && kz_http_header_name_is(p, n, "location")) {
        size_t start = 9, end = n;
        char* loc;
        while (start < end && (p[start] == ' ' || p[start] == '\t')) start++;
        while (end > start && (p[end - 1] == '\r' || p[end - 1] == '\n' ||
                               p[end - 1] == ' ' || p[end - 1] == '\t')) end--;
        loc = (char*)malloc(end - start + 1);
        if (loc == NULL) {
            ctx->denied = 1;
            return 0;
        }
        memcpy(loc, p + start, end - start);
        loc[end - start] = 0;
        if (!kz_http_redirect_ok(loc, ctx->host, sizeof ctx->host)) ctx->denied = 1;
        free(loc);
        if (ctx->denied) return 0;
    }
    return n;
}

/* The allowlist is an explicit grant, not a hint. An empty or unset value
 * means loopback only, so a test cannot accidentally reach the public network. */
KZ_MAYBE_UNUSED static inline int kz_http_allowed(const char* host) {
    const char* env = getenv("KAZE_HTTP_ALLOW");
    const char* list = (env != NULL && env[0] != 0) ? env : "127.0.0.1,localhost,::1,[::1]";
    const char* p = list;
    while (*p != 0) {
        const char* comma = strchr(p, ',');
        size_t n = comma ? (size_t)(comma - p) : strlen(p);
        while (n > 0 && (p[n - 1] == ' ' || p[n - 1] == '\t')) n--;
        while (*p == ' ' || *p == '\t') { p++; if (n > 0) n--; }
        if (n > 0) {
            size_t h = strlen(host);
            if (h == n && strncmp(p, host, n) == 0) return 1;
            /* "host" in the list also admits "host:port" in the URL, but
             * "host:port" in the list must match that port, not any port. */
            if (h > n && strncmp(p, host, n) == 0 && host[n] == ':') {
                int entry_has_port = 0;
                if (p[0] == '[') {
                    size_t i;
                    for (i = 0; i < n; i++) if (p[i] == ']') break;
                    entry_has_port = (i + 1 < n);
                } else {
                    entry_has_port = (memchr(p, ':', n) != NULL);
                }
                if (!entry_has_port) return 1;
            }
        }
        if (comma == NULL) break;
        p = comma + 1;
    }
    return 0;
}

KZ_MAYBE_UNUSED static inline int kz_http_host_of(const char* url, char* out, size_t cap) {
    const char* p = strstr(url, "://");
    const char* q;
    const char* at;
    size_t n;
    if (p == NULL) return 0;
    p += 3;
    q = p;
    while (*q != 0 && *q != '/' && *q != '?' && *q != '#') q++;
    n = (size_t)(q - p);
    if (n == 0 || n >= cap) return 0;
    memcpy(out, p, n);
    out[n] = 0;
    at = strrchr(out, '@');
    if (at != NULL) {
        size_t keep = strlen(at + 1);
        memmove(out, at + 1, keep + 1);
    }
    return 1;
}

KZ_MAYBE_UNUSED static int kz_http_is_private_ip(const char* ip) {
    unsigned int a, b, c, d;
    if (ip == NULL || ip[0] == 0) return 0;
    if (strcmp(ip, "::1") == 0 || strcmp(ip, "0:0:0:0:0:0:0:1") == 0) return 1;
    if (strncmp(ip, "fc", 2) == 0 || strncmp(ip, "fd", 2) == 0 ||
        strncmp(ip, "FC", 2) == 0 || strncmp(ip, "FD", 2) == 0 ||
        strncmp(ip, "fe80", 4) == 0 || strncmp(ip, "FE80", 4) == 0) return 1;
    if (sscanf(ip, "%u.%u.%u.%u", &a, &b, &c, &d) == 4) {
        if (a == 127) return 1;                         /* 127.0.0.0/8 loopback */
        if (a == 10) return 1;                          /* 10.0.0.0/8 private */
        if (a == 172 && (b >= 16 && b <= 31)) return 1; /* 172.16.0.0/12 private */
        if (a == 192 && b == 168) return 1;             /* 192.168.0.0/16 private */
        if (a == 169 && b == 254) return 1;             /* 169.254.0.0/16 link-local */
        if (a == 0) return 1;                           /* 0.0.0.0/8 */
    }
    return 0;
}

KZ_MAYBE_UNUSED static int kz_http_ip_port_allowed(const char* ip, const char* port) {
    char ip_with_port[256];
    const char* allow_priv = getenv("KAZE_HTTP_ALLOW_PRIVATE");
    if (allow_priv != NULL && strcmp(allow_priv, "1") == 0) return 1;
    if (!kz_http_is_private_ip(ip)) return 1;
    if (port != NULL && port[0] != 0) {
        snprintf(ip_with_port, sizeof ip_with_port, "%s:%s", ip, port);
        if (kz_http_allowed(ip_with_port)) return 1;
    }
    return kz_http_allowed(ip);
}

KZ_MAYBE_UNUSED static int kz_http_check_host_dns(const char* host_str, char* err_ip, size_t err_ip_cap) {
    char bare[256];
    char port_buf[32] = {0};
    size_t len;
    struct addrinfo hints, *res = NULL, *p;
    const char* allow_priv = getenv("KAZE_HTTP_ALLOW_PRIVATE");
    if (allow_priv != NULL && strcmp(allow_priv, "1") == 0) return 1;

    len = strlen(host_str);
    if (len >= sizeof bare) len = sizeof(bare) - 1;
    memcpy(bare, host_str, len);
    bare[len] = 0;

    if (bare[0] == '[') {
        char* bracket = strchr(bare, ']');
        if (bracket != NULL) {
            *bracket = 0;
            if (bracket[1] == ':') {
                snprintf(port_buf, sizeof port_buf, "%s", bracket + 2);
            }
            memmove(bare, bare + 1, strlen(bare));
        }
    } else {
        char* colon = strchr(bare, ':');
        if (colon != NULL) {
            *colon = 0;
            snprintf(port_buf, sizeof port_buf, "%s", colon + 1);
        }
    }

    if (kz_http_is_private_ip(bare)) {
        if (!kz_http_ip_port_allowed(bare, port_buf)) {
            snprintf(err_ip, err_ip_cap, "%s", bare);
            return 0;
        }
        return 1;
    }

    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    if (getaddrinfo(bare, NULL, &hints, &res) != 0 || res == NULL) {
        return 1;
    }
    for (p = res; p != NULL; p = p->ai_next) {
        char ip_str[INET6_ADDRSTRLEN] = {0};
        if (p->ai_family == AF_INET) {
            struct sockaddr_in* s = (struct sockaddr_in*)p->ai_addr;
            inet_ntop(AF_INET, &s->sin_addr, ip_str, sizeof ip_str);
        } else if (p->ai_family == AF_INET6) {
            struct sockaddr_in6* s = (struct sockaddr_in6*)p->ai_addr;
            inet_ntop(AF_INET6, &s->sin6_addr, ip_str, sizeof ip_str);
        }
        if (ip_str[0] != 0 && kz_http_is_private_ip(ip_str)) {
            if (!kz_http_ip_port_allowed(ip_str, port_buf)) {
                snprintf(err_ip, err_ip_cap, "%s", ip_str);
                freeaddrinfo(res);
                return 0;
            }
        }
    }
    freeaddrinfo(res);
    return 1;
}

KZ_MAYBE_UNUSED static inline const char* kz_http_error(CURLcode rc, int too_large) {
    if (too_large) return "E_HTTP_TOO_LARGE: response exceeds KAZE_HTTP_MAX_BYTES";
    switch (rc) {
        case CURLE_OPERATION_TIMEDOUT: return "E_HTTP_TIMEOUT: the request exceeded its deadline";
        case CURLE_COULDNT_RESOLVE_HOST: return "E_HTTP_DNS: the host did not resolve";
        case CURLE_COULDNT_CONNECT: return "E_HTTP_CONNECT: the connection was refused";
        case CURLE_SSL_CONNECT_ERROR:
        case CURLE_PEER_FAILED_VERIFICATION: return "E_HTTP_TLS: certificate or hostname verification failed";
        case CURLE_TOO_MANY_REDIRECTS: return "E_HTTP_REDIRECT: more than five redirects";
        case CURLE_UNSUPPORTED_PROTOCOL: return "E_HTTP_SCHEME: only http and https are supported";
        default: return "E_HTTP: the request failed";
    }
}

KZ_MAYBE_UNUSED static inline KzValue kz_http_request(KzValue method_v, KzValue url_v,
                                                       KzValue headers_v, KzValue body_v,
                                                       KzValue timeout_v) {
    CURL* curl;
    CURLcode rc;
    struct curl_slist* headers = NULL;
    kz_http_buf body = {0}, response = {0}, response_headers = {0};
    kz_http_header_ctx hctx = {0};
    const char* method = kz_http_text(method_v);
    const char* url = kz_http_text(url_v);
    const char* header_text = kz_http_text(headers_v);
    const char* body_text = kz_http_text(body_v);
    long timeout = (long)(timeout_v.kind == KZ_VAL_NUM ? timeout_v.as.number : 5000);
    long status = 0;
    char host[512];
    char error[256];
    size_t max_bytes = 1024 * 1024;
    const char* env_max = getenv("KAZE_HTTP_MAX_BYTES");
    int too_large = 0;
    KzValue result;

    {
        const char* grant = getenv("KAZE_HTTP");
        if (grant == NULL || strcmp(grant, "1") != 0) {
            return kz_http_result(0, 0, "", "", "E_HTTP_DENIED: set KAZE_HTTP=1 to grant network access");
        }
    }
    if (!kz_http_host_of(url, host, sizeof host)) {
        return kz_http_result(0, 0, "", "", "E_HTTP_URL: expected an absolute http(s) URL");
    }
    if (!kz_http_allowed(host)) {
        snprintf(error, sizeof error, "E_HTTP_DENIED: %s is not in KAZE_HTTP_ALLOW", host);
        return kz_http_result(0, 0, "", "", error);
    }
    {
        char bad_ip[64] = {0};
        if (!kz_http_check_host_dns(host, bad_ip, sizeof bad_ip)) {
            snprintf(error, sizeof error, "E_HTTP_DENIED: %s resolves to prohibited private IP %s", host, bad_ip);
            return kz_http_result(0, 0, "", "", error);
        }
    }
    if (strncmp(url, "http://", 7) != 0 && strncmp(url, "https://", 8) != 0) {
        return kz_http_result(0, 0, "", "", "E_HTTP_SCHEME: only http and https are supported");
    }
    if (strcmp(method, "GET") != 0 && strcmp(method, "POST") != 0 &&
        strcmp(method, "PUT") != 0 && strcmp(method, "PATCH") != 0 &&
        strcmp(method, "DELETE") != 0 && strcmp(method, "HEAD") != 0) {
        return kz_http_result(0, 0, "", "", "E_HTTP_METHOD: use GET, POST, PUT, PATCH, DELETE or HEAD");
    }
    if (env_max != NULL && env_max[0] != 0) {
        long n = strtol(env_max, NULL, 10);
        if (n > 0) max_bytes = (size_t)n;
    }
    if (max_bytes > 16 * 1024 * 1024) max_bytes = 16 * 1024 * 1024;
    if (timeout <= 0) timeout = 5000;
    if (timeout > 60000) timeout = 60000;

    if (strlen(header_text) > 0) {
        const char* line = header_text;
        while (*line != 0) {
            const char* nl = strchr(line, '\n');
            size_t n = nl ? (size_t)(nl - line) : strlen(line);
            if (n > 0) {
                char* one = (char*)malloc(n + 1);
                if (one == NULL) break;
                memcpy(one, line, n);
                one[n] = 0;
                if (n > 0 && one[n - 1] == '\r') one[n - 1] = 0;
                headers = curl_slist_append(headers, one);
                free(one);
            }
            if (nl == NULL) break;
            line = nl + 1;
        }
    }

    curl = curl_easy_init();
    if (curl == NULL) {
        curl_slist_free_all(headers);
        return kz_http_result(0, 0, "", "", "E_HTTP_BACKEND: curl_easy_init failed");
    }
    body.limit = max_bytes > 64 * 1024 ? 64 * 1024 : max_bytes;
    response.limit = max_bytes;
    response_headers.limit = 64 * 1024;

    curl_easy_setopt(curl, CURLOPT_URL, url);
    curl_easy_setopt(curl, CURLOPT_CUSTOMREQUEST, method);
    curl_easy_setopt(curl, CURLOPT_NOSIGNAL, 1L);
    curl_easy_setopt(curl, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(curl, CURLOPT_MAXREDIRS, 5L);
    curl_easy_setopt(curl, CURLOPT_PROTOCOLS, (long)(CURLPROTO_HTTP | CURLPROTO_HTTPS));
    curl_easy_setopt(curl, CURLOPT_SSL_VERIFYPEER, 1L);
    curl_easy_setopt(curl, CURLOPT_SSL_VERIFYHOST, 2L);
    {
        const char* cainfo = getenv("KAZE_HTTP_CAINFO");
        if (cainfo != NULL && cainfo[0] != 0) curl_easy_setopt(curl, CURLOPT_CAINFO, cainfo);
    }
    curl_easy_setopt(curl, CURLOPT_CONNECTTIMEOUT_MS, timeout);
    curl_easy_setopt(curl, CURLOPT_TIMEOUT_MS, timeout);
    curl_easy_setopt(curl, CURLOPT_USERAGENT, "kaze/1.0");
    curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, kz_http_on_body);
    curl_easy_setopt(curl, CURLOPT_WRITEDATA, &response);
    hctx.buf = &response_headers;
    curl_easy_setopt(curl, CURLOPT_HEADERFUNCTION, kz_http_on_header);
    curl_easy_setopt(curl, CURLOPT_HEADERDATA, &hctx);
    if (headers != NULL) curl_easy_setopt(curl, CURLOPT_HTTPHEADER, headers);
    if (strcmp(method, "HEAD") == 0) {
        curl_easy_setopt(curl, CURLOPT_NOBODY, 1L);
    } else if (body_text[0] != 0) {
        curl_easy_setopt(curl, CURLOPT_POSTFIELDS, body_text);
        curl_easy_setopt(curl, CURLOPT_POSTFIELDSIZE, (long)strlen(body_text));
    }

    rc = curl_easy_perform(curl);
    {
        char* primary_ip = NULL;
        curl_easy_getinfo(curl, CURLINFO_PRIMARY_IP, &primary_ip);
        if (primary_ip != NULL && primary_ip[0] != 0) {
            char port_str[32] = {0};
            char* col = strchr(host, ':');
            if (col != NULL) snprintf(port_str, sizeof port_str, "%s", col + 1);
            if (kz_http_is_private_ip(primary_ip) && !kz_http_ip_port_allowed(primary_ip, port_str)) {
                curl_easy_cleanup(curl);
                curl_slist_free_all(headers);
                free(response.data);
                free(response_headers.data);
                snprintf(error, sizeof error, "E_HTTP_DENIED: connected to prohibited private IP %s", primary_ip);
                return kz_http_result(0, 0, "", "", error);
            }
        }
    }
    too_large = response.overflow || response_headers.overflow;
    curl_easy_getinfo(curl, CURLINFO_RESPONSE_CODE, &status);
    curl_easy_cleanup(curl);
    curl_slist_free_all(headers);

    if (hctx.denied) {
        snprintf(error, sizeof error, "E_HTTP_DENIED: redirect to %s is not in KAZE_HTTP_ALLOW",
                 hctx.host[0] ? hctx.host : "another host");
        result = kz_http_result(0, status, "", "", error);
        free(response.data);
        free(response_headers.data);
        return result;
    }
    if (rc != CURLE_OK || too_large) {
        snprintf(error, sizeof error, "%s", kz_http_error(rc, too_large));
        result = kz_http_result(0, status, response_headers.data ? response_headers.data : "",
                                response.data ? response.data : "", error);
    } else {
        result = kz_http_result(1, status, response_headers.data ? response_headers.data : "",
                                response.data ? response.data : "", "");
    }
    free(response.data);
    free(response_headers.data);
    return result;
}

#else

KZ_MAYBE_UNUSED static inline KzValue kz_http_request(KzValue method_v, KzValue url_v,
                                                       KzValue headers_v, KzValue body_v,
                                                       KzValue timeout_v) {
    (void)method_v; (void)url_v; (void)headers_v; (void)body_v; (void)timeout_v;
    return kz_http_result(0, 0, "", "",
        "E_HTTP_BACKEND: this build has no <curl/curl.h>; install libcurl and rebuild");
}

#endif

#endif /* KAZE_HTTP_H */
