// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'overview.ts',
  script_path: 'execviz-ui/src/overview.ts',
  module_name: 'overview',
  version: '0.13.0',
  description: 'The overview, built from the rollup rather than from spans.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['model'],
  external_dependencies: [],
  features: ['overview', 'rollup'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { Cluster, Host, Model, placeCluster, placeHost } from './model.js';

// ========================================================================
// TYPES
// ========================================================================

/**
 * The overview, built from the rollup rather than from spans.
 *
 * A reader looking at a whole system is not looking at spans, so the spans need
 * not be present. Every tier above the individual span is exactly what the
 * rollup carries, so the map can be drawn from a summary measured in kilobytes
 * while the leaves stay where they are until someone descends.
 */
export interface RollupNode {
  id: string; tier: string; digest: string;
  rollup: { spans: number; errors: number; open: number; io_share: number;
            total_ms: number; first?: number; last?: number; kinds: Record<string, number> };
  children: number;
  nodes?: RollupNode[];
}

// ========================================================================
// INTERNALS
// ========================================================================

function regionOf(label: string): string {
  switch (label) {
    case 'gateway': case 'MainThread': case 'api': case 'edge-agent': return 'entry';
    case 'sensors': case 'queue': return 'data';
    default: return 'logic';
  }
}

/**
 * A model with real hosts and clusters and no spans at all.
 *
 * The counts are the rollup's counts, so what the upper tiers show is the same
 * arithmetic the leaves would have produced. Nothing here fabricates an
 * individual: a cluster standing for thirty thousand operations draws what the
 * summary supports and reports how many it stands for.
 */
function labelOf(c: RollupNode): string {
  return c.id.split('/').pop() || c.id;
}

/** World units a row of clusters needs so each keeps its own text. */
function needFor(list: RollupNode[]): number {
  return list.reduce((a, c) => a + Math.max(96, labelOf(c).length * 12 + 34), 0);
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function fromRollup(tree: RollupNode): Model {
  const clusters: Cluster[] = [];
  const clusterById = new Map<string, Cluster>();
  const hostNodes = tree.nodes ?? [];

  const hosts: Host[] = hostNodes.map((h, i) => {
    // the same placement the span-built map uses, not a second copy of it
    const { wx, wy, wr } = placeHost(i, hostNodes.length);
    return {
      id: h.id, wx, wy, wr,
      starts: new Float64Array(0), ends: new Float64Array(0), errorStarts: new Float64Array(0),
      total: h.rollup.spans,
    };
  });

  hostNodes.forEach((h, hi) => {
    const host = hosts[hi];
    const kids = h.nodes ?? [];
    const byRegion = new Map<string, RollupNode[]>();
    for (const c of kids) {
      const label = c.id.split('/').slice(1).join('/') || c.id;
      const r = regionOf(label);
      let l = byRegion.get(r); if (!l) { l = []; byRegion.set(r, l); }
      l.push(c);
    }
    for (const [region, list] of byRegion) {
      list.sort((a, b) => a.id.localeCompare(b.id));
      const n = list.length;
      list.forEach((c, i) => {
        const label = c.id.split('/').slice(1).join('/') || c.id;
        const p = placeCluster(host, region, i, n, needFor(list));
        const cl: Cluster = {
          id: c.id, label, region, slot: i, host: host.id,
          wx: p.wx, wy: p.wy, wr: p.wr, roomR: 0,
          spans: [], byFamily: new Map(),
          starts: new Float64Array(0), ends: new Float64Array(0), errorStarts: new Float64Array(0),
          total: c.rollup.spans,
          summary: { spans: c.rollup.spans, errors: c.rollup.errors, open: c.rollup.open },
        };
        clusters.push(cl);
        clusterById.set(cl.id, cl);
      });
      const row = clusters.slice(clusters.length - n);
      row.forEach((c, i) => {
        let nearest = Infinity;
        if (i > 0) nearest = Math.min(nearest, Math.abs(c.wx - row[i - 1].wx));
        if (i < row.length - 1) nearest = Math.min(nearest, Math.abs(row[i + 1].wx - c.wx));
        c.roomR = nearest === Infinity ? c.wr : Math.max(1, nearest / 2);
      });
    }
    host.summary = { spans: h.rollup.spans, errors: h.rollup.errors, open: h.rollup.open };
  });

  // Edges come with the summary, so the fleet view still shows what moves
  // between nodes. They carry counts only: no span identities and no payloads,
  // which is the detail the summary leaves behind.
  const routes: Model['routes'] = [];
  let maxRouteCount = 1;
  for (const e of ((tree as unknown) as { edges?: any[] }).edges ?? []) {
    const from = String(e.from ?? ''), to = String(e.to ?? '');
    if (!clusterById.has(from) || !clusterById.has(to)) continue;
    const count = Number(e.count ?? 0);
    if (count > maxRouteCount) maxRouteCount = count;
    routes.push({
      from, to, count, errors: Number(e.errors ?? 0),
      crossHost: from.split('/')[0] !== to.split('/')[0],
      variance: 0, spans: [], open: [],
    });
  }

  return {
    spans: [], byId: new Map(), clusters, clusterById, hosts,
    routes, maxRouteCount, tMax: 1000,
  };
}

