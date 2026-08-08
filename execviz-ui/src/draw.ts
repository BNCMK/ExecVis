// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'draw.ts',
  script_path: 'execviz-ui/src/draw.ts',
  module_name: 'draw',
  version: '0.13.0',
  description: 'Draw log lines ON the map, beside the thing they belong to.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['camera', 'colour', 'lod', 'model', 'shapes', 'types'],
  external_dependencies: [],
  features: ['draw'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { Camera } from './camera.js';
import { Model, Cluster, opennessOf, activeOnRoute, CLOCK } from './model.js';
import { BUDGET, SHAPE_MIN_PX, bandAlpha, expandFactor, tierFor } from './lod.js';
import { statusColour, statusMark } from './colour.js';
import { FAMILY_ANGLE, Family, familyOf, Span } from './types.js';

// ========================================================================
// TYPES
// ========================================================================

export interface FlipState { cluster: string; family: string; index: number; count: number }

export interface Options {
  expand: number; labels: boolean; canopy: boolean; flip?: FlipState;
  /**
   * Draw log lines ON the map, beside the thing they belong to.
   *
   * A log line is a fact about a span, so it belongs where that span is rather
   * than in a list down the side. The side console remains, because a thousand
   * lines want a list, but it is the second way to read them and not the first.
   */
  logsInMap?: boolean;
  /** what the reader has navigated to: its lines are the ones drawn */
  focus?: string | null;
}

export interface HitNode { span: Span; cluster: Cluster; x: number; y: number; r: number }

export interface FrameStats {
  routesDrawn: number; clustersDrawn: number; interiors: number;
  nodes: number; tier: string;
  /** the largest family currently resolved, which is what the flipbook steps through */
  biggest?: { cluster: string; family: string; count: number };
  /** Where the flipbook wheel is on screen, so a drag can tell whether it
   *  started on the wheel or on the map. Absent when the wheel is not up. */
  flipAt?: { x: number; y: number; r: number };
}

// ========================================================================
// CONSTANTS
// ========================================================================

const WEDGE_HALF = 17;
void WEDGE_HALF;   // kept: the wedge still names the family's direction

/** How many lines are drawn beside one node before the rest are counted. */
const LINES_SHOWN = 7;

// ========================================================================
// INTERNALS
// ========================================================================

/**
 * Draws a span's own log lines in the world, next to the span.
 *
 * Placed to the right of the node and clipped to a fixed width, so a long line
 * does not push the map around: the map's layout never moves because of the data
 * in it, and a log line is data.
 */
function drawLinesAt(ctx: CanvasRenderingContext2D, s: Span, x: number, y: number,
                     t: number, scale: number) {
  const events = (s.events || []).filter((e) => e.t <= t);
  if (!events.length) return;
  const shown = events.slice(-LINES_SHOWN);
  const hidden = events.length - shown.length;

  // The box is anchored to the node but does not grow without limit with it:
  // at depth several spans sit close together and boxes that scale with zoom
  // cover each other and the rails they belong to.
  const bs = Math.min(scale, 1.35);
  const lh = 11 * bs;
  const pad = 6 * bs;
  // Measure the lines and size the box to them. Estimating a character width and
  // hoping the text fits is what put the message outside its own box.
  ctx.save();
  ctx.font = `${Math.max(7, 9 * bs)}px ui-monospace, monospace`;
  const texts = shown.map((e) => {
    const lvl = String(e.level || 'info');
    const mark = lvl === 'error' ? '!' : lvl === 'warning' ? '~' : '\u00b7';
    return { lvl, text: `${mark} ${String(e.msg ?? '')}` };
  });
  const cap = 300 * bs;
  const fit = (txt: string) => {
    if (ctx.measureText(txt).width <= cap - pad * 2) return txt;
    let lo = 1, hi = txt.length;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (ctx.measureText(txt.slice(0, mid) + '\u2026').width <= cap - pad * 2) lo = mid; else hi = mid - 1;
    }
    return txt.slice(0, lo) + '\u2026';
  };
  const lines = texts.map((o) => ({ ...o, text: fit(o.text) }));
  const extra = hidden ? `+${hidden} earlier` : '';
  const w = Math.min(cap, Math.max(
    90 * bs,
    ...lines.map((o) => ctx.measureText(o.text).width),
    extra ? ctx.measureText(extra).width : 0) + pad * 2);
  const h = (lines.length + (hidden ? 1 : 0)) * lh + pad;
  // Anchored to the node, but never off the edge: a box half outside the canvas
  // is a message no reader can read.
  let bx = x + 14 * bs, by = y - h / 2;
  // Keep clear of the panels drawn over the canvas. A box under the log console
  // is not hidden, it shows through the gaps between its controls, which is what
  // makes both unreadable.
  const over = document.querySelector('.console.on') as HTMLElement | null;
  if (over) {
    const cr = over.getBoundingClientRect();
    const kr = ctx.canvas.getBoundingClientRect();
    const scale = ctx.canvas.width / Math.max(1, kr.width);
    const rx = (cr.left - kr.left) * scale, ry = (cr.top - kr.top) * scale;
    const rw = cr.width * scale, rh = cr.height * scale;
    const hits = !(bx + w < rx - 6 || bx > rx + rw + 6 || by + h < ry - 6 || by > ry + rh + 6);
    if (hits) {
      if (rx - w - 12 > 8) bx = rx - w - 12;          // to the left of it
      else if (ry - h - 12 > 8) by = ry - h - 12;     // or above it
      else return;                                    // no room: the console has it
    }
  }
  if (bx + w > ctx.canvas.width - 8) bx = x - 14 * bs - w;
  if (bx < 8) bx = 8;
  if (by < 8) by = 8;
  if (by + h > ctx.canvas.height - 8) by = ctx.canvas.height - 8 - h;
  ctx.restore();

  ctx.save();
  ctx.globalAlpha = 1;
  ctx.fillStyle = 'rgba(8,11,16,0.97)';
  ctx.strokeStyle = 'rgba(120,140,165,0.28)';
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.rect(bx, by, w, h);
  ctx.fill(); ctx.stroke();

  // a hairline from the node to its lines, so which belongs to what is never
  // a matter of proximity alone
  ctx.strokeStyle = 'rgba(120,140,165,0.45)';
  ctx.beginPath(); ctx.moveTo(x, y); ctx.lineTo(bx, y); ctx.stroke();

  ctx.font = `${Math.max(7, 9 * bs)}px ui-monospace, monospace`;
  ctx.textBaseline = 'middle';
  ctx.textAlign = 'left';          // inherited centring drew the text off the box
  let ly = by + lh / 2 + 3 * bs;
  for (const o of lines) {
    // status by colour AND by a leading mark, never by colour alone
    ctx.fillStyle = o.lvl === 'error' ? statusColour('err', 0.95)
                  : o.lvl === 'warning' ? statusColour('warn', 0.95)
                  : 'rgba(190,205,220,0.82)';
    ctx.fillText(o.text, bx + pad, ly);
    ly += lh;
  }
  if (hidden) {
    // absence stated rather than implied by a list that  stops
    ctx.fillStyle = 'rgba(150,165,185,0.7)';
    ctx.fillText(extra, bx + pad, ly);
  }
  ctx.restore();
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function draw(
  ctx: CanvasRenderingContext2D, model: Model, cam: Camera, t: number, opt: Options,
): { hits: HitNode[]; stats: FrameStats } {
  const hits: HitNode[] = [];
  const stats: FrameStats = { routesDrawn: 0, clustersDrawn: 0, interiors: 0, nodes: 0, tier: 'field' };
  ctx.lineJoin = 'round'; ctx.lineCap = 'round';

// ========================================================================
// HOSTS: THE TIER ABOVE THE FIELD
// ========================================================================
  for (const h of model.hosts) {
    if (!cam.visible(h.wx, h.wy, h.wr)) continue;
    const hp = { x: cam.toScreenX(h.wx), y: cam.toScreenY(h.wy) }, hr = h.wr * cam.z;
    const st = opennessOf(h, t);
    const col = st.worst === 'err' ? statusColour('err', 0.45)
      : st.active > 0 ? statusColour('run', 0.4) : statusColour('ok', 0.35);
    if (hr < 7) {
      ctx.globalAlpha = 0.85; ctx.fillStyle = col;
      ctx.beginPath(); ctx.arc(hp.x, hp.y, Math.max(2.5, hr), 0, 7); ctx.fill();
      ctx.globalAlpha = 1; continue;
    }
    ctx.fillStyle = 'rgba(18,26,34,0.35)'; ctx.globalAlpha = 0.55;
    ctx.beginPath(); ctx.arc(hp.x, hp.y, hr, 0, 7); ctx.fill();
    ctx.strokeStyle = col; ctx.globalAlpha = 0.32; ctx.lineWidth = 1.5;
    ctx.setLineDash([7, 6]); ctx.beginPath(); ctx.arc(hp.x, hp.y, hr, 0, 7); ctx.stroke();
    ctx.setLineDash([]); ctx.globalAlpha = 1;
    const ha = bandAlpha(hr, 14, 40, 430, 780);
    if (opt.labels && ha > 0.01) {
      ctx.globalAlpha = ha; ctx.fillStyle = '#8b97a6';
      // The gap between the two lines follows the type size. Pinned at a fixed
      // 11px it held only while the font was small, and the lines ran together
      // as soon as the circle grew enough to enlarge it.
      const fs = Math.max(10, Math.min(19, hr * 0.05));
      ctx.font = `600 ${fs}px system-ui,sans-serif`;
      ctx.textAlign = 'center';
      const gap = Math.round(fs * 1.25);
      ctx.fillText(`\u25c8 ${h.id}`, hp.x, hp.y - hr - 9 - gap);
      ctx.save();
      ctx.globalAlpha = 0.62;
      ctx.fillText(`${st.total} spans`, hp.x, hp.y - hr - 9);
      ctx.restore();
      ctx.globalAlpha = 1;
    }
  }

  // While the flipbook is open the map beneath it is dimmed. Reading one span
  // out of a lit map is the thing the flipbook is for, and a bright map behind
  // it is the noise it was opened to escape.
  const flipDim = false;   // the flipbook reads fine on a lit map
  let flipAt: { x: number; y: number; r: number } | null = null;

// ========================================================================
// CANOPY: THE LAYER IS ALREADY COMPOSITED; ONLY THE TOKENS VARY WITH T
// ========================================================================
  if (opt.canopy) {
    // Tokens are bounded for the frame, not only per route. A wide view has
    // hundreds of routes in it, and six tokens each is thousands of moving
    // things drawn to a few pixels: cost with nothing to read for it.
    let tokenBudget = BUDGET.tokensPerFrame;
    for (const r of model.routesByWeight ?? []) {
      const g = r.screen;
      if (!g) continue;
      stats.routesDrawn++;
      if (cam.z <= 0.18 || g.thick <= 1.6) continue;
      if (tokenBudget <= 0) break;
      const perRoute = Math.max(1, Math.min(BUDGET.tokensPerRoute,
                                            Math.round(cam.z * BUDGET.tokensPerRoute)));

      // A summary route carries a count and no spans, so there is nothing to
      // place a token from. Traffic between hosts is what the fleet view is for,
      // so the rate is drawn instead: tokens spaced by the route's share of the
      // busiest one, moving with the playhead. They are paced, not individual
      // calls, and they are drawn hollow to say so.
      if (!r.spans.length && r.count > 0) {
        const share = r.count / Math.max(1, model.maxRouteCount);
        const many = Math.max(1, Math.round(share * perRoute));
        const err = r.errors > 0;
        for (let k = 0; k < many && tokenBudget > 0; k++) {
          tokenBudget--;
          const phase = ((t / Math.max(1, CLOCK * 0.5)) + k / many) % 1;
          const u2 = 1 - phase;
          const x = u2 * u2 * g.ax + 2 * u2 * phase * g.cx + phase * phase * g.bx;
          const y = u2 * u2 * g.ay + 2 * u2 * phase * g.cy + phase * phase * g.by;
          ctx.strokeStyle = err ? statusColour('err', 0.8) : statusColour('ok', 0.7);
          ctx.lineWidth = 1.4;
          ctx.beginPath(); ctx.arc(x, y, r.crossHost ? 3.5 : 2.6, 0, 7); ctx.stroke();
        }
        continue;
      }

      for (const s of activeOnRoute(r, t, perRoute)) {
        tokenBudget--;
        const open = s.end === null;
        // An unfinished span has no known end, so computing progress against
        // `t` made it exactly 1 and drew the token ON the destination: work
        // still in flight, shown as arrived. It travels but never lands, and it
        // is drawn hollow so "still going" is not a matter of noticing where a
        // dot stopped.
        const f = open
          ? Math.min(0.82, (t - s.start) / Math.max(1, CLOCK * 0.25))
          : Math.max(0, Math.min(1, (t - s.start) / Math.max(1, (s.end as number) - s.start)));
        const u = 1 - f;
        const x = u * u * g.ax + 2 * u * f * g.cx + f * f * g.bx;
        const y = u * u * g.ay + 2 * u * f * g.cy + f * f * g.by;
        const rad = r.crossHost ? 4 : 3;
        // status carried by shape as well as colour
        if (s.status === 'error') {
          ctx.fillStyle = statusColour('err', 0.95);
          ctx.beginPath(); ctx.arc(x, y, rad + 0.5, 0, 7); ctx.fill();
        } else if (open) {
          ctx.strokeStyle = statusColour('run', 0.95); ctx.lineWidth = 1.6;
          ctx.beginPath(); ctx.arc(x, y, rad, 0, 7); ctx.stroke();
        } else {
          ctx.fillStyle = '#fff';
          ctx.beginPath(); ctx.arc(x, y, rad, 0, 7); ctx.fill();
        }
      }
    }
  }

// ========================================================================
// CLUSTERS, EACH AT THE TIER ITS ON-SCREEN SIZE EARNS
// ========================================================================
  let interiors = 0;
  const order = model.clusters
    .map((c) => ({ c, r: c.wr * cam.z }))
    .sort((x, y) => y.r - x.r);
  for (const { c, r: rawR } of order) {
    if (!cam.visible(c.wx, c.wy, c.wr, 20)) continue;
    const tier = tierFor(rawR);
    if (opt.flip && c.id === opt.flip.cluster) {
      flipAt = { x: cam.toScreenX(c.wx), y: cam.toScreenY(c.wy),
                 r: Math.max(140, c.wr * cam.z * 1.35) };
    }
    // A cluster sitting inside a host alongside others is drawn within the room
    // it has, so neighbours never clash at the fleet view. As the level is zoomed
    // into, its neighbours leave the frame and it grows back to its true size:
    // the size at depth is unchanged, only how much of it is drawn while nested.
    const roomScreen = c.roomR * cam.z * 0.92;
    const settled = Math.min(1, Math.max(0, (rawR - 70) / 70));
    const fitted = Math.min(rawR, roomScreen);
    const base = fitted + (rawR - fitted) * settled;
    // Expansion is bounded by the room the node has. A cluster grows into space
    // that is free, never over the one beside it, so nothing expands until there
    // is somewhere to expand into. The node's own size is always available.
    const grown = base * expandFactor(rawR, opt.expand);
    // Room wins. A node is drawn within the space it has and grows as zooming
    // makes that space larger, so it reaches full size and expands only once
    // there is somewhere to expand into.
    // Two hard limits, at every scale: a circle never reaches its neighbour,
    // and never grows past the circle that contains it. Whichever is tighter
    // wins, so nothing overlaps anything at any zoom, and a node only enlarges
    // once zooming has made room for it.
    const host = model.hosts.find((hh) => hh.id === c.host);
    const parentR = host ? host.wr * cam.z : Infinity;
    const insideParent = parentR * 0.34;
    const screenR = Math.min(
      grown,
      roomScreen > 0 ? roomScreen : grown,
      insideParent,
    );
    const p = { x: cam.toScreenX(c.wx), y: cam.toScreenY(c.wy) };
    const st = opennessOf(c, t);
    const member = c.total ? Math.min(0.95, 0.25 + (st.active / Math.max(1, c.total)) * 0.7) : 0.4;
    const fill = statusColour(st.worst === 'err' ? 'err' : st.active > 0 ? 'run' : 'ok', member);
    stats.clustersDrawn++;
    if (tier === 'dot') {
      ctx.fillStyle = fill; ctx.globalAlpha = st.active > 0 ? 0.9 : 0.5;
      ctx.beginPath(); ctx.arc(p.x, p.y, Math.max(2.5, screenR), 0, 7); ctx.fill();
      ctx.globalAlpha = 1; continue;
    }
    ctx.fillStyle = 'rgba(16,22,28,0.5)';
    ctx.beginPath(); ctx.arc(p.x, p.y, screenR, 0, 7); ctx.fill();
    ctx.strokeStyle = fill; ctx.lineWidth = st.active > 0 ? 2 : 1.2;
    ctx.globalAlpha = st.active > 0 ? 0.9 : 0.55;
    ctx.beginPath(); ctx.arc(p.x, p.y, screenR, 0, 7); ctx.stroke();
    ctx.globalAlpha = 1;

    // Status is never carried by colour alone: a cluster holding an
    // error is also marked, so the picture survives greyscale and the most
    // common colour vision deficiencies.
    if (st.worst === 'err' && screenR >= 7) {
      ctx.fillStyle = '#ff9aa2';
      ctx.font = `${Math.max(9, Math.min(15, screenR * 0.34))}px system-ui,sans-serif`;
      ctx.textAlign = 'center'; ctx.textBaseline = 'middle';
      ctx.fillText(statusMark('err'), p.x, p.y - screenR - 3);
    } else if (st.active > 0 && screenR >= 7) {
      ctx.strokeStyle = fill; ctx.globalAlpha = 0.8; ctx.lineWidth = 1.4;
      ctx.setLineDash([3, 3]);
      ctx.beginPath(); ctx.arc(p.x, p.y, screenR + 3.5, 0, 7); ctx.stroke();
      ctx.setLineDash([]); ctx.globalAlpha = 1;
    }

    if (opt.labels) {
      const small = bandAlpha(screenR, 9, 16, 74, 110);
      if (small > 0.01) {
        ctx.globalAlpha = small; ctx.fillStyle = '#7f8f9f';
        ctx.font = '11px system-ui,sans-serif'; ctx.textAlign = 'center'; ctx.textBaseline = 'middle';
        ctx.fillText(c.label, p.x, p.y + screenR + 13); ctx.globalAlpha = 1;
      }
      const big = Math.max(0, Math.min(1, (screenR - 96) / 54));
      if (big > 0.01) {
        ctx.globalAlpha = big; ctx.fillStyle = '#cdd9e5';
        ctx.font = `600 ${Math.max(12, Math.min(22, screenR * 0.055))}px system-ui,sans-serif`;
        ctx.textAlign = 'center'; ctx.textBaseline = 'alphabetic';
        ctx.fillText(c.label, p.x, p.y - screenR - 10); ctx.globalAlpha = 1;
      }
    }

    // Interiors are the expensive part, so only the largest few are resolved.
    // At any zoom a reader is looking at one cluster, not forty.
    if (interiors >= BUDGET.clustersWithInteriors) continue;
    interiors++;
    drawInterior(ctx, c, p.x, p.y, screenR, tier, t, opt, hits, stats);
    stats.tier = tier;
  }
  stats.interiors = interiors;

// ========================================================================
// LOG LINES, ON THE MAP, BESIDE WHAT THEY BELONG TO
// ========================================================================
  //
  // Drawn last so they sit above the geometry, and only for what the reader has
  // navigated to: every span's lines at once would be a wall of text, which is
  // the thing the side console is for.
  if (opt.logsInMap !== false) {
    const focus = opt.focus;
    // Boxes must not pile on top of each other or run off the edge: a log line
    // no reader can read is not on the map, it is in the way. Occupied bands are
    // remembered and a box that would collide is skipped, with the count of
    // what was skipped drawn instead so the omission is stated.
    const taken: Array<[number, number]> = [];
    let skipped = 0;
    const W = ctx.canvas.width, H = ctx.canvas.height;
    for (const hit of hits) {
      const isFocus = focus ? hit.span.span_id === focus : false;
      const hasErrors = (hit.span.events || []).some((e) => e.level === 'error');
      // the focused span always; otherwise anything that has something to say
      // and is resolved large enough for the text to be readable
      if (!isFocus && !(hasErrors && hit.r > 9)) continue;
      const scale = Math.max(0.55, Math.min(1.25, hit.r / 14));
      const boxH = 90 * scale;
      // off the edge is the same as not drawn, and worse, because it looks drawn
      if (hit.x < 20 || hit.x > W - 40 || hit.y < 20 || hit.y > H - 20) { skipped++; continue }
      // Where a box would land on one already drawn it moves down until it is
      // clear, keeping a leader line back to the span it belongs to. Past a few
      // rows there is no longer room to read them, and the remainder is counted
      // and left to the console rather than stacked into a smear.
      const clashes = (yy: number) =>
        taken.some(([a, b]) => yy - boxH / 2 < b && yy + boxH / 2 > a);
      let by = hit.y, rows = 0;
      while (clashes(by) && rows < 3 && by + boxH < H - 20) { by += boxH * 0.62; rows++; }
      if (clashes(by)) { skipped++; continue }
      taken.push([by - boxH / 2, by + boxH / 2]);
      if (by !== hit.y) {
        ctx.save();
        ctx.strokeStyle = 'rgba(150,165,185,0.45)';
        ctx.lineWidth = 1;
        ctx.beginPath(); ctx.moveTo(hit.x, hit.y); ctx.lineTo(hit.x, by - boxH / 2 + 4); ctx.stroke();
        ctx.restore();
      }
      drawLinesAt(ctx, hit.span, hit.x, by, t, scale);
      if (taken.length >= 6) break;
    }
    if (skipped > 0) {
      ctx.save();
      ctx.fillStyle = 'rgba(150,165,185,0.75)';
      ctx.font = '11px ui-monospace, monospace';
      ctx.textAlign = 'left'; ctx.textBaseline = 'bottom';
      ctx.fillText(`${skipped} more with lines here, not drawn: zoom in or open the console`,
                   16, H - 42);
      ctx.restore();
    }
  }

  if (flipDim) {
    // Dim what is behind the flipbook, and only what is behind it. Covering the
    // node it is stepping through defeats the point: the reader is left with a
    // dark screen and a label.
    ctx.save();
    ctx.fillStyle = 'rgba(5,7,10,0.78)';
    ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height);
    if (flipAt) {
      const g = ctx.createRadialGradient(flipAt.x, flipAt.y, flipAt.r * 0.35,
                                         flipAt.x, flipAt.y, flipAt.r);
      g.addColorStop(0, 'rgba(0,0,0,1)');
      g.addColorStop(1, 'rgba(0,0,0,0)');
      ctx.globalCompositeOperation = 'destination-out';
      ctx.fillStyle = g;
      ctx.beginPath(); ctx.arc(flipAt.x, flipAt.y, flipAt.r, 0, 7); ctx.fill();
    }
    ctx.restore();
  }

  // Worked out from the flipped cluster directly rather than left to whichever
  // loop iteration happened to run: the wheel is up, so its position is known.
  if (opt.flip) {
    const fc = model.clusters.find((c) => c.id === opt.flip!.cluster);
    if (fc) {
      stats.flipAt = { x: cam.toScreenX(fc.wx), y: cam.toScreenY(fc.wy),
                       r: Math.max(140, fc.wr * cam.z * 1.35) };
    }
  }
  if (!stats.flipAt && flipAt) stats.flipAt = flipAt;
  return { hits, stats };
}

// ========================================================================
// INTERNALS
// ========================================================================

function drawInterior(
  ctx: CanvasRenderingContext2D, c: Cluster, cx: number, cy: number, R: number,
  tier: string, t: number, opt: Options, hits: HitNode[], stats: FrameStats,
) {
  ctx.save();
  ctx.beginPath(); ctx.arc(cx, cy, R, 0, 7); ctx.clip();

  for (const [famName, angle] of Object.entries(FAMILY_ANGLE)) {
    const fam = famName as Family;
    const a = (angle * Math.PI) / 180, ux = Math.cos(a), uy = -Math.sin(a);
    ctx.strokeStyle = 'rgba(28,38,48,0.9)'; ctx.lineWidth = 0.5;
    ctx.beginPath(); ctx.moveTo(cx, cy); ctx.lineTo(cx + ux * R, cy + uy * R); ctx.stroke();
    // If the circle is large enough to see inside, it shows what is inside. A
    // node that resolves and then empties as you keep zooming differs from one
    // that never resolved, so the band has a recorder rather than falling to zero.
    const la = R > 60 ? Math.max(0.35, bandAlpha(R, 78, 118, 900, 2000)) : bandAlpha(R, 78, 118, 300, 520);
    if (opt.labels && la > 0.01) {
      const bundle = c.byFamily.get(fam);
      ctx.globalAlpha = la; ctx.fillStyle = '#6b7a88';
      ctx.font = `${Math.max(9, Math.min(20, R * 0.038))}px ui-monospace,monospace`;
      ctx.textAlign = 'center'; ctx.textBaseline = 'middle';
      // Pull the label inside by its own measured width. Anchoring at a fixed
      // fraction of the radius clipped it out of the circle as soon as the count
      // made the text longer.
      const txt = bundle ? `${fam}  \u00b7${bundle.length}` : fam;
      const halfW = ctx.measureText(txt).width / 2;
      const inset = Math.max(R * 0.34, Math.min(R - halfW - 6, R * 0.88));
      ctx.lineWidth = 3; ctx.lineJoin = 'round';
      ctx.strokeStyle = 'rgba(4,6,9,0.9)';
      ctx.strokeText(txt, cx + ux * inset, cy + uy * inset);
      ctx.fillStyle = '#93a3b3';
      ctx.fillText(txt, cx + ux * inset, cy + uy * inset);
      ctx.globalAlpha = 1;
    }
  }

  for (const [fam, bundle] of c.byFamily) {
    const heading = FAMILY_ANGLE[fam];
    if (tier === 'compass' || tier === 'cluster') {
      const anyActive = activeCount(bundle, t) > 0;
      const anyErr = bundle.some((s) => s.status === 'error' && t >= s.start);
      const col = anyErr ? statusColour('err', 0.5) : anyActive ? statusColour('run', 0.5) : statusColour('ok', 0.45);
      const a = (heading * Math.PI) / 180, ux = Math.cos(a), uy = -Math.sin(a);
      ctx.strokeStyle = col; ctx.globalAlpha = 0.85;
      ctx.lineWidth = Math.max(3, Math.min(R * 0.05, 2 + bundle.length * 0.7));
      ctx.beginPath();
      ctx.moveTo(cx + ux * R * 0.16, cy + uy * R * 0.16);
      ctx.lineTo(cx + ux * R * 0.74, cy + uy * R * 0.74);
      ctx.stroke(); ctx.globalAlpha = 1;
      continue;
    }
    // rails: a bounded sample, because a family of hundreds cannot be read as
    // hundreds of rails. The flipbook is how an individual is reached, so the
    // sample is a way of seeing the shape, never the only way to reach a row.
    const flip = opt.flip && opt.flip.cluster === c.id && opt.flip.family === fam;
    const list = bundle.length > BUDGET.railsPerFamily
      ? sample(bundle, BUDGET.railsPerFamily) : bundle;
    const n = list.length;
    list.forEach((s, idx) => {
      let u = n > 1 ? (idx + 0.5) / n : 0.5;
      if (flip && opt.flip) {
        // One row at a time, held in the middle of its own wedge, with the rest
        // bunched to the edges: the family's direction never changes, only the
        // spacing inside it, so the map still reads the same at a glance.
        // The selected row steps aside rather than leaping to the middle, and
        // the rest keep enough of the wedge to stay separable. Pinning it dead
        // centre and crushing everything else into the last few percent leaves a
        // hole in the middle and a solid block at each edge.
        const sel = Math.max(0, Math.min(n - 1, opt.flip.index));
        const at = n > 1 ? (sel + 0.5) / n : 0.5;
        const GAP = 0.10;                     // how far the chosen row is set apart
        u = idx === sel ? at
          : idx < sel ? at - GAP - (sel - idx) * ((at - GAP) / Math.max(1, sel + 1))
          : at + GAP + (idx - sel) * ((1 - at - GAP) / Math.max(1, n - sel));
        u = Math.max(0.02, Math.min(0.98, u));
      }
      // A straight line out along the family's heading, offset sideways. Fanning
      // by angle curls the far end into a spiral, where the rows crowd together
      // and the text stops being readable.
      const rad = (heading * Math.PI) / 180;
      const baseR = R * (0.34 + 0.54 * u);
      const off = (u - 0.5) * 2 * R * 0.30;          // sideways, not around
      const px = -Math.sin(rad), py = -Math.cos(rad); // perpendicular to heading
      const dur = s.end === null ? 200 : s.end - s.start;
      const len = R * 0.14 * (0.5 + 0.9 * Math.min(1, dur / 200));
      const x0 = cx + Math.cos(rad) * (baseR - len) + px * off,
            y0 = cy - Math.sin(rad) * (baseR - len) + py * off;
      const x1 = cx + Math.cos(rad) * baseR + px * off,
            y1 = cy - Math.sin(rad) * baseR + py * off;
      const reached = t >= s.start;
      const col = s.status === 'error' ? statusColour('err', 0.5)
        : reached ? statusColour('ok', 0.5) : 'rgba(120,140,160,1)';
      const isSel = flip && opt.flip !== undefined && idx === Math.max(0, Math.min(n - 1, opt.flip.index));
      ctx.strokeStyle = reached ? col : 'rgba(90,103,115,0.3)';
      ctx.globalAlpha = isSel ? 1 : flip ? (reached ? 0.35 : 0.15) : (reached ? 0.8 : 0.3);
      ctx.lineWidth = Math.max(2, R * (isSel ? 0.03 : 0.016));
      ctx.beginPath(); ctx.moveTo(x0, y0); ctx.lineTo(x1, y1); ctx.stroke();
      const nodeR = Math.max(2.5, R * 0.018);
      ctx.globalAlpha = reached ? 0.9 : 0.25;
      if (nodeR >= SHAPE_MIN_PX) {
        const { path, scale } = shapeOf(s.kind, nodeR * 1.45);
        ctx.save(); ctx.translate(x1, y1); ctx.scale(scale, scale);
        ctx.strokeStyle = col; ctx.lineWidth = Math.max(1.1, nodeR * 0.3) / scale;
        ctx.stroke(path); ctx.restore();
      } else {
        ctx.fillStyle = col; ctx.beginPath(); ctx.arc(x1, y1, nodeR, 0, 7); ctx.fill();
      }
      ctx.globalAlpha = 1;
      stats.nodes++;
      if (!stats.biggest || bundle.length > stats.biggest.count) {
        stats.biggest = { cluster: c.id, family: fam, count: bundle.length };
      }
      hits.push({ span: s, cluster: c, x: x1, y: y1, r: Math.max(6, R * 0.035) });
      if (opt.labels && tier === 'span' && R > 700) {
        ctx.fillStyle = '#9fb0c0'; ctx.font = '10px ui-monospace,monospace';
        ctx.textAlign = 'left'; ctx.textBaseline = 'middle';
        const d = s.duration_ms;
        const dTxt = s.end === null ? ' · running' : d !== null ? ` · ${fmtMs(d)}` : '';
        ctx.fillText(`${s.name} · ${s.kind}${dTxt}`, x1 + 8, y1);
      }
    });
  }

  ctx.fillStyle = '#5a6573';
  ctx.beginPath(); ctx.arc(cx, cy, Math.max(3, R * 0.05), 0, 7); ctx.fill();
  ctx.restore();
}

function activeCount(list: Span[], t: number): number {
  let n = 0;
  for (const s of list) if (t >= s.start && t <= (s.end ?? Number.POSITIVE_INFINITY)) { n++; if (n > 3) break; }
  return n;
}

function sample<T>(list: T[], k: number): T[] {
  const out: T[] = []; const step = list.length / k;
  for (let i = 0; i < k; i++) out.push(list[Math.floor(i * step)]);
  return out;
}

function fmtMs(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  if (ms >= 1) return `${Math.round(ms)}ms`;
  return `${ms.toFixed(2)}ms`;
}

import { shapeFor } from './shapes.js';
function shapeOf(kind: string, r: number) { return shapeFor(kind, r); }

export { familyOf };
