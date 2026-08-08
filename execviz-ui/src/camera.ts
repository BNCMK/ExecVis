// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'camera.ts',
  script_path: 'execviz-ui/src/camera.ts',
  module_name: 'camera',
  version: '0.13.0',
  description: 'World-to-screen transform and the culling that follows from it. */',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['model'],
  external_dependencies: [],
  features: ['camera'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { WORLD_W, WORLD_H } from './model.js';

// ========================================================================
// CLASSES
// ========================================================================

/** World-to-screen transform and the culling that follows from it. */
export class Camera {
  x = WORLD_W / 2; y = WORLD_H / 2; z = 0.5;
  w = 1; h = 1;
  targetZ = 0.5;
  targetX: number | null = null;
  targetY: number | null = null;

  /** The tightest and widest the view is allowed to be. */
  static readonly MIN_Z = 0.02;
  static readonly MAX_Z = 40;

  /**
   * A viewport is never zero-sized.
   *
   * A hidden tab and an element measured before layout both report 0×0, and a
   * zero width made `fit()` compute a zoom of exactly 0; after which every
   * world coordinate was Infinity and hit testing, panning and fly-to all
   * produced garbage. The clamp costs a comparison and removes the whole class.
   */
  resize(w: number, h: number) {
    this.w = Math.max(1, w || 0);
    this.h = Math.max(1, h || 0);
  }

  fit() {
    // the one assignment that used to bypass the zoom clamp every other path
    // applies
    const z = Math.min(this.w / WORLD_W, this.h / WORLD_H) * 0.92;
    this.z = this.targetZ = Math.max(Camera.MIN_Z, Math.min(Camera.MAX_Z, z || Camera.MIN_Z));
    this.x = WORLD_W / 2; this.y = WORLD_H / 2;
    this.targetX = this.targetY = null;
  }

  toScreenX(wx: number) { return (wx - this.x) * this.z + this.w / 2; }
  toScreenY(wy: number) { return (wy - this.y) * this.z + this.h / 2; }
  toWorldX(sx: number) { return (sx - this.w / 2) / this.z + this.x; }
  toWorldY(sy: number) { return (sy - this.h / 2) / this.z + this.y; }

  /** Zoom anchored so the world point under the cursor stays there. */
  zoomAt(sx: number, sy: number, factor: number) {
    const wx = this.toWorldX(sx), wy = this.toWorldY(sy);
    const z = Math.max(Camera.MIN_Z, Math.min(Camera.MAX_Z, this.z * factor));
    this.x = wx - (sx - this.w / 2) / z;
    this.y = wy - (sy - this.h / 2) / z;
    this.z = this.targetZ = z;
    this.targetX = this.targetY = null;
  }

  flyTo(wx: number, wy: number, z?: number) {
    this.targetX = wx; this.targetY = wy;
    if (z !== undefined) this.targetZ = Math.max(Camera.MIN_Z, Math.min(Camera.MAX_Z, z));
  }

  step() {
    if (this.targetX !== null && this.targetY !== null) {
      this.x += (this.targetX - this.x) * 0.18;
      this.y += (this.targetY - this.y) * 0.18;
      if (Math.abs(this.targetX - this.x) < 0.5 && Math.abs(this.targetY - this.y) < 0.5) {
        this.x = this.targetX; this.y = this.targetY;
        this.targetX = this.targetY = null;
      }
    }
    // Ease, then snap. Without the snap the zoom converges asymptotically and
    // never arrives, so anything keyed on the camera state believes it
    // is still moving forever and never gets to reuse its work.
    this.z += (this.targetZ - this.z) * 0.18;
    if (Math.abs(this.targetZ - this.z) < this.targetZ * 0.0005) this.z = this.targetZ;
  }

  /** True when a world circle intersects the viewport at all. */
  visible(wx: number, wy: number, wr: number, pad = 0): boolean {
    const sx = this.toScreenX(wx), sy = this.toScreenY(wy), r = wr * this.z + pad;
    return sx + r >= 0 && sx - r <= this.w && sy + r >= 0 && sy - r <= this.h;
  }

  /** True when a world segment could touch the viewport. */
  segmentVisible(ax: number, ay: number, bx: number, by: number, pad = 40): boolean {
    const x1 = this.toScreenX(ax), y1 = this.toScreenY(ay);
    const x2 = this.toScreenX(bx), y2 = this.toScreenY(by);
    if (x1 < -pad && x2 < -pad) return false;
    if (x1 > this.w + pad && x2 > this.w + pad) return false;
    if (y1 < -pad && y2 < -pad) return false;
    if (y1 > this.h + pad && y2 > this.h + pad) return false;
    return true;
  }
}
