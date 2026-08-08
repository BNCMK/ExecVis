// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: execviz_preload.c
//  script_path: execviz-syscall/execviz_preload.c
//  module_name: execviz_preload
//  version: 0.53.1
//  description: Loaded ahead of libc, this wraps the call sites, forwards to the real implementation, and records around it. It needs no privilege and works wherever dynamic linking does. It sees only what goes through libc: a static
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: dlfcn.h, fcntl.h, socket.h, stdarg.h, stdio.h, stdlib.h, string.h, syscall.h, time.h, types.h, uio.h, unistd.h
//  features: execviz preload
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

/* execviz syscall adapter: library interposition.

// ========================================================================
// CONSTANTS
// ========================================================================
 *
 * Loaded ahead of libc, this wraps the call sites, forwards to the real
 * implementation, and records around it. It needs no privilege and works
 * wherever dynamic linking does. It sees only what goes through libc: a static
 * binary or a direct syscall is invisible to it, which is a coverage limit of
 * the mechanism and is reported as such.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/uio.h>
#include <time.h>
#include <fcntl.h>
#include <sys/types.h>
#include <sys/syscall.h>
#include <sys/socket.h>
#include <stdarg.h>

// ========================================================================
// INTERNALS
// ========================================================================

static int out_fd = -1;
static __thread int in_hook = 0;          /* never trace ourselves */
/* Bounded and escaped at initialisation rather than at every emit: the value
   comes from the environment, and a quote in it made every emitted line invalid
   JSON while a long one truncated the record mid-field and produced bytes that
   were not even UTF-8. */

// ========================================================================
// CONSTANTS
// ========================================================================
#define EXECVIZ_HOST_MAX 96

// ========================================================================
// INTERNALS
// ========================================================================
static char host[EXECVIZ_HOST_MAX] = "local";

/* Copies src into dst with the characters JSON cannot carry escaped, never
   exceeding cap (including the terminator) and never splitting an escape. */

static void json_escape_into(char *dst, size_t cap, const char *src) {
    size_t o = 0;
    for (size_t i = 0; src[i] && o + 2 < cap; i++) {
        unsigned char ch = (unsigned char)src[i];
        const char *esc = NULL;
        switch (ch) {
            case '"':  esc = "\\\""; break;
            case '\\': esc = "\\\\"; break;
            case '\n': esc = "\\n";  break;
            case '\r': esc = "\\r";  break;
            case '\t': esc = "\\t";  break;
            default: break;
        }
        if (esc) {
            if (o + 3 >= cap) break;          /* never write half an escape */
            dst[o++] = esc[0];
            dst[o++] = esc[1];
        } else if (ch < 0x20) {
            continue;                          /* a control byte carries nothing */
        } else {
            dst[o++] = (char)ch;
        }
    }
    dst[o] = '\0';
}

static double now_wall(void) {
    struct timespec ts; clock_gettime(CLOCK_REALTIME, &ts);
    return ts.tv_sec + ts.tv_nsec / 1e9;
}
static pid_t tid(void) { return (pid_t)syscall(SYS_gettid); }

/* Writes to the record file without going through the interposed `write`.
   Recording a write by performing a write is the shape of an infinite loop, and
   the in_hook guard should not be the only thing standing between this library
   and one. */

static ssize_t write_raw(const char *buf, size_t n) {
    return (ssize_t)syscall(SYS_write, out_fd, buf, n);
}

__attribute__((constructor)) static void init(void) {
    /* This library interposes open(), so opening the output file must not go
       through the interposed symbol: the hook would run before initialisation
       has finished. in_hook keeps the constructor out of its own trap. */
    in_hook = 1;
    const char *path = getenv("EXECVIZ_SYSCALL_OUT");
    const char *h = getenv("EXECVIZ_HOST");
    if (h) json_escape_into(host, sizeof host, h);
    if (path) {
        out_fd = (int)syscall(SYS_openat, AT_FDCWD, path,
                              O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
    }
    in_hook = 0;
}

/* Two phases, as everywhere else: a call is recorded with the duration it
   took, so a call that never returns leaves no completion behind. */

static void emit(const char *name, double t0, double t1, long ret) {
    if (out_fd < 0 || in_hook) return;
    in_hook = 1;
    char buf[256];
    int n = snprintf(buf, sizeof buf,
        "{\"t\":%.6f,\"dur\":%.6f,\"tid\":%d,\"pid\":%d,\"call\":\"%s\",\"ret\":%ld,\"host\":\"%s\",\"src\":\"preload\"}\n",
        t0, t1 - t0, tid(), getpid(), name, ret, host);
    /* A partial write tears a line in half, and half a JSON object is a record
       no reader can use. Writing the remainder costs nothing and keeps every
       line whole. */
    if (n > 0 && n < (int)sizeof buf) {
        size_t off = 0;
        while (off < (size_t)n) {
            ssize_t w = write_raw(buf + off, (size_t)n - off);
            if (w <= 0) break;               /* the reader is gone; drop the rest */
            off += (size_t)w;
        }
    }
    in_hook = 0;
}

/* Emits one line of a program's own output as a log record.
 
   Split on newlines because a single write can carry several lines, and a
   reader wants lines rather than a block. Text is escaped through the same
   helper the host name uses: a program's output is arbitrary bytes, and
   arbitrary bytes in a JSON string is how a record becomes unparseable. */

static void emit_line(const char *level, const char *buf, size_t n, double t) {
    if (out_fd < 0) return;
    in_hook = 1;
    size_t start = 0;
    for (size_t i = 0; i <= n; i++) {
        if (i == n || buf[i] == '\n') {
            size_t len = i - start;
            if (len > 0 && len < 4096) {
                char raw[4096], esc[4200], out[4600];
                memcpy(raw, buf + start, len);
                raw[len] = '\0';
                json_escape_into(esc, sizeof esc, raw);
                int w = snprintf(out, sizeof out,
                    "{\"t\":%.6f,\"tid\":%d,\"pid\":%d,\"log\":\"%s\",\"level\":\"%s\","
                    "\"host\":\"%s\",\"src\":\"preload\"}\n",
                    t, tid(), getpid(), esc, level, host);
                if (w > 0 && w < (int)sizeof out) {
                    size_t off = 0;
                    while (off < (size_t)w) {
                        ssize_t k = write_raw(out + off, (size_t)w - off);
                        if (k <= 0) break;
                        off += (size_t)k;
                    }
                }
            }
            start = i + 1;
        }
    }
    in_hook = 0;
}

// ========================================================================
// CONSTANTS
// ========================================================================

#define WRAP(ret_t, name, proto, args, fmt_ret)                       \
    ret_t name proto {                                                \
        static ret_t (*real) proto = NULL;                            \
        if (!real) real = dlsym(RTLD_NEXT, #name);                    \
        if (in_hook) return real args;                                \
        double t0 = now_wall();                                       \
        ret_t r = real args;                                          \
        emit(#name, t0, now_wall(), (long)(fmt_ret));                 \
        return r;                                                     \
    }

WRAP(ssize_t, read,  (int fd, void *b, size_t n), (fd, b, n), r)
/* write() is wrapped by hand rather than by the macro, because a write to fd 1
   or 2 is not merely a syscall; it is a log line, and this library is already
   standing exactly where it is written.
 
   This is the only runtime here where log capture needs no cooperation at all:
   no handler to install, no stream to replace, no code to change, and it works
   for a binary nobody has the source to. */

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

ssize_t write(int fd, const void *b, size_t n) {
    static ssize_t (*real)(int, const void *, size_t) = NULL;
    if (!real) real = dlsym(RTLD_NEXT, "write");
    if (in_hook) return real(fd, b, n);
    double t0 = now_wall();
    ssize_t r = real(fd, b, n);
    if ((fd == 1 || fd == 2) && b && n > 0) {
        emit_line(fd == 2 ? "error" : "info", (const char *)b, n, t0);
    }
    emit("write", t0, now_wall(), (long)r);
    return r;
}
/* writev() as well as write(), because a great many programs never call write.
   The C library's buffered output, and anything assembling a line from several
   pieces, goes out through writev; `ls` reporting a missing file captured
   nothing until this was here. Wrapping one and not the other is the difference
   between capturing a program's output and capturing some of it. */

ssize_t writev(int fd, const struct iovec *iov, int iovcnt) {
    static ssize_t (*real)(int, const struct iovec *, int) = NULL;
    if (!real) real = dlsym(RTLD_NEXT, "writev");
    if (in_hook) return real(fd, iov, iovcnt);
    double t0 = now_wall();
    ssize_t r = real(fd, iov, iovcnt);
    if ((fd == 1 || fd == 2) && iov && iovcnt > 0) {
        for (int i = 0; i < iovcnt; i++) {
            if (iov[i].iov_base && iov[i].iov_len > 0) {
                emit_line(fd == 2 ? "error" : "info",
                          (const char *)iov[i].iov_base, iov[i].iov_len, t0);
            }
        }
    }
    emit("writev", t0, now_wall(), (long)r);
    return r;
}

/* Why there are no fwrite/puts/fputs wrappers here.
 
   They were written and measured, and they did not work: a C program emitting
   five lines by puts, fputs, fwrite, write and printf had exactly ONE captured,
   the direct `write`. glibc resolves its own stdio internally, so those calls
   never pass through the dynamic symbol table where LD_PRELOAD can reach them.
 
   Code that does not do its job differs from no code, because the next reader
   assumes it works. So this library captures what it can capture,
   direct `write` and `writev` to fd 1 or 2; and the limit is written down here
   and in the README rather than implied by wrappers that appear to cover more.
 
   A C program that wants its output attributed calls `execviz_log` from
   execviz-c/execviz.h, which is one line and always works. */

WRAP(int,     close, (int fd), (fd), r)
WRAP(int,     fsync, (int fd), (fd), r)
WRAP(ssize_t, sendto,(int s, const void *b, size_t n, int f, const struct sockaddr *a, socklen_t l), (s,b,n,f,a,l), r)
WRAP(ssize_t, recvfrom,(int s, void *b, size_t n, int f, struct sockaddr *a, socklen_t *l), (s,b,n,f,a,l), r)
WRAP(int,     connect,(int s, const struct sockaddr *a, socklen_t l), (s,a,l), r)

int open(const char *path, int flags, ...) {
    static int (*real)(const char *, int, ...) = NULL;
    if (!real) real = dlsym(RTLD_NEXT, "open");
    mode_t mode = 0;
    if (flags & O_CREAT) { va_list ap; va_start(ap, flags); mode = va_arg(ap, int); va_end(ap); }
    if (in_hook) return real(path, flags, mode);
    double t0 = now_wall();
    int r = real(path, flags, mode);
    emit("open", t0, now_wall(), r);
    return r;
}

int openat(int dirfd, const char *path, int flags, ...) {
    static int (*real)(int, const char *, int, ...) = NULL;
    if (!real) real = dlsym(RTLD_NEXT, "openat");
    mode_t mode = 0;
    if (flags & O_CREAT) { va_list ap; va_start(ap, flags); mode = va_arg(ap, int); va_end(ap); }
    if (in_hook) return real(dirfd, path, flags, mode);
    double t0 = now_wall();
    int r = real(dirfd, path, flags, mode);
    emit("openat", t0, now_wall(), r);
    return r;
}
