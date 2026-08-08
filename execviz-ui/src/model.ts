// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'model.ts',
  script_path: 'execviz-ui/src/model.ts',
  module_name: 'model',
  version: '0.13.0',
  description: 'Everything derived from a feed, computed once per ingest rather than per frame. The renderer that this replaces recomputed most of it on every frame, because it collapsed on a large capture.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['types'],
  external_dependencies: [],
  features: ['model', 'capture', 'render'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { Span, Feed, Family, familyOf } from './types.js';

// ========================================================================
// TYPES
// ========================================================================

/**
 * Everything derived from a feed, computed once per ingest rather than per
 * frame. The renderer that this replaces recomputed most of it on every frame,
 * because it collapsed on a large capture.
 */
export interface Summary { spans: number; errors: number; open: number }

export interface Cluster {
  id: string; label: string; region: string; slot: number; host: string;
  wx: number; wy: number; wr: number;
  /**
   * How much room this cluster has before it would touch the one beside it, in
   * world units. The renderer draws a nested cluster within this while its
   * neighbours are still in frame, then lets it grow back to its true wr as the
   * level is zoomed into. The cluster's own size never changes; only how much of
   * it is drawn while it sits inside a parent alongside others.
   */
  roomR: number;
  spans: Span[];
  byFamily: Map<Family, Span[]>;
  /** sorted, so "how many are open at t" is two binary searches, not a scan */
  starts: Float64Array;
  ends: Float64Array;
  errorStarts: Float64Array;
  total: number;
  /** present when this tier was built from a rollup rather than from spans */
  summary?: Summary;
}

export interface Host {
  id: string; wx: number; wy: number; wr: number;
  starts: Float64Array; ends: Float64Array; errorStarts: Float64Array; total: number;
  summary?: Summary;
}

export interface RouteScreen { ax: number; ay: number; cx: number; cy: number; bx: number; by: number; thick: number }

export interface Route {
  from: string; to: string; count: number; errors: number;
  crossHost: boolean; variance: number;
  /** screen geometry, cached when the canopy layer is rebuilt */
  screen?: RouteScreen;
  /** kept sorted by start so the active window can be found by search */
  spans: Span[];
  /**
   * Spans on this route that never finished, held apart from the rest.
   *
   * The search below walks back a bounded number of finished spans, which is
   * right for cost and wrong for an unfinished one: a span still open from
   * earlier fell outside that window and vanished from the map; the death
   * signal, dropped for performance. Open spans are rare by definition, so
   * keeping them separately costs nothing and they are always found.
   */
  open: Span[];
}

export interface Model {
  spans: Span[];
  byId: Map<string, Span>;
  clusters: Cluster[];
  clusterById: Map<string, Cluster>;
  hosts: Host[];
  routes: Route[];
  maxRouteCount: number;
  tMax: number;
  /** routes ordered by weight, computed on first use and reused */
  routesByWeight?: Route[];
}

// ========================================================================
// CONSTANTS
// ========================================================================

const OPEN = Number.POSITIVE_INFINITY;

// ========================================================================
// INTERNALS
// ========================================================================

function sortedOf(list: Span[], pick: (s: Span) => number): Float64Array {
  const a = new Float64Array(list.length);
  for (let i = 0; i < list.length; i++) a[i] = pick(list[i]);
  a.sort();
  return a;
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function countLE(a: Float64Array, v: number): number {
  let lo = 0, hi = a.length;
  while (lo < hi) { const m = (lo + hi) >> 1; if (a[m] <= v) lo = m + 1; else hi = m; }
  return lo;
}

export function countLT(a: Float64Array, v: number): number {
  let lo = 0, hi = a.length;
  while (lo < hi) { const m = (lo + hi) >> 1; if (a[m] < v) lo = m + 1; else hi = m; }
  return lo;
}

// ========================================================================
// CONSTANTS
// ========================================================================

export const WORLD_W = 2400;

export const WORLD_H = 1600;

// ========================================================================
// INTERNALS
// ========================================================================

function regionOf(label: string, fallback: string): string {
  switch (label) {
    case 'gateway': case 'MainThread': case 'api': case 'edge-agent': return 'entry';
    case 'sensors': case 'queue': return 'data';
    default: return fallback;
  }
}

// ========================================================================
// CONSTANTS
// ========================================================================

export const CLOCK = 1000;

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/**
 * Places raw capture times on the shared 0..CLOCK clock.
 *
 * The scale comes from the window the store reports, not from whatever subset
 * happens to be in hand, so a delta lands where the spans already held would
 * have landed. The rule the clock has always followed still applies: this is a
 * position and never a duration, and anything to be read as a real quantity
 * travels as its own field.
 */
export function placeOnClock(spans: Span[], window?: { lo: number; hi: number }): Span[] {
  let lo = window ? window.lo : Number.POSITIVE_INFINITY;
  let hi = window ? window.hi : Number.NEGATIVE_INFINITY;
  if (!window) {
    for (const s of spans) {
      if (s.start < lo) lo = s.start;
      if (s.start > hi) hi = s.start;
      if (s.end !== null && s.end > hi) hi = s.end;
    }
  }
  if (!isFinite(lo) || !isFinite(hi) || hi <= lo) { lo = 0; hi = 1; }
  const scale = CLOCK / (hi - lo);
  const at = (v: number) => Math.round((v - lo) * scale * 100) / 100;
  return spans.map((s) => ({
    ...s,
    start: at(s.start),
    end: s.end === null ? null : at(s.end),
    events: s.events.map((e) => ({ ...e, t: at(e.t) })),
    lifecycle: s.lifecycle.map((l) => ({ ...l, t: at(l.t) })),
  }));
}

/**
 * Where a host sits in the world.
 *
 * Shared by the span-built map and the rollup-built overview. It was written
 * twice, once in each, and nothing kept the two copies agreeing; if either had
 * drifted the overview would have placed the same host somewhere else, breaking
 * the one thing the layout guarantees: the map never moves when the data
 * changes.
 */
export function placeHost(index: number, count: number): { wx: number; wy: number; wr: number } {
  const cxm = WORLD_W / 2, cym = WORLD_H / 2;
  if (count <= 1) {
    return { wx: cxm, wy: cym, wr: Math.min(WORLD_W, WORLD_H) * 0.46 };
  }
  // Hosts sit further apart than they strictly need to. Labels that stack
  // still have to go somewhere, and a row pushed down onto the next host is the
  // collision moved rather than solved.
  // A host is given at least as much room as a full row of its clusters needs.
  // Widening the row inside a host that is still packed against its neighbour
  // just moves the collision to the seam between hosts.
  const span = Math.max(WORLD_W * 1.35, count * 660), step = span / count;
  return {
    wx: cxm - span / 2 + step * (index + 0.5),
    wy: cym,
    wr: Math.min(step * 0.44, WORLD_H * 0.42),
  };
}

/** Where a cluster sits inside its host. Shared for the same reason. */
export function placeCluster(
  host: { wx: number; wy: number; wr: number }, region: string, index: number, count: number,
  /** World units the row needs so every circle's own text fits beside it. */
  needed = 0,
): { wx: number; wy: number; wr: number } {
  const band = region === 'entry' ? host.wy - host.wr * 0.6
    : region === 'data' ? host.wy + host.wr * 0.58 : host.wy;
  // The row is as wide as its contents need. Spacing a fixed fraction of the
  // host and then hiding whatever does not fit is deciding what the reader may
  // see; widening the row so each circle keeps its own text is not.
  const sw = Math.max(needed, Math.min(host.wr * 1.24, Math.max(count * 160, 160)));
  return {
    wx: host.wx - sw / 2 + (count > 1 ? (sw * index) / (count - 1) : sw / 2),
    wy: band,
    wr: Math.max(22, Math.min(56, host.wr * 0.115)),
  };
}

export function build(feed: Feed): Model {
  const spans = feed.spans;
  const byId = new Map<string, Span>();
  for (const s of spans) byId.set(s.span_id, s);

  const clusters: Cluster[] = [];
  const clusterById = new Map<string, Cluster>();
  const spansByCluster = new Map<string, Span[]>();
  for (const s of spans) {
    const key = `${s.host_id}/${s.domain ?? 'unknown'}`;
    let list = spansByCluster.get(key);
    if (!list) { list = []; spansByCluster.set(key, list); }
    list.push(s);
  }

  for (const c of feed.clusters) {
    const list = spansByCluster.get(c.id) ?? [];
    const byFamily = new Map<Family, Span[]>();
    for (const s of list) {
      const f = familyOf(s.kind);
      let fl = byFamily.get(f);
      if (!fl) { fl = []; byFamily.set(f, fl); }
      fl.push(s);
    }
    for (const fl of byFamily.values()) fl.sort((a, b) => a.start - b.start);
    const cl: Cluster = {
      id: c.id, label: c.label, region: regionOf(c.label, c.region), slot: c.slot,
      host: c.host, wx: 0, wy: 0, wr: 0, roomR: 0,
      spans: list, byFamily,
      starts: sortedOf(list, (s) => s.start),
      ends: sortedOf(list, (s) => s.end ?? OPEN),
      errorStarts: sortedOf(list.filter((s) => s.status === 'error'), (s) => s.start),
      total: list.length,
    };
    clusters.push(cl);
    clusterById.set(cl.id, cl);
  }

  // hosts contain clusters; both are placed deterministically so the map never
  // moves when the data changes
  const hostIds = [...new Set(clusters.map((c) => c.host))].sort();
  const hosts: Host[] = hostIds.map((id, i) => {
    const { wx, wy, wr } = placeHost(i, hostIds.length);
    const mine = clusters.filter((c) => c.host === id).flatMap((c) => c.spans);
    return {
      id, wx, wy, wr,
      starts: sortedOf(mine, (s) => s.start),
      ends: sortedOf(mine, (s) => s.end ?? OPEN),
      errorStarts: sortedOf(mine.filter((s) => s.status === 'error'), (s) => s.start),
      total: mine.length,
    };
  });

  for (const h of hosts) {
    const mine = clusters.filter((c) => c.host === h.id);
    const byRegion = new Map<string, Cluster[]>();
    for (const c of mine) {
      let l = byRegion.get(c.region);
      if (!l) { l = []; byRegion.set(c.region, l); }
      l.push(c);
    }
    for (const [region, list] of byRegion) {
      list.sort((a, b) => a.slot - b.slot || a.id.localeCompare(b.id));
      const n = list.length;
      // Each circle needs room for itself and for the text that names it, so the
      // row is sized from the widest thing in it rather than from the host.
      const need = list.reduce((a, c) => a + Math.max(96, (c.label?.length ?? 4) * 12 + 34), 0);
      list.forEach((c, i) => {
        const p = placeCluster(h, region, i, n, need);
        c.wx = p.wx; c.wy = p.wy; c.wr = p.wr;
      });
      // Room is half the distance to the nearest neighbour in the row: draw
      // within it and two neighbours cannot touch. A lone cluster in its row has
      // its whole size available and is never held back.
      list.forEach((c, i) => {
        let nearest = Infinity;
        if (i > 0) nearest = Math.min(nearest, Math.abs(c.wx - list[i - 1].wx));
        if (i < n - 1) nearest = Math.min(nearest, Math.abs(list[i + 1].wx - c.wx));
        c.roomR = nearest === Infinity ? c.wr : Math.max(1, nearest / 2);
      });
    }
  }

  // routes: a crossing between two clusters, aggregated
  const routeMap = new Map<string, Route>();
  for (const s of spans) {
    const p = s.parent_span_id ? byId.get(s.parent_span_id) : undefined;
    if (!p) continue;
    const a = `${p.host_id}/${p.domain ?? 'unknown'}`;
    const b = `${s.host_id}/${s.domain ?? 'unknown'}`;
    if (a === b) continue;
    const key = `${a}>${b}`;
    let r = routeMap.get(key);
    if (!r) {
      r = { from: a, to: b, count: 0, errors: 0, crossHost: p.host_id !== s.host_id,
            variance: 0, spans: [], open: [] };
      routeMap.set(key, r);
    }
    r.count++;
    if (s.status === 'error') r.errors++;
    r.spans.push(s);
    if (s.end === null) r.open.push(s);
  }
  const routes = [...routeMap.values()];
  for (const r of routes) {
    r.spans.sort((a, b) => a.start - b.start);
    r.open.sort((a, b) => a.start - b.start);
    const fin = r.spans.filter((s) => s.end !== null).map((s) => (s.end as number) - s.start);
    if (fin.length) {
      const m = fin.reduce((x, y) => x + y, 0) / fin.length;
      r.variance = Math.sqrt(fin.reduce((x, y) => x + (y - m) * (y - m), 0) / fin.length) / (m || 1);
    }
  }

  return {
    spans, byId, clusters, clusterById, hosts, routes,
    // reduced rather than spread: `Math.max(...array)` passes one argument per
    // element and overflows the stack once a federation has enough routes
    maxRouteCount: routes.reduce((mx, r) => (r.count > mx ? r.count : mx), 1),
    tMax: 1000,
  };
}

// ========================================================================
// TYPES
// ========================================================================

export interface Openness { active: number; total: number; worst: 'ok' | 'err' }

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function opennessOf(
  o: { starts: Float64Array; ends: Float64Array; errorStarts: Float64Array; total: number; summary?: Summary },
  t: number,
): Openness {
  // With no spans held, the summary is the only truth available, and it reports
  // the state when it was taken rather than pretending to vary with the playhead
  if (o.starts.length === 0 && o.summary) {
    return { active: o.summary.open, total: o.summary.spans,
             worst: o.summary.errors > 0 ? 'err' : 'ok' };
  }
  return {
    active: countLE(o.starts, t) - countLT(o.ends, t),
    total: o.total,
    worst: countLE(o.errorStarts, t) > 0 ? 'err' : 'ok',
  };
}

/** Spans of a route that are in flight at t, found by search rather than scan. */
export function activeOnRoute(r: Route, t: number, cap: number): Span[] {
  const out: Span[] = [];

  // Unfinished spans first, and never subject to the lookback below. One that
  // started long ago is exactly the span someone is looking for, and it used to
  // disappear from the map as soon as 48 later spans passed it.
  for (const s of r.open) {
    if (s.start > t) break;               // sorted, so nothing later can qualify
    out.push(s);
    if (out.length >= cap) return out;
  }

  let lo = 0, hi = r.spans.length;
  while (lo < hi) { const m = (lo + hi) >> 1; if (r.spans[m].start <= t) lo = m + 1; else hi = m; }
  // Walk back only far enough to fill the budget. A route with a long tail of
  // finished work would otherwise be rescanned every frame, which is the shape
  // of cost that only appears once a capture is large. Finished spans are the
  // only ones this bound applies to.
  const LOOKBACK = 48;
  const stop = Math.max(0, lo - LOOKBACK);
  for (let i = lo - 1; i >= stop && out.length < cap; i--) {
    const s = r.spans[i];
    if (s.end === null) continue;         // already delivered above
    if (s.end >= t) out.push(s);
  }
  return out;
}
