// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: execviz_stress.c
//  script_path: execviz-syscall/execviz_stress.c
//  module_name: execviz_stress
//  version: 0.53.1
//  description: The plan half of this feature (`execviz stress --records`) derives WHICH faults are worth injecting from what the recorder observed the program doing.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: audit.h, errno.h, fcntl.h, filter.h, ioctl.h, prctl.h, sched.h, seccomp.h, socket.h, stddef.h, stdint.h, stdio.h
//  features: execviz stress, stress
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

/* execviz-stress: run a program and make its syscalls fail on purpose.

// ========================================================================
// CONSTANTS
// ========================================================================
 *
 * The plan half of this feature (`execviz stress --records`) derives WHICH
 * faults are worth injecting from what the recorder observed the program doing.
 * This is the half that carries them out.
 *
 * The mechanism is seccomp user notification. A filter is installed in the
 * child before exec, the listener descriptor is handed back to this supervisor,
 * and from then on every selected syscall the program makes stops in the kernel
 * and asks this process what should happen. The program is not modified, not
 * relinked, and not aware. That is the same principle as the recorder: work from
 * below libc, and never require the thing under test to cooperate.
 *
 * WHY A LAUNCHER AND NOT AN ATTACH. A seccomp filter applies to the process that
 * installs it and to its children. It cannot be retrofitted onto a process that
 * is already running, so this starts the program rather than finding it. That is
 * a real limit and it is stated here rather than discovered later.
 *
 * WHAT THIS DELIBERATELY DOES NOT DO. It does not shorten reads. Returning a
 * smaller count without performing the read would hand the program a buffer of
 * stale bytes and call it data, which is corruption rather than stress. Doing it
 * accurately means performing the read on the program's behalf and writing the
 * result back into its memory, and that is worth building separately rather than
 * approximating here. Until then `short_read` is a plan entry, not a capability,
 * and this program reports it if asked for it.
 *
 * HONESTY. Every run reports how many calls were intercepted, how many were
 * failed, and how many were allowed through. A fault injector that cannot tell
 * you how often it fired is indistinguishable from one that did nothing.
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <fcntl.h>
#include <sched.h>
#include <sys/ioctl.h>
#include <sys/prctl.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <linux/unistd.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/uio.h>

#ifndef SECCOMP_FILTER_FLAG_NEW_LISTENER
#define SECCOMP_FILTER_FLAG_NEW_LISTENER (1UL << 3)
#endif
#ifndef SECCOMP_RET_USER_NOTIF
#define SECCOMP_RET_USER_NOTIF 0x7fc00000U
#endif
#ifndef SECCOMP_USER_NOTIF_FLAG_CONTINUE
#define SECCOMP_USER_NOTIF_FLAG_CONTINUE (1UL << 0)
#endif
#ifndef SECCOMP_IOCTL_NOTIF_RECV
#define SECCOMP_IOCTL_NOTIF_RECV  SECCOMP_IOR(0, struct seccomp_notif)
#define SECCOMP_IOCTL_NOTIF_SEND  SECCOMP_IOWR(1, struct seccomp_notif_resp)
#endif

#if defined(__x86_64__)
#define STRESS_AUDIT_ARCH AUDIT_ARCH_X86_64
#elif defined(__aarch64__)
#define STRESS_AUDIT_ARCH AUDIT_ARCH_AARCH64
#else
#error "execviz-stress supports x86_64 and aarch64: the syscall numbers and the audit arch are architecture-specific, and a filter built for the wrong one would intercept the wrong calls."
#endif

/* The calls each stressor stops at. Numbers are per-architecture for the same
   reason everything else in this project is: the same number is a different
   call on each machine, and a table used on the wrong one intercepts something
   nobody asked for. */
#if defined(__x86_64__)

// ========================================================================
// INTERNALS
// ========================================================================
static const int BLOCKING_CALLS[] = { 7, 23, 232, 271, 202, 35 };      /* poll select epoll_wait ppoll futex nanosleep */
static const int SOCKET_READS[]   = { 45, 47, 0 };                      /* recvfrom recvmsg read */
static const int OPEN_CALLS[]     = { 257, 2, 41 };                     /* openat open socket */
#else
static const int BLOCKING_CALLS[] = { 73, 72, 22, 98, 101, 0 };         /* ppoll pselect6 epoll_pwait futex nanosleep read-guard */
static const int SOCKET_READS[]   = { 207, 212, 63 };                   /* recvfrom recvmsg read */
static const int OPEN_CALLS[]     = { 56, 198, 202 };                   /* openat socket accept */
#endif

// ========================================================================
// TYPES
// ========================================================================

struct plan_entry { const char *name; const int *calls; int n; int err; const char *note; };

#if defined(__x86_64__)

// ========================================================================
// INTERNALS
// ========================================================================
static const int READ_CALLS[] = { 0, 17, 19, 45, 47 };                  /* read pread64 readv recvfrom recvmsg */
#else
static const int READ_CALLS[] = { 63, 67, 65, 207, 212 };
#endif

static const struct plan_entry PLANS[] = {
    { "short_read", READ_CALLS, (int)(sizeof READ_CALLS / sizeof(int)), 0,
      "reads really happen but come back with fewer bytes than were asked for, so a partial read must not be treated as a whole one" },
    { "interrupted_wait", BLOCKING_CALLS, (int)(sizeof BLOCKING_CALLS / sizeof(int)), EINTR,
      "blocking calls return EINTR, so a wait that was interrupted must be retried rather than treated as finished" },
    { "peer_disappears", SOCKET_READS, (int)(sizeof SOCKET_READS / sizeof(int)), ECONNRESET,
      "reads from a peer fail as if the connection ended, so an ended connection must be distinguished from an idle one" },
    { "descriptor_exhaustion", OPEN_CALLS, (int)(sizeof OPEN_CALLS / sizeof(int)), EMFILE,
      "opening anything new fails as if the descriptor table were full" },
};

static int install_filter(const int *calls, int n) {
    /* Classic BPF: stop at the listed calls, allow everything else. Built by
       hand because a filter is small and a dependency is not. */
    int len = 3 + n * 2 + 1;
    struct sock_filter *f = calloc((size_t)len, sizeof *f);
    int i = 0;
    /* reject a foreign architecture outright rather than filtering by numbers
       that mean something else there */
    f[i++] = (struct sock_filter)BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, arch));
    f[i++] = (struct sock_filter)BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, STRESS_AUDIT_ARCH, 1, 0);
    f[i++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
    f[i++] = (struct sock_filter)BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr));
    for (int k = 0; k < n; k++) {
        /* Jump to the notify at the very end. Offsets are counted from the NEXT
           instruction, and each entry occupies two slots, so from entry k there
           are 2*(n-k) instructions to clear. Landing one short puts a match on
           the ALLOW that precedes the notify, which does not fail: it silently
           permits everything and reports nothing intercepted. */
        int to_notify = 2 * (n - k);
        f[i++] = (struct sock_filter)BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, (unsigned)calls[k], to_notify, 0);
        f[i++] = (struct sock_filter)BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr));
    }
    f[i++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
    f[i++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_USER_NOTIF);
    struct sock_fprog prog = { .len = (unsigned short)i, .filter = f };
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0) { perror("no_new_privs"); return -1; }
    int fd = (int)syscall(__NR_seccomp, SECCOMP_SET_MODE_FILTER,
                          SECCOMP_FILTER_FLAG_NEW_LISTENER, &prog);
    free(f);
    return fd;
}

static int send_fd(int sock, int fd) {
    char buf[CMSG_SPACE(sizeof(int))] = {0};
    struct iovec io = { .iov_base = (void *)"x", .iov_len = 1 };
    struct msghdr m = {0};
    m.msg_iov = &io; m.msg_iovlen = 1; m.msg_control = buf; m.msg_controllen = sizeof buf;
    struct cmsghdr *c = CMSG_FIRSTHDR(&m);
    c->cmsg_level = SOL_SOCKET; c->cmsg_type = SCM_RIGHTS; c->cmsg_len = CMSG_LEN(sizeof(int));
    memcpy(CMSG_DATA(c), &fd, sizeof fd);
    return sendmsg(sock, &m, 0) < 0 ? -1 : 0;
}

static int recv_fd(int sock) {
    char buf[CMSG_SPACE(sizeof(int))] = {0}, d;
    struct iovec io = { .iov_base = &d, .iov_len = 1 };
    struct msghdr m = {0};
    m.msg_iov = &io; m.msg_iovlen = 1; m.msg_control = buf; m.msg_controllen = sizeof buf;
    if (recvmsg(sock, &m, 0) < 0) return -1;
    struct cmsghdr *c = CMSG_FIRSTHDR(&m);
    if (!c) return -1;
    int fd; memcpy(&fd, CMSG_DATA(c), sizeof fd);
    return fd;
}

/* Pick a stressor out of a plan that `execviz stress --records` derived.

 *
 * This makes the feature reflexive rather than two commands that happen
 * to share a name: the fault carried out here was chosen by what the recorder saw
 * the program doing, not by whoever is running it.
 *
 * The plan is this project's own compact JSON, so it is scanned rather than
 * parsed. Only the `proposed` section is considered: `not_proposed` carries
 * stressor names too, and injecting one of those would be carrying out precisely
 * the fault the derivation said did not apply. The keys are emitted in sorted
 * order, so `proposed` runs until `records_read`. */
static long plan_suggest(const char *buf, const char *key) {
    /* the derived suggestion sits under "suggested"; -1 means the plan did not
       carry one, and the caller keeps its own default rather than inventing */
    const char *sec = strstr(buf, "\"suggested\":");
    if (!sec) return -1;
    char pat[32];
    snprintf(pat, sizeof pat, "\"%s\":", key);
    const char *p = strstr(sec, pat);
    if (!p) return -1;
    p += strlen(pat);
    return strtol(p, NULL, 10);
}

static long g_after = -1, g_rate = -1;

static const char *pick_from_plan(const char *path, const char **why) {
    static char name[64];
    FILE *fp = fopen(path, "r");
    if (!fp) { *why = "the plan file could not be read"; return NULL; }
    static char buf[1 << 20];
    size_t n = fread(buf, 1, sizeof buf - 1, fp);
    fclose(fp);
    buf[n] = 0;

    g_after = plan_suggest(buf, "after");
    g_rate  = plan_suggest(buf, "rate");

    char *start = strstr(buf, "\"proposed\":");
    if (!start) { *why = "the plan contains no proposed section"; return NULL; }
    char *end = strstr(start, "\"records_read\"");
    if (end) *end = 0;

    /* first proposed stressor this supervisor can carry out */
    for (char *p = start; (p = strstr(p, "\"stressor\":\"")) != NULL; ) {
        p += 12;
        char *q = strchr(p, '"');
        if (!q || (size_t)(q - p) >= sizeof name) break;
        memcpy(name, p, (size_t)(q - p));
        name[q - p] = 0;
        for (unsigned i = 0; i < sizeof PLANS / sizeof PLANS[0]; i++)
            if (!strcmp(PLANS[i].name, name)) return name;
        p = q;
    }
    *why = "the plan proposed nothing this supervisor implements yet "
           "(short_read and the timing stressors are derived but not injectable here)";
    return NULL;
}

#ifndef SECCOMP_IOCTL_NOTIF_ID_VALID

// ========================================================================
// CONSTANTS
// ========================================================================
#define SECCOMP_IOCTL_NOTIF_ID_VALID SECCOMP_IOW(2, __u64)
#endif
#ifndef __NR_pidfd_open
#define __NR_pidfd_open 434
#endif
#ifndef __NR_pidfd_getfd
#define __NR_pidfd_getfd 438
#endif

/* A read that comes back short, done accurately.

// ========================================================================
// INTERNALS
// ========================================================================
 *
 * The tempting version of this is to answer the notification with a smaller
 * count and let the call never happen. That is not a short read, it is
 * corruption: the program's buffer still holds whatever was there before, and
 * it has just been told that many bytes are valid.
 *
 * A real short read means the read HAPPENED and returned less. So this performs
 * it: borrow the descriptor out of the target with pidfd_getfd, read fewer bytes
 * than were asked for, write those bytes into the target's own buffer with
 * process_vm_writev, and answer with the count transferred. The bytes
 * not taken stay in the file or socket, exactly as they would after a genuine
 * short read, so the program can go back for the rest if it knows to.
 *
 * The id is revalidated after the descriptor work and before any write into
 * another process's memory. Without that check a target that died between the
 * notification and the write could have had its pid reused, and this would be
 * writing into a stranger.
 */
static int short_read_handle(int listener, struct seccomp_notif *req,
                             struct seccomp_notif_resp *resp) {
    unsigned long long fd = req->data.args[0];
    unsigned long long buf = req->data.args[1];
    unsigned long long count = req->data.args[2];
    if (count < 2 || buf == 0) return 0;           /* nothing to shorten */

    int pidfd = (int)syscall(__NR_pidfd_open, (pid_t)req->pid, 0);
    if (pidfd < 0) return -1;
    int local = (int)syscall(__NR_pidfd_getfd, pidfd, (int)fd, 0);
    if (local < 0) { close(pidfd); return -1; }

    size_t want = (size_t)(count / 2);
    if (want == 0) want = 1;
    if (want > (1u << 16)) want = 1u << 16;
    char *tmp = malloc(want);
    if (!tmp) { close(local); close(pidfd); return -1; }
    ssize_t got = read(local, tmp, want);

    /* the call this answers must still be the one that was intercepted */
    __u64 id = req->id;
    if (ioctl(listener, SECCOMP_IOCTL_NOTIF_ID_VALID, &id) != 0) {
        free(tmp); close(local); close(pidfd); return -1;
    }
    if (got > 0) {
        struct iovec liov = { .iov_base = tmp, .iov_len = (size_t)got };
        struct iovec riov = { .iov_base = (void *)(uintptr_t)buf, .iov_len = (size_t)got };
        if (syscall(__NR_process_vm_writev, (pid_t)req->pid, &liov, 1UL, &riov, 1UL, 0UL) < 0) {
            free(tmp); close(local); close(pidfd); return -1;
        }
    }
    resp->val = got < 0 ? 0 : got;
    resp->error = got < 0 ? -EIO : 0;
    resp->flags = 0;
    free(tmp); close(local); close(pidfd);
    return 1;
}

static void usage(void) {
    fprintf(stderr,
        "execviz-stress: run a program with a derived fault injected\n\n"
        "  execviz-stress --plan NAME       [--rate N] [--after N] -- <command> [args...]\n"
        "  execviz-stress --from-plan FILE  [--rate N] [--after N] -- <command> [args...]\n\n"
        "--from-plan takes a plan derived by `execviz stress --records` and carries out\n"
        "the first stressor in it that this supervisor implements, so the fault is chosen\n"
        "by what the program was seen doing rather than by whoever runs it.\n\n"
        "plans:\n");
    for (unsigned i = 0; i < sizeof PLANS / sizeof PLANS[0]; i++)
        fprintf(stderr, "  %-24s %s\n", PLANS[i].name, PLANS[i].note);
    fprintf(stderr,
        "\n  --rate N    fail one call in N (default 5); the rest are allowed through\n"
        "  --after N   allow the first N intercepted calls, then begin failing\n\n"
        "short_read really performs the read, shortened, and writes the bytes it got\n"
        "into the program's own buffer. Answering with a smaller count and skipping the\n"
        "read would leave stale bytes in that buffer and call them data.\n");
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

int main(int argc, char **argv) {
    const char *plan_name = NULL, *from_plan = NULL;
    int rate = 5, after = 0, argi = 0, rate_set = 0, after_set = 0;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--plan") && i + 1 < argc) plan_name = argv[++i];
        else if (!strcmp(argv[i], "--from-plan") && i + 1 < argc) from_plan = argv[++i];
        else if (!strcmp(argv[i], "--rate") && i + 1 < argc) { rate = atoi(argv[++i]); rate_set = 1; }
        else if (!strcmp(argv[i], "--after") && i + 1 < argc) { after = atoi(argv[++i]); after_set = 1; }
        else if (!strcmp(argv[i], "--")) { argi = i + 1; break; }
        else if (!strcmp(argv[i], "--help")) { usage(); return 0; }
    }
    if (from_plan && !plan_name) {
        const char *why = "unknown";
        plan_name = pick_from_plan(from_plan, &why);
        if (!plan_name) {
            fprintf(stderr, "execviz-stress: no stressor taken from %s: %s\n", from_plan, why);
            return 1;
        }
        fprintf(stderr, "execviz-stress: %s chosen from the derived plan, "
                        "because that is what this program was observed doing\n", plan_name);
        /* the plan also worked out where startup ends and how often to fire;
           an explicit flag still wins, because the operator may know better */
        if (!rate_set && g_rate > 0)  { rate  = (int)g_rate;
            fprintf(stderr, "execviz-stress: rate 1 in %d, derived from the capture\n", rate); }
        if (!after_set && g_after >= 0) { after = (int)g_after;
            fprintf(stderr, "execviz-stress: allowing the first %d calls, which the capture "
                            "shows are this program's startup\n", after); }
    }
    if (!plan_name || !argi || argi >= argc) { usage(); return 2; }
    if (rate < 1) rate = 1;

    const struct plan_entry *plan = NULL;
    for (unsigned i = 0; i < sizeof PLANS / sizeof PLANS[0]; i++)
        if (!strcmp(PLANS[i].name, plan_name)) plan = &PLANS[i];
    if (!plan) {
        fprintf(stderr, "execviz-stress: no such plan: %s\n", plan_name);
        return 2;
    }

    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0) { perror("socketpair"); return 1; }

    pid_t pid = fork();
    if (pid == 0) {
        close(sv[0]);
        int listener = install_filter(plan->calls, plan->n);
        if (listener < 0) { perror("seccomp"); _exit(127); }
        if (send_fd(sv[1], listener) != 0) { perror("send listener"); _exit(127); }
        close(listener); close(sv[1]);
        execvp(argv[argi], &argv[argi]);
        fprintf(stderr, "execviz-stress: cannot run %s: %s\n", argv[argi], strerror(errno));
        _exit(127);
    }
    close(sv[1]);
    int listener = recv_fd(sv[0]);
    if (listener < 0) { fprintf(stderr, "execviz-stress: no listener from the child\n"); return 1; }

    unsigned long long seen = 0, failed = 0, passed = 0, unreachable = 0;
    struct seccomp_notif *req = calloc(1, sizeof *req);
    struct seccomp_notif_resp *resp = calloc(1, sizeof *resp);

    for (;;) {
        memset(req, 0, sizeof *req);
        if (ioctl(listener, SECCOMP_IOCTL_NOTIF_RECV, req) != 0) {
            if (errno == EINTR) continue;
            break;                      /* the child is gone: the listener ends */
        }
        seen++;
        memset(resp, 0, sizeof *resp);
        resp->id = req->id;
        if ((long long)seen > after && (seen % (unsigned)rate) == 0) {
            if (plan->err == 0) {
                /* short_read: the call must really happen, just return less */
                int r = short_read_handle(listener, req, resp);
                if (r == 1) { failed++; }
                else {
                    /* Could not borrow the descriptor or reach the memory. Let
                       the call proceed untouched and do NOT count it as
                       injected: a fault that did not happen must not be
                       reported as one. */
                    resp->flags = SECCOMP_USER_NOTIF_FLAG_CONTINUE;
                    resp->error = 0; resp->val = 0;
                    passed++; unreachable++;
                }
            } else {
                resp->error = -plan->err;   /* the fault */
                resp->val = 0;
                resp->flags = 0;
                failed++;
            }
        } else {
            resp->flags = SECCOMP_USER_NOTIF_FLAG_CONTINUE;   /* let it really happen */
            passed++;
        }
        if (ioctl(listener, SECCOMP_IOCTL_NOTIF_SEND, resp) != 0 && errno != ENOENT) break;
    }

    int status = 0;
    waitpid(pid, &status, 0);

    /* An injector that cannot say how often it fired is indistinguishable from
       one that did nothing, so this is reported whatever the outcome. */
    fprintf(stderr,
        "{\"plan\":\"%s\",\"intercepted\":%llu,\"failed\":%llu,\"allowed\":%llu,"
        "\"injected\":\"%s\",\"could_not_inject\":%llu,\"rate\":\"1 in %d\",\"exit\":%d}\n",
        plan->name, seen, failed, passed,
        plan->err ? strerror(plan->err) : "a short read", unreachable, rate,
        WIFEXITED(status) ? WEXITSTATUS(status) : -1);

    if (seen == 0) {
        fprintf(stderr,
            "execviz-stress: nothing was intercepted. The program did not make any of "
            "the calls this plan stops at, so this run demonstrated nothing about it.\n");
        return 1;
    }
    return WIFEXITED(status) ? WEXITSTATUS(status) : 1;
}
