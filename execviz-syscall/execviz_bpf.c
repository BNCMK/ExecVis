// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: execviz_bpf.c
//  script_path: execviz-syscall/execviz_bpf.c
//  module_name: execviz_bpf
//  version: 0.53.1
//  description: A BPF program is attached to the raw syscall entry tracepoint, filtered to a single process, and writes one record per syscall into a ring buffer that this process drains and prints as JSON lines.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: bpf.h, errno.h, mman.h, poll.h, signal.h, stdio.h, stdlib.h, string.h, syscall.h, time.h, unistd.h, utsname.h
//  features: execviz bpf
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

/* execviz syscall adapter: kernel tracepoints.
 *
 * A BPF program is attached to the raw syscall entry tracepoint, filtered to a
 * single process, and writes one record per syscall into a ring buffer that
 * this process drains and prints as JSON lines.
 *
 * The program is hand-assembled rather than compiled from C, because a BPF
 * compiler is not a reasonable dependency for a capture adapter that has to run
 * wherever the traced program runs.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <sys/utsname.h>
#include <signal.h>
#include <time.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <linux/bpf.h>
#include <poll.h>

// ========================================================================
// CONSTANTS
// ========================================================================

#define EXECVIZ_HOST_MAX 96

// ========================================================================
// INTERNALS
// ========================================================================
static char host_buf[EXECVIZ_HOST_MAX];

/* Copies src into dst with the characters JSON cannot carry escaped, never
   exceeding cap and never splitting an escape in half. */
/* A record no reader can read is not a record.

 *
 * Everything above the recorder reads this stream as UTF-8, so one stray byte does
 * not corrupt one line, it makes the WHOLE FILE unreadable and every consumer
 * refuses it at once. Two things produce those bytes: payloads that were never
 * text in the first place, and the bounded slice cutting a multi-byte character
 * in half at the 176th byte. The read side made both common, because reads carry
 * whatever a file or socket happened to hold.
 *
 * So multi-byte sequences are validated before being trusted: a well-formed one
 * is copied through, which keeps real text in other languages intact, and
 * anything malformed or cut short is escaped to \u00XX, which is legible, valid,
 * and reversible. */
static void json_escape_into(char *dst, size_t cap, const char *src) {
    static const char HEXD[] = "0123456789abcdef";
    size_t o = 0;
    for (size_t i = 0; src[i] && o + 2 < cap; ) {
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
            if (o + 3 >= cap) break;
            dst[o++] = esc[0];
            dst[o++] = esc[1];
            i++;
        } else if (ch < 0x20) {
            i++;                      /* other control bytes are not text */
        } else if (ch < 0x80) {
            dst[o++] = (char)ch;
            i++;
        } else {
            /* A well-formed lead byte is not enough. 0xC0 and 0xC1 can only ever
               begin an overlong encoding, and a lead byte followed by valid
               continuation bytes can still encode an overlong form, a surrogate,
               or a value past the last code point. Copying those through emits a
               byte sequence no reader will accept, and one of them makes the
               whole capture unreadable. */
            int need = (ch & 0xE0) == 0xC0 ? 1
                     : (ch & 0xF0) == 0xE0 ? 2
                     : (ch & 0xF8) == 0xF0 ? 3 : -1;
            int ok = need > 0 && ch != 0xC0 && ch != 0xC1 && ch <= 0xF4;
            for (int k = 1; ok && k <= need; k++) {
                if (((unsigned char)src[i + k] & 0xC0) != 0x80) ok = 0;
            }
            if (ok && need == 2) {
                unsigned char b1 = (unsigned char)src[i + 1];
                if (ch == 0xE0 && b1 < 0xA0) ok = 0;              /* overlong */
                if (ch == 0xED && b1 >= 0xA0) ok = 0;             /* surrogate */
            }
            if (ok && need == 3) {
                unsigned char b1 = (unsigned char)src[i + 1];
                if (ch == 0xF0 && b1 < 0x90) ok = 0;              /* overlong */
                if (ch == 0xF4 && b1 >= 0x90) ok = 0;             /* past U+10FFFF */
            }
            if (ok) {
                if (o + (size_t)need + 2 >= cap) break;
                for (int k = 0; k <= need; k++) dst[o++] = src[i + k];
                i += (size_t)need + 1;
            } else {
                if (o + 7 >= cap) break;
                dst[o++] = '\\'; dst[o++] = 'u'; dst[o++] = '0'; dst[o++] = '0';
                dst[o++] = HEXD[ch >> 4]; dst[o++] = HEXD[ch & 15];
                i++;
            }
        }
    }
    dst[o] = '\0';
}

static int bpf(int cmd, union bpf_attr *attr) { return syscall(SYS_bpf, cmd, attr, sizeof(*attr)); }

/* instruction helpers, mirroring the kernel's encoding */

// ========================================================================
// CONSTANTS
// ========================================================================
#define INSN(C,D,S,O,I) ((struct bpf_insn){ .code=(C), .dst_reg=(D), .src_reg=(S), .off=(O), .imm=(I) })
#define MOV64_REG(D,S)   INSN(0xbf,D,S,0,0)
#define MOV64_IMM(D,I)   INSN(0xb7,D,0,0,I)
#define RSH64_IMM(D,I)   INSN(0x77,D,0,0,I)
#define JNE_IMM(D,I,OFF) INSN(0x55,D,0,OFF,I)
#define JEQ_IMM(D,I,OFF) INSN(0x15,D,0,OFF,I)
#define CALL(F)          INSN(0x85,0,0,0,F)
#define EXIT()           INSN(0x95,0,0,0,0)
#define LDX_DW(D,S,OFF)  INSN(0x79,D,S,OFF,0)
#define STX_DW(D,S,OFF)  INSN(0x7b,D,S,OFF,0)
#define LD_MAP_FD(D,FD)  INSN(0x18,D,1,0,FD)      /* followed by a zero insn */
#define ZERO_INSN()      INSN(0,0,0,0,0)
#define ADD64_IMM(D,I)   INSN(0x07,D,0,0,I)
#define JNE_IMM_J(D,I,OFF) INSN(0x55,D,0,OFF,I)
#define JSLE_IMM(D,I,OFF)  INSN(0xd5,D,0,OFF,I)   /* signed: a short read returns <= 0 */

/* helper ids */
#define H_KTIME 5
#define H_PIDTGID 14
#define H_RB_RESERVE 131
#define H_RB_SUBMIT 132
#define H_RB_DISCARD 133
#define H_PROBE_READ_USER 112
#define H_PROBE_READ_KERNEL 113
#define H_GET_COMM 16

/* TWO ARCHITECTURES, TWO TABLES, AND A REFUSAL FOR EVERYTHING ELSE.
 *
 * The register offsets and syscall numbers below are per-architecture. The same
 * offset names a different register on each, and the same number names a
 * different syscall, so a table used on the wrong machine loads cleanly and
 * reports incorrect values reported without error: reading a "file descriptor" out of whatever
 * happens to sit at that offset, and filtering for "write" on a number that
 * means something else.
 *
 * Reporting wrong values without error is an unacceptable failure. So the
 * architecture is selected at compile time, checked again at startup, and the
 * table is proved against the running kernel before any record is believed (see
 * the offset self-check further down). An unsupported architecture is refused
 * with its name rather than served badly.
 */
#if !defined(__x86_64__) && !defined(__aarch64__)
#error "execviz_bpf supports x86_64 and aarch64: the pt_regs offsets and syscall numbers are architecture-specific, and building elsewhere would produce a program that reports incorrect values reported without error. A new architecture means a new offset table, not a recompile."
#endif

#if defined(__x86_64__)
#define EXECVIZ_ARCH "x86_64"

/* x86_64 pt_regs offsets. The raw tracepoint hands us a pointer to the register
   state the syscall was entered with, and the System V calling convention puts
   the first three arguments in rdi, rsi, rdx; for write(2) that is fd, buffer
   and length. */
#define PT_REGS_DX 96      /* arg 3: count */
#define PT_REGS_SI 104     /* arg 2: buf   */
#define PT_REGS_DI 112     /* arg 1: fd    */
#define PT_REGS_ORIG_AX 120 /* the syscall number, still readable at exit */
#define PT_REGS_NR_SIZE 8

#define SYS_read_NR 0
#define SYS_pread64_NR 17
#define SYS_readv_NR 19
#define SYS_recvfrom_NR 45
#define SYS_recvmsg_NR 47
#define SYS_write_NR 1

#elif defined(__aarch64__)
#define EXECVIZ_ARCH "aarch64"

/* aarch64 pt_regs offsets.
 *
 * struct pt_regs on arm64 opens with `u64 regs[31]`, then sp, pc and pstate,
 * and then orig_x0. The procedure call standard passes the first three
 * arguments in x0, x1 and x2, so for write(2) fd/buffer/length are  the
 * first three slots of that array rather than three scattered offsets as on
 * x86_64. The syscall number is in x8 at entry, and orig_x0 is what survives to
 * exit, because the exit path reads the number from the array rather than
 * from the context.
 *
 * These are the layout as published by the kernel for arm64. They are proved
 * against the running kernel at startup rather than trusted, because a wrong
 * table here is exactly the failure this whole comment exists to prevent. */
#define PT_REGS_DI 0       /* arg 1: fd     (x0) */
#define PT_REGS_SI 8       /* arg 2: buf    (x1) */
#define PT_REGS_DX 16      /* arg 3: count  (x2) */
#define PT_REGS_X8 64      /* the syscall number at ENTRY (x8) */
/* At EXIT, x8 is not guaranteed to still hold the syscall number: the register
   is caller-saved and the kernel has run a whole syscall since. arm64 keeps the
   number in its own field, `syscallno`, which is what survives. Reading x8 here
   instead makes every read-family test fail to match, the exit program discards
   every reservation, and the read side captures NOTHING while looking healthy.
   struct pt_regs: regs[31] then sp, pc, pstate (264), orig_x0 (272), and
   syscallno at 280 as a 32-bit value, so this read is 4 bytes, not 8. */
#define PT_REGS_ORIG_AX 280
#define PT_REGS_NR_SIZE 4

/* arm64 uses the asm-generic syscall table, so these numbers differ from
   x86_64's entirely. write is 64, not 1. */
#define SYS_read_NR 63
#define SYS_pread64_NR 67
#define SYS_readv_NR 65
#define SYS_recvfrom_NR 207
#define SYS_recvmsg_NR 212
#define SYS_write_NR 64

#endif

/* How much of each write is carried up. A log line is evidence, not a payload,
   and the ring buffer is a fixed budget shared with every other record. */
#define PAYLOAD 176

#define RB_SIZE (1 << 18)

// ========================================================================
// TYPES
// ========================================================================

struct rec {
    unsigned long long ts, pid_tgid, nr;
    unsigned long long fd, len;
    /* 0 = written by this process, 1 = read into it. Without this a read
       recorded at exit and a write recorded at entry are the same shape, and a
       reader cannot tell which direction the bytes travelled. */
    unsigned long long dir;
    char data[PAYLOAD];
    /* The running task's name, taken in the kernel at the moment of the call.
       Resolving it afterwards from /proc/<pid>/comm loses every process that
       exits before the record is drained, which is exactly the short-lived work
       nobody can catch another way: build steps, shell commands, forked workers,
       anything that crashes. Those arrived as a bare pid with no way to tell
       what they were. Placed last so no existing field moves. */
    char comm[16];
};

/* Is `pid` the traced program, or something it started?
 
   Walks up /proc/<pid>/stat's parent chain. A shell script that runs python and
   node is four processes, and the interesting output is in the three it
   started. This is done in userspace because the walk needs several reads and a
   verified program has no business looping over /proc. */

// ========================================================================
// INTERNALS
// ========================================================================

static int is_descendant(int pid, int root, int depth) {
    if (pid <= 0 || depth > 24) return 0;
    if (pid == root) return 1;
    char path[64], buf[512];
    snprintf(path, sizeof path, "/proc/%d/stat", pid);
    FILE *f = fopen(path, "r");
    if (!f) return 0;
    if (!fgets(buf, sizeof buf, f)) { fclose(f); return 0; }
    fclose(f);
    /* the command field can contain spaces and parentheses, so parse after it */
    char *close_paren = strrchr(buf, ')');
    if (!close_paren) return 0;
    int ppid = 0;
    if (sscanf(close_paren + 1, " %*c %d", &ppid) != 1) return 0;
    return is_descendant(ppid, root, depth + 1);
}

/* Which program a pid is, and where a descriptor points.

// ========================================================================
// TYPES
// ========================================================================
 *
 * Both are read from /proc in userspace. A verified program has no business
 * walking a path table, and the kernel does not carry the answer in the record
 * anyway; but a system-wide capture that says "pid 4021 wrote a line" and
 * nothing else is a log no reader can read.
 *
 * Cached, because a busy machine writes far more often than it opens: the same
 * pid and descriptor answer thousands of lines. */
struct idcache { int pid; int fd; char val[160]; };

// ========================================================================
// INTERNALS
// ========================================================================
static struct idcache comm_cache[256], path_cache[512];

/* What kind of write is this?
 *
 * Every write is emitted. Nothing is dropped for not looking like a log line,
 * an eventfd counter, a pipe carrying one byte to wake a loop, the framing of a
 * binary protocol: those are all things a program did, and deciding they are
 * not worth showing is deciding for the developer what they are allowed to look
 * at. The classification is here so a reader can sort, filter or ignore by
 * kind; the record is here either way.
 *
 *   text    mostly printable; a line somebody wrote
 *   binary  mostly non-printable; protocol framing, a serialised payload
 *   signal  one or two bytes; a wakeup, a semaphore post, a poke down a pipe
 *   blank   whitespace only; a separator the program emitted on purpose
 */
/* The decision path that produced a record, as a short string.
 *
 * Hashed rather than the data, so two records that went through the same code
 * path carry the same digest whatever they say. That turns "did you treat
 * yourself differently" into a comparison of hashes rather than a promise
 *.
 *
 * Every decision this program makes about a record appears here. Adding a rule
 * without adding it to this string is the one mistake that would defeat the
 * whole idea, so the fields are kept in the same order as the code that sets
 * them and each is one character.
 */
static void policy_of(char *dst, size_t cap, int suppressed,
                      const char *kind, int truncated, int fd_resolved, int hexed) {
    /* WHO the record is about is deliberately absent.
     *
     * A first version put `self=0|1` in here, which gave every one of the
     * recorder's own records a different digest from an identically treated
     * foreign one, and would have reported the whole capture as special-cased.
     * The policy describes the TREATMENT. Whether a treatment is applied only to
     * the recorder is then a real question with a real answer, rather than a
     * tautology. */
    snprintf(dst, cap, "v1.sup=%d.kind=%s.trunc=%d.fd=%d.hex=%d",
             suppressed, kind, truncated, fd_resolved, hexed);
}

/* A small, fixed, published hash. Not a security primitive: it only has to make
   two different decision paths land on different values. */

static unsigned long long policy_digest(const char *s) {
    unsigned long long h = 1469598103934665603ULL;      /* FNV-1a 64 */
    for (size_t i = 0; s[i]; i++) {
        h ^= (unsigned char)s[i];
        h *= 1099511628211ULL;
    }
    return h;
}

static const char *classify(const char *s, size_t n) {
    if (n == 0) return "empty";
    size_t printable = 0, considered = 0, space = 0;
    for (size_t i = 0; i < n; i++) {
        unsigned char ch = (unsigned char)s[i];
        if (ch == '\n' || ch == '\t' || ch == '\r' || ch == ' ') { space++; continue; }
        considered++;
        if (ch >= 0x20) printable++;
    }
    if (considered == 0) return "blank";
    /* Two bytes or fewer of substance is a poke, not a message. Naming it means
       a reader can hide every eventfd wakeup on the machine with one filter
       rather than losing them from the record entirely. */
    if (considered <= 2 && n <= 8) return "signal";
    return printable * 10 >= considered * 9 ? "text" : "binary";
}

/* Bytes that cannot go in a JSON string, rendered so nothing is lost.

 *
 * A binary write is still evidence: hex keeps it readable and reversible rather
 * than reducing it to "something happened here". */
static void hex_into(char *dst, size_t cap, const char *src, size_t n) {
    static const char H[] = "0123456789abcdef";
    size_t o = 0;
    for (size_t i = 0; i < n && o + 3 < cap; i++) {
        unsigned char ch = (unsigned char)src[i];
        dst[o++] = H[ch >> 4];
        dst[o++] = H[ch & 15];
    }
    dst[o] = '\0';
}

static const char *comm_of(int pid);

/* The name the kernel gave for this record, escaped for output.

 *
 * `comm_of` reads /proc/<pid>/comm, which is gone the moment the process is, so
 * anything short-lived arrived as a bare pid. The record now carries the name
 * taken at the instant of the call, and that is used whenever it is present. */
static const char *comm_of_rec(const struct rec *r, int pid) {
    static char out[96];
    char raw[17] = {0};
    memcpy(raw, r->comm, 16);
    if (raw[0]) {
        json_escape_into(out, sizeof out, raw);
        return out;
    }
    return comm_of(pid);
}

static const char *comm_of(int pid) {
    unsigned h = ((unsigned)pid) & 255;
    if (comm_cache[h].pid == pid && comm_cache[h].val[0]) return comm_cache[h].val;
    char p[64], buf[160] = {0};
    snprintf(p, sizeof p, "/proc/%d/comm", pid);
    FILE *f = fopen(p, "r");
    if (f) {
        if (fgets(buf, sizeof buf, f)) {
            size_t n = strlen(buf);
            while (n && (buf[n - 1] == '\n' || buf[n - 1] == ' ')) buf[--n] = '\0';
        }
        fclose(f);
    }
    if (!buf[0]) snprintf(buf, sizeof buf, "pid%d", pid);
    comm_cache[h].pid = pid;
    json_escape_into(comm_cache[h].val, sizeof comm_cache[h].val, buf);
    return comm_cache[h].val;
}

static const char *path_of(int pid, int fd) {
    if (fd == 1) return "stdout";
    if (fd == 2) return "stderr";
    unsigned h = (((unsigned)pid << 4) ^ (unsigned)fd) & 511;
    if (path_cache[h].pid == pid && path_cache[h].fd == fd && path_cache[h].val[0])
        return path_cache[h].val;
    char p[64], target[160] = {0};
    snprintf(p, sizeof p, "/proc/%d/fd/%d", pid, fd);
    ssize_t n = readlink(p, target, sizeof target - 1);
    if (n > 0) {
        target[n] = '\0';
        path_cache[h].pid = pid; path_cache[h].fd = fd;
        json_escape_into(path_cache[h].val, sizeof path_cache[h].val, target);
        return path_cache[h].val;
    }
    /* The descriptor could not be resolved, which for a process that has
       already exited is the normal case rather than an error. Two things follow.

       It is not cached. Caching it stores a guess under a (pid, fd) key, and
       Linux reuses pids: a later process handed the same descriptor number
       would be told it points at a file belonging to a dead one. A missing
       answer is a gap; a confidently wrong one is worse than no tool.

       And it is reported as unresolved rather than dressed up as a name. `fd7`
       reads like a path when it is the absence of one. */
    static char unresolved[32];
    snprintf(unresolved, sizeof unresolved, "fd%d (unresolved)", fd);
    return unresolved;
}

/* The record output, exempted from capture so that writing a record about a
   write does not produce another record. Nothing else this process does is
   exempt, so an auditor can see every socket write it makes. */
static int out_fd_public = 1;

/* Syscalls this recorder makes purely in order to observe. Everything the
   recorder does that could send data anywhere is deliberately absent from this
   list: socket, connect, sendto, sendmsg, and any write that is not the record
   output.

   THESE NUMBERS ARE PER-ARCHITECTURE, and getting that wrong is not a small
   miss. On the wrong table the recorder fails to recognise its own reads and
   polls, records itself, and each record it writes produces another record: a
   feedback loop that buries every other process on the machine under the
   recorder's own noise. That is what the arm64 test showed the first time it ran
   with other processes present, and it is the same class of failure as a wrong
   register offset. */

static int is_own_overhead(unsigned long long nr, int fd) {
#if defined(__x86_64__)
    switch (nr) {
        case 0:    /* read      */
        case 3:    /* close     */
        case 5:    /* fstat     */
        case 8:    /* lseek     */
        case 17:   /* pread64   */
        case 89:   /* readlink  */
        case 217:  /* getdents  */
        case 257:  /* openat    */
        case 262:  /* newfstatat*/
        case 7:    /* poll      */
        case 23:   /* select    */
        case 35:   /* nanosleep */
        case 202:  /* futex     */
        case 232:  /* epoll_wait*/
        case 271:  /* ppoll     */
#elif defined(__aarch64__)
    /* the asm-generic table: the same observation calls, different numbers, and
       some named differently because arm64 has only the *at and p* variants */
    switch (nr) {
        case 63:   /* read        */
        case 57:   /* close       */
        case 80:   /* fstat       */
        case 62:   /* lseek       */
        case 67:   /* pread64     */
        case 78:   /* readlinkat  */
        case 61:   /* getdents64  */
        case 56:   /* openat      */
        case 79:   /* newfstatat  */
        case 73:   /* ppoll       */
        case 72:   /* pselect6    */
        case 101:  /* nanosleep   */
        case 98:   /* futex       */
        case 22:   /* epoll_pwait */
#endif
            /* Waiting is not sending. This recorder's drain loop polls, and left
               in, that one loop produced 300,000 records about the recorder in
               six seconds and buried every other process on the machine. */
            return 1;
        case SYS_write_NR:  /* write: only the record output itself */
            return fd == out_fd_public;
        default:
            return 0;
    }
}

static unsigned long long self_suppressed = 0;

static volatile int running = 1;
static void stop(int s) { (void)s; running = 0; }

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

int main(int argc, char **argv) {
    /* Everything, by default.
     *
     * This is the recorder: it captures every process on the machine unless told
     * otherwise. A pid argument narrows it, which is useful when debugging one
     * program, but narrowing is the exception; the point of a layer underneath
     * everything is that nothing has to opt in, ask, or know it is there. */
    int target = 0;
    for (int i = 1; i < argc; i++) {
        if (argv[i][0] != '-' && atoi(argv[i]) > 0) target = atoi(argv[i]);
    }
    int follow = 0;                       /* --follow: the target and its descendants */
    for (int i = 1; i < argc; i++) if (!strcmp(argv[i], "--follow")) follow = 1;
    int selfcheck = 0;
    for (int i = 1; i < argc; i++) if (!strcmp(argv[i], "--selfcheck")) selfcheck = 1;
    int root_pid = target;
    if (follow) target = 0;
    int follow_all = (target == 0);       /* nothing to filter on in the kernel */
    int self_pid = getpid();              /* used to exempt ONE descriptor, below */
    const char *host = "local";
    for (int i = 2; i < argc - 1; i++) {
        if (!strcmp(argv[i], "--host")) {
            /* Escaped and bounded, exactly as the preload does: an operator's
               --host value reaches the JSON output, and a quote in it made every
               emitted line unparseable. Two producers of one format must agree
               about what they can emit. */
            json_escape_into(host_buf, sizeof host_buf, argv[i + 1]);
            host = host_buf;
        }
    }

    /* Preflight, before anything is loaded.
     *
     * Each of these is a real reason this cannot run, and each is reported by
     * name. A tool that fails with "operation not permitted" teaches nobody
     * anything; one that says which capability and which kernel version does. */
    {
        struct utsname u;
        if (uname(&u) == 0) {
            if (strcmp(u.machine, EXECVIZ_ARCH) != 0) {
                fprintf(stderr,
                    "execviz_bpf: this machine is %s, and this build carries the %s table.\n"
                    "  The register offsets and syscall numbers are architecture-specific;\n"
                    "  running here would report incorrect values reported without error rather than nothing.\n"
                    "  Build on %s, or cross-compile with that target's toolchain.\n",
                    u.machine, EXECVIZ_ARCH, u.machine);
                return 2;
            }
            int maj = 0, min = 0;
            if (sscanf(u.release, "%d.%d", &maj, &min) == 2) {
                if (maj < 5 || (maj == 5 && min < 8)) {
                    fprintf(stderr,
                        "execviz_bpf: kernel %s is too old.\n"
                        "  BPF ring buffers need 5.8 or newer, and reading user memory\n"
                        "  from a probe needs 5.5. RHEL 8, Ubuntu 18.04 and Debian 10 are\n"
                        "  below this line.\n", u.release);
                    return 2;
                }
            }
        }
    }

    union bpf_attr a;
    memset(&a, 0, sizeof a);
    a.map_type = BPF_MAP_TYPE_RINGBUF;
    a.max_entries = RB_SIZE;
    int rb = bpf(BPF_MAP_CREATE, &a);
    if (rb < 0) {
        fprintf(stderr, "execviz_bpf: cannot create the ring buffer: %s\n", strerror(errno));
        if (errno == EPERM || errno == EACCES) {
            fprintf(stderr,
                "  This needs CAP_BPF and CAP_PERFMON (or root). Grant them with:\n"
                "    setcap cap_bpf,cap_perfmon+ep /usr/local/bin/execviz-record\n"
                "  A container also needs them in its capability set, and a host in\n"
                "  kernel lockdown mode (common with secure boot) will refuse regardless.\n");
        }
        return 1;
    }

    /* The program, hand-assembled.
     *
     * This is the layer underneath every runtime: it reads the buffer a program
     * passed to write(2) directly out of user memory, before libc, before any
     * language's logging library, and without the program cooperating or even
     * knowing. It is the same code path for a shell script, a Go binary and a
     * C program calling printf; which is what LD_PRELOAD could not
     * reach, because glibc resolves its own stdio internally.
     *
     * r6 = ctx, r7 = pid_tgid, r8 = the reserved record, r9 = pt_regs
     */
    /* Every syscall is recorded, and a write additionally carries its buffer.
     *
     * pid_tgid has to survive the ringbuf reserve call, and r6-r9 are already
     * spoken for (ctx, nr, record, pt_regs), so it goes on the stack and is
     * reloaded afterwards. The fd/len/buffer reads run for every syscall; the
     * registers hold something either way; and userspace only reads them as a
     * payload when the call was a write. Branching in a verified program costs
     * instructions; a comparison in userspace costs nothing.
     *
     * Instruction indices are counted by hand below because the jumps are
     * relative and LD_MAP_FD occupies two slots. Exit pair sits at 45/46.
     */
    struct bpf_insn prog[] = {
        MOV64_REG(6, 1),                        /*  0 r6 = ctx                     */
        LDX_DW(7, 6, 8),                        /*  1 r7 = syscall nr              */
        CALL(H_PIDTGID),                        /*  2                              */
        STX_DW(10, 0, -8),                      /*  3 pid_tgid -> stack            */
        RSH64_IMM(0, 32),                       /*  4 r0 = tgid                    */
        JNE_IMM(0, target, follow_all ? 0 : 45),/*  5 -> exit at 51                */
        LDX_DW(9, 6, 0),                        /*  6 r9 = pt_regs                 */
        LD_MAP_FD(1, rb), ZERO_INSN(),          /*  7,8                            */
        MOV64_IMM(2, sizeof(struct rec)),       /*  9                              */
        MOV64_IMM(3, 0),                        /* 10                              */
        CALL(H_RB_RESERVE),                     /* 11                              */
        JEQ_IMM(0, 0, 38),                      /* 12 full: drop -> exit at 51     */
        MOV64_REG(8, 0),                        /* 13 r8 = record                  */
        CALL(H_KTIME),                          /* 14                              */
        STX_DW(8, 0, 0),                        /* 15 rec.ts                       */
        LDX_DW(1, 10, -8),                      /* 16 reload pid_tgid              */
        STX_DW(8, 1, 8),                        /* 17 rec.pid_tgid                 */
        STX_DW(8, 7, 16),                       /* 18 rec.nr                       */
        MOV64_IMM(1, 0),                        /* 19 outbound                     */
        STX_DW(8, 1, 40),                       /* 20 rec.dir = 0                  */

        MOV64_REG(1, 8), ADD64_IMM(1, 24),      /* 21,22 &rec.fd                   */
        MOV64_IMM(2, 8),                        /* 23                              */
        MOV64_REG(3, 9), ADD64_IMM(3, PT_REGS_DI), /* 24,25                        */
        CALL(H_PROBE_READ_KERNEL),              /* 26                              */

        MOV64_REG(1, 8), ADD64_IMM(1, 32),      /* 27,28 &rec.len                  */
        MOV64_IMM(2, 8),                        /* 29                              */
        MOV64_REG(3, 9), ADD64_IMM(3, PT_REGS_DX), /* 30,31                        */
        CALL(H_PROBE_READ_KERNEL),              /* 32                              */

        MOV64_REG(1, 8), ADD64_IMM(1, 48),      /* 33,34 buffer ptr into rec.data  */
        MOV64_IMM(2, 8),                        /* 35                              */
        MOV64_REG(3, 9), ADD64_IMM(3, PT_REGS_SI), /* 36,37                        */
        CALL(H_PROBE_READ_KERNEL),              /* 38                              */
        LDX_DW(3, 8, 48),                       /* 39 r3 = user buffer address     */

        MOV64_REG(1, 8), ADD64_IMM(1, 48),      /* 40,41 &rec.data                 */
        MOV64_IMM(2, PAYLOAD),                  /* 42                              */
        CALL(H_PROBE_READ_USER),                /* 43                              */

        /* the task's own name, taken here rather than looked up later */
        MOV64_REG(1, 8), ADD64_IMM(1, 48 + PAYLOAD), /* 44,45 &rec.comm            */
        MOV64_IMM(2, 16),                       /* 46                              */
        CALL(H_GET_COMM),                       /* 47                              */

        MOV64_REG(1, 8),                        /* 48                              */
        MOV64_IMM(2, 0),                        /* 49                              */
        CALL(H_RB_SUBMIT),                      /* 50                              */
        MOV64_IMM(0, 0),                        /* 51                              */
        EXIT(),                                 /* 52                              */
    };

    static char log[65536];
    memset(&a, 0, sizeof a);
    a.prog_type = BPF_PROG_TYPE_RAW_TRACEPOINT;
    a.insn_cnt = sizeof(prog) / sizeof(prog[0]);
    a.insns = (unsigned long)prog;
    a.license = (unsigned long)"GPL";
    a.log_level = 1; a.log_size = sizeof log; a.log_buf = (unsigned long)log;
    int p = bpf(BPF_PROG_LOAD, &a);
    if (p < 0) { fprintf(stderr, "prog load failed: %s\nverifier:\n%s\n", strerror(errno), log); return 1; }

    memset(&a, 0, sizeof a);
    a.raw_tracepoint.name = (unsigned long)"sys_enter";
    a.raw_tracepoint.prog_fd = p;
    int link = bpf(BPF_RAW_TRACEPOINT_OPEN, &a);
    if (link < 0) { fprintf(stderr, "attach failed: %s\n", strerror(errno)); return 1; }

    /* The read side, at syscall EXIT.
     *
     * A write can be recorded on the way in, because the bytes already exist.
     * A read cannot: on the way in the buffer holds whatever was there before,
     * so recording it at entry captures stale memory and calls it a request.
     * At exit the buffer holds what arrived and the return value says how much
     * of it is real, because rec.len here is the RETURN value rather than
     * the count the caller asked for. A caller asking for 4096 bytes and
     * receiving 12 has read 12.
     *
     * Unlike the enter program this one filters in the kernel. The enter program
     * records every syscall and lets userspace decide, which is affordable
     * because it is one record per call; doing the same at exit would double
     * every record on the machine for the sake of five syscall numbers. The
     * record is reserved before the number is known (it has to be read out of
     * pt_regs), so a call that turns out not to be a read is DISCARDED rather
     * than submitted; a discarded reservation costs nothing downstream.
     *
     * ctx+0 is pt_regs and ctx+8 is the return value. The syscall number is no
     * longer in the context at exit, so it comes from pt_regs->orig_ax.
     * Instruction indices are counted by hand; the exit pair sits at 59/60.
     */
    struct bpf_insn rprog[] = {
        MOV64_REG(6, 1),                        /*  0 r6 = ctx                     */
        LDX_DW(9, 6, 0),                        /*  1 r9 = pt_regs                 */
        LDX_DW(7, 6, 8),                        /*  2 r7 = return value            */
        JSLE_IMM(7, 0, 61),                     /*  3 nothing read -> exit at 65   */
        CALL(H_PIDTGID),                        /*  4                              */
        STX_DW(10, 0, -8),                      /*  5 pid_tgid -> stack            */
        RSH64_IMM(0, 32),                       /*  6 r0 = tgid                    */
        JNE_IMM(0, target, follow_all ? 0 : 57),/*  7 -> exit at 65                */
        LD_MAP_FD(1, rb), ZERO_INSN(),          /*  8,9                            */
        MOV64_IMM(2, sizeof(struct rec)),       /* 10                              */
        MOV64_IMM(3, 0),                        /* 11                              */
        CALL(H_RB_RESERVE),                     /* 12                              */
        JEQ_IMM(0, 0, 51),                      /* 13 full: drop -> exit at 65     */
        MOV64_REG(8, 0),                        /* 14 r8 = record                  */

        /* the number is read narrow on some architectures, so clear the field
           first: a reservation is not zeroed, and half a value over stale bytes
           would compare against whatever was there before */
        MOV64_IMM(1, 0),                        /* 15                              */
        STX_DW(8, 1, 16),                       /* 16 rec.nr = 0                   */

        MOV64_REG(1, 8), ADD64_IMM(1, 16),      /* 17,18 &rec.nr                   */
        MOV64_IMM(2, PT_REGS_NR_SIZE),          /* 19                              */
        MOV64_REG(3, 9), ADD64_IMM(3, PT_REGS_ORIG_AX), /* 20,21                   */
        CALL(H_PROBE_READ_KERNEL),              /* 22                              */
        LDX_DW(2, 8, 16),                       /* 23 r2 = syscall number          */

        JEQ_IMM(2, SYS_read_NR, 9),             /* 24 -> keep at 34                */
        JEQ_IMM(2, SYS_recvfrom_NR, 8),         /* 25                              */
        JEQ_IMM(2, SYS_pread64_NR, 7),          /* 26                              */
        JEQ_IMM(2, SYS_readv_NR, 6),            /* 27                              */
        JEQ_IMM(2, SYS_recvmsg_NR, 5),          /* 28                              */
        MOV64_REG(1, 8),                        /* 29 not a read: give it back     */
        MOV64_IMM(2, 0),                        /* 30                              */
        CALL(H_RB_DISCARD),                     /* 31                              */
        MOV64_IMM(0, 0),                        /* 32                              */
        EXIT(),                                 /* 33                              */

        CALL(H_KTIME),                          /* 34 keep                         */
        STX_DW(8, 0, 0),                        /* 35 rec.ts                       */
        LDX_DW(1, 10, -8),                      /* 36 reload pid_tgid              */
        STX_DW(8, 1, 8),                        /* 37 rec.pid_tgid                 */

        MOV64_REG(1, 8), ADD64_IMM(1, 24),      /* 38,39 &rec.fd                   */
        MOV64_IMM(2, 8),                        /* 40                              */
        MOV64_REG(3, 9), ADD64_IMM(3, PT_REGS_DI), /* 41,42                        */
        CALL(H_PROBE_READ_KERNEL),              /* 43                              */

        STX_DW(8, 7, 32),                       /* 44 rec.len = bytes ACTUALLY read*/
        MOV64_IMM(1, 1),                        /* 45 inbound                      */
        STX_DW(8, 1, 40),                       /* 46 rec.dir = 1                  */

        MOV64_REG(1, 8), ADD64_IMM(1, 48),      /* 47,48 buffer ptr into rec.data  */
        MOV64_IMM(2, 8),                        /* 49                              */
        MOV64_REG(3, 9), ADD64_IMM(3, PT_REGS_SI), /* 50,51                        */
        CALL(H_PROBE_READ_KERNEL),              /* 52                              */
        LDX_DW(3, 8, 48),                       /* 53 r3 = user buffer address     */

        MOV64_REG(1, 8), ADD64_IMM(1, 48),      /* 54,55 &rec.data                 */
        MOV64_IMM(2, PAYLOAD),                  /* 56                              */
        CALL(H_PROBE_READ_USER),                /* 57                              */

        MOV64_REG(1, 8), ADD64_IMM(1, 48 + PAYLOAD), /* 58,59 &rec.comm            */
        MOV64_IMM(2, 16),                       /* 60                              */
        CALL(H_GET_COMM),                       /* 61                              */

        MOV64_REG(1, 8),                        /* 62                              */
        MOV64_IMM(2, 0),                        /* 63                              */
        CALL(H_RB_SUBMIT),                      /* 64                              */
        MOV64_IMM(0, 0),                        /* 65                              */
        EXIT(),                                 /* 66                              */
    };

    memset(&a, 0, sizeof a);
    a.prog_type = BPF_PROG_TYPE_RAW_TRACEPOINT;
    a.insn_cnt = sizeof(rprog) / sizeof(rprog[0]);
    a.insns = (unsigned long)rprog;
    a.license = (unsigned long)"GPL";
    a.log_level = 1; a.log_size = sizeof log; a.log_buf = (unsigned long)log;
    int rp = bpf(BPF_PROG_LOAD, &a);
    if (rp < 0) { fprintf(stderr, "read-side prog load failed: %s\nverifier:\n%s\n", strerror(errno), log); return 1; }

    memset(&a, 0, sizeof a);
    a.raw_tracepoint.name = (unsigned long)"sys_exit";
    a.raw_tracepoint.prog_fd = rp;
    int rlink = bpf(BPF_RAW_TRACEPOINT_OPEN, &a);
    if (rlink < 0) { fprintf(stderr, "read-side attach failed: %s\n", strerror(errno)); return 1; }

    /* stdout is where records go when this is run from a shell */
    out_fd_public = 1;

    long pgsz = sysconf(_SC_PAGESIZE);
    unsigned long *cons = mmap(NULL, pgsz, PROT_READ | PROT_WRITE, MAP_SHARED, rb, 0);
    if (cons == MAP_FAILED) { perror("mmap consumer"); return 1; }
    void *prod_base = mmap(NULL, pgsz + 2 * RB_SIZE, PROT_READ, MAP_SHARED, rb, pgsz);
    if (prod_base == MAP_FAILED) { perror("mmap producer"); return 1; }
    unsigned long *prod = prod_base;
    char *data = (char *)prod_base + pgsz;

    signal(SIGINT, stop); signal(SIGTERM, stop);
    /* wall clock reference so records can be placed on the same timeline as the
       semantic stream, which timestamps in epoch seconds */
    struct timespec wall, mono;
    clock_gettime(CLOCK_REALTIME, &wall);
    clock_gettime(CLOCK_MONOTONIC, &mono);
    double wall0 = wall.tv_sec + wall.tv_nsec / 1e9;
    double mono0 = mono.tv_sec + mono.tv_nsec / 1e9;

    fprintf(stderr, "execviz_bpf: attached to pid %d, host %s\n", target, host);

    /* PROVE THE OFFSET TABLE BEFORE BELIEVING IT.
     *
     * The register offsets above are per-architecture, and the failure they
     * produce when wrong is the worst kind: the program loads, attaches, and
     * reports a plausible file descriptor and length read out of whatever
     * happened to sit at that offset. Nothing downstream can tell that apart
     * from the truth.
     *
     * So this does not trust the table, it tests it. A write is performed whose
     * descriptor, length and syscall number are known exactly, and the record
     * that comes back must agree on all three. If it does not, the table is
     * wrong for this kernel and the only honest thing to do is refuse.
     *
     * This makes a new architecture verifiable by whoever has the
     * hardware rather than by whoever wrote the table: run it on the machine and
     * it either proves itself or says it cannot.
     */
    if (selfcheck) {
        int pfd[2];
        if (pipe(pfd) != 0) { perror("selfcheck pipe"); return 1; }
        static const char probe[] = "execviz-selfcheck-probe";
        const unsigned long long want_len = sizeof probe - 1;
        const unsigned long long want_fd = (unsigned long long)pfd[1];
        ssize_t wn = write(pfd[1], probe, want_len);
        if (wn != (ssize_t)want_len) { perror("selfcheck write"); return 1; }

        int proved = 0, saw_any = 0;
        struct rec seen; memset(&seen, 0, sizeof seen);
        for (int spin = 0; spin < 40 && !proved; spin++) {
            unsigned long cpos = __atomic_load_n(cons, __ATOMIC_ACQUIRE);
            unsigned long ppos = __atomic_load_n(prod, __ATOMIC_ACQUIRE);
            if (cpos == ppos) { struct pollfd pf = { .fd = rb, .events = POLLIN }; poll(&pf, 1, 50); continue; }
            while (cpos < ppos && !proved) {
                unsigned int *hdr = (unsigned int *)(data + (cpos & (RB_SIZE - 1)));
                unsigned int len = __atomic_load_n(hdr, __ATOMIC_ACQUIRE);
                if (len & (1u << 31)) break;
                unsigned int size = len & 0x3fffffff;
                if (!(len & (1u << 30)) && size >= sizeof(struct rec) && size <= RB_SIZE) {
                    struct rec *r = (struct rec *)(hdr + 2);
                    if ((int)(r->pid_tgid >> 32) == self_pid && r->dir == 0 &&
                        r->nr == SYS_write_NR && r->fd == want_fd) {
                        saw_any = 1;
                        seen = *r;
                        if (r->len == want_len && memcmp(r->data, probe, want_len) == 0) proved = 1;
                    }
                }
                cpos += (size + 8 + 7) & ~7UL;
            }
            __atomic_store_n(cons, cpos, __ATOMIC_RELEASE);
        }
        close(pfd[0]); close(pfd[1]);

        if (proved) {
            fprintf(stderr,
                "execviz_bpf: offset table proved on %s: fd, length and syscall number "
                "all matched a write this process made.\n", EXECVIZ_ARCH);
            return 0;
        }
        /* Absent values are reported as absent, not as zero, and neither is disproved. These are different
           facts and the message says which one happened. */
        if (saw_any) {
            fprintf(stderr,
                "execviz_bpf: OFFSET TABLE IS WRONG for this kernel.\n"
                "  A write of %llu bytes to fd %llu was made and the record came back\n"
                "  claiming %llu bytes on fd %llu. The %s table does not describe this\n"
                "  machine's pt_regs, so every record it produces would be nonsense.\n",
                want_len, want_fd, seen.len, seen.fd, EXECVIZ_ARCH);
        } else {
            fprintf(stderr,
                "execviz_bpf: offset table NOT PROVED on %s.\n"
                "  A write of %llu bytes to fd %llu was made and no matching record\n"
                "  arrived. This is not a disproof: the probe may have been missed.\n"
                "  It is a refusal to certify the table on this machine.\n",
                EXECVIZ_ARCH, want_len, want_fd);
        }
        return 1;
    }

    unsigned long long emitted = 0;
    while (running) {
        unsigned long cpos = __atomic_load_n(cons, __ATOMIC_ACQUIRE);
        unsigned long ppos = __atomic_load_n(prod, __ATOMIC_ACQUIRE);
        if (cpos == ppos) { struct pollfd pf = { .fd = rb, .events = POLLIN }; poll(&pf, 1, 100); continue; }
        while (cpos < ppos) {
            unsigned int *hdr = (unsigned int *)(data + (cpos & (RB_SIZE - 1)));
            unsigned int len = __atomic_load_n(hdr, __ATOMIC_ACQUIRE);
            if (len & (1u << 31)) break;                     /* still being written */
            unsigned int size = len & 0x3fffffff;
            /* An upper bound as well as a lower one: a corrupt or hostile
               length must not make this read past the record it describes. */
            if (!(len & (1u << 30)) && size >= sizeof(struct rec) && size <= RB_SIZE) {
                struct rec *r = (struct rec *)(hdr + 2);
                double t = wall0 + (r->ts / 1e9 - mono0);
                /* A write to fd 1 or 2 is a log line, and the kernel handed us
                   the bytes. Everything else is just the call. */
                /* The loop to avoid is narrow, and the old exemption was wide.
                 *
                 * Writing a record about a write produces a write, which is a
                 * loop that ends with the machine full. But that loop runs
                 * through ONE descriptor: the record output. Exempting the whole
                 * process exempted this recorder's SOCKET writes too, which is
                 * exactly the thing anybody auditing it needs to see.
                 *
                 * So the exemption is the output descriptor, not the process.
                 * Every other write this program makes is recorded like any
                 * other program's, and a reader can see for themselves whether
                 * it sends anything anywhere. Self-observation is not proof on
                 * its own (a dishonest recorder could omit itself), because
                 * the README asks for tcpdump as the independent witness; but a
                 * recorder that cannot even be asked the question is worse.
                 */
                /* What this recorder does to observe is suppressed and COUNTED;
                   what it does that could move data is always recorded.
                   
                   Its /proc lookups and its record writes are pure observation
                   overhead, and left in they drown the capture; measured at
                   303,404 records about itself in six seconds. Its connects,
                   sends and any write to a descriptor other than the record
                   output are exactly what an auditor came to check, so those are
                   never suppressed. The suppressed count is reported, because a
                   number no reader can see is a number no reader can question. */
                if ((int)(r->pid_tgid >> 32) == self_pid && is_own_overhead(r->nr, (int)r->fd)) {
                    self_suppressed++;
                    cpos += (size + 8 + 7) & ~7UL;
                    __atomic_store_n(cons, cpos, __ATOMIC_RELEASE);
                    continue;
                }
                if (follow && !is_descendant((int)(r->pid_tgid >> 32), root_pid, 0)) {
                    cpos += (size + 8 + 7) & ~7UL;
                    __atomic_store_n(cons, cpos, __ATOMIC_RELEASE);
                    continue;
                }
                unsigned long long len = r->len < PAYLOAD ? r->len : PAYLOAD;
                /* Every descriptor, not just 1 and 2.
                 *
                 * A service that logs to /var/log/app.log, or through a socket
                 * to journald, was invisible while this only watched stdout and
                 * stderr; and that is how most production software actually
                 * logs. The descriptor is resolved to a path in userspace, where
                 * reading /proc is allowed. */
                /* Outbound writes, and now inbound reads.
                 *
                 * The read records arrive from the exit program and are already
                 * filtered to the read family, so direction alone decides: a
                 * record marked inbound carries bytes that arrived, whatever the
                 * syscall was. This makes a public frontend legible
                 * rather than half-recorded: the response it wrote was always
                 * captured, and the request that caused it now is too. */
                int inbound = r->dir == 1;
                if (((!inbound && r->nr == SYS_write_NR) || inbound) && len > 0) {
                    /* one record can carry several lines; a reader wants lines */
                    /* Splitting on newlines is for readability, not for
                       deciding what counts: a write with no newline in it is one
                       record, and a write that is nothing but newlines is a
                       blank the program chose to emit. */
                    unsigned long long start = 0;
                    for (unsigned long long i = 0; i <= len; i++) {
                        if (i == len || r->data[i] == '\n') {
                            if (i > start) {
                                char raw[PAYLOAD + 1], esc[PAYLOAD * 3 + 8];
                                unsigned long long n2 = i - start;
                                memcpy(raw, r->data + start, n2);
                                raw[n2] = '\0';
                                const char *kind = classify(raw, n2);
                                /* text is escaped and stays readable; anything
                                   else is hex, which is lossless and reversible
                                   rather than reduced to "something happened" */
                                if (!strcmp(kind, "binary") || !strcmp(kind, "signal")) {
                                    hex_into(esc, sizeof esc, raw, n2);
                                } else {
                                    json_escape_into(esc, sizeof esc, raw);
                                }
                                int wpid = (int)(r->pid_tgid >> 32);
                                char pol[96];
                                int hexed = strcmp(kind, "binary") == 0 || strcmp(kind, "signal") == 0;
                                policy_of(pol, sizeof pol, 0, kind,
                                          r->len > PAYLOAD, (int)r->fd > 2, hexed);
                                printf("{\"t\":%.6f,\"tid\":%llu,\"pid\":%d,"
                                       "\"comm\":\"%s\",\"where\":\"%s\","
                                       "\"log\":\"%s\",\"kind\":\"%s\",\"bytes\":%llu,\"level\":\"%s\","
                                       "\"direction\":\"%s\","
                                       "\"truncated\":%s,\"policy\":\"%016llx\",\"policy_text\":\"%s\","
                                       "\"host\":\"%s\",\"arch\":\"" EXECVIZ_ARCH "\",\"src\":\"bpf\"}\n",
                                       t, r->pid_tgid & 0xffffffff, wpid,
                                       comm_of_rec(r, wpid), path_of(wpid, (int)r->fd),
                                       esc, kind, n2,
                                       (!inbound && r->fd == 2) ? "error" : "info",
                                       inbound ? "in" : "out",
                                       r->len > PAYLOAD ? "true" : "false",
                                       policy_digest(pol), pol, host);
                                emitted++;
                            }
                            start = i + 1;
                        }
                    }
                }
                /* the program's name travels with every record, not just with
                   the log lines: a region named `postgres` is one a reader
                   recognises, and `tid 4021` is not.
                   An inbound record is the SECOND half of a call already counted
                   on the way in, so it contributes its payload and not another
                   call, or every read on the machine would be counted twice. */
                if (!inbound) {
                    int wp = (int)(r->pid_tgid >> 32);
                    char pol[96];
                    policy_of(pol, sizeof pol, 0, "call", 0, 0, 0);
                    printf("{\"t\":%.6f,\"tid\":%llu,\"pid\":%d,\"comm\":\"%s\","
                           "\"nr\":%llu,\"fd\":%lld,\"policy\":\"%016llx\",\"policy_text\":\"%s\","
                           "\"host\":\"%s\",\"arch\":\"" EXECVIZ_ARCH "\"}\n",
                           t, r->pid_tgid & 0xffffffff, wp, comm_of_rec(r, wp), r->nr, (long long)r->fd,
                           policy_digest(pol), pol, host);
                    emitted++;
                }
                if ((emitted & 0x3f) == 0) fflush(stdout);
            }
            cpos += (size + 8 + 7) & ~7UL;
            __atomic_store_n(cons, cpos, __ATOMIC_RELEASE);
        }
    }
    fflush(stdout);
    /* Stated rather than hidden: how much of this recorder's own activity was
       left out, and the promise that none of it could have moved data. */
    /* The exemption, DECLARED as a record rather than left as a silence.
     *
     * Suppressed records are not emitted, so without this line the one policy
     * that applies exclusively to the recorder would be the only one absent
     * from the output, which is the shape of a hidden special case
     *. Stating it makes it checkable. */
    {
        char pol[96];
        policy_of(pol, sizeof pol, 1, "suppressed", 0, 0, 0);
        printf("{\"t\":%.6f,\"pid\":%d,\"comm\":\"%s\",\"declared_exemption\":true,"
               "\"suppressed\":%llu,\"policy\":\"%016llx\",\"policy_text\":\"%s\","
               "\"why\":\"this recorder's own reads, opens, waits and record output; "
               "never a socket call or any other write\",\"host\":\"%s\",\"src\":\"bpf\"}\n",
               wall0, self_pid, comm_of(self_pid), self_suppressed,
               policy_digest(pol), pol, host);
        fflush(stdout);
    }

    fprintf(stderr, "execviz_bpf: %llu records; %llu of this recorder's own "
                    "observation calls suppressed (reads, opens, and its record "
                    "output; never a socket call or any other write)\n",
            emitted, self_suppressed);
    return 0;
}
