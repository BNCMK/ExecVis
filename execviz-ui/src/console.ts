// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'console.ts',
  script_path: 'execviz-ui/src/console.ts',
  module_name: 'console',
  version: '0.13.0',
  description: 'The log console.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['i18n', 'model', 'types'],
  external_dependencies: [],
  features: ['console'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { Model } from './model.js';
import { Span } from './types.js';
import { t as tr, num as fmt, verbatim } from './i18n.js';

// ========================================================================
// TYPES
// ========================================================================

/**
 * The log console.
 *
 * Logs are attributes of spans, so this is a query over the trace rather than a
 * grep over a file, and it shares the map's clock and selection. Lines appear as
 * the playhead reaches them, which means scrubbing the trace scrubs the log:
 * reading the two on separate clocks is the correlation problem the whole design
 * exists to remove.
 */
export type Filter = 'all' | 'warn' | 'error';

export interface Line { t: number; level: string; msg: string; span: Span }

const SEVERITY: Record<string, number> = {
  critical: 5, fatal: 5, error: 4, stderr: 4, warning: 3, warn: 3,
  info: 2, stdout: 2, debug: 1,
};
const sev = (l: string) => SEVERITY[l] ?? 0;

export type Sort = 'time' | 'level' | 'span' | 'domain' | 'host';

// ========================================================================
// CLASSES
// ========================================================================

export class Console {
  filter: Filter = 'all';
  sort: Sort = 'time';
  fold = false;
  text = '';                             // free-text narrowing
  /** when set, only this span and everything causally beneath it */
  scope: string | null = null;
  private lastKey = '';

  private descendants(model: Model, root: string): Set<string> {
    const out = new Set<string>([root]);
    let grew = true;
    while (grew) {
      grew = false;
      for (const s of model.spans) {
        if (s.parent_span_id && out.has(s.parent_span_id) && !out.has(s.span_id)) {
          out.add(s.span_id); grew = true;
        }
      }
    }
    return out;
  }

  collect(model: Model, t: number, cap = 300): Line[] {
    const scope = this.scope ? this.descendants(model, this.scope) : null;
    // Scoping to a span shows that span's lines, not the ones that happened to
    // precede the instant it was selected. Selecting sets the playhead to the
    // span's start and its lines occur during it, so a playhead gate alone
    // reports every freshly selected span as having nothing to say. Within a
    // scope the gate runs to the end of the scoped span instead.
    const scoped = this.scope ? model.byId.get(this.scope) : undefined;
    const gate = scoped ? Math.max(t, scoped.end ?? Infinity) : t;
    const out: Line[] = [];
    for (const s of model.spans) {
      if (!s.events.length) continue;
      if (scope && !scope.has(s.span_id)) continue;
      for (const e of s.events) {
        if (e.t > gate) continue;                    // playhead, or the scoped span
        if (this.filter === 'warn' && sev(e.level) < 3) continue;
        if (this.filter === 'error' && sev(e.level) < 4) continue;
        if (this.text && !matches(e.msg, e.level, s, this.text)) continue;
        out.push({ t: e.t, level: e.level, msg: e.msg, span: s });
      }
    }
    // Array.prototype.sort is stable, so lines equal on the key keep the order
    // they were recorded in and the same query always reads the same way
    out.sort((a, b) => a.t - b.t);
    switch (this.sort) {
      case 'level': out.sort((a, b) => sev(b.level) - sev(a.level)); break;
      case 'span': out.sort((a, b) => a.span.name.localeCompare(b.span.name)); break;
      case 'domain': out.sort((a, b) =>
        (a.span.domain ?? '').localeCompare(b.span.domain ?? '')); break;
      case 'host': out.sort((a, b) => a.span.host_id.localeCompare(b.span.host_id)); break;
    }
    return out.slice(-cap);
  }

  /** How many of each level, so the shape of the noise is visible first. */
  tally(model: Model, t: number): Record<string, number> {
    const out: Record<string, number> = {};
    for (const l of this.collect(model, t, 100000)) out[l.level] = (out[l.level] ?? 0) + 1;
    return out;
  }

  /** Rebuilds the rows only when what they would say has changed. */
  render(rowsEl: HTMLElement, count: HTMLElement, title: HTMLElement, model: Model, t: number) {
    const lines = this.collect(model, t);
    // folding is a reading aid, so a group always states how many it stands for
    const rows: Array<{ l: Line; n: number }> = [];
    if (this.fold) {
      for (const l of lines) {
        const prev = rows[rows.length - 1];
        if (prev && prev.l.msg === l.msg && prev.l.level === l.level
            && prev.l.span.span_id === l.span.span_id) prev.n++;
        else rows.push({ l, n: 1 });
      }
    } else for (const l of lines) rows.push({ l, n: 1 });

    const key = [lines.length, this.filter, this.sort, this.fold, this.text,
                 this.scope ?? '', lines.length ? lines[lines.length - 1].t : 0].join('|');
    const tally = this.tally(model, t);
    const shape = Object.entries(tally).sort((a, b) => b[1] - a[1])
      .map(([k, v]) => `${k} ${v}`).join(' · ');
    // no dangling separator when there is no tally to put after it
    const tail = shape ? ` · ${shape}` : '';
    count.textContent = this.fold
      ? `${fmt(rows.length, 0)} ${tr('rows')} · ${fmt(lines.length, 0)} ${tr('lines')}${tail}`
      : `${fmt(lines.length, 0)} ${tr('lines')}${tail}`;
    title.textContent = this.scope
      ? `logs under ${model.byId.get(this.scope)?.name ?? this.scope}`
      : 'logs';
    if (key === this.lastKey) return;
    this.lastKey = key;
    rowsEl.innerHTML = rows.length
      ? rows.map(({ l, n }) =>
          `<div class="lrow" data-sid="${esc(l.span.span_id)}">` +
          `<span class="lt">${Math.round(l.t)}</span>` +
          `<span class="lv ${esc(l.level)}">${esc(l.level)}</span>` +
          `<span class="lsp">${esc(verbatim(l.span.name))}</span>` +
          `<span class="lm">${esc(l.msg)}${n > 1 ? ` <b class="rep">×${n}</b>` : ''}</span></div>`).join('')
      : `<div class="lrow"><span class="lm">${
          model.spans.length === 0
            ? tr('this capture holds no spans yet')
            : this.text || this.scope || this.filter !== 'all'
              ? tr('nothing matches this filter')
              : tr('no lines had been written at this point in the trace')
        }</span></div>`;
    rowsEl.scrollTop = rowsEl.scrollHeight;
  }

  invalidate() { this.lastKey = ''; }
}

// ========================================================================
// INTERNALS
// ========================================================================

/** Free text matches the message, the level, the span, the domain or the host. */
function matches(msg: string, level: string, s: Span, q: string): boolean {
  const n = q.toLowerCase();
  return msg.toLowerCase().includes(n)
    || level.toLowerCase().includes(n)
    || s.name.toLowerCase().includes(n)
    || (s.domain ?? '').toLowerCase().includes(n)
    || s.host_id.toLowerCase().includes(n);
}

function esc(x: string): string {
  return String(x).replace(/[&<>"]/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c] as string));
}
