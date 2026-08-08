// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'menu.ts',
  script_path: 'execviz-ui/src/menu.ts',
  module_name: 'menu',
  version: '0.13.0',
  description: 'The menu bar, keybinds, and the shortcut sheet.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: ['i18n'],
  external_dependencies: [],
  features: ['menu'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

import { t } from './i18n.js';

// ========================================================================
// TYPES
// ========================================================================

/**
 * The menu bar, keybinds, and the shortcut sheet.
 *
 * One bar, named groups, every capability listed, every action showing its own
 * key. Deliberately conventional: this is not a place to invent, and a reader
 * who has used any desktop application already knows how it works.
 */
export interface Item {
  label: string;
  key?: string;                       // shown in the menu and bound globally
  run: () => void;
  /** returns whether the thing is currently on, so state is visible */
  state?: () => boolean;
  /** a radio group: exactly one member is on */
  group?: string;
}

export interface Group { name: string; items: Item[] }

// ========================================================================
// CLASSES
// ========================================================================

export class Menu {
  private groups: Group[] = [];
  private bar: HTMLElement;
  private sheet: HTMLElement;
  private open: string | null = null;

  constructor(bar: HTMLElement, sheet: HTMLElement) {
    this.bar = bar; this.sheet = sheet;
    document.addEventListener('click', (e) => {
      if (!this.bar.contains(e.target as Node)) this.close();
    });
    window.addEventListener('keydown', (e) => this.onKey(e));
  }

  add(name: string, items: Item[]) { this.groups.push({ name, items }); }

  private close() {
    this.open = null;
    this.bar.querySelectorAll('.dropdown').forEach((d) => d.classList.remove('on'));
    this.bar.querySelectorAll('.menu-title').forEach((d) => d.classList.remove('on'));
  }

  /** A key typed into a text field is text, not a shortcut. */
  private typing(e: KeyboardEvent): boolean {
    const el = e.target as HTMLElement | null;
    if (!el) return false;
    const tag = el.tagName;
    return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable;
  }

  private onKey(e: KeyboardEvent) {
    if (this.typing(e) || e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === 'Escape') { this.close(); this.hideSheet(); return; }
    if (e.key === '?') { this.toggleSheet(); e.preventDefault(); return; }
    for (const g of this.groups) {
      for (const it of g.items) {
        if (it.key && it.key === e.key) { it.run(); this.render(); e.preventDefault(); return; }
      }
    }
  }

  render() {
    this.bar.innerHTML = '';
    for (const g of this.groups) {
      const wrap = document.createElement('div');
      wrap.className = 'menu-group';
      const title = document.createElement('button');
      title.className = 'menu-title' + (this.open === g.name ? ' on' : '');
      title.textContent = t(g.name);
      title.onclick = (e) => {
        e.stopPropagation();
        this.open = this.open === g.name ? null : g.name;
        this.render();
      };
      const drop = document.createElement('div');
      drop.className = 'dropdown' + (this.open === g.name ? ' on' : '');
      for (const it of g.items) {
        const row = document.createElement('button');
        row.className = 'menu-item';
        const on = it.state ? it.state() : false;
        // state is shown where it is toggled, so a mode is never invisible
        row.innerHTML =
          `<span class="tick">${it.state ? (on ? (it.group ? '●' : '✓') : (it.group ? '○' : ' ')) : ''}</span>` +
          `<span class="lbl">${t(it.label)}</span>` +
          `<span class="key">${it.key ? escapeKey(it.key) : ''}</span>`;
        row.onclick = (e) => { e.stopPropagation(); it.run(); this.render(); };
        drop.appendChild(row);
      }
      wrap.appendChild(title); wrap.appendChild(drop);
      this.bar.appendChild(wrap);
    }
    const help = document.createElement('button');
    help.className = 'menu-title help';
    help.textContent = 'shortcuts  ?';
    help.onclick = (e) => { e.stopPropagation(); this.toggleSheet(); };
    this.bar.appendChild(help);
  }

  private hideSheet() { this.sheet.classList.remove('on'); }

  toggleSheet() {
    if (this.sheet.classList.contains('on')) { this.hideSheet(); return; }
    // one key opens the list of every key: if a reader has to leave the tool to
    // learn what it does, the tool is incomplete
    let html = `<h3>${t('every key')}</h3><div class="sheet-grid">`;
    for (const g of this.groups) {
      const bound = g.items.filter((i) => i.key);
      if (!bound.length) continue;
      html += `<div class="sheet-col"><h4>${t(g.name)}</h4>`;
      for (const it of bound) {
        html += `<div class="sheet-row"><kbd>${escapeKey(it.key!)}</kbd><span>${t(it.label)}</span></div>`;
      }
      html += '</div>';
    }
    html += `<div class="sheet-col"><h4>map</h4>
      <div class="sheet-row"><kbd>scroll</kbd><span>zoom, anchored on the cursor</span></div>
      <div class="sheet-row"><kbd>drag</kbd><span>pan</span></div>
      <div class="sheet-row"><kbd>dbl-click</kbd><span>dive in</span></div>
      <div class="sheet-row"><kbd>right dbl-click</kbd><span>back out</span></div>
      <div class="sheet-row"><kbd>click a span</kbd><span>scope the log console to it</span></div>
      <div class="sheet-row"><kbd>←  →</kbd><span>step the flipbook</span></div>
      <div class="sheet-row"><kbd>?</kbd><span>this sheet</span></div>
      <div class="sheet-row"><kbd>Esc</kbd><span>close whatever is open</span></div>
      </div>`;
    html += '</div>';
    this.sheet.innerHTML = html;
    this.sheet.classList.add('on');
  }
}

// ========================================================================
// INTERNALS
// ========================================================================

function escapeKey(k: string): string {
  return k === ' ' ? 'Space' : k;
}
