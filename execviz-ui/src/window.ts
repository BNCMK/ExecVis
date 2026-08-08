// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'window.ts',
  script_path: 'execviz-ui/src/window.ts',
  module_name: 'window',
  version: '0.13.0',
  description: 'A window in time.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['i18n', 'model', 'types'],
  external_dependencies: [],
  features: ['window'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { Model, CLOCK } from './model.js';
import { Span } from './types.js';
import { t, num } from './i18n.js';

// ========================================================================
// TYPES
// ========================================================================

/**
 * A window in time.
 *
 * A person investigating an incident is investigating a *period*, and making
 * them re-filter each view separately is making them do the join by hand. One
 * range restricts every view at once.
 */
export interface Range { from: number; to: number }

let range: Range | null = null;
const listeners: Array<() => void> = [];

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function get(): Range | null { return range; }

export function set(r: Range | null) {
  if (r && r.to - r.from < 1) r = null;        // a zero-width drag is a click
  range = r ? { from: Math.max(0, Math.min(r.from, r.to)),
                to: Math.min(CLOCK, Math.max(r.from, r.to)) } : null;
  for (const l of listeners) l();
}

export function onChange(fn: () => void) { listeners.push(fn); }

/** True when a span overlaps the window at all; a span that straddles the
 *  boundary is inside it, because work that spans the incident is the work most
 *  likely to explain it. */
export function inside(s: Span): boolean {
  if (!range) return true;
  const end = s.end ?? CLOCK;
  return end >= range.from && s.start <= range.to;
}

/** A model restricted to the window, derived rather than re-fetched. */
export function restrict(model: Model): Model {
  if (!range) return model;
  const spans = model.spans.filter(inside);
  const keep = new Set(spans.map((s) => s.span_id));
  const byId = new Map(spans.map((s) => [s.span_id, s]));
  return {
    ...model,
    spans,
    byId,
    // clusters keep their places: a window is a filter on time, not on space,
    // and moving the map when a range is picked would lose the reader
    clusters: model.clusters.map((c) => ({ ...c, spans: c.spans.filter((s) => keep.has(s.span_id)) })),
    routes: model.routes.filter((r) => r.spans.some((s) => keep.has(s.span_id))),
    // The weight ordering is a cache over the FULL route list. Carrying it
    // through the spread meant both the canopy and the token layer iterated
    // routes this window had just excluded; the one view the window did not
    // restrict. Cleared so it is rebuilt from what remains.
    routesByWeight: undefined,
  };
}

export function label(): string {
  if (!range) return t('whole capture');
  return `${t('window')} ${num(range.from, 0)}-${num(range.to, 0)} / ${num(CLOCK, 0)}`;
}
