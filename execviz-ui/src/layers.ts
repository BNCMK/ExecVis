// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'layers.ts',
  script_path: 'execviz-ui/src/layers.ts',
  module_name: 'layers',
  version: '0.13.0',
  description: 'The secondary analytic layers.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['camera', 'model', 'types'],
  external_dependencies: [],
  features: ['layers'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { Camera } from './camera.js';
import { Model, opennessOf } from './model.js';
import { Family, familyOf, Span } from './types.js';

// ========================================================================
// TYPES
// ========================================================================

/**
 * The secondary analytic layers.
 *
 * Each answers a different question over the same model, and none re-captures
 * anything: they are reads, which makes them cheap enough to toggle.
 * The canopy stays primary; these are overlays and alternate modes, never
 * replacements for it.
 */
export type Layer = 'none' | 'density' | 'rings' | 'wedges';

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/**
 * Density field: "where are the hotspots?"
 *
 * Accumulates span weight onto a coarse grid in screen space and paints it as a
 * heat overlay. The grid is coarse on purpose; a hotspot is a region, and
 * per-pixel accumulation would cost more and say the same thing.
 */
export function drawDensity(ctx: CanvasRenderingContext2D, model: Model, cam: Camera, t: number) {
  const cellFade = Math.max(0, Math.min(1, (cam.z - 1.8) / 1.2));
  if (cellFade <= 0) return;
  const CELL = 24;
  const cols = Math.ceil(cam.w / CELL), rows = Math.ceil(cam.h / CELL);
  const grid = new Float64Array(cols * rows);
  let peak = 0;

  for (const c of model.clusters) {
    const st = opennessOf(c, t);
    // weight is what the tier knows: open work if spans are held, the
    // summary's own count when the overview is all there is
    const weight = st.active > 0 ? st.active : st.total * 0.05;
    if (weight <= 0) continue;
    const sx = cam.toScreenX(c.wx), sy = cam.toScreenY(c.wy);
    const r = Math.max(CELL, c.wr * cam.z * 1.6);
    const cx = Math.floor(sx / CELL), cy = Math.floor(sy / CELL);
    const span = Math.ceil(r / CELL);
    for (let gy = cy - span; gy <= cy + span; gy++) {
      if (gy < 0 || gy >= rows) continue;
      for (let gx = cx - span; gx <= cx + span; gx++) {
        if (gx < 0 || gx >= cols) continue;
        const dx = (gx + 0.5) * CELL - sx, dy = (gy + 0.5) * CELL - sy;
        const d = Math.hypot(dx, dy);
        if (d > r) continue;
        const falloff = 1 - d / r;
        const i = gy * cols + gx;
        grid[i] += weight * falloff * falloff;
        if (grid[i] > peak) peak = grid[i];
      }
    }
  }
  if (peak <= 0) return;

  ctx.save();
  ctx.globalCompositeOperation = 'lighter';
  for (let gy = 0; gy < rows; gy++) {
    for (let gx = 0; gx < cols; gx++) {
      const v = grid[gy * cols + gx] / peak;
      if (v < 0.02) continue;
      // one hue, varying intensity: a heat map with several hues invites the
      // reader to see categories that are not there
      ctx.fillStyle = `rgba(255,${Math.round(200 - v * 140)},60,${(v * 0.4).toFixed(3)})`;
      // A cell smaller than a few pixels of node is a square with nothing in
      // it, so the layer fades out rather than tiling the map with blocks.
      ctx.globalAlpha *= cellFade;
      ctx.fillRect(gx * CELL, gy * CELL, CELL, CELL);
    }
  }
  ctx.restore();
}

// ========================================================================
// INTERNALS
// ========================================================================

function depthOf(s: Span, model: Model): number {
  let d = 0, cur: Span | undefined = s;
  while (cur && d < 40) {
    const p: string | null = cur.parent_span_id;
    if (!p) break;
    cur = model.byId.get(p);
    d++;
  }
  return d;
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/**
 * Collapsed tree-rings: "how is depth distributed?"
 *
 * One ring per causal depth, each divided by family, sized by how much work sits
 * at that depth. It answers a question the map cannot: the map shows where work
 * is, this shows how deeply nested it is, which is what distinguishes a flat
 * pipeline from a recursive one at a glance.
 */
export function drawRings(ctx: CanvasRenderingContext2D, model: Model, cam: Camera, t: number) {
  if (!model.spans.length) return;                 // depth needs the spans themselves
  const byDepth = new Map<number, Map<Family, number>>();
  let maxDepth = 0, total = 0;
  for (const s of model.spans) {
    if (s.start > t) continue;
    const d = depthOf(s, model);
    maxDepth = Math.max(maxDepth, d);
    let fams = byDepth.get(d);
    if (!fams) { fams = new Map(); byDepth.set(d, fams); }
    const f = familyOf(s.kind);
    fams.set(f, (fams.get(f) ?? 0) + 1);
    total++;
  }
  if (!total) return;
  // Below two levels the rings have nothing to separate, and drawing them puts a
  // featureless disc over the map. The caption below still reports the depth and
  // the span count, so the layer says what it found rather than showing nothing.
  const tooShallow = maxDepth < 2;

  // The wheel is read on its own. While it is up the map beneath it is dimmed
  // for the whole time it is shown, so the rings are legible instead of sitting
  // on top of the fleet.
  ctx.save();
  ctx.fillStyle = 'rgba(5,7,10,0.82)';
  ctx.fillRect(0, 0, cam.w, cam.h);
  ctx.restore();

  const cx = cam.w / 2, cy = cam.h / 2;
  const outer = Math.min(cam.w, cam.h) * 0.36;
  const ringW = outer / Math.max(1, maxDepth + 1);

  ctx.save();
  ctx.globalAlpha = 0.9;
  for (const [d, fams] of (tooShallow ? [] : [...byDepth].sort((a, b) => a[0] - b[0]))) {
    const atDepth = [...fams.values()].reduce((a, b) => a + b, 0);
    const r0 = d * ringW + 2, r1 = (d + 1) * ringW - 1;
    let angle = -Math.PI / 2;
    for (const [fam, n] of [...fams].sort((a, b) => a[0].localeCompare(b[0]))) {
      const sweep = (n / atDepth) * Math.PI * 2;
      ctx.beginPath();
      ctx.arc(cx, cy, r1, angle, angle + sweep);
      ctx.arc(cx, cy, r0, angle + sweep, angle, true);
      ctx.closePath();
      // the family's own direction is already spoken for by the map, so depth
      // rings borrow its hue only, at an intensity set by share
      ctx.fillStyle = famColour(fam, 0.25 + 0.6 * (n / atDepth));
      ctx.fill();
      angle += sweep;
    }
    if (ringW > 13) {
      ctx.fillStyle = '#8b97a6';
      ctx.font = '9px ui-monospace,monospace';
      ctx.textAlign = 'center'; ctx.textBaseline = 'middle';
      ctx.fillText(`${d}`, cx, cy - (r0 + r1) / 2);
    }
  }
  ctx.globalAlpha = 1;
  ctx.fillStyle = '#6b7a88';
  ctx.font = '10px ui-monospace,monospace';
  ctx.textAlign = 'center';
  // Below two levels there is nothing for rings to separate, so the layer
  // states that rather than drawing one featureless disc over the map.
  ctx.fillText(maxDepth < 2
    ? `depth 0..${maxDepth} · ${total} spans reached · too shallow for rings to separate`
    : `depth 0..${maxDepth} · ${total} spans reached`, cx, cy + outer + 18);
  ctx.restore();
}

// ========================================================================
// INTERNALS
// ========================================================================

function famColour(f: Family, a: number): string {
  const base: Record<Family, [number, number, number]> = {
    control: [157, 180, 200], io: [121, 184, 255], wait: [227, 179, 65],
    boundary: [179, 157, 219], fault: [255, 123, 114],
  };
  const [r, g, b] = base[f];
  return `rgba(${r},${g},${b},${a.toFixed(2)})`;
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/**
 * Filled wedges: the fingerprint as geometry, for comparison at a glance.
 *
 * This is the same six invariants the fingerprint panel plots as a profile
 *, drawn as one wedge per axis. It is offered as a comparison mode and
 * not as the primary reading, precisely because a wedge's *area* implies a
 * magnitude that the invariants do not have; because the profile, not
 * this, is the settled form.
 */
export function drawWedges(ctx: CanvasRenderingContext2D, axes: { name: string; norm: number }[],
                           cam: Camera) {
  if (!axes.length) return;
  const cx = cam.w / 2, cy = cam.h / 2;
  const R = Math.min(cam.w, cam.h) * 0.3;
  const step = (Math.PI * 2) / axes.length;
  ctx.save();
  axes.forEach((a, i) => {
    const a0 = -Math.PI / 2 + i * step, a1 = a0 + step * 0.86;
    const r = Math.max(4, R * a.norm);
    ctx.beginPath();
    ctx.moveTo(cx, cy);
    ctx.arc(cx, cy, r, a0, a1);
    ctx.closePath();
    ctx.fillStyle = `rgba(56,139,253,${(0.18 + a.norm * 0.35).toFixed(2)})`;
    ctx.fill();
    ctx.strokeStyle = 'rgba(121,184,255,0.7)'; ctx.lineWidth = 1; ctx.stroke();
    const mid = (a0 + a1) / 2;
    ctx.fillStyle = '#8b97a6';
    ctx.font = '9px ui-monospace,monospace';
    ctx.textAlign = 'center'; ctx.textBaseline = 'middle';
    ctx.fillText(a.name, cx + Math.cos(mid) * (R + 16), cy + Math.sin(mid) * (R + 16));
  });
  ctx.strokeStyle = 'rgba(30,39,48,1)';
  for (const f of [0.5, 1]) {
    ctx.beginPath(); ctx.arc(cx, cy, R * f, 0, 7); ctx.stroke();
  }
  ctx.restore();
}
