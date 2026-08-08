<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-browser/README.md
  module_name: README
  version: 0.53.1
  description: execviz capture adapter for the browser
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README, capture, adapter
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz capture adapter for the browser

    <script src="execviz.js"></script>
    const ev = new Execviz({ collector: 'https://collector', hostId: 'browser-1' });

    await ev.withSpan('page_load', 'call', async () => {
      await ev.withSpan('price_lookup', 'external', () => ev.fetch('/api/price'));
    });

Half of a user's latency happens in a page, and a request cannot be followed end
to end without this.

## Two things a page does that a server runtime does not

**The document can vanish mid-request**, and that is the case worth
recording. An ordinary post is cancelled when the page unloads, so the final
flush uses `sendBeacon`; which the browser completes after the document is gone
,  triggered from both `pagehide` and `visibilitychange`, because neither fires
reliably alone.

**The clock is the page's, not the collector's.** The offset is unknown, so it is
recorded and reported by `execviz skew` rather than corrected, exactly as for any
other host.

`ev.headers()` stamps the trace onto an outgoing request so the server side joins
the same graph, and `ev.fetch()` does it for you.

## Verified

Run in a real browser against a live collector:

    page_load      call      ok      103ms  info error
      render_cart    call      ok       31ms
      price_lookup   external  ok       50ms
      charge         io        error    21ms

Correct nesting across promise boundaries, the failure recorded, logs attached,
`execviz check` conformant with zero violations.

## Capturing the logs the page already writes

    const e = new Execviz({ collector: '...' });
    e.captureLogs();

    await e.withSpan('handle_click', 'call', async () => {
      console.log('loading user 42');      // captured, no execviz call
      console.error(new Error('downstream refused'));
    });

`console.*` is teed; the devtools console still shows everything it showed
before, because a recorder that swallowed a page's console output would be
sabotaging the place developers look. `window.onerror` and
`unhandledrejection` are attached too: what a page writes as it fails is the line
somebody came looking for.

**A line written outside any span is not given one.** Verified: three lines
inside `withSpan` are attributed, a fourth outside it is not. An unattributed
line is a fact; a guessed parent is not.

**The carrier here is an explicit stack**, because a browser has no
`AsyncLocalStorage`. That is exact for sequential work and approximate under
concurrent async work, where the most recently entered span wins; so a line
written by one overlapping request can be attributed to another. Code that needs
certainty passes the span explicitly. This is a property of the platform and is
stated rather than hidden.
