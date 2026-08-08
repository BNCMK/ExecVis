// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: demo.c
//  script_path: execviz-c/demo.c
//  module_name: demo
//  version: 0.53.1
//  description: define EXECVIZ_IMPLEMENTATION include "execviz.h" include <unistd.h> include <stdio.h>
//  kind: module
//  spec: internal
//  internal_dependencies: execviz.h
//  external_dependencies: stdio.h, unistd.h
//  features: demo
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

/* a native program that records what it did */

// ========================================================================
// CONSTANTS
// ========================================================================
#define EXECVIZ_IMPLEMENTATION
#include "execviz.h"
#include <unistd.h>
#include <stdio.h>

// ========================================================================
// INTERNALS
// ========================================================================

static void inner(void) {
    execviz_span s = execviz_begin("parse_header", "call", 0);
    usleep(20000);
    execviz_end(s, EXECVIZ_OK);
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

int main(void) {
    const char *c = getenv("EXECVIZ_COLLECTOR");
    execviz_init(c ? c : "http://127.0.0.1:8900", "native-1", "codec");

    execviz_span root = execviz_begin("decode_frame", "call", 0);
    inner();

    execviz_span io = execviz_begin("read_block", "io", 0);
    usleep(35000);
    execviz_end(io, EXECVIZ_OK);

    execviz_span bad = execviz_begin("checksum", "call", 0);
    usleep(10000);
    execviz_fail(bad, "checksum mismatch at offset 4096");

    /* fan-in: the join names the children it waited for and keeps its own
       parent, because parenting it to a child would place it outside that
       child in time */
    execviz_span a = execviz_begin("fetch_user", "io", root);
    usleep(8000);
    execviz_end(a, EXECVIZ_OK);
    execviz_span b = execviz_begin("fetch_orders", "io", root);
    usleep(9000);
    execviz_end(b, EXECVIZ_OK);
    execviz_span join = execviz_begin("profile_fanin_join", "call", root);
    execviz_link(join, a);
    execviz_link(join, b);
    execviz_end(join, EXECVIZ_OK);

    /* a queue crossing, and a line attributed to the span that wrote it */
    execviz_span q = execviz_begin("enqueue_job", "queue", root);
    execviz_lifecycle(q, "claimed");
    execviz_log("info", "worker picked up the job");
    execviz_lifecycle(q, "released");
    execviz_end(q, EXECVIZ_OK);

    /* never ended: an unfinished span in a language with no exceptions */
    execviz_begin("awaiting_device", "wait", 0);

    execviz_end(root, EXECVIZ_OK);
    int n = execviz_flush();
    fprintf(stderr, "native flushed %d spans\n", n);
    return 0;
}
