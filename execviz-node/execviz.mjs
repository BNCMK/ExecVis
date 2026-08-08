// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: execviz.mjs
//  script_path: execviz-node/execviz.mjs
//  module_name: execviz
//  version: 0.53.1
//  description: execviz capture adapter for Node.js.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: node:async_hooks, node:crypto, node:os
//  features: execviz, capture, adapter
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

// execviz capture adapter for Node.js.
//
// Obligations are the adapter contract: schema-conforming spans, a
// carrier that survives suspension, context stamped on crossings, two-phase
// writes, and no self-tracing. Analysis stays in the core.
//
// The carrier here is AsyncLocalStorage, which is the runtime's native
// equivalent of contextvars: it survives an await and is inherited by anything
// the body schedules, so the parent link stays correct under concurrency.
//
// Capture completeness: Node offers no cheap per-call hook equivalent to
// sys.setprofile, so this adapter records explicitly instrumented work plus the
// context propagation around it. That is stated rather than papered over.

import { AsyncLocalStorage } from 'node:async_hooks';
import { randomUUID } from 'node:crypto';
import { hostname } from 'node:os';

const als = new AsyncLocalStorage();

let cfg = {
  collector: 'http://127.0.0.1:8900',
  hostId: hostname(),
  domain: 'app',
  flushMs: 700,
  enabled: false,
};

const pending = new Map();      // span_id -> span (phase one and two both queued)
let traceId = null;
let timer = null;

const sid = () => randomUUID().replace(/-/g, '').slice(0, 12);
const now = () => Date.now() / 1000;

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function install(options = {}) {
  cfg = { ...cfg, ...options, enabled: true };
  traceId = options.traceId || sid();
  if (!timer) {
    timer = setInterval(flush, cfg.flushMs);
    if (timer.unref) timer.unref();
  }
  return cfg;
}

export function setDomain(d) { cfg.domain = d; }
/**
 * The span covering the whole process, when one has been opened.
 *
 * `enterWith` binds to the execution context it is called from, and an attach
 * shim that loads this module with a dynamic `import` is already inside a
 * promise, so the binding applied to that continuation and not to the main
 * script. A process-level fallback is what was meant: a line written
 * anywhere in this process, with no nearer span, belongs to the run.
 */
let processSpan = null;

export function setProcessSpan(id) { processSpan = id; }

export function currentSpan() { return als.getStore()?.spanId ?? processSpan; }

/**
 * Makes a span current for everything that follows on this execution context.
 *
 * `withSpan` scopes a span to a callback, which is right for a request and
 * wrong for a process: attaching with no source change needs one
 * span covering the whole run, and there is no callback to wrap it in.
 * `enterWith` is AsyncLocalStorage's own mechanism for exactly that.
 */
export function enter(spanId) {
  als.enterWith({ spanId });
  setProcessSpan(spanId);
}

// ========================================================================
// INTERNALS
// ========================================================================

/** The kernel thread id of the thread this is called on. */
function threadTid() {
  try {
    // worker threads have their own kernel thread id; the main thread's is the pid
    const wt = process.__execvizWorker;
    if (wt && wt.threadId > 0) return wt.nativeTid ?? process.pid;
  } catch { /* fall through to the process id */ }
  return process.pid;
}

function record(span) { pending.set(span.span_id, span); }

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function refusedByCollector() { return refusedCount; }

// ========================================================================
// INTERNALS
// ========================================================================

async function reportRefusals(res) {
  let reply;
  try {
    reply = await res.json();
  } catch {
    return;                       // an unreadable reply must never break delivery
  }
  if (!reply || !reply.rejected) return;
  refusedCount += reply.rejected;
  for (const reason of reply.reasons ?? []) {
    // the span id changes every time, so key on the explanation itself
    const key = reason.includes(':') ? reason.slice(reason.indexOf(':') + 1).trim() : reason;
    if (reportedRefusals.has(key)) continue;
    reportedRefusals.add(key);
    process.stderr.write(
      `execviz: the collector refused a span; ${reason}\n` +
      `  (further spans refused for this reason will not be reported again)\n`);
  }
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

// Phase one. A span is queued the moment it starts, so a process that dies
// mid-span still reports it as open rather than never at all.
export function spanStart(name, kind = 'call', opts = {}) {
  if (!cfg.enabled) return null;
  const id = sid();
  record({
    span_id: id,
    trace_id: traceId,
    parent_span_id: opts.parent ?? currentSpan(),
    links: opts.links ?? [],
    name, kind,
    start: now(),
    end: null,
    status: 'running',
    lifecycle: [],
    origin: 'semantic',
    host_id: cfg.hostId,
    clock_source: 'Date.now',
    domain: opts.domain ?? cfg.domain,
    // The thread this ran on, so a span can be put against the syscalls that
    // thread made. Without it `witness` has nothing to match on and reports
    // every span as claimed-but-not-performed, which is a wrong answer rather
    // than a missing one. Node runs its main work on the main thread, whose
    // kernel thread id equals the process id.
    attributes: { tid: threadTid(), ...(opts.attributes ?? {}) },
    events: [],
  });
  return id;
}

// Phase two. Updates in place; the collector upserts on span_id.
export function spanEnd(id, status = 'ok', attributes) {
  const s = pending.get(id);
  if (!s) return;
  s.end = now();
  s.status = status;
  if (attributes) s.attributes = { ...s.attributes, ...attributes };
  pending.set(id, s);
}

export function spanLifecycle(id, type, context) {
  const s = pending.get(id);
  if (!s) return;
  const ev = { t: now(), type };
  if (context) ev.context = context;
  s.lifecycle.push(ev);
}

// The active span rides the carrier for the whole callback, including anything
// it awaits or schedules.
export function withSpan(name, kind, fn, opts = {}) {
  const id = spanStart(name, kind, opts);
  return als.run({ spanId: id }, async () => {
    try {
      const r = await fn(id);
      spanEnd(id, 'ok');
      return r;
    } catch (e) {
      spanEnd(id, 'error', { error: String(e && e.message || e) });
      throw e;
    }
  });
}

// Scheduling work is a crossing: the child inherits its creator as parent
// because the carrier is captured at creation time.
export function spawn(fn, name = 'task', kind = 'spawn') {
  const parent = currentSpan();
  const id = spanStart(name, kind, { parent });
  return als.run({ spanId: id }, async () => {
    try {
      const r = await fn();
      spanEnd(id, 'ok');
      return r;
    } catch (e) {
      spanEnd(id, 'error', { error: String(e && e.message || e) });
      throw e;
    }
  });
}

// Awaiting records suspended and resumed. The resume may land in a different
// tick with a different stack; that change is what the events carry.
export async function awaitSpan(promise, name, kind = 'wait') {
  const id = spanStart(name, kind);
  spanLifecycle(id, 'suspended');
  return als.run({ spanId: id }, async () => {
    try {
      const r = await promise;
      spanLifecycle(id, 'resumed');
      spanEnd(id, 'ok');
      return r;
    } catch (e) {
      spanEnd(id, 'error', { error: String(e && e.message || e) });
      throw e;
    }
  });
}

// A Promise.all is a fan-in: the continuation records the additional causes in
// links rather than pretending to several parents.
export async function gatherSpan(name, thunks) {
  const parent = currentSpan();
  const childIds = [];
  const runs = thunks.map((t, i) => {
    const id = spanStart(`${name}[${i}]`, 'call', { parent });
    childIds.push(id);
    return als.run({ spanId: id }, async () => {
      try { const r = await t(); spanEnd(id, 'ok'); return r; }
      catch (e) { spanEnd(id, 'error'); throw e; }
    });
  });
  const results = await Promise.all(runs);
  // The join is contained by the scope that called gather, so that is its
  // parent; every child that fed it is a secondary cause and goes in links.
  // Parenting the join to a child would place it outside its parent in time,
  // which breaks causal containment.
  const join = spanStart(`${name}_join`, 'call', { parent, links: childIds });
  spanEnd(join, 'ok');
  return results;
}

// Context stamped onto a crossing the adapter can see, and read back on the far
// side, so causality is preserved at the moment it exists rather than guessed.
export function stamp(message = {}) {
  return { ...message, __execviz__: { trace_id: traceId, span: currentSpan() } };
}

export function claim(message, name = 'claimed_work') {
  const ctx = message?.__execviz__;
  if (!ctx) return { item: message, spanId: null };
  traceId = ctx.trace_id;
  const receiver = currentSpan();
  if (receiver) {
    const s = pending.get(receiver);
    if (s && !s.links.includes(ctx.span)) s.links.push(ctx.span);
  }
  spanLifecycle(ctx.span, 'claimed', { host: cfg.hostId });
  return { item: { ...message, __execviz__: undefined }, spanId: ctx.span };
}

export function release(spanId) {
  if (!spanId) return;
  spanLifecycle(spanId, 'released', { host: cfg.hostId });
  spanEnd(spanId, 'ok');
}

// Delivery: push straight to a collector. Sent spans are re-sent once their
// second phase lands so completion updates the collector's row.
const sentState = new Map();

export async function flush() {
  if (!cfg.enabled || pending.size === 0) return 0;
  const batch = [];
  for (const [id, s] of pending) {
    const state = `${s.end !== null}|${s.status}`;
    if (sentState.get(id) !== state) batch.push(s);
  }
  if (batch.length === 0) return 0;
  try {
    const res = await fetch(`${cfg.collector.replace(/\/$/, '')}/api/ingest`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ host_id: cfg.hostId, spans: batch }),
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    await reportRefusals(res);
    for (const s of batch) sentState.set(s.span_id, `${s.end !== null}|${s.status}`);
    for (const [id, s] of pending) if (s.end !== null && sentState.has(id)) pending.delete(id);
    return batch.length;
  } catch {
    return 0;              // retried on the next tick; nothing is dropped
  }
}

export async function shutdown() {
  await flush();
  if (timer) { clearInterval(timer); timer = null; }
}
