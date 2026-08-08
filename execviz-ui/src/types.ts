// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'types.ts',
  script_path: 'execviz-ui/src/types.ts',
  module_name: 'types',
  version: '0.13.0',
  description: 'The wire format, as the core serves it. */',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: [],
  external_dependencies: [],
  features: ['types'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;

// ========================================================================

// ========================================================================
// TYPES
// ========================================================================

/** The wire format, as the core serves it. */
export interface Span {
  span_id: string;
  trace_id: string;
  parent_span_id: string | null;
  links: string[];
  name: string;
  kind: string;
  /** derived by the sender for convenience; this client recomputes it and
   *  ignores whatever arrives, so the two can never disagree */
  family?: string;
  /** raw time as captured; the client places it on the shared clock */
  start: number;
  /** raw, or null while the span is open */
  end: number | null;
  status: string;
  lifecycle: Array<{ t: number; type: string; context?: unknown }>;
  events: Array<{ t: number; level: string; msg: string }>;
  host_id: string;
  domain: string | null;
  attributes: Record<string, unknown>;
  /** real milliseconds, computed before normalisation; the only field that may
   *  be read as a duration (see the clock rule in the spec) */
  duration_ms: number | null;
}

export interface ClusterFeed {
  id: string;
  label: string;
  region: string;
  slot: number;
  host: string;
}

export interface Feed {
  spans: Span[];
  clusters: ClusterFeed[];
  /** the span of real time the whole store currently covers */
  window?: { lo: number; hi: number };
  cursor?: number;
  total?: number;
  truncated?: boolean;
}

export type Family = 'control' | 'io' | 'wait' | 'boundary' | 'fault';

export const FAMILY_ANGLE: Record<Family, number> = {
  control: 180, io: 0, wait: 45, boundary: 225, fault: 90,
};

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/**
 * The family a primitive belongs to.
 *
 * Derived from `kind`, never taken from the wire. The feed does send a `family`
 * for convenience, and this deliberately ignores it: accepting one would let a
 * sender contradict its own primitive, and two adapters could then classify the
 * same thing differently and draw a difference the program never had.
 *
 * Total by design; an unrecognised kind lands in `control` rather than leaving
 * a gap, and the conformance checker reports the unknown kind separately.
 */
export function familyOf(kind: string): Family {
  switch (kind) {
    case 'io': case 'external': return 'io';
    case 'wait': return 'wait';
    case 'queue': return 'boundary';
    case 'error': return 'fault';
    default: return 'control';
  }
}
