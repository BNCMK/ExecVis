// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: attach.mjs
//  script_path: execviz-attach/attach.mjs
//  module_name: attach
//  version: 0.53.1
//  description: Attaches the Node adapter with no change to the program.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: node:path, node:url
//  features: attach, adapter
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

/**
 * Attaches the Node adapter with no change to the program.
 *
 *   NODE_OPTIONS="--import file:///path/to/execviz-attach/attach.mjs" \
 *   EXECVIZ_COLLECTOR=http://host:8900 node app.js
 *
 * `--import` rather than `--require`: the adapter is an ES module, and a
 * `--require` shim can only reach it through a dynamic `import`, which resolves
 * *after* the main module has started running. The hooks were installed too
 * late to see anything the program logged. `--import` awaits this file before
 * the program begins, which is the reason for it of attaching.
 */
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

if (process.env.EXECVIZ_COLLECTOR) {
  try {
    const mod = await import(path.join(here, '..', 'execviz-node', 'execviz.mjs'));
    mod.install({
      collector: process.env.EXECVIZ_COLLECTOR,
      hostId: process.env.EXECVIZ_HOST || 'node',
      domain: process.env.EXECVIZ_DOMAIN || 'app',
    });

    // A process is a unit of execution: without a span for the run itself every
    // captured line is dropped for having no parent. The program really did
    // run, so this is a true parent rather than an invented one.
    const root = mod.spanStart(path.basename(process.argv[1] || 'node'), 'call', {
      attributes: { argv: process.argv.slice(0, 8).join(' ').slice(0, 400), pid: process.pid },
    });
    mod.enter(root);

    // `exit` cannot run asynchronous work and delivery is a fetch, so a flush
    // started there never leaves. `beforeExit` still allows it.
    let closed = false;
    const close = async () => {
      if (closed) return;
      closed = true;
      try { mod.spanEnd(root, 'ok'); await mod.flush(); } catch { /* leaving anyway */ }
    };
    process.on('beforeExit', close);
    process.on('SIGINT', async () => { await close(); process.exit(130); });
    process.on('SIGTERM', async () => { await close(); process.exit(143); });

    // A span for the process alone says a program ran and nothing about what it
    // did. Every request a server handles and every request it makes is a unit
    // of work with its own timing and status, so each becomes a span. Without
    // this the map holds one span per process and `witness` has no claim to
    // check against what the kernel saw.
    try {
      const http = await import('node:http');
      const https = await import('node:https');

      for (const m of [http, https]) {
        const proto = m.default ?? m;
        const scheme = proto === (https.default ?? https) ? 'https' : 'http';

        // inbound: one span per request the server handles
        const origEmit = proto.Server.prototype.emit;
        if (!origEmit.__execviz) {
          const patched = function (event, ...args) {
            if (event !== 'request') return origEmit.call(this, event, ...args);
            const [req, res] = args;
            const name = `${req.method} ${String(req.url || '/').split('?')[0]}`.slice(0, 120);
            // The connection's descriptor, so the read that arrived before this
            // handler and the write that leaves after it can be tied back to
            // this span. A time window cannot hold an event loop's I/O; the
            // descriptor is the same on both sides of it.
            let fd = -1;
            try { fd = req.socket?._handle?.fd ?? -1; } catch { fd = -1; }
            const span = mod.spanStart(name, 'io', {
              attributes: {
                method: req.method, path: String(req.url || '').slice(0, 200), scheme,
                ...(fd >= 0 ? { fd } : {}),
              },
            });
            let ended = false;
            const finish = () => {
              if (ended) return;
              ended = true;
              const code = res.statusCode ?? 0;
              mod.spanEnd(span, code >= 500 ? 'error' : 'ok', { status_code: code });
            };
            res.on('finish', finish);
            res.on('close', finish);
            return mod.within
              ? mod.within(span, () => origEmit.call(this, event, ...args))
              : (mod.enter(span), origEmit.call(this, event, ...args));
          };
          patched.__execviz = true;
          proto.Server.prototype.emit = patched;
        }

        // outbound: one span per request this process makes
        const origReq = proto.request;
        if (origReq && !origReq.__execviz) {
          const patchedReq = function (...args) {
            const r = origReq.apply(this, args);
            let target = '';
            try {
              const o = typeof args[0] === 'string' ? new URL(args[0]) : args[0];
              target = o?.host ? `${o.host}${o.path ?? ''}` : String(o?.hostname ?? '');
            } catch { target = ''; }
            const span = mod.spanStart(`${scheme} out ${target}`.slice(0, 120), 'external', {
              attributes: { target: target.slice(0, 200) },
            });
            let done = false;
            const end = (st) => { if (done) return; done = true; mod.spanEnd(span, st); };
            r.on('response', (resp) => end((resp.statusCode ?? 0) >= 500 ? 'error' : 'ok'));
            r.on('error', () => end('error'));
            r.on('close', () => end('ok'));
            return r;
          };
          patchedReq.__execviz = true;
          proto.request = patchedReq;
        }
      }
    } catch (e) {
      process.stderr.write(`execviz: http spans unavailable (${e.message}); the process span still records\n`);
    }

    if (process.env.EXECVIZ_VERBOSE === '1') {
      process.stderr.write('execviz: attached to this process, no source change\n');
    }
  } catch (e) {
    // a program must never fail to start because a recorder could not attach
    process.stderr.write(`execviz: could not attach (${e.message}); the program runs unchanged\n`);
  }
}
