// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: execviz.js
//  script_path: execviz-browser/execviz.js
//  module_name: execviz
//  version: 0.9.0
//  description: execviz capture adapter for the browser (spec 5.3a.1).
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: 
//  features: execviz, capture, adapter
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

/**
 * execviz capture adapter for the browser (spec 5.3a.1).
 *
 * Half of a user's latency happens in a page. Without this, a request cannot be
 * followed end to end, and the half that the person experienced is the
 * half that is missing.
 *
 * Two things make a page unlike a server runtime, and both are handled below:
 * the document can vanish mid-request, and its clock is not the collector's.
 */
(function (global) {
  'use strict';

  const sid = () => {
    const b = new Uint8Array(6);
    (global.crypto || {}).getRandomValues ? global.crypto.getRandomValues(b)
      : b.forEach((_, i) => (b[i] = (Math.random() * 256) | 0));
    return Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
  };

  class Execviz {
    constructor(opts) {
      opts = opts || {};
      this.collector = (opts.collector || '').replace(/\/$/, '');
      this.hostId = opts.hostId || 'browser';
      this.domain = opts.domain || 'ui';
      this.traceId = sid();
      this.pending = new Map();
      /**
       * The most spans held while delivery is failing.
       *
       * A browser tab is somebody's, not a server's: an unreachable collector
       * used to mean the map grew until the tab did. Smaller than the server
       * adapters' bound for the same reason.
       */
      this.maxPending = opts.maxPending || 5000;
      this.dropped = 0;
      this.droppedTraces = 0;
      this.droppedAbnormal = 0;
      this.oversizedTraces = 0;
      this.refusedByCollector = 0;
      this.reportedRefusals = new Set();
      this.sent = new Map();
      this.batch = opts.batch || 50;
      // The carrier: a promise chain, an event handler and a rAF callback are
      // three different continuations of one logical flow, and only an explicit
      // carrier links them. The browser has no AsyncLocalStorage, so the caller
      // passes the parent or uses withSpan.
      this.stack = [];
      this.run = opts.run || null;

      // A tab can be closed mid-request, and that is exactly the case worth
      // recording. Both events are used because neither fires reliably alone.
      const flushFinal = () => this.flush(true);
      // Guarded because this adapter also runs where a page's lifecycle does
      // not exist: a Web Worker, a service worker, a test harness. Assuming
      // addEventListener threw from the constructor, so the adapter took the
      // program down in exactly the environments nobody tests first.
      if (typeof global.addEventListener === 'function') {
        global.addEventListener('pagehide', flushFinal);
        global.addEventListener('visibilitychange', () => {
          if (global.document && global.document.visibilityState === 'hidden') flushFinal();
        });
      } else {
        // no page lifecycle: a periodic flush is the only guarantee available,
        // and saying so beats appearing to have one
        this.noPageLifecycle = true;
      }
    }

    current() { return this.stack.length ? this.stack[this.stack.length - 1] : null; }

    spanStart(name, kind, opts) {
      opts = opts || {};
      const id = sid();
      this.pending.set(id, {
        span_id: id,
        trace_id: this.traceId,
        parent_span_id: opts.parent !== undefined ? opts.parent : this.current(),
        links: opts.links || [],
        name: name,
        kind: kind || 'call',
        // the page's own clock; the offset against the collector is unknown and
        // is reported rather than corrected (spec 5.5)
        start: Date.now() / 1000,
        end: null,
        status: 'running',
        lifecycle: [],
        events: [],
        origin: 'semantic',
        host_id: this.hostId,
        clock_source: 'browser-wall',
        domain: opts.domain || this.domain,
        attributes: Object.assign({ url: (global.location || {}).pathname || '' },
                                  opts.attributes || {}),
        run: this.run || undefined,
      });
      return id;
    }

    spanEnd(id, status, attributes) {
      const s = this.pending.get(id);
      if (!s) return;
      s.end = Date.now() / 1000;
      s.status = status || 'ok';
      if (attributes) Object.assign(s.attributes, attributes);
      this.evict();
      if (this.pending.size >= this.batch) this.flush();
    }

    /** Runs fn inside a span, keeping the carrier correct across a promise. */
    async withSpan(name, kind, fn, opts) {
      const id = this.spanStart(name, kind, opts);
      this.stack.push(id);
      try {
        const r = await fn(id);
        this.spanEnd(id, 'ok');
        return r;
      } catch (e) {
        this.spanEnd(id, 'error', {
          error_type: (e && e.name) || 'Error',
          error_message: String((e && e.message) || e).slice(0, 300),
        });
        throw e;
      } finally {
        const i = this.stack.lastIndexOf(id);
        if (i >= 0) this.stack.splice(i, 1);
      }
    }

    log(level, msg) {
      const id = this.current();
      const s = id && this.pending.get(id);
      if (!s) return;
      s.events.push({ t: Date.now() / 1000, level: level, msg: String(msg).slice(0, 500) });
    }

    /** Stamps the trace onto an outgoing request so the server side joins it. */
    headers(extra) {
      return Object.assign({
        'X-Execviz-Trace': this.traceId,
        'X-Execviz-Parent': this.current() || '',
      }, extra || {});
    }

    /** Wraps fetch so a network call is a span and the far side is reachable. */
    fetch(input, init) {
      init = init || {};
      const url = typeof input === 'string' ? input : (input && input.url) || '';
      return this.withSpan('GET ' + url, 'external', async () => {
        init.headers = this.headers(init.headers);
        const r = await global.fetch(input, init);
        return r;
      });
    }

    /**
     * A span is re-sent once its second phase lands; a completed span is dropped
     * after delivery. `final` uses the keep-alive send, which the browser
     * completes after the document is gone; an ordinary post would be cancelled
     * by the very unload worth recording.
     */
    /**
     * Drops whole traces when the buffer is full, never individual spans.
     *
     * The same two rules the specification states and every other adapter
     * follows: trace-level only, because dropping a span whose siblings remain
     * punches a hole in that trace's graph; and never a trace holding an error
     * or a still-running span while an ordinary one remains, because those are
     * the traces someone came looking for.
     */
    evict() {
      if (this.pending.size <= this.maxPending) return;
      const traces = new Map();
      for (const [id, s] of this.pending) {
        const t = s.trace_id || id;
        let rec = traces.get(t);
        if (!rec) { rec = { ids: [], last: 0, keep: false }; traces.set(t, rec); }
        rec.ids.push(id);
        rec.last = Math.max(rec.last, s.end != null ? s.end : s.start);
        if (s.end == null || s.status === 'error') rec.keep = true;
      }
      // a single trace larger than the whole buffer cannot be held at trace
      // granularity, and holding it is the growth the bound exists to prevent
      for (const [t, rec] of traces) {
        if (rec.ids.length > this.maxPending) {
          for (const id of rec.ids) { if (this.pending.delete(id)) this.dropped++; }
          this.droppedTraces++;
          this.oversizedTraces++;
          // an oversized trace can still hold an error or a stuck span, and
          // losing that is the worse fact whether or not the trace was too big
          if (rec.keep) this.droppedAbnormal++;
        }
      }
      const order = [...traces.entries()]
        .filter(([t]) => traces.get(t).ids.some((id) => this.pending.has(id)))
        .sort((a, b) => (a[1].keep === b[1].keep ? a[1].last - b[1].last : (a[1].keep ? 1 : -1)));
      for (const [, rec] of order) {
        if (this.pending.size <= this.maxPending) break;
        for (const id of rec.ids) { if (this.pending.delete(id)) this.dropped++; }
        this.droppedTraces++;
        if (rec.keep) this.droppedAbnormal++;
      }
    }

    /**
     * Reads what the collector said about the batch.
     *
     * Reported once per distinct reason: a bug in an adapter repeats every
     * second, and a message that repeats with it is one nobody reads.
     */
    reportRefusals(reply) {
      if (!reply || !reply.rejected) return;
      this.refusedByCollector += reply.rejected;
      for (const reason of reply.reasons || []) {
        const key = reason.includes(':') ? reason.slice(reason.indexOf(':') + 1).trim() : reason;
        if (this.reportedRefusals.has(key)) continue;
        this.reportedRefusals.add(key);
        // console.warn rather than stderr: this is the browser's own channel
        console.warn('execviz: the collector refused a span; ' + reason +
          '\n  (further spans refused for this reason will not be reported again)');
      }
    }

    flush(final) {
      const batch = [];
      for (const [id, s] of this.pending) {
        const state = (s.end !== null ? '1' : '0') + '|' + s.status;
        if (this.sent.get(id) !== state) batch.push(s);
      }
      if (!batch.length) return Promise.resolve(0);
      const payload = { host_id: this.hostId, spans: batch };
      if (this.dropped) {
        payload.dropped = this.dropped;
        payload.dropped_traces = this.droppedTraces;
        payload.dropped_abnormal = this.droppedAbnormal;
        if (this.oversizedTraces) payload.oversized_traces = this.oversizedTraces;
      }
      const body = JSON.stringify(payload);
      const done = () => {
        // reported, so the counters start again rather than being resent forever
        this.dropped = 0;
        this.droppedTraces = 0;
        this.droppedAbnormal = 0;
        this.oversizedTraces = 0;
        for (const s of batch) {
          this.sent.set(s.span_id, (s.end !== null ? '1' : '0') + '|' + s.status);
          if (s.end !== null) this.pending.delete(s.span_id);
        }
      };
      const url = this.collector + '/api/ingest';
      if (final && global.navigator && global.navigator.sendBeacon) {
        const ok = global.navigator.sendBeacon(url, new Blob([body], { type: 'application/json' }));
        if (ok) done();
        return Promise.resolve(batch.length);
      }
      return global.fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: body,
        keepalive: !!final,
      }).then((r) => {
        done();
        // the collector names every span it refused; discarding that leaves an
        // adapter author with nothing to fix
        return r.json().then((reply) => { this.reportRefusals(reply); return batch.length; },
                             () => batch.length);
      }).catch(() => 0);
    }
  }

  global.Execviz = Execviz;
  if (typeof module !== 'undefined' && module.exports) module.exports = Execviz;
})(typeof window !== 'undefined' ? window : globalThis);
