// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'source.ts',
  script_path: 'execviz-ui/src/source.ts',
  module_name: 'source',
  version: '0.13.0',
  description: 'A link from a span to the code it came from.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['types'],
  external_dependencies: [],
  features: ['source'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { Span } from './types.js';

// ========================================================================
// CONSTANTS
// ========================================================================

/**
 * A link from a span to the code it came from.
 *
 * Frames already carry file and line and nothing did anything with them. The
 * editor is a matter of preference, so this is a template the person sets once
 * rather than a guess the tool makes and gets wrong.
 */
const KEY = 'execviz.source-template';

export const PRESETS: Record<string, string> = {
  'VS Code': 'vscode://file/{file}:{line}',
  'JetBrains': 'idea://open?file={file}&line={line}',
  'Sublime': 'subl://open?url=file://{file}&line={line}',
  'GitHub': 'https://github.com/ORG/REPO/blob/main/{file}#L{line}',
};

let template = '';

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function setTemplate(t: string) {
  template = t.trim();
  try { if (template) localStorage.setItem(KEY, template); else localStorage.removeItem(KEY); }
  catch { /* a browser that refuses storage still works, it just forgets */ }
}

export function getTemplate(): string {
  if (!template) {
    try { template = localStorage.getItem(KEY) ?? ''; } catch { template = ''; }
  }
  return template;
}

export function locationOf(s: Span): { file: string; line: number } | null {
  const a = s.attributes as Record<string, unknown> | undefined;
  if (!a) return null;
  const file = a['file'];
  const line = a['line'];
  if (typeof file !== 'string' || !file) return null;
  return { file, line: typeof line === 'number' ? line : 0 };
}

/** The URL, or null when either the template or the location is missing,
 *  an inert link differs from none, because it looks like it should work. */
export function urlFor(s: Span): string | null {
  const t = getTemplate();
  const loc = locationOf(s);
  if (!t || !loc) return null;
  return t.replace('{file}', loc.file).replace('{line}', String(loc.line || 1));
}
