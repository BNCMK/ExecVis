// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'lod.ts',
  script_path: 'execviz-ui/src/lod.ts',
  module_name: 'lod',
  version: '0.13.0',
  description: 'Level of detail, keyed on how large a thing is on screen.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: [],
  external_dependencies: [],
  features: ['lod'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;

// ========================================================================

// ========================================================================
// TYPES
// ========================================================================

/**
 * Level of detail, keyed on how large a thing is on screen.
 *
 * Everything is always present in the model; the renderer only decides how much
 * of it is worth drawing. Below a threshold a node is not simplified for
 * aesthetic reasons, it is simplified because the detail would land on fewer
 * pixels than it costs.
 */
export type Tier = 'dot' | 'cluster' | 'compass' | 'rails' | 'span';

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function tierFor(screenRadius: number): Tier {
  // Individual spans resolve as soon as a circle is big enough to hold them,
  // rather than being held back until it fills the screen. A node that shows
  // nothing you can point at is a node you cannot work with, and everything
  // that acts on a span (the flipbook, its logs, its rails) needs one to exist
  // before it has anything to act on.
  if (screenRadius < 8) return 'dot';
  if (screenRadius < 70) return 'cluster';
  if (screenRadius < 260) return 'compass';
  if (screenRadius < 700) return 'rails';
  return 'span';
}

// ========================================================================
// CONSTANTS
// ========================================================================

export const SHAPE_MIN_PX = 5.5;

/** A stroke thinner than this paints nothing a reader can use. */
export const HAIRLINE = 0.35;

/** Budgets. A view that draws more than this per frame is not more informative,
 *  it is just slower: the extra marks land on top of each other. */
export const BUDGET = {
  routes: 220,
  tokensPerRoute: 6,
  /** Moving tokens drawn in one frame, whatever the zoom. */
  tokensPerFrame: 400,
  railsPerFamily: 60,
  clustersWithInteriors: 24,
};

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function expandFactor(screenRadius: number, max: number): number {
  if (screenRadius < 70) return 1;
  return 1 + (max - 1) * Math.min(1, (screenRadius - 70) / 70);
}

/** Fades a label in over one range and out over another, so two tiers never
 *  print text on top of each other during a zoom. */
export function bandAlpha(v: number, inA: number, inB: number, outA: number, outB: number): number {
  // Equal bounds divide by zero, and at x === a that is 0/0 → NaN, which becomes
  // a NaN globalAlpha and a mark that silently does not appear. No caller does
  // it today; a shared helper should not depend on that staying true.
  const ramp = (x: number, a: number, b: number) => {
    if (b === a) return x >= a ? 1 : 0;
    return Math.max(0, Math.min(1, (x - a) / (b - a)));
  };
  return ramp(v, inA, inB) * (1 - ramp(v, outA, outB));
}

// ========================================================================
// TYPES
// ========================================================================

/** Signals silenced by the reader.
 *
 * A busy fleet is unreadable when every path is drawn, so any source can have
 * its signals turned down: what it sends, what it receives, or both. Muted
 * paths are drawn faintly rather than removed, because a path that vanishes is
 * indistinguishable from one that never existed. */
export type MuteDir = 'to' | 'from' | 'both';

export const MUTED: Map<string, MuteDir> = new Map();

// ========================================================================
// CONSTANTS
// ========================================================================

/** When set, only this node's own traffic stays lit. Everything else is drawn
 *  faintly, so one thing can be read out of a busy fleet without hiding the
 *  rest of it. */
export const ISOLATE = { id: null as string | null };

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function muteVersion(): string {
  return (ISOLATE.id ?? '') + '|' + [...MUTED].map(([k, v]) => k + ':' + v).sort().join(',');
}

export function routeMuted(from: string, to: string): boolean {
  if (ISOLATE.id && from !== ISOLATE.id && to !== ISOLATE.id) return true;
  const f = MUTED.get(from), t = MUTED.get(to);
  return (f === 'from' || f === 'both') || (t === 'to' || t === 'both');
}
