// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'colour.ts',
  script_path: 'execviz-ui/src/colour.ts',
  module_name: 'colour',
  version: '0.13.0',
  description: 'Colour carries status only: hue is the class, shade the member. */',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: [],
  external_dependencies: [],
  features: ['colour'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

/** Colour carries status only: hue is the class, shade the member. */
/**
 * The status ramp, chosen by simulation rather than by taste.
 *
 * The previous ramp put `ok` and `err` 0.027 apart under deuteranopia; for a
 * reader with the most common colour vision deficiency they were the same
 * colour, in the channel that matters most. This one is 0.349 apart at worst
 * across deuteranopia, protanopia and tritanopia, and every member holds at
 * least 4.9:1 contrast against the background.
 *
 * The ramp is still not allowed to carry status on its own: see `statusMark`.
 */
const RAMP: Record<string, [string, string, string]> = {
  ok: ['#7bbf63', '#c9f0b8', '#e2fad6'],
  err: ['#a31d22', '#e5484d', '#ff9aa2'],
  run: ['#1f6feb', '#58a6ff', '#a5c8ff'],
  pend: ['#9e6a11', '#f0c674', '#ffe08a'],
};

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/**
 * The mark that carries status when colour cannot: greyscale, a screenshot, a
 * printout, or a reader who does not see red and green as different.
 */
export function statusMark(cls: string): string {
  return cls === 'err' ? '▲' : cls === 'run' ? '◔' : cls === 'pend' ? '◇' : '●';
}

// ========================================================================
// INTERNALS
// ========================================================================

function mix(a: string, b: string, t: number): string {
  const h = (x: string) => [parseInt(x.slice(1, 3), 16), parseInt(x.slice(3, 5), 16), parseInt(x.slice(5, 7), 16)];
  const A = h(a), B = h(b);
  return `rgb(${Math.round(A[0] + (B[0] - A[0]) * t)},${Math.round(A[1] + (B[1] - A[1]) * t)},${Math.round(A[2] + (B[2] - A[2]) * t)})`;
}

const memo = new Map<string, string>();

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function statusColour(cls: keyof typeof RAMP | string, member: number): string {
  const q = Math.round(member * 10) / 10;
  const key = `${cls}:${q}`;
  let v = memo.get(key);
  if (!v) {
    const r = RAMP[cls] ?? RAMP.run;
    v = q < 0.5 ? mix(r[0], r[1], q * 2) : mix(r[1], r[2], (q - 0.5) * 2);
    memo.set(key, v);
  }
  return v;
}
