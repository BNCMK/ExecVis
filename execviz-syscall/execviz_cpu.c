// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: execviz_cpu.c
//  script_path: execviz-syscall/execviz_cpu.c
//  module_name: execviz_cpu
//  version: 0.53.1
//  description: Samples where the CPU actually is, on a timer, with call chains, and folds them
//  kind: native
//  spec: internal
//  internal_dependencies:
//  external_dependencies: linux/perf_event.h, sys/mman.h, sys/syscall.h
//  features: timed sampling, call chains, folded output, per process, no instrumentation
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

/* Where the CPU is, sampled rather than derived.
 *
 * The span tree answers where measured time went, exactly, for work somebody
 * instrumented. It cannot answer anything about a slow function nobody wrapped,
 * because no span exists to carry it. That is the question this answers, and the
 * only way to answer it is to interrupt the machine on a timer and record where
 * it was standing.
 *
 * `perf_event_open` on the software CPU clock, one event per CPU, sampling at a
 * fixed frequency with `PERF_SAMPLE_CALLCHAIN` so each sample carries the return
 * addresses above it. Samples arrive in a per CPU ring buffer that this drains.
 *
 * Addresses, not names. Resolving a kernel address needs `/proc/kallsyms` and a
 * user address needs the symbol table of whatever mapped it, so the output
 * carries raw addresses plus the map each fell in, and a resolver turns them
 * into names afterwards where the symbols are available. Emitting a name this
 * could not verify would be inventing one.
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/perf_event.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

// ========================================================================
// CONFIGURATION
// ========================================================================

#define MAX_CPUS 256
#define MAX_CHAIN 127        /* deep enough for real stacks, bounded on purpose */
#define PAGES 8              /* ring buffer pages per cpu, plus one for the head */

static volatile sig_atomic_t stop_now = 0;
static void on_signal(int s) { (void)s; stop_now = 1; }

// ========================================================================
// THE SAMPLE RECORD
// ========================================================================

/* The layout perf writes for PERF_RECORD_SAMPLE with the fields asked for, in
   the order the kernel documents. Reading it any other way silently misaligns. */
struct sample_head {
    struct perf_event_header header;
    uint64_t ip;
    uint32_t pid, tid;
    uint64_t time;
    uint64_t nr;             /* call chain length, then that many addresses */
};

static int perf_open(int cpu, int freq, pid_t target) {
    struct perf_event_attr a;
    memset(&a, 0, sizeof a);
    a.type = PERF_TYPE_SOFTWARE;
    a.config = PERF_COUNT_SW_CPU_CLOCK;
    a.size = sizeof a;
    a.sample_type = PERF_SAMPLE_IP | PERF_SAMPLE_TID | PERF_SAMPLE_TIME
                  | PERF_SAMPLE_CALLCHAIN;
    a.sample_freq = freq;
    a.freq = 1;
    a.disabled = 1;
    a.exclude_idle = 1;      /* an idle cpu is not where the time went */
    a.wakeup_events = 1;
    return (int)syscall(SYS_perf_event_open, &a, target, cpu, -1, 0);
}

// ========================================================================
// DRAINING A RING
// ========================================================================

/* One folded line per sample: the addresses from the outermost frame inward,
   semicolon separated, then a count of 1. Folding identical stacks is left to
   whatever reads this, which is what every other folded-stack tool expects. */
static void drain(void *base, size_t sz, FILE *out, unsigned long *seen)
{
    struct perf_event_mmap_page *meta = base;
    uint64_t head = __atomic_load_n(&meta->data_head, __ATOMIC_ACQUIRE);
    uint64_t tail = meta->data_tail;
    char *data = (char *)base + sysconf(_SC_PAGESIZE);
    size_t mask = sz - 1;

    while (tail < head) {
        struct perf_event_header *h = (void *)(data + (tail & mask));
        if (h->size == 0) break;

        /* A record can wrap the end of the ring, so it is copied flat before
           being read. Reading it in place works until the day it wraps, and
           then it reads whatever was at the start of the buffer. */
        char flat[8192];
        size_t n = h->size < sizeof flat ? h->size : sizeof flat;
        for (size_t i = 0; i < n; i++) flat[i] = data[(tail + i) & mask];

        if (h->type == PERF_RECORD_SAMPLE && n >= sizeof(struct sample_head)) {
            struct sample_head *s = (void *)flat;
            uint64_t *chain = (uint64_t *)(flat + sizeof(struct sample_head));
            uint64_t depth = s->nr;
            if (depth > MAX_CHAIN) depth = MAX_CHAIN;
            if (sizeof(struct sample_head) + depth * 8 <= n) {
                fprintf(out, "{\"t\":%.6f,\"pid\":%u,\"tid\":%u,\"kind\":\"cpu\",\"stack\":[",
                        (double)s->time / 1e9, s->pid, s->tid);
                int wrote = 0;
                /* outermost first, so the folded line reads like a stack */
                for (int64_t i = (int64_t)depth - 1; i >= 0; i--) {
                    uint64_t a = chain[i];
                    if (a >= PERF_CONTEXT_MAX) continue;   /* a marker, not an address */
                    fprintf(out, "%s\"0x%llx\"", wrote ? "," : "",
                            (unsigned long long)a);
                    wrote = 1;
                }
                fprintf(out, "]}\n");
                (*seen)++;
            }
        }
        tail += h->size;
    }
    __atomic_store_n(&meta->data_tail, head, __ATOMIC_RELEASE);
}

// ========================================================================
// MAIN
// ========================================================================

int main(int argc, char **argv)
{
    int freq = 99;                 /* off any round number, to avoid lockstep */
    int secs = 0;
    pid_t target = -1;             /* -1 is every process on the machine */

    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--freq") && i + 1 < argc) freq = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--seconds") && i + 1 < argc) secs = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--pid") && i + 1 < argc) target = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--help")) {
            fprintf(stderr,
                "execviz-cpu: sample where the cpu is, with call chains\n\n"
                "  --freq N        samples per second per cpu (default 99)\n"
                "  --seconds N     stop after N seconds (default: until interrupted)\n"
                "  --pid N         one process, rather than the whole machine\n\n"
                "Writes one JSON record per sample to stdout, addresses outermost\n"
                "first. Names are resolved afterwards, because a name this could\n"
                "not verify would be invented.\n");
            return 0;
        }
    }

    signal(SIGINT, on_signal);
    signal(SIGTERM, on_signal);

    long ncpu = sysconf(_SC_NPROCESSORS_ONLN);
    if (ncpu > MAX_CPUS) ncpu = MAX_CPUS;
    long page = sysconf(_SC_PAGESIZE);
    size_t map_sz = (size_t)page * (PAGES + 1);

    int fds[MAX_CPUS];
    void *rings[MAX_CPUS];
    int opened = 0;

    for (long c = 0; c < ncpu; c++) {
        fds[c] = perf_open((int)c, freq, target);
        if (fds[c] < 0) {
            if (opened == 0 && c == ncpu - 1) {
                fprintf(stderr,
                    "execviz-cpu: cannot open a sampling event (%s).\n"
                    "  This needs CAP_PERFMON, or perf_event_paranoid at 2 or lower:\n"
                    "    sudo sysctl kernel.perf_event_paranoid=2\n"
                    "  or grant it once:\n"
                    "    sudo setcap cap_perfmon+ep ./execviz-cpu\n",
                    strerror(errno));
                return 1;
            }
            rings[c] = NULL;
            continue;
        }
        rings[c] = mmap(NULL, map_sz, PROT_READ | PROT_WRITE, MAP_SHARED, fds[c], 0);
        if (rings[c] == MAP_FAILED) { close(fds[c]); fds[c] = -1; rings[c] = NULL; continue; }
        ioctl(fds[c], PERF_EVENT_IOC_RESET, 0);
        ioctl(fds[c], PERF_EVENT_IOC_ENABLE, 0);
        opened++;
    }

    if (!opened) {
        fprintf(stderr, "execviz-cpu: no cpu could be sampled\n");
        return 1;
    }
    fprintf(stderr, "execviz-cpu: sampling %d cpu(s) at %d Hz%s\n",
            opened, freq, target < 0 ? ", whole machine" : "");

    unsigned long seen = 0;
    time_t began = time(NULL);
    while (!stop_now) {
        struct pollfd pfd[MAX_CPUS];
        int n = 0;
        for (long c = 0; c < ncpu; c++) {
            if (fds[c] < 0) continue;
            pfd[n].fd = fds[c];
            pfd[n].events = POLLIN;
            n++;
        }
        poll(pfd, n, 200);
        for (long c = 0; c < ncpu; c++) {
            if (rings[c]) drain(rings[c], (size_t)page * PAGES, stdout, &seen);
        }
        fflush(stdout);
        if (secs && time(NULL) - began >= secs) break;
    }

    for (long c = 0; c < ncpu; c++) {
        if (fds[c] >= 0) { ioctl(fds[c], PERF_EVENT_IOC_DISABLE, 0); close(fds[c]); }
        if (rings[c]) munmap(rings[c], map_sz);
    }
    /* The count is reported because a run that sampled nothing and a run that
       found an idle machine produce the same empty output otherwise. */
    fprintf(stderr, "execviz-cpu: %lu samples\n", seen);
    return 0;
}
