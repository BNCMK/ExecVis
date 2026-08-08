// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'waterfall.ts',
  script_path: 'execviz-ui/src/waterfall.ts',
  module_name: 'waterfall',
  version: '0.13.0',
  description: 'The waterfall.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['model', 'source', 'types'],
  external_dependencies: [],
  features: ['waterfall'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { Model, CLOCK } from './model.js';
import { Span } from './types.js';
import * as source from './source.js';

// ========================================================================
// TYPES
// ========================================================================

/**
 * The waterfall.
 *
 * The map answers what shape a system has. This answers what happened, in what
 * order, and what was waiting on what; the question people arrive
 * with. It is drawn as rows rather than on the canvas, which also makes it the
 * one view a screen reader can read.
 */
export interface Row { span: Span; depth: number; selfFrac: number; onPath: boolean }

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/** Depth-first by causality, ordered by start, which is the reading order. */
export function rows(model: Model, criticalPath: Set<string>): Row[] {
  const children = new Map<string, Span[]>();
  const roots: Span[] = [];
  for (const s of model.spans) {
    if (s.parent_span_id && model.byId.has(s.parent_span_id)) {
      const list = children.get(s.parent_span_id) ?? [];
      list.push(s); children.set(s.parent_span_id, list);
    } else roots.push(s);
  }
  for (const list of children.values()) list.sort((a, b) => a.start - b.start);
  roots.sort((a, b) => a.start - b.start);

  const out: Row[] = [];
  const walk = (s: Span, depth: number) => {
    const kids = children.get(s.span_id) ?? [];
    out.push({ span: s, depth, selfFrac: selfFraction(s, kids), onPath: criticalPath.has(s.span_id) });
    if (out.length > 4000) return;                 // a readable ceiling, stated
    for (const k of kids) walk(k, depth + 1);
  };
  for (const r of roots) walk(r, 0);
  return out;
}

// ========================================================================
// INTERNALS
// ========================================================================

/**
 * How much of a span's own width was itself rather than its children.
 *
 * Children are merged before subtracting: concurrent children must not be
 * double-counted, which would drive a real duration below zero.
 */
function selfFraction(s: Span, kids: Span[]): number {
  const end = s.end ?? s.start;
  const total = end - s.start;
  if (total <= 0 || !kids.length) return 1;
  const iv = kids.map((k) => [k.start, k.end ?? k.start] as [number, number])
    .sort((a, b) => a[0] - b[0]);
  let covered = 0, cur = iv[0];
  for (const w of iv.slice(1)) {
    if (w[0] <= cur[1]) { if (w[1] > cur[1]) cur = [cur[0], w[1]]; }
    else { covered += cur[1] - cur[0]; cur = w; }
  }
  covered += cur[1] - cur[0];
  return Math.max(0, Math.min(1, (total - covered) / total));
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/** The chain that set the total: at each step, the child finishing last. */
export function criticalPath(model: Model): Set<string> {
  const out = new Set<string>();
  const children = new Map<string, Span[]>();
  for (const s of model.spans) {
    if (s.parent_span_id) {
      const l = children.get(s.parent_span_id) ?? [];
      l.push(s); children.set(s.parent_span_id, l);
    }
  }
  const roots = model.spans.filter((s) => !s.parent_span_id || !model.byId.has(s.parent_span_id));
  let cur = roots.sort((a, b) =>
    ((b.end ?? b.start) - b.start) - ((a.end ?? a.start) - a.start))[0];
  let guard = 0;
  while (cur && guard++ < 512) {
    out.add(cur.span_id);
    const kids = children.get(cur.span_id) ?? [];
    if (!kids.length) break;
    cur = kids.reduce((a, b) => ((a.end ?? a.start) > (b.end ?? b.start) ? a : b));
  }
  return out;
}

/**
 * Status as text and mark, never colour alone.
 *
 * The picture has to survive greyscale, a screenshot, and a reader who does not
 * see red and green as different, so every status carries a shape and a word in
 * addition to its hue.
 */
export function statusMark(s: Span): { mark: string; word: string; cls: string } {
  if (s.status === 'error') return { mark: '▲', word: 'error', cls: 'err' };
  if (s.end === null) return { mark: '◔', word: 'running', cls: 'run' };
  return { mark: '●', word: 'ok', cls: 'ok' };
}

export function render(el: HTMLElement, model: Model, t: number, onPick: (s: Span) => void) {
  const path = criticalPath(model);
  const list = rows(model, path).filter((r) => r.span.start <= t);
  if (!list.length) {
    // "nothing yet" and "nothing at all" are different facts, and telling a
    // reader to wait for data that does not exist is the same error as showing
    // stale data without saying it is stale.
    const empty = model.spans.length === 0;
    el.innerHTML = `<div class="wf-empty">${empty
      ? 'this capture holds no spans yet; start a program with an adapter, or open a replay'
      : 'nothing had started at this point in the trace; scrub forward to see the work'}</div>`;
    return;
  }
  const html = list.map((r) => {
    const s = r.span;
    const end = s.end ?? t;
    const x = (s.start / CLOCK) * 100;
    const w = Math.max(0.4, ((end - s.start) / CLOCK) * 100);
    const st = statusMark(s);
    const dur = s.duration_ms;
    const durTxt = dur === null ? 'running' : dur >= 1000 ? `${(dur / 1000).toFixed(2)}s` : `${Math.round(dur)}ms`;
    // Working or waiting: a duration alone cannot tell a span that
    // burned a core from one that slept. Absent where the runtime did not
    // measure it, never shown as zero.
    const cost = (s.attributes as any)?.cost;
    const spent: string = cost?.spent ?? '';
    const ratio: number | null = typeof cost?.cpu_ratio === 'number' ? cost.cpu_ratio : null;
    const spentTxt = spent ? `, ${spent}` : '';
    // the label carries the same facts as the bar, so the row is readable
    // without seeing the bar at all
    const label = `${s.name}, ${st.word}, ${durTxt}${spentTxt}${r.onPath ? ', on the critical path' : ''}`;
    return `<div class="wf-row${r.onPath ? ' path' : ''}" data-sid="${esc(s.span_id)}" tabindex="0" role="listitem" aria-label="${esc(label)}">
      <span class="wf-name" style="padding-left:${r.depth * 11}px">
        <span class="wf-mark ${st.cls}">${st.mark}</span>${esc(s.name)}</span>
      <span class="wf-track">
        <span class="wf-bar ${st.cls}" style="left:${x.toFixed(2)}%;width:${w.toFixed(2)}%">
          <span class="wf-self" style="width:${(r.selfFrac * 100).toFixed(1)}%"></span>
        </span>
      </span>
      <span class="wf-spent ${esc(spent)}">${ratio === null ? '' : (spent === 'working' ? '▮' : '▯') + ' ' + Math.round(ratio * 100) + '%'}</span>
      <span class="wf-dur">${durTxt}</span>${srcLink(s)}
    </div>`;
  }).join('');
  el.innerHTML = html;
  el.querySelectorAll<HTMLElement>('.wf-row').forEach((row) => {
    const pick = () => {
      const s = model.byId.get(row.getAttribute('data-sid') ?? '');
      if (s) onPick(s);
    };
    row.addEventListener('click', pick);
    // everything reachable by keyboard, not only by pointer
    row.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ') { pick(); e.preventDefault(); }
    });
  });
}

// ========================================================================
// INTERNALS
// ========================================================================

/** A link only when both the template and the location exist: an inert link is
 *  worse than none, because it looks like it should work. */
function srcLink(s: Span): string {
  const u = source.urlFor(s);
  if (!u) return '';
  const loc = source.locationOf(s);
  return `<a class="wf-src" href="${esc(u)}" title="${esc(loc!.file)}:${loc!.line}">source</a>`;
}

/**
 * Escapes text before it becomes markup.
 *
 * Everything a capture carries; names, ids, log lines; comes from the observed
 * program, and a capture crosses a trust boundary every time it is exchanged
 * with a peer. An unescaped span id put a live `onmouseover` handler into the
 * page of whoever opened the capture.
 */
function esc(x: string): string {
  return String(x).replace(/[&<>"]/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c] as string));
}
