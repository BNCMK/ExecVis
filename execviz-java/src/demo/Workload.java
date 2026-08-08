// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: Workload.java
//  script_path: execviz-java/src/demo/Workload.java
//  module_name: Workload
//  version: 0.53.1
//  description: A real JVM service, traced. Requests run concurrently on a pool, a worker claims stamped work off a queue, one request fails, and a lock never releases. */
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: 
//  features: Workload
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

package demo;

import execviz.ExecViz;
import java.util.*;
import java.util.concurrent.*;

/** A real JVM service, traced. Requests run concurrently on a pool, a worker
 *  claims stamped work off a queue, one request fails, and a lock never
 *  releases. */
public class Workload {

    static void sleep(long ms) { try { Thread.sleep(ms); } catch (InterruptedException ignored) {} }

    public static void main(String[] args) throws Exception {
        String collector = System.getenv().getOrDefault("EXECVIZ_COLLECTOR", "http://127.0.0.1:8900");
        ExecViz.install(new ExecViz.Config(collector, "jvm-1", "api", 300));

        // a pool that carries the span across submissions, so call sites do not
        // have to remember to wrap
        ExecutorService pool = ExecViz.decorate(Executors.newFixedThreadPool(4));
        BlockingQueue<Map<String,Object>> jobs = new LinkedBlockingQueue<>();

        String rootId = ExecViz.start("service", "call");
        String stuck = ExecViz.start("reconcile_lock", "wait", rootId, null, "billing");
        ExecViz.lifecycle(stuck, "suspended", null);

        Thread worker = new Thread(() -> {
            for (;;) {
                try {
                    Map<String,Object> m = jobs.take();
                    if (m.containsKey("__stop__")) return;
                    String qs = ExecViz.claim(m);
                    ExecViz.in("process_" + m.get("item"), "call", "worker", () -> {
                        ExecViz.log("info", "processing " + m.get("item"));
                        sleep(30);
                        return null;
                    });
                    ExecViz.release(qs);
                } catch (Exception e) { return; }
            }
        }, "worker-1");
        worker.setDaemon(true);
        worker.start();

        int failed = 0;
        for (int uid = 0; uid < 3; uid++) {
            final int u = uid;
            try {
                ExecViz.in("GET /profile/" + u, "call", "api", () -> {
                    ExecViz.gather("profile_fanin", pool, List.of(
                        () -> ExecViz.in("fetch_user_" + u, "call", "users", () -> {
                            ExecViz.log("info", "loading user " + u);
                            ExecViz.in("db_user", "io", () -> { sleep(40 + u * 15L); return null; });
                            return null;
                        }),
                        () -> ExecViz.in("fetch_orders_" + u, "call", "orders", () -> {
                            ExecViz.in("db_orders", "io", () -> {
                                sleep(60);
                                if (u == 2) {
                                    ExecViz.log("error", "order store unavailable");
                                    throw new IllegalStateException("order store unavailable");
                                }
                                return null;
                            });
                            return null;
                        })
                    ));
                    ExecViz.in("render_" + u, "call", "render", () -> { sleep(20); return null; });
                    String qid = ExecViz.start("enqueue_job", "queue");
                    Map<String,Object> m = ExecViz.stamp("invoice-" + u);
                    // the stamp must name the queue span, not the enclosing one
                    m.put("span", qid);
                    jobs.put(m);
                    return null;
                });
            } catch (Exception e) { failed++; }
        }

        sleep(400);
        jobs.put(Map.of("__stop__", true));
        pool.shutdown();
        pool.awaitTermination(5, TimeUnit.SECONDS);
        ExecViz.end(rootId, null);
        ExecViz.shutdown();
        System.err.println("jvm workload complete, " + failed + " failed");
    }
}
