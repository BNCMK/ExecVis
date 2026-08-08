// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'i18n.ts',
  script_path: 'execviz-ui/src/i18n.ts',
  module_name: 'i18n',
  version: '0.13.0',
  description: 'Localisation.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: [],
  external_dependencies: [],
  features: ['i18n'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

/**
 * Localisation.
 *
 * The chrome is translated; the record is quoted. Span names, log lines, domains
 * and error messages come from the program being observed; they are evidence,
 * and translating evidence would make two people looking at one capture see
 * different text.
 */
type Catalogue = Record<string, string>;

const EN: Catalogue = {
  'view': 'view', 'layers': 'layers', 'logs': 'logs', 'time': 'time',
  'capture': 'capture', 'views': 'views', 'shortcuts': 'shortcuts',
  'reset view': 'reset view', 'zoom in': 'zoom in', 'zoom out': 'zoom out',
  'labels': 'labels', 'log console': 'log console', 'play / pause': 'play / pause',
  'waterfall': 'waterfall', 'whole capture': 'whole capture',
  'clear the window': 'clear the window', 'save this view': 'save this view',
  'add a note': 'add a note', 'every key': 'every key',
  'nothing matches at this point in the trace': 'nothing matches at this point in the trace',
  'lines': 'lines', 'rows': 'rows', 'window': 'window',
  'this capture holds no spans yet': 'this capture holds no spans yet',
  'nothing matches this filter': 'nothing matches this filter',
  'no lines had been written at this point in the trace': 'no lines had been written at this point in the trace',
};

const CATALOGUES: Record<string, Catalogue> = {
  en: EN,
  es: {
    'view': 'vista', 'layers': 'capas', 'logs': 'registros', 'time': 'tiempo',
    'capture': 'captura', 'views': 'vistas', 'shortcuts': 'atajos',
    'reset view': 'restablecer la vista', 'zoom in': 'acercar', 'zoom out': 'alejar',
    'labels': 'etiquetas', 'log console': 'consola de registros',
    'play / pause': 'reproducir / pausar', 'waterfall': 'cascada',
    'whole capture': 'captura completa', 'clear the window': 'borrar la ventana',
    'save this view': 'guardar esta vista', 'add a note': 'añadir una nota',
    'every key': 'todas las teclas',
    'nothing matches at this point in the trace': 'nada coincide en este punto de la traza',
    'lines': 'líneas', 'rows': 'filas', 'window': 'ventana',
    'this capture holds no spans yet': 'esta captura aún no contiene tramos',
    'nothing matches this filter': 'nada coincide con este filtro',
    'no lines had been written at this point in the trace': 'no se habían escrito líneas en este punto de la traza',
  },
  de: {
    'view': 'Ansicht', 'layers': 'Ebenen', 'logs': 'Protokolle', 'time': 'Zeit',
    'capture': 'Aufzeichnung', 'views': 'Ansichten', 'shortcuts': 'Tastenkürzel',
    'reset view': 'Ansicht zurücksetzen', 'zoom in': 'vergrößern', 'zoom out': 'verkleinern',
    'labels': 'Beschriftungen', 'log console': 'Protokollkonsole',
    'play / pause': 'Wiedergabe / Pause', 'waterfall': 'Wasserfall',
    'whole capture': 'gesamte Aufzeichnung', 'clear the window': 'Fenster löschen',
    'save this view': 'diese Ansicht speichern', 'add a note': 'Notiz hinzufügen',
    'every key': 'alle Tasten',
    'nothing matches at this point in the trace': 'an dieser Stelle der Spur passt nichts',
    'lines': 'Zeilen', 'rows': 'Zeilen', 'window': 'Fenster',
    'this capture holds no spans yet': 'diese Aufzeichnung enthält noch keine Spans',
    'nothing matches this filter': 'nichts entspricht diesem Filter',
    'no lines had been written at this point in the trace': 'an dieser Stelle der Spur wurden keine Zeilen geschrieben',
  },
};

let locale = 'en';

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

export function available(): string[] { return Object.keys(CATALOGUES); }

export function setLocale(l: string) {
  locale = CATALOGUES[l] ? l : 'en';
  try { localStorage.setItem('execviz.locale', locale); } catch { /* forgets, still works */ }
  document.documentElement.setAttribute('lang', locale);
}

export function getLocale(): string {
  try {
    const saved = localStorage.getItem('execviz.locale');
    if (saved && CATALOGUES[saved]) locale = saved;
    else {
      const nav = (navigator.language || 'en').slice(0, 2);
      if (CATALOGUES[nav]) locale = nav;
    }
  } catch { /* default stands */ }
  return locale;
}

/** A missing string falls back to English rather than to a blank or a key: a
 *  half-translated interface is usable, an interface of empty labels is not. */
export function t(key: string): string {
  return CATALOGUES[locale]?.[key] ?? EN[key] ?? key;
}

/** Numbers and dates follow the reader's convention, not the author's. */
export function num(v: number, digits = 1): string {
  try { return new Intl.NumberFormat(locale, { maximumFractionDigits: digits }).format(v); }
  catch { return v.toFixed(digits); }
}

export function ms(v: number): string {
  if (v >= 1000) return `${num(v / 1000, 2)} s`;
  return `${num(v, v < 10 ? 2 : 0)} ms`;
}

export function when(epochSeconds: number): string {
  try { return new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeStyle: 'medium' })
    .format(new Date(epochSeconds * 1000)); }
  catch { return String(epochSeconds); }
}

/**
 * Text that came from the observed program, passed through untouched.
 *
 * Exists to make the boundary explicit at the call site: anything wrapped here
 * is evidence, and evidence is quoted rather than translated.
 */
export function verbatim(text: string): string { return text; }
