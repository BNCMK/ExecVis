// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: ExecViz.java
//  script_path: execviz-java/src/execviz/ExecViz.java
//  module_name: ExecViz
//  version: 0.53.1
//  description: execviz capture adapter for the JVM.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: 
//  features: ExecViz, capture, adapter
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

package execviz;

import java.io.PrintStream;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicLong;

/**
 * execviz capture adapter for the JVM.
 *
 * The carrier is an InheritableThreadLocal, which is the closest thing the
 * platform offers to an automatic one: a thread started inside a span inherits
 * it, including a virtual thread started per task. It does NOT cross a pooled
 * executor, because a pool thread was created long before the work it later
 * runs. That is a real limit of the runtime rather than of this adapter, so the
 * adapter provides {@link #wrap} and {@link #decorate} to carry the span across
 * a submission explicitly, and says plainly that unwrapped pool submissions lose
 * the parent link.
 *
 * Coverage: the JVM offers no cheap per-call hook comparable to a Python profile
 * function without bytecode instrumentation, so this records explicitly
 * instrumented work plus the propagation around it.
 */
public final class ExecViz {

    public record Config(String collector, String hostId, String domain, long flushMs) {
        public static Config defaults() {
            String c = System.getenv("EXECVIZ_COLLECTOR");
            return new Config(c != null ? c : "http://127.0.0.1:8900", "jvm", "app", 500);
        }
    }

    private static final class Span {
        String id, trace, parent, name, kind, status, domain;
        List<String> links = new ArrayList<>();
        double start; Double end;
        List<Map<String,Object>> lifecycle = new ArrayList<>(), events = new ArrayList<>();
        Map<String,Object> attrs = new LinkedHashMap<>();
    }

    private static final InheritableThreadLocal<String> ACTIVE = new InheritableThreadLocal<>();
    private static final Map<String, Span> PENDING = new ConcurrentHashMap<>();

    /**
     * The most spans held while delivery is failing.
     *
     * An unreachable collector otherwise means unbounded growth inside the
     * program being observed; a tracing tool that eventually kills the process
     * it is watching, which is the worst failure this design could have.
     */
    private static final int MAX_PENDING = 20000;
    private static final java.util.concurrent.atomic.AtomicLong DROPPED =
        new java.util.concurrent.atomic.AtomicLong();
    private static final java.util.concurrent.atomic.AtomicLong DROPPED_TRACES =
        new java.util.concurrent.atomic.AtomicLong();
    private static final java.util.concurrent.atomic.AtomicLong DROPPED_ABNORMAL =
        new java.util.concurrent.atomic.AtomicLong();

    /**
     * Drops whole traces when the buffer is full, never individual spans.
     *
     * Two invariants the specification states and the core's retention already
     * honours. Trace-level only: dropping a span whose siblings remain punches a
     * hole in that trace's graph, leaving a parent pointing at a child that no
     * longer exists. And bias toward the abnormal: a trace holding an error or a
     * still-running span is never dropped while an ordinary one remains, because
     * those are the traces someone came looking for.
     */
    private static void evict() {
        if (PENDING.size() <= MAX_PENDING) return;

        java.util.Map<String, java.util.List<Span>> traces = new java.util.HashMap<>();
        for (Span s : PENDING.values()) {
            String t = s.trace != null ? s.trace : s.id;
            traces.computeIfAbsent(t, k -> new java.util.ArrayList<>()).add(s);
        }
        java.util.List<String> order = new java.util.ArrayList<>(traces.keySet());
        order.sort((a, b) -> {
            boolean ka = keepTrace(traces.get(a)), kb = keepTrace(traces.get(b));
            if (ka != kb) return ka ? 1 : -1;            // ordinary traces first
            return Double.compare(lastAt(traces.get(a)), lastAt(traces.get(b)));
        });
        for (String t : order) {
            if (PENDING.size() <= MAX_PENDING) break;
            java.util.List<Span> group = traces.get(t);
            boolean abnormal = keepTrace(group);
            for (Span s : group) {
                if (PENDING.remove(s.id) != null) DROPPED.incrementAndGet();
            }
            DROPPED_TRACES.incrementAndGet();
            if (abnormal) DROPPED_ABNORMAL.incrementAndGet();
        }
    }

    /** A trace holding an error or a still-running span is kept while any
     *  ordinary trace remains. */
    private static boolean keepTrace(java.util.List<Span> group) {
        for (Span s : group) {
            if (s.end == null || "error".equals(s.status)) return true;
        }
        return false;
    }

    private static double lastAt(java.util.List<Span> group) {
        double last = 0;
        for (Span s : group) {
            double at = s.end != null ? s.end : s.start;
            if (at > last) last = at;
        }
        return last;
    }
    private static final Map<String, String> SENT = new ConcurrentHashMap<>();
    private static final AtomicLong SEQ = new AtomicLong();
    private static Config cfg = Config.defaults();
    private static String traceId;
    private static ScheduledExecutorService flusher;
    private static HttpClient http;

    private ExecViz() {}

    private static String sid() {
        // Hex of a long is not a fixed width, so pad rather than slice blindly.
        String h = Long.toHexString(System.nanoTime() ^ (SEQ.incrementAndGet() << 20));
        if (h.length() >= 12) return h.substring(h.length() - 12);
        return "0".repeat(12 - h.length()) + h;
    }
    private static double now() { return System.currentTimeMillis() / 1000.0; }

    public static void install(Config c) {
        cfg = c;
        traceId = sid();
        http = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(5)).build();
        flusher = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread t = new Thread(r, "execviz-flush"); t.setDaemon(true); return t;
        });
        flusher.scheduleAtFixedRate(ExecViz::flush, cfg.flushMs(), cfg.flushMs(), TimeUnit.MILLISECONDS);
    }

    public static void shutdown() {
        flush();
        if (flusher != null) flusher.shutdownNow();
    }

    public static String currentSpan() { return ACTIVE.get(); }

    /** Phase one. A span is recorded the moment it opens, so a process that dies
     *  mid-span still reports it as open rather than not at all. */
    public static String start(String name, String kind, String parent,
                               List<String> links, String domain) {
        Span s = new Span();
        s.id = sid(); s.trace = traceId;
        s.parent = parent != null ? parent : ACTIVE.get();
        if (links != null) s.links.addAll(links);
        s.name = name; s.kind = kind; s.start = now(); s.status = "running";
        s.domain = domain != null ? domain : cfg.domain();
        s.attrs.put("thread", Thread.currentThread().getName());
        PENDING.put(s.id, s);
        // the bound is enforced where the buffer grows: a burst between two
        // flushes is exactly when it would otherwise run away
        evict();
        return s.id;
    }
    public static String start(String name, String kind) { return start(name, kind, null, null, null); }

    /** Phase two. The collector upserts on span id, so completion updates the
     *  open row rather than duplicating it. */
    public static void end(String id, Throwable err) {
        Span s = PENDING.get(id);
        if (s == null) return;
        s.end = now();
        if (err != null) { s.status = "error"; s.attrs.put("error", String.valueOf(err.getMessage())); }
        else s.status = "ok";
    }

    public static void lifecycle(String id, String type, Map<String,Object> context) {
        Span s = PENDING.get(id);
        if (s == null) return;
        Map<String,Object> e = new LinkedHashMap<>();
        e.put("t", now()); e.put("type", type);
        if (context != null) e.put("context", context);
        s.lifecycle.add(e);
    }

    /** A log line belongs to the span that was running when it was written (2.6). */
    public static void log(String level, String msg) {
        String id = ACTIVE.get();
        if (id == null) return;
        Span s = PENDING.get(id);
        if (s == null) return;
        Map<String,Object> e = new LinkedHashMap<>();
        e.put("t", now()); e.put("level", level); e.put("msg", msg);
        s.events.add(e);
    }

    public interface Body<T> { T run() throws Exception; }

    /** Runs the body inside a span, with the span active for anything it reaches. */
    public static <T> T in(String name, String kind, String domain, Body<T> body) throws Exception {
        String prev = ACTIVE.get();
        String id = start(name, kind, prev, null, domain);
        ACTIVE.set(id);
        try {
            T r = body.run();
            end(id, null);
            return r;
        } catch (Exception e) {
            end(id, e);
            throw e;
        } finally {
            if (prev == null) ACTIVE.remove(); else ACTIVE.set(prev);
        }
    }
    public static <T> T in(String name, String kind, Body<T> body) throws Exception {
        return in(name, kind, null, body);
    }

    /** Carries the current span across a submission. A pool thread predates the
     *  work it runs, so without this the parent link is lost. */
    public static Runnable wrap(Runnable r) {
        String captured = ACTIVE.get();
        return () -> {
            String prev = ACTIVE.get();
            ACTIVE.set(captured);
            try { r.run(); } finally { if (prev == null) ACTIVE.remove(); else ACTIVE.set(prev); }
        };
    }
    public static <T> Callable<T> wrap(Callable<T> c) {
        String captured = ACTIVE.get();
        return () -> {
            String prev = ACTIVE.get();
            ACTIVE.set(captured);
            try { return c.call(); } finally { if (prev == null) ACTIVE.remove(); else ACTIVE.set(prev); }
        };
    }
    /** An executor that wraps everything submitted through it, so call sites do
     *  not have to remember. */
    public static ExecutorService decorate(ExecutorService inner) {
        return new AbstractExecutorServiceDelegate(inner);
    }

    /** A fan-in: the join keeps the enclosing scope as its parent and lists every
     *  child in links (3.0a). Parenting it to a child would place it outside its
     *  parent in time. */
    public static void gather(String name, ExecutorService pool, List<Body<Void>> bodies) throws Exception {
        String parent = ACTIVE.get();
        List<String> ids = new ArrayList<>();
        List<Future<?>> futures = new ArrayList<>();
        for (int i = 0; i < bodies.size(); i++) {
            final int idx = i;
            String id = start(name + "[" + i + "]", "call", parent, null, null);
            ids.add(id);
            futures.add(pool.submit(() -> {
                String prev = ACTIVE.get();
                ACTIVE.set(id);
                try { bodies.get(idx).run(); end(id, null); }
                catch (Exception e) { end(id, e); throw new CompletionException(e); }
                finally { if (prev == null) ACTIVE.remove(); else ACTIVE.set(prev); }
                return null;
            }));
        }
        Exception first = null;
        for (Future<?> f : futures) {
            try { f.get(); } catch (Exception e) { if (first == null) first = e; }
        }
        String join = start(name + "_join", "call", parent, ids, null);
        end(join, null);
        if (first != null) throw first;
    }

    /** Stamps the current span onto a value being handed across a boundary. */
    public static Map<String,Object> stamp(Object item) {
        Map<String,Object> m = new LinkedHashMap<>();
        m.put("item", item);
        m.put("trace_id", traceId);
        m.put("span", ACTIVE.get());
        return m;
    }
    /** Reads the stamp back on the receiving side. */
    public static String claim(Map<String,Object> msg) {
        String qs = (String) msg.get("span");
        if (qs == null) return null;
        lifecycle(qs, "claimed", Map.of("thread", Thread.currentThread().getName()));
        String receiver = ACTIVE.get();
        if (receiver != null) {
            Span s = PENDING.get(receiver);
            if (s != null && !s.links.contains(qs)) s.links.add(qs);
        }
        ACTIVE.set(qs);
        return qs;
    }
    public static void release(String qs) {
        if (qs == null) return;
        lifecycle(qs, "released", Map.of("thread", Thread.currentThread().getName()));
        end(qs, null);
        ACTIVE.remove();
    }

// ========================================================================
// DELIVERY
// ========================================================================

    /**
     * Reads what the collector said about the batch.
     *
     * It names every span it refused and why. That explanation reached nobody
     * while the reply was discarded and any 200 treated as complete success, so
     * an adapter emitting malformed spans went on emitting them with nothing to
     * show its author.
     *
     * Reported once per distinct reason: a bug in an adapter repeats every
     * second, and a message that repeats with it is one nobody reads.
     */
    private static final java.util.Set<String> REPORTED_REFUSALS =
        java.util.concurrent.ConcurrentHashMap.newKeySet();
    private static final java.util.concurrent.atomic.AtomicLong REFUSED =
        new java.util.concurrent.atomic.AtomicLong();

    public static long refusedByCollector() { return REFUSED.get(); }

    private static void reportRefusals(String body) {
        if (body == null || body.isEmpty()) return;
        try {
            // Whitespace-tolerant on purpose: assuming a peer formats JSON
            // compactly is an assumption about someone else's serialiser, and it
            // fails silently; the reply is read, nothing matches, and no
            // refusal is ever reported.
            int n = readInt(body, "rejected");
            if (n <= 0) return;
            REFUSED.addAndGet(n);
            int at = body.indexOf("\"reasons\"");
            if (at < 0) return;
            int open = body.indexOf('[', at);
            int end = body.indexOf(']', open);
            if (open < 0 || end < 0) return;
            for (String raw : body.substring(open + 1, end).split("\",")) {
                String reason = raw.replace("\"", "").trim();
                if (reason.isEmpty()) continue;
                // the span id changes every time, so key on the explanation itself
                int colon = reason.indexOf(':');
                String key = colon >= 0 ? reason.substring(colon + 1).trim() : reason;
                if (!REPORTED_REFUSALS.add(key)) continue;
                System.err.println("execviz: the collector refused a span; " + reason);
                System.err.println("  (further spans refused for this reason will not be reported again)");
            }
        } catch (Exception e) {
            // an unreadable reply must never break delivery
        }
    }

    /** Reads an integer field regardless of the spacing a peer chose. */
    private static int readInt(String body, String field) {
        int at = body.indexOf("\"" + field + "\"");
        if (at < 0) return 0;
        int i = body.indexOf(':', at);
        if (i < 0) return 0;
        i++;
        while (i < body.length() && Character.isWhitespace(body.charAt(i))) i++;
        StringBuilder d = new StringBuilder();
        while (i < body.length() && Character.isDigit(body.charAt(i))) d.append(body.charAt(i++));
        return d.length() == 0 ? 0 : Integer.parseInt(d.toString());
    }

    public static synchronized int flush() {
        List<Span> batch = new ArrayList<>();
        for (Span s : PENDING.values()) {
            String state = (s.end != null) + "|" + s.status;
            if (!state.equals(SENT.get(s.id))) batch.add(s);
        }
        if (batch.isEmpty()) return 0;
        long reportedLoss = DROPPED.get(), reportedTraces = DROPPED_TRACES.get();
        long reportedAbnormal = DROPPED_ABNORMAL.get();
        String body = encode(batch);
        try {
            HttpRequest req = HttpRequest.newBuilder(URI.create(cfg.collector() + "/api/ingest"))
                    .header("Content-Type", "application/json")
                    .timeout(Duration.ofSeconds(8))
                    .POST(HttpRequest.BodyPublishers.ofString(body)).build();
            HttpResponse<String> res = http.send(req, HttpResponse.BodyHandlers.ofString());
            if (res.statusCode() != 200) return 0;
            reportRefusals(res.body());
        } catch (Exception e) {
            return 0;                      // retried on the next tick
        }
        for (Span s : batch) {
            SENT.put(s.id, (s.end != null) + "|" + s.status);
            if (s.end != null) PENDING.remove(s.id);
        }
        // reported, so the counters start again rather than being resent forever
        DROPPED.addAndGet(-reportedLoss);
        DROPPED_TRACES.addAndGet(-reportedTraces);
        DROPPED_ABNORMAL.addAndGet(-reportedAbnormal);
        return batch.size();
    }

    private static String encode(List<Span> batch) {
        StringBuilder b = new StringBuilder();
        b.append("{\"host_id\":").append(str(cfg.hostId()));
        long lost = DROPPED.get();
        if (lost > 0) {
            // the collector is told the record is incomplete, and how badly
            b.append(",\"dropped\":").append(lost)
             .append(",\"dropped_traces\":").append(DROPPED_TRACES.get())
             .append(",\"dropped_abnormal\":").append(DROPPED_ABNORMAL.get());
        }
        b.append(",\"spans\":[");
        for (int i = 0; i < batch.size(); i++) {
            Span s = batch.get(i);
            if (i > 0) b.append(',');
            b.append("{\"span_id\":").append(str(s.id))
             .append(",\"trace_id\":").append(str(s.trace))
             .append(",\"parent_span_id\":").append(s.parent == null ? "null" : str(s.parent))
             .append(",\"links\":").append(arr(s.links))
             .append(",\"name\":").append(str(s.name))
             .append(",\"kind\":").append(str(s.kind))
             .append(",\"start\":").append(s.start)
             .append(",\"end\":").append(s.end == null ? "null" : s.end.toString())
             .append(",\"status\":").append(str(s.status))
             .append(",\"origin\":\"semantic\"")
             // which clock stamped this, so skew analysis knows what it compares
             .append(",\"clock_source\":\"System.currentTimeMillis\"")
             .append(",\"host_id\":").append(str(cfg.hostId()))
             .append(",\"domain\":").append(str(s.domain))
             .append(",\"lifecycle\":").append(maps(s.lifecycle))
             .append(",\"events\":").append(maps(s.events))
             .append(",\"attributes\":").append(map(s.attrs))
             .append('}');
        }
        return b.append("]}").toString();
    }

    private static String arr(List<String> xs) {
        StringBuilder b = new StringBuilder("[");
        for (int i = 0; i < xs.size(); i++) { if (i > 0) b.append(','); b.append(str(xs.get(i))); }
        return b.append(']').toString();
    }
    private static String maps(List<Map<String,Object>> xs) {
        StringBuilder b = new StringBuilder("[");
        for (int i = 0; i < xs.size(); i++) { if (i > 0) b.append(','); b.append(map(xs.get(i))); }
        return b.append(']').toString();
    }
    @SuppressWarnings("unchecked")
    private static String map(Map<String,Object> m) {
        StringBuilder b = new StringBuilder("{");
        boolean first = true;
        for (Map.Entry<String,Object> e : m.entrySet()) {
            if (!first) b.append(','); first = false;
            b.append(str(e.getKey())).append(':');
            Object v = e.getValue();
            if (v == null) b.append("null");
            else if (v instanceof Number || v instanceof Boolean) b.append(v);
            else if (v instanceof Map) b.append(map((Map<String,Object>) v));
            else b.append(str(String.valueOf(v)));
        }
        return b.append('}').toString();
    }
    private static String str(String s) {
        if (s == null) return "null";
        StringBuilder b = new StringBuilder("\"");
        for (char c : s.toCharArray()) {
            switch (c) {
                case '"' -> b.append("\\\"");
                case '\\' -> b.append("\\\\");
                case '\n' -> b.append("\\n");
                case '\r' -> b.append("\\r");
                case '\t' -> b.append("\\t");
                default -> { if (c < 0x20) b.append(String.format("\\u%04x", (int) c)); else b.append(c); }
            }
        }
        return b.append('"').toString();
    }

    /** Minimal delegate so submissions carry the span without call sites opting in. */
    private static final class AbstractExecutorServiceDelegate extends AbstractExecutorService {
        private final ExecutorService inner;
        AbstractExecutorServiceDelegate(ExecutorService inner) { this.inner = inner; }
        @Override public void execute(Runnable command) { inner.execute(wrap(command)); }
        @Override public void shutdown() { inner.shutdown(); }
        @Override public List<Runnable> shutdownNow() { return inner.shutdownNow(); }
        @Override public boolean isShutdown() { return inner.isShutdown(); }
        @Override public boolean isTerminated() { return inner.isTerminated(); }
        @Override public boolean awaitTermination(long t, TimeUnit u) throws InterruptedException {
            return inner.awaitTermination(t, u);
        }
    }
}
