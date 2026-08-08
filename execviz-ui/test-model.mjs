// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: test-model.mjs
//  script_path: execviz-ui/test-model.mjs
//  module_name: test-model
//  version: 0.53.1
//  description: Unit tests for the model logic.
//  kind: module
//  spec: internal
//  internal_dependencies: model
//  external_dependencies: 
//  features: test-model
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

/**
 * Unit tests for the model logic.
 *
 * The renderer had none: every check was behavioural, run against a browser,
 * because a hung span silently falling outside a lookback window survived
 * every previous pass. These test the two rules that were wrong.
 *
 *   node test-model.mjs
 */
import { build, activeOnRoute, placeOnClock, countLE, countLT } from './test-build/model.js';

let failures = 0;

// ========================================================================
// INTERNALS
// ========================================================================
function check(name, cond, detail) {
  if (cond) { console.log(`  \x1b[32mPASS\x1b[0m ${name}`); }
  else { failures++; console.log(`  \x1b[31mFAIL\x1b[0m ${name}${detail ? ': ' + detail : ''}`); }
}

function span(id, start, end, extra = {}) {
  return {
    span_id: id, trace_id: 't', parent_span_id: null, links: [], name: id,
    kind: 'call', start, end, status: end === null ? 'running' : 'ok',
    lifecycle: [], events: [], origin: 'semantic', host_id: 'h',
    domain: 'd', attributes: {}, duration_ms: null, ...extra,
  };
}

// ========================================================================
// AN UNFINISHED SPAN IS ALWAYS FOUND, HOWEVER FAR BACK IT STARTED
// ========================================================================
{
  const route = { from: 'a', to: 'b', count: 0, errors: 0, crossHost: false,
                  variance: 0, spans: [], open: [] };
  const hung = span('HUNG', 10, null);
  route.spans.push(hung);
  route.open.push(hung);
  for (let i = 0; i < 200; i++) route.spans.push(span('s' + i, 20 + i, 21 + i));

  const found = activeOnRoute(route, 500, 64);
  check('a span still running is found beyond the lookback window',
        found.some((s) => s.span_id === 'HUNG'),
        'an unfinished span fell outside a 48-span window and vanished from the map');

  const early = activeOnRoute(route, 5, 64);
  check('a span that has not started yet is not reported',
        !early.some((s) => s.span_id === 'HUNG'));
}

// ========================================================================
// THE CLOCK PLACES POSITIONS, NEVER DURATIONS
// ========================================================================
{
  const placed = placeOnClock([span('a', 1000, 1001), span('b', 1001, 1002)]);
  check('the clock spans 0..1000', placed[0].start === 0 && placed[1].end === 1000,
        JSON.stringify(placed.map((s) => [s.start, s.end])));
  const degenerate = placeOnClock([span('a', 5, 5), span('b', 5, 5)]);
  check('a zero-width capture produces finite positions',
        degenerate.every((s) => Number.isFinite(s.start) && Number.isFinite(s.end)));
}

// ========================================================================
// THE SEARCH HELPERS AGREE WITH A LINEAR COUNT
// ========================================================================
{
  const a = Float64Array.from([1, 2, 2, 3, 5]);
  const slowLE = (v) => [...a].filter((x) => x <= v).length;
  const slowLT = (v) => [...a].filter((x) => x < v).length;
  let ok = true;
  for (const v of [0, 1, 2, 2.5, 3, 5, 9]) {
    if (countLE(a, v) !== slowLE(v) || countLT(a, v) !== slowLT(v)) ok = false;
  }
  check('binary counts agree with a linear count, including on repeats', ok);
}

// ========================================================================
// A ROUTE'S WEIGHT ORDERING IS NOT CARRIED ACROSS A RESTRICTION
// ========================================================================
{
  const feed = {
    spans: [span('p', 0, 10, { host_id: 'a', domain: 'api' }),
            span('c', 1, 2, { host_id: 'b', domain: 'worker', parent_span_id: 'p' })],
    clusters: [{ id: 'a/api', label: 'api', region: 'entry', slot: 0, host: 'a' },
               { id: 'b/worker', label: 'worker', region: 'logic', slot: 0, host: 'b' }],
  };
  const m = build(feed);
  check('a cross-cluster parentage becomes a route', m.routes.length === 1,
        `got ${m.routes.length}`);
  check('maxRouteCount is computed without spreading every route',
        m.maxRouteCount === 1);
  check('an open list exists on every route',
        m.routes.every((r) => Array.isArray(r.open)));
}

// ========================================================================
// THE OVERVIEW PLACES THINGS EXACTLY WHERE THE SPAN MAP DOES
// ========================================================================
{
  const { placeHost, placeCluster } = await import('./test-build/model.js');
  const a = placeHost(1, 3), b = placeHost(1, 3);
  check('host placement is deterministic', a.wx === b.wx && a.wy === b.wy && a.wr === b.wr);
  const solo = placeHost(0, 1);
  check('a single host takes the centre', solo.wx === 1200 && solo.wy === 800,
        JSON.stringify(solo));
  const c0 = placeCluster(solo, 'entry', 0, 1);
  const c1 = placeCluster(solo, 'entry', 0, 1);
  check('cluster placement is deterministic', c0.wx === c1.wx && c0.wy === c1.wy);
  check('an entry cluster sits above its host centre', c0.wy < solo.wy);
  check('a data cluster sits below it', placeCluster(solo, 'data', 0, 1).wy > solo.wy);
}

// ========================================================================
// THE CAMERA NEVER REACHES A ZOOM THAT MAKES THE WORLD INFINITE
// ========================================================================
{
  const { Camera } = await import('./test-build/camera.js');
  const cam = new Camera();
  cam.resize(0, 0);        // a hidden tab, or measured before layout
  cam.fit();
  check('a zero-sized viewport still yields a finite world transform',
        Number.isFinite(cam.toWorldX(10)) && cam.z > 0,
        `z=${cam.z} toWorldX=${cam.toWorldX(10)}`);
  cam.resize(1400, 900); cam.fit();
  cam.zoomAt(700, 450, 1e9);
  check('zoom is capped', cam.z === Camera.MAX_Z);
  cam.zoomAt(700, 450, 1e-9);
  check('zoom is floored', cam.z === Camera.MIN_Z);
}

// ========================================================================
// A SHARED ALPHA RAMP NEVER RETURNS NAN
// ========================================================================
{
  const { bandAlpha } = await import('./test-build/lod.js');
  const inputs = [[10,0,20,100,200],[10,10,10,100,200],[0,0,0,0,0],[999,0,20,100,200]];
  check('bandAlpha is finite for every input including equal bounds',
        inputs.every((i) => Number.isFinite(bandAlpha(...i))));
}

// ========================================================================
// A PERMALINK CARRIES THE WHOLE FINDING, AND A LINK IS INPUT
// ========================================================================
{
  const vp = await import('./test-build/viewpoint.js');
  const { Camera } = await import('./test-build/camera.js');
  const cam = new Camera();
  cam.resize(1400, 900); cam.fit();

  const withWindow = vp.capture(cam, 500, 'none', null, false, false, { from: 199, to: 550 });
  const q = vp.toQuery(withWindow);
  check('the selected range travels in the link', q.includes('w=199%2C550') || q.includes('w=199,550'), q);
  const back = vp.fromQuery('?' + q);
  check('and comes back', back.from === 199 && back.to === 550,
        JSON.stringify([back.from, back.to]));

  const none = vp.fromQuery('?' + vp.toQuery(vp.capture(cam, 500, 'none', null, false, false, null)));
  check('no window means no window, not a bogus one',
        none.from === undefined && none.to === undefined);

  // a link is input like any other
  const hostile = vp.fromQuery('?v=1200,800,0,500');
  check('a link cannot set a zoom that makes the world infinite',
        hostile.z >= Camera.MIN_Z && Number.isFinite(hostile.z), `z=${hostile.z}`);
  const huge = vp.fromQuery('?v=1200,800,99999,500');
  check('nor an absurd one', huge.z <= Camera.MAX_Z);
  const negT = vp.fromQuery('?v=1200,800,0.5,-99');
  check('nor a negative playhead', negT.t >= 0);
  check('a malformed window is dropped rather than fatal',
        vp.fromQuery('?v=1200,800,0.5,500&w=nonsense') !== null);
}

console.log(failures ? `\n  ${failures} failure(s)` : '\n  all model checks passed');
process.exit(failures ? 1 : 0);
