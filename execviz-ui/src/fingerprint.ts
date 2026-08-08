// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'fingerprint.ts',
  script_path: 'execviz-ui/src/fingerprint.ts',
  module_name: 'fingerprint',
  version: '0.13.0',
  description: 'The fingerprint view: a profile across fixed axes, one line per reading.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: [],
  external_dependencies: [],
  features: ['fingerprint', 'profile'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;

// ========================================================================

// ========================================================================
// TYPES
// ========================================================================

/**
 * The fingerprint view: a profile across fixed axes, one line per
 * reading.
 *
 * The form is settled by measurement, not taste. A radial glyph would make a
 * memorable shape whose outline depends on the arbitrary order of the axes and
 * whose area implies a magnitude that means nothing. A waveform needs an axis
 * with intrinsic order, which these quantities do not have. A profile matches
 * the operation a reader performs; is this the same as before; and it
 * says *which* axis moved, which a shape cannot.
 */
export interface Axis { name: string; raw: number; norm: number }

export interface Band { axis: string; baseline: number; band: number; value: number; outside_band: boolean }

export interface Signature {
  axes: Axis[];
  bands?: Band[];
  matches?: boolean;
  largestDeparture?: string;
}

const LABEL: Record<string, string> = {
  branching: 'branching', concentration: 'concentration', loop_density: 'loops',
  jitter: 'jitter', io_ratio: 'io share', depth: 'depth',
};

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function parse(feed: any): Signature | null {
  if (feed && Array.isArray(feed.invariants)) {
    return { axes: feed.invariants as Axis[] };
  }
  if (feed && Array.isArray(feed.axes)) {
    const bands = feed.axes as Band[];
    return {
      axes: bands.map((b) => ({ name: b.axis, raw: b.value, norm: b.value })),
      bands,
      matches: feed.matches_baseline === true,
      largestDeparture: feed.largest_departure,
    };
  }
  return null;
}

export function draw(ctx: CanvasRenderingContext2D, sig: Signature, w: number, h: number) {
  ctx.clearRect(0, 0, w, h);
  const n = sig.axes.length;
  if (!n) return;
  const padL = 12, padR = 12, padT = 26, padB = 30;
  const plotW = w - padL - padR, plotH = h - padT - padB;
  const x = (i: number) => padL + (n > 1 ? (plotW * i) / (n - 1) : plotW / 2);
  const y = (v: number) => padT + plotH * (1 - Math.max(0, Math.min(1, v)));

  // axes, drawn first and faint: the reading is the subject, not the grid
  ctx.strokeStyle = 'rgba(30,39,48,1)';
  ctx.lineWidth = 1;
  for (let i = 0; i < n; i++) {
    ctx.beginPath(); ctx.moveTo(x(i), padT); ctx.lineTo(x(i), padT + plotH); ctx.stroke();
  }
  ctx.strokeStyle = 'rgba(30,39,48,0.7)';
  for (const v of [0, 0.5, 1]) {
    ctx.beginPath(); ctx.moveTo(padL, y(v)); ctx.lineTo(padL + plotW, y(v)); ctx.stroke();
  }

  // the stability band, where a baseline exists. A narrow band is the useful
  // case: it means repeated runs agree, so a departure from it is the signal.
  if (sig.bands) {
    ctx.fillStyle = 'rgba(56,139,253,0.16)';
    ctx.beginPath();
    sig.bands.forEach((b, i) => { const yy = y(b.baseline + b.band); i ? ctx.lineTo(x(i), yy) : ctx.moveTo(x(i), yy); });
    for (let i = sig.bands.length - 1; i >= 0; i--) {
      const b = sig.bands[i];
      ctx.lineTo(x(i), y(b.baseline - b.band));
    }
    ctx.closePath(); ctx.fill();
    ctx.strokeStyle = 'rgba(56,139,253,0.5)';
    ctx.setLineDash([3, 3]); ctx.lineWidth = 1;
    ctx.beginPath();
    sig.bands.forEach((b, i) => { const yy = y(b.baseline); i ? ctx.lineTo(x(i), yy) : ctx.moveTo(x(i), yy); });
    ctx.stroke(); ctx.setLineDash([]);
  }

  // the reading itself
  const departed = sig.bands ? sig.bands.some((b) => b.outside_band) : false;
  ctx.strokeStyle = departed ? '#ff7b72' : '#56d364';
  ctx.lineWidth = 2;
  ctx.beginPath();
  sig.axes.forEach((a, i) => { const yy = y(a.norm); i ? ctx.lineTo(x(i), yy) : ctx.moveTo(x(i), yy); });
  ctx.stroke();

  sig.axes.forEach((a, i) => {
    const out = sig.bands?.[i]?.outside_band;
    ctx.fillStyle = out ? '#ff7b72' : departed ? '#8b97a6' : '#56d364';
    ctx.beginPath(); ctx.arc(x(i), y(a.norm), out ? 4 : 3, 0, 7); ctx.fill();
  });

  // labels last, so nothing draws over them
  ctx.font = '9px ui-monospace,monospace';
  ctx.textAlign = 'center'; ctx.textBaseline = 'top';
  sig.axes.forEach((a, i) => {
    const out = sig.bands?.[i]?.outside_band;
    ctx.fillStyle = out ? '#ff7b72' : '#6b7a88';
    const name = LABEL[a.name] ?? a.name;
    ctx.fillText(name, x(i), padT + plotH + 6);
    ctx.fillStyle = out ? '#ff7b72' : '#8b97a6';
    ctx.fillText(a.norm.toFixed(2), x(i), padT + plotH + 17);
  });

  ctx.textAlign = 'left'; ctx.textBaseline = 'alphabetic';
  ctx.font = '10px ui-monospace,monospace';
  if (sig.bands) {
    ctx.fillStyle = departed ? '#ff7b72' : '#56d364';
    ctx.fillText(departed
      ? `departs from baseline · largest: ${sig.largestDeparture ?? '?'}`
      : 'matches baseline', padL, 16);
  } else {
    ctx.fillStyle = '#6b7a88';
    ctx.fillText('signature of this capture', padL, 16);
  }
}
