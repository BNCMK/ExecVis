// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'viewpoint.ts',
  script_path: 'execviz-ui/src/viewpoint.ts',
  module_name: 'viewpoint',
  version: '0.13.0',
  description: 'A permalink.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['camera'],
  external_dependencies: [],
  features: ['viewpoint'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { Camera } from './camera.js';

// ========================================================================
// TYPES
// ========================================================================

/**
 * A permalink.
 *
 * A person who finds something must be able to hand the finding to a colleague.
 * Replay preserves the data and discards the viewpoint; this carries the
 * viewpoint and nothing else; a link that carries data is a copy pretending to
 * be a reference.
 */
export interface Viewpoint {
  x: number; y: number; z: number;
  t: number;
  layer: string;
  span?: string;
  logs?: boolean;
  wf?: boolean;
  /**
   * The selected time range, when one is set.
   *
   * Required by the design and absent from this until now: "the selected range
   * travels in the permalink, since a window no reader can share is half a
   * feature". A colleague opening the link landed on the right map
   * at the right instant with the window silently cleared, which is the one
   * part of the finding that was hardest to arrive at.
   */
  from?: number;
  to?: number;
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function capture(cam: Camera, t: number, layer: string,
                        span: string | null, logs: boolean, wf: boolean,
                        range?: { from: number; to: number } | null): Viewpoint {
  const r = (v: number) => Math.round(v * 100) / 100;
  const vp: Viewpoint = { x: r(cam.x), y: r(cam.y), z: r(cam.z), t: Math.round(t), layer };
  if (span) vp.span = span;
  if (logs) vp.logs = true;
  if (wf) vp.wf = true;
  if (range) { vp.from = Math.round(range.from); vp.to = Math.round(range.to); }
  return vp;
}

export function toQuery(vp: Viewpoint): string {
  const p = new URLSearchParams();
  p.set('v', `${vp.x},${vp.y},${vp.z},${vp.t}`);
  if (vp.layer !== 'none') p.set('layer', vp.layer);
  if (vp.span) p.set('span', vp.span);
  if (vp.logs) p.set('logs', '1');
  if (vp.wf) p.set('wf', '1');
  if (vp.from !== undefined && vp.to !== undefined) p.set('w', `${vp.from},${vp.to}`);
  return p.toString();
}

/** Reads a viewpoint from a link, ignoring anything malformed rather than
 *  refusing the whole link: a partly-usable view beats an error page. */
export function fromQuery(search: string): Viewpoint | null {
  const p = new URLSearchParams(search);
  const v = p.get('v');
  if (!v) return null;
  const parts = v.split(',').map(Number);
  if (parts.length < 4 || parts.some((n) => !isFinite(n))) return null;
  const out: Viewpoint = {
    x: parts[0], y: parts[1],
    // A link is input like any other. Finite is not the same as usable: a zoom
    // of 0 arrives finite and makes every world coordinate infinite, so the
    // value is clamped to the same bounds the camera enforces everywhere else.
    z: Math.max(Camera.MIN_Z, Math.min(Camera.MAX_Z, parts[2])),
    t: Math.max(0, parts[3]),
    layer: p.get('layer') ?? 'none',
    span: p.get('span') ?? undefined,
    logs: p.get('logs') === '1',
    wf: p.get('wf') === '1',
  };
  const w = p.get('w');
  if (w) {
    const [a, b] = w.split(',').map(Number);
    // a malformed window is dropped, not fatal: a partly-usable view beats an
    // error page
    if (isFinite(a) && isFinite(b) && b > a) { out.from = a; out.to = b; }
  }
  return out;
}
