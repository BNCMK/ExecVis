// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: inline.mjs
//  script_path: execviz-ui/tools/inline.mjs
//  module_name: inline
//  version: 0.53.1
//  description: Produces a single self-contained page from the shell and the bundle. The page fetches its data, so nothing about a capture is baked into the artifact.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: node:fs
//  features: inline, capture
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

// Produces a single self-contained page from the shell and the bundle. The page
// fetches its data, so nothing about a capture is baked into the artifact.
import { readFileSync, writeFileSync } from 'node:fs';
const shell = readFileSync(new URL('../src/shell.html', import.meta.url), 'utf8');
const js = readFileSync(new URL('../dist/execviz.js', import.meta.url), 'utf8');
// A replacer FUNCTION, not a replacement string. A replacement string treats
// `$&`, `$1` and friends as patterns, and minified JavaScript is full of `$&`
// where a variable named `$` meets `&&`. With a string the bundle silently
// rewrites itself and the page dies with a syntax error nowhere near the cause.
writeFileSync(new URL('../dist/index.html', import.meta.url),
  shell.replace('<!--BUNDLE-->', () => `<script>${js}</script>`));
console.log('dist/index.html written');
