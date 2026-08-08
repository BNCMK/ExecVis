// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: execviz.h
//  script_path: execviz-c/execviz.h
//  module_name: execviz
//  version: 0.53.1
//  description: execviz capture for native code.
//  kind: module
//  spec: internal
//  internal_dependencies: execviz.h
//  external_dependencies: in.h, inet.h, socket.h, stddef.h, stdint.h, stdio.h, stdlib.h, string.h, time.h, unistd.h
//  features: execviz, capture
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

/*
 * execviz capture for native code.
 *
 * C, C++ and Rust have no managed runtime to interpose on, because the
 * syscall stream was built first. What this header adds is SEMANTICS: the
 * syscall layer sees a write; only the program knows it was a checkout.
 *
 * The interface is three calls on purpose. Anything larger would not survive
 * contact with three languages and every build system they use.
 *
 *   #define EXECVIZ_IMPLEMENTATION
 *   #include "execviz.h"
 *
 *   execviz_init("http://127.0.0.1:8900", "svc-1", "orders");
 *   uint64_t s = execviz_begin("charge", "io", 0);
 *   ...
 *   execviz_end(s, EXECVIZ_OK);
 *   execviz_log("info", "charged");        // attributed to the running span
 *   execviz_lifecycle(q, "claimed");
 *   execviz_link(join, child);             // fan-in
 *   execviz_flush();
 *
 * Single header, no dependencies beyond libc, and no threads of its own: a
 * capture layer that starts a thread inside a program it is measuring has
 * changed the thing it observes.
 */
#ifndef EXECVIZ_H

// ========================================================================
// CONSTANTS
// ========================================================================
#define EXECVIZ_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define EXECVIZ_OK    0
#define EXECVIZ_ERROR 1

/* A handle, not a pointer: the caller must not be able to reach inside. */

// ========================================================================
// TYPES
// ========================================================================
typedef uint64_t execviz_span;

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

void         execviz_init(const char *collector, const char *host, const char *domain);

execviz_span execviz_begin(const char *name, const char *kind, execviz_span parent);

void         execviz_end(execviz_span s, int status);
/* the message is copied immediately, never referenced */

void         execviz_fail(execviz_span s, const char *message);

void         execviz_link(execviz_span join, execviz_span child);

void         execviz_lifecycle(execviz_span s, const char *type);

void         execviz_log(const char *level, const char *message);

int          execviz_flush(void);
/* the innermost span on this thread, so a caller need not thread handles through */

execviz_span execviz_current(void);

#ifdef __cplusplus
}
#endif

#ifdef EXECVIZ_IMPLEMENTATION

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>

// ========================================================================
// CONSTANTS
// ========================================================================

#define EXECVIZ_MAX_SPANS 4096
#define EXECVIZ_NAME_MAX  96
#define EXECVIZ_MSG_MAX   200
#define EXECVIZ_DEPTH_MAX 64
/* Fan-in, lifecycle and log lines are bounded per span, in keeping with the
 * allocation-free character of this header: an adapter that mallocs inside the
 * program it observes changes that program's behaviour under memory pressure,
 * which is when someone is watching. */
#define EXECVIZ_LINKS_MAX 8
#define EXECVIZ_LIFE_MAX  8
#define EXECVIZ_EVENTS_MAX 16
#define EXECVIZ_LEVEL_MAX 12

// ========================================================================
// TYPES
// ========================================================================

typedef struct {
    char     id[13];
    char     parent[13];
    char     name[EXECVIZ_NAME_MAX];
    char     kind[16];
    char     msg[EXECVIZ_MSG_MAX];
    double   start;
    double   end;          /* < 0 means still open, which is an unfinished span */
    int      status;
    int      used;
    int      sent_phase;   /* 0 nothing, 1 opened, 2 completed */

    /* A join names every child it waited for: parenting it to one
     * child would place it outside its own parent in time. */
    char     links[EXECVIZ_LINKS_MAX][13];
    int      nlinks;

    /* claimed / released / suspended: the transitions a reader needs to tell a
     * queue that was picked up from one that was abandoned. */
    struct { double t; char type[16]; } life[EXECVIZ_LIFE_MAX];
    int      nlife;

    /* A log line belongs to the span that was running when it was written
     *, rather than to a second stream no reader can correlate. */
    struct { double t; char level[EXECVIZ_LEVEL_MAX]; char msg[EXECVIZ_MSG_MAX]; }
             events[EXECVIZ_EVENTS_MAX];
    int      nevents;
    int      events_dropped;   /* stated, never silent */
} execviz_rec;

// ========================================================================
// INTERNALS
// ========================================================================

static execviz_rec  ev_spans[EXECVIZ_MAX_SPANS];
static int          ev_count = 0;
static char         ev_collector[256] = {0};
static char         ev_host[64] = "native";
static char         ev_domain[64] = "native";
static char         ev_trace[13] = {0};
/* one stack per thread: a frame stack is valid here because C has no await */
static __thread execviz_span ev_stack[EXECVIZ_DEPTH_MAX];
static __thread int ev_depth = 0;

static double ev_now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}

static void ev_id(char *out) {
    static uint64_t seq = 0;
    uint64_t x = (uint64_t)ev_now() ^ ((uint64_t)getpid() << 20) ^ (++seq * 0x9E3779B97F4A7C15ULL);
    for (int i = 0; i < 12; i++) { out[i] = "0123456789abcdef"[x & 0xF]; x >>= 4; }
    out[12] = 0;
}

static void ev_copy(char *dst, const char *src, size_t n) {
    if (!src) { dst[0] = 0; return; }
    strncpy(dst, src, n - 1);
    dst[n - 1] = 0;
    /* a quote or backslash in a name would break the payload, so they go */
    for (size_t i = 0; dst[i]; i++)
        if (dst[i] == '"' || dst[i] == '\\' || dst[i] == '\n') dst[i] = ' ';
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

void execviz_init(const char *collector, const char *host, const char *domain) {
    if (collector) ev_copy(ev_collector, collector, sizeof ev_collector);
    if (host)      ev_copy(ev_host, host, sizeof ev_host);
    if (domain)    ev_copy(ev_domain, domain, sizeof ev_domain);
    ev_id(ev_trace);
    ev_count = 0;
    ev_depth = 0;
}

execviz_span execviz_begin(const char *name, const char *kind, execviz_span parent) {
    if (ev_count >= EXECVIZ_MAX_SPANS) return 0;   /* bounded: never grow into the program's memory */
    execviz_rec *r = &ev_spans[ev_count];
    memset(r, 0, sizeof *r);
    ev_id(r->id);
    ev_copy(r->name, name, sizeof r->name);
    ev_copy(r->kind, kind ? kind : "call", sizeof r->kind);
    r->start = ev_now();
    r->end = -1.0;
    r->status = -1;
    r->used = 1;
    execviz_span p = parent ? parent : execviz_current();
    if (p) {
        execviz_rec *pr = &ev_spans[p - 1];
        memcpy(r->parent, pr->id, sizeof r->parent);
    }
    ev_count++;
    execviz_span h = (execviz_span)ev_count;   /* 1-based so 0 means none */
    if (ev_depth < EXECVIZ_DEPTH_MAX) ev_stack[ev_depth++] = h;
    return h;
}

execviz_span execviz_current(void) {
    return ev_depth > 0 ? ev_stack[ev_depth - 1] : 0;
}

void execviz_end(execviz_span s, int status) {
    if (!s || s > (execviz_span)ev_count) return;
    execviz_rec *r = &ev_spans[s - 1];
    r->end = ev_now();
    r->status = status;
    for (int i = ev_depth - 1; i >= 0; i--) {
        if (ev_stack[i] == s) {
            for (int j = i; j < ev_depth - 1; j++) ev_stack[j] = ev_stack[j + 1];
            ev_depth--;
            break;
        }
    }
}

void execviz_fail(execviz_span s, const char *message) {
    if (!s || s > (execviz_span)ev_count) return;
    /* copied now: a buffer freed after the call must not rewrite history */
    ev_copy(ev_spans[s - 1].msg, message, EXECVIZ_MSG_MAX);
    execviz_end(s, EXECVIZ_ERROR);
}

/* A join names the children it waited for. The join keeps its own parent: a

 * fan-in parented to one of its children would sit outside that child in time
 *. */
void execviz_link(execviz_span join, execviz_span child) {
    if (!join || join > (execviz_span)ev_count) return;
    if (!child || child > (execviz_span)ev_count) return;
    execviz_rec *j = &ev_spans[join - 1];
    if (j->nlinks >= EXECVIZ_LINKS_MAX) return;   /* bounded, like everything here */
    ev_copy(j->links[j->nlinks], ev_spans[child - 1].id, 13);
    j->nlinks++;
    j->sent_phase = 0;                            /* re-sent, so the link lands */
}

/* A transition worth recording: claimed, released, suspended. */

void execviz_lifecycle(execviz_span s, const char *type) {
    if (!s || s > (execviz_span)ev_count || !type) return;
    execviz_rec *r = &ev_spans[s - 1];
    if (r->nlife >= EXECVIZ_LIFE_MAX) return;
    r->life[r->nlife].t = ev_now();
    ev_copy(r->life[r->nlife].type, type, 16);
    r->nlife++;
    r->sent_phase = 0;
}

/* A log line belongs to the span that was running when it was written. There is

 * no second stream to correlate afterwards, which is the reason for it.
 *
 * Once a span's buffer is full the overflow is COUNTED rather than dropped in
 * silence: a reader who sees ten lines must be able to tell that from a reader
 * who sees ten of forty. */
void execviz_log(const char *level, const char *message) {
    execviz_span s = execviz_current();
    if (!s || s > (execviz_span)ev_count || !message) return;
    execviz_rec *r = &ev_spans[s - 1];
    if (r->nevents >= EXECVIZ_EVENTS_MAX) { r->events_dropped++; return; }
    r->events[r->nevents].t = ev_now();
    ev_copy(r->events[r->nevents].level, level ? level : "info", EXECVIZ_LEVEL_MAX);
    ev_copy(r->events[r->nevents].msg, message, EXECVIZ_MSG_MAX);
    r->nevents++;
    r->sent_phase = 0;
}

// ========================================================================
// INTERNALS
// ========================================================================

static int ev_post(const char *body, size_t len) {
    char hostbuf[128]; int port = 80;
    const char *p = strstr(ev_collector, "://");
    p = p ? p + 3 : ev_collector;
    const char *colon = strchr(p, ':');
    if (colon) {
        size_t n = (size_t)(colon - p);
        if (n >= sizeof hostbuf) n = sizeof hostbuf - 1;
        memcpy(hostbuf, p, n); hostbuf[n] = 0;
        port = atoi(colon + 1);
    } else {
        strncpy(hostbuf, p, sizeof hostbuf - 1); hostbuf[sizeof hostbuf - 1] = 0;
        char *slash = strchr(hostbuf, '/'); if (slash) *slash = 0;
    }
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    struct sockaddr_in a; memset(&a, 0, sizeof a);
    a.sin_family = AF_INET; a.sin_port = htons((uint16_t)port);
    if (inet_pton(AF_INET, hostbuf, &a.sin_addr) != 1) { close(fd); return -1; }
    if (connect(fd, (struct sockaddr *)&a, sizeof a) != 0) { close(fd); return -1; }
    char head[512];
    int hn = snprintf(head, sizeof head,
        "POST /api/ingest HTTP/1.1\r\nHost: %s\r\nContent-Type: application/json\r\n"
        "Content-Length: %zu\r\nConnection: close\r\n\r\n", hostbuf, len);
    if (write(fd, head, (size_t)hn) < 0) { close(fd); return -1; }
    if (write(fd, body, len) < 0) { close(fd); return -1; }
    char sink[256]; while (read(fd, sink, sizeof sink) > 0) {}
    close(fd);
    return 0;
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

int execviz_flush(void) {
    if (!ev_collector[0] || ev_count == 0) return 0;
    size_t cap = 512 + (size_t)ev_count * 640;
    char *buf = (char *)malloc(cap);
    if (!buf) return -1;
    size_t n = (size_t)snprintf(buf, cap, "{\"host_id\":\"%s\",\"spans\":[", ev_host);
    int emitted = 0;
    for (int i = 0; i < ev_count; i++) {
        execviz_rec *r = &ev_spans[i];
        /* phase two re-sends a span whose end has landed; nothing else is resent */
        int phase = (r->end >= 0) ? 2 : 1;
        if (r->sent_phase >= phase) continue;
        const char *status = r->end < 0 ? "running"
                           : (r->status == EXECVIZ_OK ? "ok" : "error");
        n += (size_t)snprintf(buf + n, cap - n,
            "%s{\"span_id\":\"%s\",\"trace_id\":\"%s\",%s%s%s"
            "\"name\":\"%s\",\"kind\":\"%s\",\"start\":%.6f,",
            emitted ? "," : "", r->id, ev_trace,
            r->parent[0] ? "\"parent_span_id\":\"" : "\"parent_span_id\":null",
            r->parent[0] ? r->parent : "", r->parent[0] ? "\"," : ",",
            r->name, r->kind, r->start);
        if (r->end >= 0) n += (size_t)snprintf(buf + n, cap - n, "\"end\":%.6f,", r->end);
        else             n += (size_t)snprintf(buf + n, cap - n, "\"end\":null,");
        n += (size_t)snprintf(buf + n, cap - n,
            "\"status\":\"%s\",\"host_id\":\"%s\",\"domain\":\"%s\",\"origin\":\"semantic\","
            /* which clock stamped this, so skew analysis knows what it is
             * comparing rather than assuming every host is alike */
            "\"clock_source\":\"CLOCK_REALTIME\"",
            status, ev_host, ev_domain);
        if (r->nlinks) {
            n += (size_t)snprintf(buf + n, cap - n, ",\"links\":[");
            for (int li = 0; li < r->nlinks; li++)
                n += (size_t)snprintf(buf + n, cap - n, "%s\"%s\"",
                                      li ? "," : "", r->links[li]);
            n += (size_t)snprintf(buf + n, cap - n, "]");
        }
        if (r->nlife) {
            n += (size_t)snprintf(buf + n, cap - n, ",\"lifecycle\":[");
            for (int li = 0; li < r->nlife; li++)
                n += (size_t)snprintf(buf + n, cap - n,
                    "%s{\"t\":%.6f,\"type\":\"%s\"}",
                    li ? "," : "", r->life[li].t, r->life[li].type);
            n += (size_t)snprintf(buf + n, cap - n, "]");
        }
        if (r->nevents) {
            n += (size_t)snprintf(buf + n, cap - n, ",\"events\":[");
            for (int ei = 0; ei < r->nevents; ei++)
                n += (size_t)snprintf(buf + n, cap - n,
                    "%s{\"t\":%.6f,\"level\":\"%s\",\"msg\":\"%s\"}",
                    ei ? "," : "", r->events[ei].t, r->events[ei].level,
                    r->events[ei].msg);
            /* the overflow is stated rather than hidden: ten of forty lines
             * must not look like ten lines */
            if (r->events_dropped)
                n += (size_t)snprintf(buf + n, cap - n,
                    ",{\"t\":%.6f,\"level\":\"meta\",\"msg\":\"%d further line(s) suppressed for this span\"}",
                    r->end >= 0 ? r->end : r->start, r->events_dropped);
            n += (size_t)snprintf(buf + n, cap - n, "]");
        }
        if (r->msg[0])
            n += (size_t)snprintf(buf + n, cap - n,
                ",\"error\":{\"type\":\"native\",\"message\":\"%s\"}", r->msg);
        n += (size_t)snprintf(buf + n, cap - n, "}");
        r->sent_phase = phase;
        emitted++;
        if (n > cap - 3000) break;   /* a span may now carry links, lifecycle and events */
    }
    n += (size_t)snprintf(buf + n, cap - n, "]}");
    int rc = emitted ? ev_post(buf, n) : 0;
    free(buf);
    return rc == 0 ? emitted : -1;
}

#endif /* EXECVIZ_IMPLEMENTATION */
#endif /* EXECVIZ_H */
