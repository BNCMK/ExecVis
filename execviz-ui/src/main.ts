// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'main.ts',
  script_path: 'execviz-ui/src/main.ts',
  module_name: 'main',
  version: '0.13.0',
  description: 'The page fetches its data. Nothing is compiled into it, so the renderer and the capture it shows are independent artifacts: the same build serves any store, and changing one cannot silently break the other.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['camera', 'canopy', 'console', 'draw', 'fingerprint', 'gif', 'i18n', 'layers', 'lod', 'menu', 'model', 'overview'],
  external_dependencies: [],
  features: ['main', 'capture', 'render', 'store'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { Camera } from './camera.js';
import { build, Model, placeOnClock, CLOCK } from './model.js';
import { BUDGET, MUTED, ISOLATE } from './lod.js';
import { draw, familyOf, HitNode, Options, FlipState } from './draw.js';
import { CanopyLayer } from './canopy.js';
import * as fingerprint from './fingerprint.js';
import { Console } from './console.js';
import { fromRollup, RollupNode } from './overview.js';
import { recordReplay } from './gif.js';
import * as layers from './layers.js';
import { Menu } from './menu.js';
import * as waterfall from './waterfall.js';
import * as timeWindow from './window.js';
import * as source from './source.js';
import * as i18n from './i18n.js';
import * as viewpoint from './viewpoint.js';
import { Feed, Span } from './types.js';

/**
 * The page fetches its data. Nothing is compiled into it, so the renderer and
 * the capture it shows are independent artifacts: the same build serves any
 * store, and changing one cannot silently break the other.
 */
const opt: Options = { expand: 2.6, labels: true, canopy: true, logsInMap: true, focus: null };
// The flipbook is driven by buttons and arrow keys, never by a drag: a drag
// would compete with panning, and two gestures on one surface is how a map
// becomes unusable.
let flip: FlipState | null = null;

// ========================================================================
// CONSTANTS
// ========================================================================
/** How much of a live feed the map holds. Bounded on purpose: the collector
 *  keeps the whole capture, this keeps what can be drawn. */
const LIVE_SPAN_CAP = 6000;
let model: Model = build({ spans: [], clusters: [] });
// Raw spans are kept and merged; the model is derived from them. A delta
// updates the ones it names and leaves the rest alone, which is what lets a
// large capture be watched instead of re-downloaded.
const held = new Map<string, Span>();
let clusters: Feed['clusters'] = [];
let cursor = 0;
let window_: { lo: number; hi: number } | undefined;
let cam = new Camera();
let T = 0, playing = true, speed = 0.15, scrubbed = false;
let hits: HitNode[] = [];
let hover: HitNode | null = null;
let feedMode = 'poll';
let lastSpanCount = -1, lastGrowthAt = 0;

/** Turn a source's signals down: what it sends, what it receives, or both.
 *  Muted paths stay on the map, drawn faintly, so silencing is visible. */
(window as any).__mute = (id: string, dir: 'to' | 'from' | 'both' | 'off') => {
  if (dir === 'off') MUTED.delete(id); else MUTED.set(id, dir);
  canopy.invalidate();
};
/** Light one node's traffic and quieten the rest, so a single thing can be read
 *  out of a busy fleet without the rest of it being hidden. */
// The same menu the right button opens, reachable from the bar for anyone who
// does not think to right click. It acts on the last thing touched, or on the
// busiest service when nothing has been.
document.getElementById('actionsBtn')?.addEventListener('click', (ev) => {
  const r = cv.getBoundingClientRect();
  const target = hover ?? null;
  const x = target ? r.left + target.x : r.left + r.width / 2;
  const y = target ? r.top + target.y : r.top + r.height / 2;
  cv.dispatchEvent(new MouseEvent('contextmenu', {
    clientX: x, clientY: y, bubbles: true, cancelable: true,
  }));
  ev.stopPropagation();
});

document.getElementById('helpBtn')?.addEventListener('click', () => {
  const open = document.getElementById('side')?.classList.toggle('on');
  document.body.classList.toggle('info-open', !!open);
});


(window as any).__isolate = (id: string | null) => { ISOLATE.id = id; canopy.invalidate(); };
(window as any).__muteAll = (dir: 'to' | 'from' | 'both' | 'off') => {
  MUTED.clear();
  if (dir !== 'off') for (const c of model.clusters) MUTED.set(c.id, dir);
  canopy.invalidate();
};
// a replay pauses the live edge: what is on screen is a recording, not a system
let liveFeed = true;
// Overview mode: the map is drawn from the rollup and holds no spans at all.
// Detail is fetched only for what a reader descends into.
let overview = false;
/**
 * Bumped whenever the mode changes.
 *
 * `poll()` checked the mode and then awaited a fetch and a parse. A switch to
 * the overview during either of those left the reply to be ingested anyway, so
 * one in-flight delta of 20,000 spans landed *after* the switch and the mode
 * that exists to hold no spans held twenty thousand. Checking a value that
 * cannot have changed across an await is not the same as checking it after one.
 */
let feedEpoch = 0;
let overviewTree: RollupNode | null = null;
(window as any).__layer = (l: string) => { layer = l as layers.Layer; };
let layer: layers.Layer = 'none';

const cv = document.getElementById('cv') as HTMLCanvasElement;
const ctx = cv.getContext('2d')!;
const DPR = Math.min(window.devicePixelRatio || 1, 2);
const canopy = new CanopyLayer(DPR);
const logConsole = new Console();

// ========================================================================
// INTERNALS
// ========================================================================

function resize() {
  const r = cv.parentElement!.getBoundingClientRect();
  cv.width = r.width * DPR; cv.height = r.height * DPR;
  ctx.setTransform(DPR, 0, 0, DPR, 0, 0);
  cam.resize(r.width, r.height);
  if (!(cam.z > 0)) cam.fit();
}
window.addEventListener('resize', resize);

function ingest(feed: Feed) {
  // A delivery that carries nothing changed nothing. Rebuilding the model from
  // every held span on each empty tick is the same mistake as recomputing at T
  // something that does not vary with T, one layer up.
  if (feed.spans.length === 0 && held.size > 0) {
    if (feed.cursor !== undefined) cursor = feed.cursor;
    return;
  }
  for (const s of feed.spans) held.set(s.span_id, s);
  // A live feed never stops arriving, so what the map holds is bounded and the
  // oldest completed work is dropped first. Left unbounded this grows until the
  // page cannot paint a frame, which reads as a hang rather than as a limit.
  // Spans still open are kept whatever their age: an unfinished span is the
  // death signal, and evicting it would delete the finding.
  if (held.size > LIVE_SPAN_CAP) {
    const over = held.size - LIVE_SPAN_CAP;
    let dropped = 0;
    for (const [id, sp] of held) {
      if (dropped >= over) break;
      if (sp.end === null || sp.end === undefined) continue;
      held.delete(id); dropped++;
    }
    (window as any).__evicted = ((window as any).__evicted ?? 0) + dropped;
  }
  if (feed.clusters.length) clusters = feed.clusters;
  if (feed.cursor !== undefined) cursor = feed.cursor;
  if (feed.window) window_ = feed.window;
  const placed = placeOnClock([...held.values()], window_);
  model = build({ spans: placed, clusters });
  (window as any).__delivered = feed.spans.length;
  (window as any).__truncated = feed.truncated === true;
  (window as any).__spanCount = model.spans.length;
  if (!(cam.z > 0.001)) cam.fit();
}

/**
 * Turns a refused or failed request into something the reader can act on.
 * Returns true when the caller should stop asking.
 */
function handleFeedStatus(status: number): boolean {
  if (status === 401) {
    banner('signin', 'your session has ended, so this map is no longer updating',
           { label: 'sign in again', run: () => location.reload() });
    return true;
  }
  if (status === 403) {
    banner('forbidden', 'this account may not read this capture', {
      label: 'reload', run: () => location.reload() });
    return true;
  }
  if (status >= 500) {
    banner('error', `the server failed while answering (${status}); showing the last data received`);
    return false;
  }
  return false;
}

async function poll() {
  if (!liveFeed) return;
  if (overview) { await pollRollup(); return; }
  const epoch = feedEpoch;
  try {
    const r = await fetch(`/spans?since=${cursor}`);
    if (!r.ok) {
      if (handleFeedStatus(r.status)) { liveFeed = false; stream?.close(); stream = null; }
      return;
    }
    if (epoch !== feedEpoch) return;      // the mode changed while this was in flight
    banner('');
    const payload = await r.json();
    if (epoch !== feedEpoch) return;      // ...and again after the parse
    ingest(payload);
    if (!scrubbed) { T = model.tMax; (document.getElementById('scrub') as HTMLInputElement).value = String(T); }
  } catch (e) {
    (window as any).__feedError = String(e);
    // an unreachable instance is a state, not a silence
    banner('offline', 'not connected to the instance; showing the last data received');
  }
}

/**
 * The overview costs the size of the summary. A subtree whose digest has not
 * changed is not fetched again, which is the same skip that makes syncing cheap.
 */
async function pollRollup() {
  try {
    const r = await fetch('/api/rollup?depth=2');
    if (!r.ok) {
      if (handleFeedStatus(r.status)) { liveFeed = false; }
      return;
    }
    banner('');
    const tree = await r.json() as RollupNode;
    if (overviewTree && overviewTree.digest === tree.digest) return;   // nothing moved
    overviewTree = tree;
    model = fromRollup(tree);
    (window as any).__spanCount = 0;
    (window as any).__overview = {
      spans: tree.rollup.spans, hosts: tree.children, digest: tree.digest,
      bytes: JSON.stringify(tree).length,
    };
    if (!(cam.z > 0.001)) cam.fit();
  } catch { /* the map keeps what it had */ }
}

let stream: EventSource | null = null;
function connect() {
  let timer: number | null = null;
  const startPoll = () => { if (timer === null) { timer = window.setInterval(poll, 900); void poll(); } };
  if (typeof EventSource === 'undefined') { startPoll(); return; }
  try {
    const es = new EventSource(`/events?since=${cursor}`);
    stream = es;
    let got = false;
    es.onmessage = (ev) => {
      got = true; feedMode = 'push';
      // In overview the client holds no spans, so a span delivery is exactly
      // the cost the mode exists to avoid. Ignoring it here is not enough on
      // its own; the stream is closed below; but it keeps the invariant true
      // for anything already in flight.
      if (overview) return;
      if (timer !== null) { clearInterval(timer); timer = null; }
      try {
        ingest(JSON.parse(ev.data));
        if (!scrubbed) { T = model.tMax; (document.getElementById('scrub') as HTMLInputElement).value = String(T); }
      } catch { /* a malformed frame is not worth tearing the stream down */ }
      (window as any).__feed = 'push';
    };
    // the server says why before it closes a stream it can no longer authorise
    es.addEventListener('unauthorized', () => {
      es.close(); stream = null; liveFeed = false;
      banner('signin', 'your session has ended, so this map is no longer updating',
             { label: 'sign in again', run: () => location.reload() });
    });
    es.onerror = () => {
      // the stream dropping is normal and recoverable; polling takes over and
      // reports it only if it also fails
      feedMode = 'poll'; (window as any).__feed = 'poll'; startPoll();
    };
    window.setTimeout(() => { if (!got) startPoll(); }, 2500);
  } catch { startPoll(); }
}

// ========================================================================
// INTERACTION
// ========================================================================
cv.addEventListener('wheel', (e) => {
  e.preventDefault();
  const r = cv.getBoundingClientRect();
  cam.zoomAt(e.clientX - r.left, e.clientY - r.top, Math.exp(-e.deltaY * 0.0015));
}, { passive: false });

let dragging = false, dx = 0, dy = 0, cx0 = 0, cy0 = 0, movedWhileDown = false;
// Drag pans, always. The one exception is a drag that starts on the flipbook
// wheel itself, which turns its pages. The two never compete because they are
// separated by where the drag began, not by what happens to be open.
let flipDrag = false, flipX = 0, flipFrom = 0;
// The row is the handle. A drag that starts on any span of the family the
// flipbook is showing turns its pages; a drag anywhere else pans. The wheel
// counts too, when it happens to be on screen.
const onWheel = (sx: number, sy: number) => {
  if (!flip) return false;
  for (const h of hits) {
    if (h.cluster.id !== flip.cluster) continue;
    if (familyOf(h.span.kind) !== flip.family) continue;
    if (Math.hypot(h.x - sx, h.y - sy) <= Math.max(h.r, 10)) return true;
  }
  return false;   // the rows are the handle; a screen-sized wheel is not
};
cv.addEventListener('mousedown', (e) => {
  if (e.button !== 0) return;
  const r0 = cv.getBoundingClientRect();
  if (flip && onWheel(e.clientX - r0.left, e.clientY - r0.top)) {
    flipDrag = true; flipX = e.clientX; flipFrom = flip.index;
    cv.classList.add('drag');
    return;
  }
  dragging = true; movedWhileDown = false; dx = e.clientX; dy = e.clientY; cx0 = cam.x; cy0 = cam.y;
  cv.classList.add('drag');
});
window.addEventListener('mouseup', () => {
  dragging = false; flipDrag = false; cv.classList.remove('drag');
});
window.addEventListener('mousemove', (e) => {
  if (flipDrag && flip) {
    // one page every 26 pixels, so a short pull moves a few and a long one runs
    const steps = Math.round((e.clientX - flipX) / 26);
    const want = Math.max(0, Math.min(flip.count - 1, flipFrom + steps));
    if (want !== flip.index) { flip.index = want; syncFlip(); }
    return;
  }
  if (!dragging) return;
  if (Math.abs(e.clientX - dx) > 3 || Math.abs(e.clientY - dy) > 3) movedWhileDown = true;
  cam.x = cx0 - (e.clientX - dx) / cam.z;
  cam.y = cy0 - (e.clientY - dy) / cam.z;
  cam.targetX = cam.targetY = null;
});
cv.addEventListener('click', (e) => {
  const r = cv.getBoundingClientRect(), mx = e.clientX - r.left, my = e.clientY - r.top;
  for (const h of hits) {
    if (Math.hypot(h.x - mx, h.y - my) < h.r) {
      logConsole.scope = h.span.span_id; logConsole.invalidate();
      document.getElementById('console')!.classList.add('on');
      return;
    }
  }
});
cv.addEventListener('dblclick', (e) => {
  const r = cv.getBoundingClientRect();
  cam.flyTo(cam.toWorldX(e.clientX - r.left), cam.toWorldY(e.clientY - r.top), cam.z * 2.4);
});
// Right click opens the menu and does nothing else. It previously also zoomed
// out on a double press, which fired while the menu was being opened.
/* Clicking a span opens it: its logs, and the flipbook for the family it
 * belongs to. Requiring a particular zoom before either is reachable makes them
 * things you have to know about rather than things you can find. */
cv.addEventListener('click', (e) => {
  if (movedWhileDown) return;                 // a drag is not a click
  const r = cv.getBoundingClientRect(), mx = e.clientX - r.left, my = e.clientY - r.top;
  let hit: typeof hits[number] | null = null, best = Infinity;
  for (const h of hits) {
    const d = Math.hypot(h.x - mx, h.y - my);
    if (d < Math.max(h.r, 7) && d < best) { best = d; hit = h; }
  }
  if (!hit) {
    // No individual span under the pointer, which is every zoom above the rails.
    // The circle itself is the thing being clicked, so open what it holds: its
    // largest family in the flipbook, and its logs. Waiting for a particular
    // zoom before anything responds is what made this feel dead.
    const wx = cam.toWorldX(mx), wy = cam.toWorldY(my);
    let c: typeof model.clusters[number] | null = null, cd = Infinity;
    for (const cl of model.clusters) {
      const d = Math.hypot(cl.wx - wx, cl.wy - wy);
      if (d < cd && d < Math.max(cl.wr * 1.6, 26 / cam.z)) { cd = d; c = cl; }
    }
    if (!c) return;
    let famBest: any = null, famName: any = null;
    for (const [f, list] of c.byFamily) {
      if (!famBest || list.length > famBest.length) { famBest = list; famName = f; }
    }
    logConsole.scope = famBest?.[0]?.span_id ?? c.spans?.[0]?.span_id ?? null;
    document.getElementById('console')?.classList.add('on');
    logConsole.invalidate();
    if (famBest && famBest.length) {
      flip = { cluster: c.id, family: famName, index: 0, count: famBest.length };
      syncFlip();
    }
    return;
  }
  logConsole.scope = hit.span.span_id;
  document.getElementById('console')?.classList.add('on');
  logConsole.invalidate();
  const fam = familyOf(hit.span.kind);
  const bundle = hit.cluster.byFamily.get(fam) ?? [];
  if (bundle.length) {
    const idx = Math.max(0, bundle.findIndex((b) => b.span_id === hit!.span.span_id));
    flip = { cluster: hit.cluster.id, family: fam, index: idx, count: bundle.length };
    syncFlip();
  }
});

cv.addEventListener('mousemove', (e) => {
  if (dragging) return;
  const r = cv.getBoundingClientRect(), mx = e.clientX - r.left, my = e.clientY - r.top;
  hover = null; let best = Infinity;
  for (const h of hits) {
    const d = Math.hypot(h.x - mx, h.y - my);
    if (d < h.r && d < best) { best = d; hover = h; }
  }
  const el = document.getElementById('inspect')!;
  if (!hover) { el.textContent = 'hover the map…'; return; }
  const s = hover.span;
  el.textContent = [
    s.name,
    `in: ${hover.cluster.label}`,
    `kind: ${s.kind}`,
    `status: ${s.status}`,
    s.duration_ms !== null ? `duration: ${s.duration_ms}ms` : 'running',
    s.links.length ? `links: ${s.links.length}` : '',
  ].filter(Boolean).join('\n');
});

// ========================================================================
// LOG CONSOLE: SHARES THE MAP'S CLOCK AND ITS SELECTION
// ========================================================================
/**
 * The connection banner.
 *
 * A feed that has stopped must say so. Stale spans on screen with no
 * explanation are indistinguishable from a system that  went quiet, and
 * that is the one thing a tool like this must never be ambiguous about.
 */
const bannerEl = document.getElementById('banner')!;
const bannerText = document.getElementById('bannerText')!;
const bannerAction = document.getElementById('bannerAction') as HTMLButtonElement;
let bannerState = '';
function banner(state: '' | 'signin' | 'forbidden' | 'offline' | 'error',
                detail = '', action?: { label: string; run: () => void }) {
  if (state === bannerState && state !== 'error') return;
  bannerState = state;
  if (!state) { bannerEl.classList.remove('on'); return; }
  bannerEl.classList.toggle('warn', state === 'offline');
  bannerText.textContent = detail;
  bannerAction.textContent = action?.label ?? '';
  bannerAction.onclick = action?.run ?? null;
  bannerEl.classList.add('on');
  (window as any).__banner = state;
}

const conEl = document.getElementById('console')!;
const conRows = document.getElementById('conRows')!;
const conCount = document.getElementById('conCount')!;
const conTitle = document.getElementById('conTitle')!;
document.getElementById('logsBtn')!.addEventListener('click', () => {
  conEl.classList.toggle('on'); logConsole.invalidate();
});
document.getElementById('conHide')!.addEventListener('click', () => conEl.classList.remove('on'));
document.getElementById('conClear')!.addEventListener('click', () => {
  logConsole.scope = null; logConsole.invalidate();
});
for (const [id, f] of [['conAll', 'all'], ['conWarn', 'warn'], ['conErr', 'error']] as const) {
  document.getElementById(id)!.addEventListener('click', () => {
    logConsole.filter = f as any;
    for (const other of ['conAll', 'conWarn', 'conErr']) {
      document.getElementById(other)!.classList.toggle('on', other === id);
    }
    logConsole.invalidate();
  });
}
// choosing a line selects its span; choosing a span scopes the console to it
const conText = document.getElementById('conText') as HTMLInputElement;
conText.addEventListener('input', () => { logConsole.text = conText.value.trim(); logConsole.invalidate(); });
const conSort = document.getElementById('conSort') as HTMLSelectElement;
conSort.addEventListener('change', () => { logConsole.sort = conSort.value as any; logConsole.invalidate(); });
const conFold = document.getElementById('conFold')!;
conFold.addEventListener('click', () => {
  logConsole.fold = !logConsole.fold;
  conFold.classList.toggle('on', logConsole.fold);
  logConsole.invalidate();
});

conRows.addEventListener('click', (e) => {
  const row = (e.target as HTMLElement).closest('.lrow') as HTMLElement | null;
  if (!row) return;
  const sid = row.getAttribute('data-sid');
  const sp = sid ? model.byId.get(sid) : undefined;
  if (!sp) return;
  const c = model.clusterById.get(`${sp.host_id}/${sp.domain ?? 'unknown'}`);
  if (c) cam.flyTo(c.wx, c.wy, Math.max(cam.z, 5));
});

// ========================================================================
// REPLAY: THE CAPTURE AS DELIVERED, SO IT OPENS WITHOUT ITS INSTANCE
// ========================================================================
document.getElementById('saveReplay')!.addEventListener('click', () => {
  const payload = {
    format: 'execviz-replay/2',
    saved: new Date().toISOString(),
    window: window_,
    clusters,
    spans: [...held.values()],
  };
  const blob = new Blob([JSON.stringify(payload)], { type: 'application/json' });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = `execviz-replay-${Date.now()}.json`;
  a.click();
  URL.revokeObjectURL(a.href);
  document.getElementById('replayNote')!.textContent =
    `saved ${held.size} spans as delivered; raw times and the window travel with them.`;
});
const replayFile = document.getElementById('replayFile') as HTMLInputElement;
document.getElementById('loadReplay')!.addEventListener('click', () => replayFile.click());
replayFile.addEventListener('change', async () => {
  const f = replayFile.files?.[0];
  if (!f) return;
  try {
    const d = JSON.parse(await f.text());
    if (!Array.isArray(d.spans)) throw new Error('not a replay');
    // A replay replaces what is held rather than merging into it: two captures
    // on one map would be a graph of something that never ran.
    held.clear();
    for (const s of d.spans) held.set(s.span_id, s);
    clusters = d.clusters ?? [];
    window_ = d.window;
    cursor = 0;
    liveFeed = false;                       // a replay is not a live edge
    scrubbed = true;
    const placed = placeOnClock([...held.values()], window_);
    model = build({ spans: placed, clusters });
    (window as any).__spanCount = model.spans.length;
    fpLast = -1; logConsole.invalidate();
    cam.fit();
    document.getElementById('replayNote')!.textContent =
      `loaded ${held.size} spans from ${f.name}; the live feed is paused.`;
  } catch (e) {
    document.getElementById('replayNote')!.textContent = `could not read that file: ${e}`;
  }
});

// ========================================================================
// SETTINGS
// ========================================================================
for (const [id, name] of [['layNone', 'none'], ['layDensity', 'density'],
                          ['layRings', 'rings'], ['layWedges', 'wedges']] as const) {
  document.getElementById(id)!.addEventListener('click', () => {
    layer = name as layers.Layer;
    for (const other of ['layNone', 'layDensity', 'layRings', 'layWedges']) {
      document.getElementById(other)!.classList.toggle('off', other !== id);
    }
  });
}
document.getElementById('layNone')!.classList.remove('off');

// ========================================================================
// LANGUAGE: THE CHROME ONLY
// ========================================================================
const localeSel = document.getElementById('locale') as HTMLSelectElement;
const NAMES: Record<string, string> = { en: 'English', es: 'Español', de: 'Deutsch' };
localeSel.innerHTML = i18n.available()
  .map((l) => `<option value="${l}">${NAMES[l] ?? l}</option>`).join('');
localeSel.value = i18n.getLocale();
i18n.setLocale(localeSel.value);
localeSel.addEventListener('change', () => {
  i18n.setLocale(localeSel.value);
  menu.render();                      // menu labels are chrome, so they change
  wfLast = ''; logConsole.invalidate();
  document.getElementById('winLabel')!.textContent = timeWindow.label();
});

// ========================================================================
// THE WINDOW: ONE RANGE, EVERY VIEW
// ========================================================================
const brush = document.getElementById('brush')!;
const brushSel = document.getElementById('brushsel')!;
function paintBrush() {
  const r = timeWindow.get();
  document.getElementById('winLabel')!.textContent = timeWindow.label();
  if (!r) { brushSel.classList.remove('on'); return; }
  const w = brush.getBoundingClientRect().width || 1;
  brushSel.classList.add('on');
  (brushSel as HTMLElement).style.left = `${(r.from / 1000) * w}px`;
  (brushSel as HTMLElement).style.width = `${((r.to - r.from) / 1000) * w}px`;
}
let brushFrom: number | null = null;
const posOf = (e: MouseEvent) => {
  const b = brush.getBoundingClientRect();
  return Math.max(0, Math.min(1000, ((e.clientX - b.left) / (b.width || 1)) * 1000));
};
brush.addEventListener('mousedown', (e) => { brushFrom = posOf(e); e.preventDefault(); });
window.addEventListener('mousemove', (e) => {
  if (brushFrom === null) return;
  timeWindow.set({ from: brushFrom, to: posOf(e) });
});
window.addEventListener('mouseup', () => { brushFrom = null; });
document.getElementById('winClear')!.addEventListener('click', () => timeWindow.set(null));
timeWindow.onChange(() => { paintBrush(); wfLast = ''; logConsole.invalidate(); canopy.invalidate(); });
paintBrush();

// ========================================================================
// SOURCE LINKS
// ========================================================================
const srcTemplate = document.getElementById('srcTemplate') as HTMLInputElement;
const srcPreset = document.getElementById('srcPreset') as HTMLSelectElement;
srcTemplate.value = source.getTemplate();

// The editor templates were defined and never offered, so anyone wanting to
// jump to source had to already know their editor's URL scheme. They are
// listed here; the field stays, because an editor not on the list is the
// reason the field is a field.
{
  const preset = document.getElementById('srcPreset') as HTMLSelectElement | null;
  if (preset) {
    for (const [name, template] of Object.entries(source.PRESETS)) {
      const o = document.createElement('option');
      o.value = template; o.textContent = name;
      preset.append(o);
    }
    preset.addEventListener('change', () => {
      if (!preset.value) return;
      srcTemplate.value = preset.value;
      source.setTemplate(preset.value);
    });
  }
}
srcTemplate.addEventListener('input', () => {
  source.setTemplate(srcTemplate.value);
  document.getElementById('srcNote')!.textContent = srcTemplate.value
    ? 'a span with a recorded file and line now opens there.'
    : 'frames carry file and line; a template turns them into a link.';
  wfLast = '';
});
srcPreset.addEventListener('change', () => {
  if (!srcPreset.value) return;
  srcTemplate.value = srcPreset.value;
  srcTemplate.dispatchEvent(new Event('input'));
});

// ========================================================================
// FINDINGS, STORED BESIDE THE CAPTURE
// ========================================================================
document.getElementById('saveView')!.addEventListener('click', async () => {
  const name = prompt('name this view');
  if (!name) return;
  const r = timeWindow.get();
  const state = `z=${cam.z.toFixed(3)}&x=${Math.round(cam.x)}&y=${Math.round(cam.y)}` +
                (r ? `&from=${Math.round(r.from)}&to=${Math.round(r.to)}` : '');
  await post('/api/view', { name, state });
});
document.getElementById('addNote')!.addEventListener('click', async () => {
  const body = prompt('what did you find?');
  if (!body) return;
  await post('/api/note', { body, span_id: logConsole.scope });
});
async function post(path: string, obj: unknown) {
  const note = document.getElementById('findNote')!;
  try {
    const r = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' },
                                  body: JSON.stringify(obj) });
    if (r.status === 401 || r.status === 403) {
      note.textContent = r.status === 401
        ? 'not saved: your session has ended.'
        : 'not saved: this account may read but not change a capture.';
      handleFeedStatus(r.status);
    } else {
      note.textContent = r.ok ? 'saved beside the capture.' : `could not save: ${r.status}`;
    }
  } catch (e) { note.textContent = `could not save: ${e}`; }
}

const overviewBtn = document.getElementById('toggleOverview')!;
overviewBtn.addEventListener('click', () => { void setOverview(!overview); });

// ========================================================================
// CONSTANTS
// ========================================================================

/**
 * Rollup at the fleet scale, spans once a reader descends into something.
 *
 * The overview holds no spans at all: it is drawn from digests, and a subtree
 * whose digest has not moved costs nothing. Held off until someone presses a
 * key, a machine running twenty thousand processes ingests every span of every
 * one of them to draw a view that cannot show an individual span anyway.
 *
 * The two thresholds are apart on purpose. One value would flip modes on every
 * small camera movement across it, and each flip discards what is held.
 */
const OVERVIEW_IN = 0.55;    // zoomed out past this: summary
const OVERVIEW_OUT = 0.95;   // zoomed in past this: spans
let switching = false;

// ========================================================================
// INTERNALS
// ========================================================================

async function setOverview(next: boolean) {
  if (next === overview || switching) return;
  switching = true;
  overview = next;
  overviewBtn.classList.toggle('off', !overview);
  // Close the span stream outright: leaving it open would keep pulling the very
  // data the overview exists not to carry, and the saving would be imaginary.
  if (overview) { stream?.close(); stream = null; }
  else if (!stream) connect();
  document.getElementById('overviewState')!.textContent = overview ? 'summary' : 'spans';
  // Switching modes replaces what is held rather than mixing the two: a map half
  // drawn from spans and half from a summary would be neither.
  // anything already in flight belongs to the mode that requested it
  feedEpoch++;
  held.clear(); clusters = []; cursor = 0; overviewTree = null;
  model = build({ spans: [], clusters: [] });
  logConsole.invalidate();
  try { await poll(); } finally { switching = false; }
}

const labelsBtn = document.getElementById('toggleLabels')!;
const canopyBtn = document.getElementById('toggleCanopy')!;
labelsBtn.addEventListener('click', () => {
  opt.labels = !opt.labels;
  labelsBtn.classList.toggle('off', !opt.labels);
  document.getElementById('labelState')!.textContent = opt.labels ? 'on' : 'off';
});
canopyBtn.addEventListener('click', () => {
  opt.canopy = !opt.canopy;
  canopyBtn.classList.toggle('off', !opt.canopy);
});
const railRange = document.getElementById('railRange') as HTMLInputElement;
railRange.addEventListener('input', () => {
  BUDGET.railsPerFamily = Number(railRange.value);
  document.getElementById('railLbl')!.textContent = railRange.value;
});
const routeRange = document.getElementById('routeRange') as HTMLInputElement;
routeRange.addEventListener('input', () => {
  BUDGET.routes = Number(routeRange.value);
  document.getElementById('routeLbl')!.textContent = routeRange.value;
  canopy.invalidate();                      // the budget changed what it draws
});

// ========================================================================
// FLIPBOOK
// ========================================================================
const flipbar = document.getElementById('flipbar')!;
const flipLabel = document.getElementById('flipLabel')!;
function flipStep(d: number) {
  const big = (window as any).__stats?.biggest;
  if (!flip) {
    if (!big) return;
    flip = { cluster: big.cluster, family: big.family, index: 0, count: big.count };
  } else {
    flip.index = (flip.index + d + flip.count) % flip.count;
  }
  opt.flip = flip;
  flipbar.classList.add('on');
  syncFlip();
}
function syncFlip() {
  if (!flip) { flipbar.classList.remove('on'); opt.flip = undefined; return; }
  const c = model.clusterById.get(flip.cluster);
  const bundle = c?.byFamily.get(flip.family as any) ?? [];
  flip.count = Math.max(1, Math.min(bundle.length, 60));
  const row = bundle[Math.min(flip.index, bundle.length - 1)];
  flipLabel.textContent = row
    ? `${c?.label ?? flip.cluster} · ${flip.family} · ${flip.index + 1}/${flip.count} · ${row.name}`
    : `${flip.family} · empty`;
  // The bar has to be shown as well as filled in. This only ever removed the
  // class, so the flipbook could be fully loaded and still invisible.
  flipbar.classList.toggle('on', !!row);
  opt.flip = flip;
}
// The bar is a handle as well as a stepper: grab its label and pull.
{
  const bar = document.getElementById('flipbar')!;
  const lab = document.getElementById('flipLabel')!;
  let flipbarDrag = false, bx = 0, from = 0;
  lab.style.cursor = 'ew-resize';
  lab.addEventListener('mousedown', (e) => {
    if (!flip) return;
    flipbarDrag = true; bx = e.clientX; from = flip.index; e.preventDefault();
  });
  window.addEventListener('mousemove', (e) => {
    if (!flipbarDrag || !flip) return;
    const want = Math.max(0, Math.min(flip.count - 1, from + Math.round((e.clientX - bx) / 22)));
    if (want !== flip.index) { flip.index = want; syncFlip(); }
  });
  window.addEventListener('mouseup', () => { flipbarDrag = false; });
  bar.addEventListener('wheel', (e) => {
    if (!flip) return;
    e.preventDefault();
    flipStep(e.deltaY > 0 ? 1 : -1);
  }, { passive: false });
}

document.getElementById('flipPrev')!.addEventListener('click', () => flipStep(-1));
document.getElementById('flipNext')!.addEventListener('click', () => flipStep(1));
document.getElementById('flipOff')!.addEventListener('click', () => { flip = null; syncFlip(); });
// arrow keys and Escape are bound through the menu, so there is exactly one
// place a key is defined and the shortcut sheet cannot drift from the behaviour
/**
 * Escape closes whatever is open, topmost first, and nothing more.
 *
 * It previously closed the shortcut sheet and left the console and the
 * waterfall open, so the same key meant different things depending on which
 * panel had been used last. One rule, applied in the order things sit on top of
 * each other, is the behaviour every reader already expects.
 */
window.addEventListener('keydown', (e) => {
  if (e.key !== 'Escape') return;
  const target = e.target as HTMLElement | null;
  if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) {
    target.blur();                       // leave the field before closing panels
    return;
  }
  const sheet = document.getElementById('sheet')!;
  if (sheet.classList.contains('on')) { sheet.classList.remove('on'); return; }
  const panel = document.getElementById('panel')!;
  if (panel.classList.contains('on')) { panel.classList.remove('on'); return; }
  if (flip) { flip = null; syncFlip(); return; }
  if (wfEl.classList.contains('on')) { toggleWaterfall(false); return; }
  if (conEl.classList.contains('on')) { conEl.classList.remove('on'); return; }
  if (timeWindow.get()) { timeWindow.set(null); }
});

document.getElementById('zin')!.addEventListener('click', () => cam.zoomAt(cam.w / 2, cam.h / 2, 1.5));
document.getElementById('zout')!.addEventListener('click', () => cam.zoomAt(cam.w / 2, cam.h / 2, 1 / 1.5));
document.getElementById('reset')!.addEventListener('click', () => cam.fit());

// ========================================================================
// THE SETTINGS GEAR
// ========================================================================
// The controls live in a panel rather than down the side, so the map is the
// page. The gear toggles it; its own close button and Escape both dismiss it.
const settingsPanel = document.getElementById('panel')!;
function toggleSettings(on?: boolean) {
  settingsPanel.classList.toggle('on', on ?? !settingsPanel.classList.contains('on'));
}
document.getElementById('gear')!.addEventListener('click', () => toggleSettings());
document.getElementById('panelClose')!.addEventListener('click', () => toggleSettings(false));

// The LOD ladder in the legend marks the rung the current zoom lands on, so the
// reader can see where "cluster" or "rails" sits without reading the numbers.
const lodRungs = Array.from(document.querySelectorAll<HTMLElement>('#lodladder > div'));
function highlightLod(tier: string) {
  for (const r of lodRungs) r.classList.toggle('here', r.dataset.lod === tier);
}
const scrub = document.getElementById('scrub') as HTMLInputElement;
scrub.addEventListener('input', () => { scrubbed = true; playing = false; T = Number(scrub.value); });
document.getElementById('play')!.addEventListener('click', function (this: HTMLElement) {
  playing = !playing; this.textContent = playing ? '❚❚' : '▶';
});
const speedNum = document.getElementById('speedNum') as HTMLInputElement;
// The footer carries both a slider and a box for the same value: the slider is
// the quick reach, the box is the exact one. Whichever moves, the other follows,
// so they can never disagree about the current speed.
const speedRange = document.getElementById('speedRange') as HTMLInputElement | null;
function setSpeed(v: number) {
  speed = Math.max(0.01, Math.min(20, v || 1));
  speedNum.value = speed.toFixed(2);
  if (speedRange) speedRange.value = String(Math.max(0.1, Math.min(4, speed)));
}
speedNum.addEventListener('input', () => { setSpeed(Number(speedNum.value)); });
if (speedRange) speedRange.addEventListener('input', () => { setSpeed(Number(speedRange.value)); });
const expRange = document.getElementById('expRange') as HTMLInputElement;
expRange.addEventListener('input', () => {
  opt.expand = Number(expRange.value);
  document.getElementById('expLbl')!.textContent = `${opt.expand.toFixed(1)}×`;
});

// ========================================================================
// FRAME LOOP
// ========================================================================
let last = 0, frames = 0, acc = 0;
function frame(ts: number) {
  // The list view replaces the map, so the map stops being drawn. Leaving it
  // running behind an opaque panel spends the frame budget the list view exists
  // to give back.
  if ((window as any).__listView) { requestAnimationFrame(frame); return; }
  const dt = last ? ts - last : 16; last = ts;
  if (playing && !scrubbed) T = model.tMax;
  else if (playing) { T += dt * speed * 0.28; if (T > model.tMax) T = model.tMax; scrub.value = String(T); }
  cam.step();
  // the layer rebuilds only when the camera or the model changed
  // one range restricts every view, rather than each filtering separately
  const view = timeWindow.restrict(model);
  // Both exposed from the frame loop, not from ingest.
  //
  // `__model` used to be assigned only when spans arrived, so it kept pointing
  // at the last span-built model after the overview replaced it; a probe that
  // reported the old object while the renderer drew a new one. A debug hook that
  // lies differs from none: it cost a wrong diagnosis before it was noticed.
  (window as any).__model = model;
  (window as any).__cam = cam;
  (window as any).__windowedModel = view;
  canopy.ensure(view, cam);
  ctx.clearRect(0, 0, cam.w, cam.h);
  if (opt.canopy) canopy.composite(ctx, cam);
  opt.focus = selectedSpan;
  const { hits: h, stats } = draw(ctx, view, cam, T, opt);
  // the analytic layers are overlays over the same model, drawn after the map
  // so the primary geometry stays legible beneath them
  if (layer === 'density') layers.drawDensity(ctx, model, cam, T);
  else if (layer === 'rings') layers.drawRings(ctx, model, cam, T);
  else if (layer === 'wedges') {
    const sig = (window as any).__fingerprint;
    if (sig) layers.drawWedges(ctx, sig.axes, cam);
  }
  hits = h;
  acc += dt; frames++;
  if (acc > 500) {
    (window as any).__stats = { ...stats, fps: Math.round((frames * 1000) / acc), feed: feedMode };
    if (!switching) {
      if (!overview && cam.z < OVERVIEW_IN) void setOverview(true);
      else if (overview && cam.z > OVERVIEW_OUT) void setOverview(false);
    }
    // One is always pulled out and ready. Waiting for a click before anything
    // is engaged is what made the spans feel inert.
    // Only once the node has been zoomed into. Appearing the moment a run shows
    // up puts a control over the map before there is anything to step through.
    if (!flip && stats.biggest && stats.biggest.count > 1 && cam.z >= 12) {
      flip = { cluster: stats.biggest.cluster, family: stats.biggest.family as any,
               index: 0, count: stats.biggest.count };
      syncFlip();
    }
    // The panel earns its space by saying what is in front of you.
    {
      const put = (id: string, v: string | number) => {
        const el = document.getElementById(id);
        if (el && el.textContent !== String(v)) el.textContent = String(v);
      };
      let errored = 0, open = 0;
      for (const sp of model.spans) {
        if (sp.status === 'error') errored++;
        if (sp.end === null || sp.end === undefined) open++;
      }
      put('statHosts', model.hosts.length);
      put('statClusters', model.clusters.length);
      put('statSpans', model.spans.length);
      put('statRoutes', model.routes.length);
      put('statErrors', errored);
      put('statOpen', open);
      put('statDrawn', `${stats.nodes ?? 0} nodes, ${stats.routesDrawn ?? 0} routes`);
    }
    // The badge reports what is happening rather than what the page was built
    // with. A capture that is still growing is live; a stored one being scrubbed
    // is a replay, and saying otherwise is the interface asserting something
    // untrue about its own data.
    {
      const n = model.spans.length;
      if (n !== lastSpanCount) { lastSpanCount = n; lastGrowthAt = Date.now(); }
      const live = Date.now() - lastGrowthAt < 4000;
      const b = document.getElementById('badge');
      if (b) {
        const want = live ? '\u25cf LIVE CAPTURE' : '\u25a0 REPLAY \u00b7 stored capture';
        if (b.textContent !== want) b.textContent = want;
        b.classList.toggle('replay', !live);
      }
    }
    // The header readout is the map's state in the map's own language: the zoom,
    // and which LOD rung that lands on. Route/node counts and fps are diagnostics,
    // not what a reader of the map needs, and they live in __stats for anyone who does.
    document.getElementById('readout')!.innerHTML =
      `zoom <b>${cam.z.toFixed(2)}×</b> · LOD: ${stats.tier}`;
    highlightLod(stats.tier);
    acc = 0; frames = 0;
  }
  // the waterfall shares the map's clock and selection; rebuilt only when what
  // it would say has changed
  if (showWaterfall) {
    const key = `${view.spans.length}|${Math.round(T / 8)}|${timeWindow.label()}|${source.getTemplate()}`;
    if (key !== wfLast) {
      wfLast = key;
      waterfall.render(wfEl, view, T, (s) => {
        logConsole.scope = s.span_id; logConsole.invalidate();
        const c = model.clusterById.get(`${s.host_id}/${s.domain ?? 'unknown'}`);
        if (c) cam.flyTo(c.wx, c.wy, Math.max(cam.z, 4));
      });
    }
  }
  if (flip) syncFlip();
  if (conEl.classList.contains('on')) logConsole.render(conRows, conCount, conTitle, view, T);
  document.getElementById('tl')!.textContent = `t = ${Math.round(T)} / ${model.tMax}`;
  requestAnimationFrame(frame);
}

// The fingerprint is a property of the whole capture, not of the playhead, so
// it is fetched when the capture changes rather than drawn every frame.
const fpCanvas = document.getElementById('fp') as HTMLCanvasElement;
const fpCtx = fpCanvas.getContext('2d')!;
let fpLast = -1;
async function refreshFingerprint() {
  if (held.size === fpLast) return;
  fpLast = held.size;
  try {
    const against = new URLSearchParams(location.search).get('baseline');
    const r = await fetch('/api/fingerprint' + (against ? `?against=${encodeURIComponent(against)}` : ''));
    const sig = fingerprint.parse(await r.json());
    if (!sig) return;
    (window as any).__fingerprint = sig;
    fingerprint.draw(fpCtx, sig, fpCanvas.width, fpCanvas.height);
    const note = document.getElementById('fpnote')!;
    note.textContent = sig.bands
      ? (sig.matches ? 'this run matches its baseline on every axis.'
                     : `this run departs from its baseline; the largest move is ${sig.largestDeparture}.`)
      : 'the signature of this capture. add ?baseline=a.db,b.db to compare against earlier runs.';
  } catch { /* the map is still useful without it */ }
}
window.setInterval(refreshFingerprint, 2000);

// ========================================================================
// MENUS: THE COMPLETE LIST OF WHAT THIS CAN DO
// ========================================================================
const menu = new Menu(document.getElementById('menubar')!, document.getElementById('sheet')!);

menu.add('view', [
  { label: 'reset view', key: 'r', run: () => cam.fit() },
  { label: 'zoom in', key: '+', run: () => cam.zoomAt(cam.w / 2, cam.h / 2, 1.5) },
  { label: 'zoom out', key: '-', run: () => cam.zoomAt(cam.w / 2, cam.h / 2, 1 / 1.5) },
  { label: 'labels', key: 'l', state: () => opt.labels,
    run: () => { opt.labels = !opt.labels; labelsBtn.classList.toggle('off', !opt.labels);
                 document.getElementById('labelState')!.textContent = opt.labels ? 'on' : 'off'; } },
  { label: 'canopy (routes)', key: 'c', state: () => opt.canopy,
    run: () => { opt.canopy = !opt.canopy; canopyBtn.classList.toggle('off', !opt.canopy); } },
]);

menu.add('layers', [
  { label: 'none', key: '0', group: 'layer', state: () => layer === 'none', run: () => setLayer('none') },
  { label: 'density; where are the hotspots', key: '1', group: 'layer',
    state: () => layer === 'density', run: () => setLayer('density') },
  { label: 'tree-rings; how depth is distributed', key: '2', group: 'layer',
    state: () => layer === 'rings', run: () => setLayer('rings') },
  { label: 'wedges; fingerprint as geometry', key: '3', group: 'layer',
    state: () => layer === 'wedges', run: () => setLayer('wedges') },
]);

const wfEl = document.getElementById('waterfall')!;
let showWaterfall = false;
function toggleWaterfall(on?: boolean) {
  showWaterfall = on === undefined ? !showWaterfall : on;
  wfEl.classList.toggle('on', showWaterfall);
  if (showWaterfall) wfLast = '';
}
let wfLast = '';

menu.add('views', [
  { label: 'waterfall; what happened, in order', key: 'w', state: () => showWaterfall,
    run: () => toggleWaterfall() },
]);

menu.add('logs', [
  { label: 'log console', key: 'g', state: () => conEl.classList.contains('on'),
    run: () => { conEl.classList.toggle('on'); logConsole.invalidate(); } },
  { label: 'show all levels', group: 'loglevel', state: () => logConsole.filter === 'all',
    run: () => setLogFilter('all', 'conAll') },
  { label: 'warnings and worse', group: 'loglevel', state: () => logConsole.filter === 'warn',
    run: () => setLogFilter('warn', 'conWarn') },
  { label: 'errors only', group: 'loglevel', state: () => logConsole.filter === 'error',
    run: () => setLogFilter('error', 'conErr') },
  { label: 'clear the scope', run: () => { logConsole.scope = null; logConsole.invalidate(); } },
  { label: 'fold repeated lines', key: 'f', state: () => logConsole.fold,
    run: () => { conFold.click(); conEl.classList.add('on'); } },
  { label: 'focus the narrow box', key: '/', run: () => { conEl.classList.add('on'); conText.focus(); } },
]);

menu.add('time', [
  { label: 'play / pause', key: ' ', state: () => playing,
    run: () => { playing = !playing; document.getElementById('play')!.textContent = playing ? '❚❚' : '▶'; } },
  { label: 'jump to the live edge', key: 'e',
    run: () => { scrubbed = false; T = model.tMax; scrub.value = String(T); } },
  { label: 'step the flipbook back', key: 'ArrowLeft', run: () => flipStep(-1) },
  { label: 'step the flipbook on', key: 'ArrowRight', run: () => flipStep(1) },
  { label: 'close the flipbook', run: () => { flip = null; syncFlip(); } },
]);

menu.add('find', [
  // navigate by question: the questions a person asks, bound to keys
  { label: 'next error', key: 'n', run: () => jumpTo((s) => s.status === 'error') },
  { label: 'next still running', key: 'u', run: () => jumpTo((s) => s.end === null) },
  { label: 'slowest span', key: 'm', run: () => {
      let best: any = null;
      for (const s of model.spans) {
        if (s.duration_ms !== null && (!best || s.duration_ms > best.duration_ms)) best = s;
      }
      if (best) goToSpan(best);
    } },
]);

menu.add('share', [
  { label: 'copy a link to this view', key: 'y', run: () => copyPermalink() },
  { label: 'record the replay as a GIF', key: 'G', run: () => { void recordGif(); } },
]);

menu.add('capture', [
  { label: 'overview only (hold no spans)', key: 'o', state: () => overview,
    run: () => overviewBtn.click() },
  { label: 'save replay', key: 's', run: () => document.getElementById('saveReplay')!.click() },
  { label: 'load replay', run: () => replayFile.click() },
]);

// Navigation by question, which is what a reader is doing: not
// "go to coordinates" but "show me the next thing that went wrong".
let jumpIdx = 0;
function jumpTo(pred: (s: any) => boolean) {
  const hits = model.spans.filter(pred).sort((a, b) => a.start - b.start);
  if (!hits.length) { flash('nothing matches that here'); return; }
  jumpIdx = (jumpIdx + 1) % hits.length;
  goToSpan(hits[jumpIdx % hits.length]);
}
function goToSpan(s: any) {
  const c = model.clusterById.get(`${s.host_id}/${s.domain ?? 'unknown'}`);
  if (c) cam.flyTo(c.wx, c.wy, Math.max(cam.z, 5));
  scrubbed = true; playing = false;
  T = Math.max(0, s.start);
  scrub.value = String(T);
  logConsole.scope = s.span_id; logConsole.invalidate();
  selectedSpan = s.span_id;
  flash(`${s.name} · ${s.status}${s.duration_ms !== null ? ` · ${Math.round(s.duration_ms)}ms` : ''}`);
}
let selectedSpan: string | null = null;

function flash(msg: string) {
  const el = document.getElementById('flash')!;
  el.textContent = msg;
  el.classList.add('on');
  window.setTimeout(() => el.classList.remove('on'), 2200);
}

/**
 * Records the replay as a GIF a reader can open anywhere.
 *
 * The window decides what is recorded: a diagnostic is a period, and sending
 * somebody the whole capture when the interesting part is four seconds long
 * wastes the one thing they were going to give this.
 */
async function recordGif() {
  const range = timeWindow.get();
  const from = range ? range.from : 0;
  const to = range ? range.to : CLOCK;
  const wasScrubbed = scrubbed;
  const wasT = T;
  flash('recording the replay…');
  try {
    const blob = await recordReplay(
      cv as HTMLCanvasElement,
      (t) => { scrubbed = true; T = t; },
      from, to,
    );
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `execviz-${Math.round(from)}-${Math.round(to)}.gif`;
    a.click();
    URL.revokeObjectURL(url);
    flash(`replay saved (${Math.round(blob.size / 1024)} KB)`);
  } catch (e) {
    // A recorder that breaks the view it was recording differs from one that
    // fails, so the clock is put back whatever happened.
    flash(`could not record: ${(e as Error).message}`);
  } finally {
    scrubbed = wasScrubbed;
    T = wasT;
  }
}

async function copyPermalink() {
  const vp = viewpoint.capture(cam, T, layer, selectedSpan,
    conEl.classList.contains('on'), showWaterfall, timeWindow.get());
  const url = `${location.origin}${location.pathname}?${viewpoint.toQuery(vp)}`;
  try {
    await navigator.clipboard.writeText(url);
    flash('link copied; it carries the view, not the data');
  } catch {
    // clipboard access is not guaranteed; showing the link is still useful
    history.replaceState(null, '', url);
    flash('link is in the address bar');
  }
}

/** Restores a viewpoint arriving in the address bar. */
function applyViewpoint() {
  const vp = viewpoint.fromQuery(location.search);
  if (!vp) return;
  cam.x = vp.x; cam.y = vp.y; cam.z = vp.z; cam.targetZ = vp.z;
  // the window is part of the finding, not decoration around it
  timeWindow.set(vp.from !== undefined && vp.to !== undefined
    ? { from: vp.from, to: vp.to } : null);
  T = vp.t; scrubbed = true; playing = false;
  scrub.value = String(T);
  document.getElementById('play')!.textContent = '▶';
  if (vp.layer !== 'none') setLayer(vp.layer as layers.Layer);
  if (vp.logs) conEl.classList.add('on');
  if (vp.wf) toggleWaterfall(true);
  if (vp.span) { logConsole.scope = vp.span; selectedSpan = vp.span; logConsole.invalidate(); }
  flash('opened at a shared view');
}

function setLayer(l: layers.Layer) {
  layer = l;
  const id = l === 'none' ? 'layNone' : l === 'density' ? 'layDensity'
    : l === 'rings' ? 'layRings' : 'layWedges';
  for (const other of ['layNone', 'layDensity', 'layRings', 'layWedges']) {
    document.getElementById(other)!.classList.toggle('off', other !== id);
  }
}
function setLogFilter(f: 'all' | 'warn' | 'error', id: string) {
  logConsole.filter = f;
  for (const other of ['conAll', 'conWarn', 'conErr']) {
    document.getElementById(other)!.classList.toggle('on', other === id);
  }
  logConsole.invalidate();
  conEl.classList.add('on');           // changing a log filter should show logs
}
menu.render();
(window as any).__menu = menu;

resize();
cam.fit();
applyViewpoint();
connect();
requestAnimationFrame(frame);

/* Right click acts on the thing under the cursor.
 *
 * Isolation and path muting were reachable only from a console, which is the
 * same as not existing. The menu names the node it applies to, so an action can
 * never be aimed at something other than what was clicked. */
{
  const menu = document.getElementById('ctxmenu')!;
  const hide = () => { menu.hidden = true; };
  const nearestCluster = (sx: number, sy: number) => {
    const wx = cam.toWorldX(sx), wy = cam.toWorldY(sy);
    let best = null as any, bd = Infinity;
    for (const c of model.clusters) {
      const d = (c.wx - wx) ** 2 + (c.wy - wy) ** 2;
      if (d < bd) { bd = d; best = c; }
    }
    return best;
  };
  cv.addEventListener('contextmenu', (e: MouseEvent) => {
    e.preventDefault();
    const r = cv.getBoundingClientRect();
    const c = nearestCluster(e.clientX - r.left, e.clientY - r.top);
    if (!c) return;
    const item = (label: string, run: () => void) => {
      const b = document.createElement('button');
      b.textContent = label;
      b.addEventListener('click', () => { run(); hide(); });
      return b;
    };
    menu.replaceChildren();
    const who = document.createElement('div');
    who.className = 'who';
    who.textContent = c.label ? `${c.host} / ${c.label}` : c.id;
    menu.append(who);
    menu.append(
      item('Isolate: draw only routes touching this service', () => { (window as any).__isolate(c.id); }),
      item('Clear isolation (draw all routes again)', () => { (window as any).__isolate(null); }),
      item('Mute outbound routes (leaving this service)', () => { MUTED.set(c.id, 'from'); canopy.invalidate(); }),
      item('Mute inbound routes (arriving at this service)', () => { MUTED.set(c.id, 'to'); canopy.invalidate(); }),
      item('Mute inbound and outbound routes', () => { MUTED.set(c.id, 'both'); canopy.invalidate(); }),
      item('Unmute this service', () => { MUTED.delete(c.id); canopy.invalidate(); }),
      item('Open log console, scoped to this service', () => {
        logConsole.scope = c.spans?.[0]?.span_id ?? null;
        document.getElementById('console')?.classList.add('on');
        logConsole.invalidate();
      }),
      item('Centre the camera on this service', () => { cam.flyTo(c.wx, c.wy, Math.max(cam.z, 2.4)); }),
    );
    menu.style.left = `${e.clientX - r.left}px`;
    menu.style.top = `${e.clientY - r.top}px`;
    menu.hidden = false;
  });
  window.addEventListener('mousedown', (e) => { if (!menu.contains(e.target as Node)) hide(); });
  window.addEventListener('keydown', (e) => { if (e.key === 'Escape') hide(); });
}

/* The log console can be moved. It is a panel over a map, and where it lands by
 * default will sometimes be over the thing being read. Dragging its header
 * moves it; it is kept inside the window. */
{
  const con = document.getElementById('console');
  const head = con?.querySelector('header') as HTMLElement | null;
  if (con && head) {
    let conDrag = false, ox = 0, oy = 0, sx = 0, sy = 0;
    head.addEventListener('mousedown', (e) => {
      if ((e.target as HTMLElement).closest('button,input,select')) return;
      const r = con.getBoundingClientRect();
      conDrag = true; sx = e.clientX; sy = e.clientY; ox = r.left; oy = r.top;
      con.style.right = 'auto'; con.style.bottom = 'auto';
      con.style.left = `${r.left}px`; con.style.top = `${r.top}px`;
      e.preventDefault();
    });
    window.addEventListener('mousemove', (e) => {
      if (!conDrag) return;
      const r = con.getBoundingClientRect();
      const x = Math.max(0, Math.min(window.innerWidth - r.width, ox + e.clientX - sx));
      const y = Math.max(44, Math.min(window.innerHeight - r.height - 46, oy + e.clientY - sy));
      con.style.left = `${x}px`; con.style.top = `${y}px`;
    });
    window.addEventListener('mouseup', () => { conDrag = false; });
  }
}

/* The list view: the same capture as sorted text.
 *
 * A map is the wrong answer for some questions and for some captures. This
 * renders what is held as bracketed, sorted tables and stops the canvas drawing
 * while it is up, so a capture too large to draw is still readable. */
{
  const view = document.getElementById('listview')!;
  const btn = document.getElementById('listBtn')!;
  let listOn = false;

  const esc = (x: string) => x.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c] as string));
  const render = () => {
    const hosts = [...model.hosts].sort((a, b) => (b.summary?.spans ?? 0) - (a.summary?.spans ?? 0));
    const cls = [...model.clusters].sort((a, b) => b.total - a.total);
    const routes = [...model.routes].sort((a, b) => b.count - a.count).slice(0, 200);
    const row = (cells: string[], cn = '') =>
      `<tr class="${cn}">` + cells.map((c, i) => `<td class="${i ? 'n' : ''}">${c}</td>`).join('') + '</tr>';
    view.innerHTML =
      `<h3>Capture</h3><table>` +
      row(['mode', document.getElementById('overviewState')?.textContent ?? '']) +
      row(['hosts', String(model.hosts.length)]) +
      row(['services', String(model.clusters.length)]) +
      row(['spans in the map', String(model.spans.length)]) +
      row(['routes', String(model.routes.length)]) +
      `</table>` +
      `<h3>Hosts [${hosts.length}]</h3><table><tr><th>host</th><th>spans</th><th>errors</th><th>open</th></tr>` +
      hosts.map((h) => row([esc(h.id), String(h.summary?.spans ?? 0),
                            String(h.summary?.errors ?? 0), String(h.summary?.open ?? 0)],
                           (h.summary?.errors ?? 0) > 0 ? 'err' : '')).join('') +
      `</table>` +
      `<h3>Services [${cls.length}]</h3><table><tr><th>service</th><th>spans</th></tr>` +
      cls.map((c) => row([esc(c.id), String(c.total)])).join('') +
      `</table>` +
      `<h3>Routes [${model.routes.length}${routes.length < model.routes.length ? ', showing ' + routes.length : ''}]</h3>` +
      `<table><tr><th>from &rarr; to</th><th>count</th><th>errors</th></tr>` +
      routes.map((r) => row([esc(r.from) + ' &rarr; ' + esc(r.to), String(r.count), String(r.errors)],
                            r.errors > 0 ? 'err' : '')).join('') +
      `</table>`;
  };

  btn.addEventListener('click', () => {
    listOn = !listOn;
    view.classList.toggle('on', listOn);
    btn.classList.toggle('off', !listOn);
    // the canvas stops drawing while the list is up
    (opt as any).paused = listOn;
    (window as any).__listView = listOn;
    if (listOn) render();
  });
  setInterval(() => { if (listOn) render(); }, 1500);
}

/* Every analysis, with a button.
 *
 * The suite's commands were reachable only from a terminal, so a reader looking
 * at a capture had no way to ask any of the questions the tool exists to answer.
 * Each entry states what it answers, runs it where the collector serves it, and
 * otherwise gives the exact command for this capture rather than a general one.
 */
{
  const panel = document.getElementById('tools')!;
  const btn = document.getElementById('toolsBtn')!;
  let on = false;

  type Tool = { name: string; what: string; api?: string; cli: string };
  const TOOLS: Tool[] = [
    { name: 'health', what: 'is this instance accepting writes, and what does it hold', api: '/api/health', cli: 'curl /api/health' },
    { name: 'stats', what: 'counts by host, service and status', api: '/api/stats', cli: 'curl /api/stats' },
    { name: 'check', what: 'does this capture conform to the span contract', api: '/api/check', cli: 'execviz check <db>' },
    { name: 'capture', what: 'is the capture complete, and is it sound', api: '/api/capture', cli: 'execviz integrity <db>' },
    { name: 'concurrency', what: 'how much ran at once, and where it queued', api: '/api/concurrency', cli: 'execviz concurrency <db>' },
    { name: 'selftime', what: 'time spent in a span itself rather than in its children', api: '/api/selftime', cli: 'execviz selftime <db>' },
    { name: 'cost', what: 'what the capture costs to keep', api: '/api/cost', cli: 'execviz cost <db>' },
    { name: 'skew', what: 'do the hosts agree on the clock', api: '/api/skew', cli: 'execviz skew <db>' },
    { name: 'fingerprint', what: 'each program named by its behaviour', api: '/api/fingerprint', cli: 'execviz identity --records F' },
    { name: 'peers', what: 'other instances this one knows about', api: '/api/peers', cli: 'execviz peers <db>' },
    { name: 'views', what: 'saved views of this capture', api: '/api/views', cli: 'execviz views <db>' },
    { name: 'witness', what: 'spans checked against the syscalls their thread made', cli: 'execviz witness <db> --records capture.ndjson' },
    { name: 'unclaimed', what: 'syscalls no span accounts for, by program', cli: 'execviz unclaimed <db> --records capture.ndjson' },
    { name: 'decode', what: 'payloads parsed, and the fraction not parsed', cli: 'execviz decode --records capture.ndjson' },
    { name: 'detect', what: 'stuck, orphaned, inverted and unwitnessed spans', cli: 'execviz detect <db> --rules shape.rules' },
    { name: 'stress', what: 'fault injections this capture implies, and those it excludes', api: '/api/stress', cli: 'execviz stress --records capture.ndjson' },
    { name: 'stress run', what: 'carry the derived plan out against an unmodified program', cli: 'execviz-stress --from-plan plan.json -- ./your-program' },
    { name: 'flame', what: 'where measured time went: the span tree folded by self time', api: '/api/flame', cli: 'execviz flame <db>' },
    { name: 'critical', what: 'the chain that set the duration, not everything that was slow', api: '/api/critical', cli: 'execviz critical <db>' },
    { name: 'cpu', what: 'where the cpu actually was, sampled, including code nobody instrumented', cli: 'execviz-cpu --freq 99 --seconds 10 > cpu.ndjson && execviz cpu --records cpu.ndjson' },
    { name: 'profile', what: 'this capture counted against the project\u2019s indicators', cli: 'execviz profile --records capture.ndjson --profile execviz.profile.json' },
    { name: 'drift', what: 'programs whose behavioural shape moved against a baseline', cli: 'execviz drift --records now.json --baseline before.json' },
    { name: 'iouring', what: 'work submitted outside the syscall boundary', cli: 'execviz iouring --records capture.ndjson' },
  ];

  const out = document.createElement('pre');

  const render = () => {
    panel.replaceChildren();
    const h = document.createElement('h3');
    h.textContent = 'Analyses';
    panel.append(h);
    for (const t of TOOLS) {
      const row = document.createElement('div');
      row.className = 'cmd';
      const b = document.createElement('button');
      b.textContent = t.api ? 'run' : 'copy';
      b.addEventListener('click', async () => {
        if (t.api) {
          out.textContent = 'running ' + t.name + '…';
          try {
            const r = await fetch(t.api);
            const txt = await r.text();
            out.textContent = t.name + '\n\n' + txt;
          } catch (e) {
            out.textContent = t.name + '\n\nnot available here: ' + String(e)
              + '\n\nfrom a terminal:\n  ' + t.cli;
          }
        } else {
          out.textContent = t.name + '\n\nthis one runs from a terminal:\n  ' + t.cli;
          navigator.clipboard?.writeText(t.cli).catch(() => {});
        }
      });
      const what = document.createElement('div');
      what.className = 'what';
      what.innerHTML = '<b>' + t.name + '</b> <span>' + t.what + '</span>';
      row.append(b, what);
      panel.append(row);
    }
    const h2 = document.createElement('h3');
    h2.textContent = 'Result';
    panel.append(h2, out);
  };

  btn.addEventListener('click', () => {
    on = !on;
    panel.classList.toggle('on', on);
    btn.classList.toggle('off', !on);
    (window as any).__listView = on || document.getElementById('listview')!.classList.contains('on');
    if (on) render();
  });
}

// ========================================================================
// THE COMMAND BAR
// ========================================================================
/* The same analyses the terminal runs, typed here.
 *
 * It sends a name, not a command line: the collector matches the name against
 * a list and calls the matching function, so nothing here can become a shell.
 * Administration, `account` above all, has no route over the network and is
 * refused with the reason rather than failing blankly.
 */
{
  const bar = document.getElementById('cmdbar');
  const input = document.getElementById('cmdInput') as HTMLInputElement | null;
  const out = document.getElementById('cmdOut');
  const btn = document.getElementById('cmdBtn');
  const history: string[] = [];
  let at = 0;

  const show = (text: string) => { if (out) out.textContent = text; };

  const run = async (cmd: string) => {
    if (!cmd.trim()) return;
    history.push(cmd); at = history.length;
    show(`execviz ${cmd}\n\nrunning...`);
    try {
      const r = await fetch(`/api/command?cmd=${encodeURIComponent(cmd)}`);
      const text = await r.text();
      try {
        show(`execviz ${cmd}\n\n` + JSON.stringify(JSON.parse(text), null, 2));
      } catch {
        show(`execviz ${cmd}\n\n` + text);      // not JSON, show it as it came
      }
    } catch (e) {
      show(`execviz ${cmd}\n\nthe collector did not answer: ${String(e)}`);
    }
  };

  btn?.addEventListener('click', () => {
    const on = bar?.classList.toggle('on');
    btn.classList.toggle('off', !on);
    if (on) { input?.focus(); if (!out?.textContent) void run('help'); }
  });

  input?.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { void run(input.value); input.value = ''; }
    // the last few commands, because the useful one is usually the previous one
    else if (e.key === 'ArrowUp' && at > 0) { at--; input.value = history[at] ?? ''; }
    else if (e.key === 'ArrowDown') { at = Math.min(history.length, at + 1); input.value = history[at] ?? ''; }
    else if (e.key === 'Escape') { bar?.classList.remove('on'); }
    e.stopPropagation();                 // the map's keys are not the console's
  });
}
