// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'canopy.ts',
  script_path: 'execviz-ui/src/canopy.ts',
  module_name: 'canopy',
  version: '0.13.0',
  description: 'The routes, drawn to their own layer.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['camera', 'lod', 'model'],
  external_dependencies: [],
  features: ['canopy'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { Camera } from './camera.js';
import { Model } from './model.js';
import { BUDGET, HAIRLINE, routeMuted, muteVersion } from './lod.js';

// ========================================================================
// CLASSES
// ========================================================================

/**
 * The routes, drawn to their own layer.
 *
 * A route's geometry is a function of the model and the camera and nothing
 * else: it does not vary with the playhead. Redrawing it on every frame is
 * therefore work whose result is already known. The layer is rebuilt when the
 * camera moves or the model changes, and composited otherwise, which is the
 * general rule rather than a trick for this one case: anything that does not
 * vary with T should not be recomputed at T.
 */
export class CanopyLayer {
  private canvas: HTMLCanvasElement | OffscreenCanvas;
  private ctx: CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D;
  private key = '';
  private dpr: number;
  drawn = 0;

  constructor(dpr: number) {
    this.dpr = dpr;
    this.canvas = typeof OffscreenCanvas !== 'undefined'
      ? new OffscreenCanvas(1, 1) : document.createElement('canvas');
    this.ctx = (this.canvas as HTMLCanvasElement).getContext('2d') as CanvasRenderingContext2D;
  }

  /** Cheap identity for "would this redraw produce the same pixels?" */
  private stateKey(model: Model, cam: Camera): string {
    return [muteVersion(), 
      model.spans.length, model.routes.length,
      Math.round(cam.x), Math.round(cam.y), cam.z.toFixed(4),
      Math.round(cam.w), Math.round(cam.h),
    ].join('|');
  }

  /** Forces a rebuild when something other than the camera changed what it draws. */
  invalidate() { this.key = ''; }

  ensure(model: Model, cam: Camera): HTMLCanvasElement | OffscreenCanvas {
    const key = this.stateKey(model, cam);
    if (key === this.key) return this.canvas;
    this.key = key;
    const w = Math.max(1, Math.round(cam.w * this.dpr));
    const h = Math.max(1, Math.round(cam.h * this.dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w; this.canvas.height = h;
    }
    const c = this.ctx;
    c.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    c.clearRect(0, 0, cam.w, cam.h);
    c.lineJoin = 'round'; c.lineCap = 'round';

    if (!model.routesByWeight) {
      model.routesByWeight = [...model.routes].sort((a, b) => b.count - a.count);
    }
    // Geometry from a previous camera position differs from none: a route not
    // drawn this pass kept its old screen path, and the token layer went on
    // drawing tokens along a line that is no longer where that route is.
    for (const r of model.routes) r.screen = undefined;

    let drawn = 0;
    for (const r of model.routesByWeight) {
      if (drawn >= BUDGET.routes) break;
      const a = model.clusterById.get(r.from), b = model.clusterById.get(r.to);
      if (!a || !b) continue;
      if (!cam.segmentVisible(a.wx, a.wy, b.wx, b.wy)) continue;
      const muted = routeMuted(r.from, r.to);
      const thick = (0.8 + (r.count / model.maxRouteCount) * 5.5) * Math.min(1.2, cam.z * 1.3);
      if (thick < HAIRLINE) continue;
      const pa = { x: cam.toScreenX(a.wx), y: cam.toScreenY(a.wy) };
      const pb = { x: cam.toScreenX(b.wx), y: cam.toScreenY(b.wy) };
      const bend = r.crossHost ? 0.55 : 0.22;
      const mx = (pa.x + pb.x) / 2, my = (pa.y + pb.y) / 2;
      const dx = pb.x - pa.x, dy = pb.y - pa.y, len = Math.hypot(dx, dy) || 1;
      const cx = mx - (dy / len) * bend * len * 0.16, cy = my + (dx / len) * bend * len * 0.16;
      // Muted signals stay on the map, drawn faintly. A path that disappears
      // cannot be told apart from one that was never there.
      c.strokeStyle = muted ? 'rgba(110,125,145,0.07)'
                    : r.errors > 0 ? 'rgba(210,86,90,0.28)' : 'rgba(120,150,180,0.16)';
      c.lineWidth = thick;
      c.setLineDash(r.crossHost ? [2, 5] : []);
      c.beginPath(); c.moveTo(pa.x, pa.y); c.quadraticCurveTo(cx, cy, pb.x, pb.y); c.stroke();
      c.setLineDash([]);
      // the control point is what the token path needs, so it is cached with
      // the geometry rather than recomputed per frame
      r.screen = { ax: pa.x, ay: pa.y, cx, cy, bx: pb.x, by: pb.y, thick };
      drawn++;
    }
    this.drawn = drawn;
    return this.canvas;
  }

  composite(ctx: CanvasRenderingContext2D, cam: Camera) {
    // The layer is composited every frame. With smoothing on this is a
    // filtered resample of a full-screen image, which is most of the frame's
    // cost where there is no GPU to do it. The source is already the size it
    // needs to be, so the filter buys nothing.
    const smooth = ctx.imageSmoothingEnabled;
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(this.canvas as CanvasImageSource, 0, 0, cam.w, cam.h);
    ctx.imageSmoothingEnabled = smooth;
  }
}
