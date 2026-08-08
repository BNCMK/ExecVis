// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'shapes.ts',
  script_path: 'execviz-ui/src/shapes.ts',
  module_name: 'shapes',
  version: '0.13.0',
  description: 'The shape channel. These forms are not invented here: flowchart, activity and process notation settled them decades ago, so a reader who has seen any of them arrives already knowing most of the vocabulary.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: [],
  external_dependencies: [],
  features: ['shapes'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

/**
 * The shape channel. These forms are not invented here: flowchart,
 * activity and process notation settled them decades ago, so a reader who has
 * seen any of them arrives already knowing most of the vocabulary.
 */
const cache = new Map<string, Path2D>();

// ========================================================================
// INTERNALS
// ========================================================================

function build(kind: string, r: number): Path2D {
  const p = new Path2D();
  switch (kind) {
    case 'call':                                   // predefined process, ANSI X3.5
      p.rect(-r, -r * 0.66, r * 2, r * 1.32);
      p.moveTo(-r * 0.62, -r * 0.66); p.lineTo(-r * 0.62, r * 0.66);
      p.moveTo(r * 0.62, -r * 0.66); p.lineTo(r * 0.62, r * 0.66); break;
    case 'branch':                                 // decision
      p.moveTo(0, -r); p.lineTo(r, 0); p.lineTo(0, r); p.lineTo(-r, 0); p.closePath(); break;
    case 'loop':                                   // loop limit
      p.moveTo(-r * 0.5, -r * 0.7); p.lineTo(r * 0.5, -r * 0.7); p.lineTo(r, 0);
      p.lineTo(r * 0.5, r * 0.7); p.lineTo(-r * 0.5, r * 0.7); p.lineTo(-r, 0); p.closePath(); break;
    case 'io':                                     // input/output
      p.moveTo(-r * 0.6, -r * 0.62); p.lineTo(r, -r * 0.62);
      p.lineTo(r * 0.6, r * 0.62); p.lineTo(-r, r * 0.62); p.closePath(); break;
    case 'wait':                                   // timer event, BPMN
      p.arc(0, 0, r, 0, Math.PI * 2);
      p.moveTo(0, -r * 0.62); p.lineTo(0, 0); p.lineTo(r * 0.45, r * 0.28); break;
    case 'queue':                                  // multiple instance, BPMN
      for (let i = -1; i <= 1; i++) { const x = i * r * 0.58; p.moveTo(x, -r * 0.75); p.lineTo(x, r * 0.75); }
      break;
    case 'error':                                  // error event, BPMN
      p.moveTo(-r * 0.45, -r); p.lineTo(r * 0.25, -r * 0.18);
      p.lineTo(-r * 0.18, -r * 0.18); p.lineTo(r * 0.5, r);
      p.lineTo(-r * 0.1, r * 0.12); p.lineTo(r * 0.32, r * 0.12); p.closePath(); break;
    case 'spawn':                                  // fork, UML activity
      p.moveTo(-r, -r * 0.4); p.lineTo(r, -r * 0.4);
      p.moveTo(-r * 0.55, -r * 0.4); p.lineTo(-r * 0.55, r * 0.8);
      p.moveTo(0, -r * 0.4); p.lineTo(0, r * 0.8);
      p.moveTo(r * 0.55, -r * 0.4); p.lineTo(r * 0.55, r * 0.8); break;
    case 'external':                               // off-page connector
      p.moveTo(-r, -r * 0.72); p.lineTo(r * 0.35, -r * 0.72); p.lineTo(r, 0);
      p.lineTo(r * 0.35, r * 0.72); p.lineTo(-r, r * 0.72); p.closePath(); break;
    default:
      p.arc(0, 0, r, 0, Math.PI * 2);
  }
  return p;
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/**
 * Shapes are built at a quantised radius and reused. Rebuilding a path per node
 * per frame is the kind of cost that only shows up on a large capture.
 */
export function shapeFor(kind: string, radius: number): { path: Path2D; scale: number } {
  const q = Math.max(2, Math.round(radius));
  const key = `${kind}:${q}`;
  let p = cache.get(key);
  if (!p) { p = build(kind, q); cache.set(key, p); }
  return { path: p, scale: radius / q };
}
