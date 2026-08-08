<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-ui/README.md
  module_name: README
  version: 0.53.1
  description: execviz renderer
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README, render
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz renderer

TypeScript, bundled to a single self-contained page.

    npm run check     # tsc --noEmit, strict
    npm run build     # esbuild -> dist/execviz.js -> dist/index.html

`tools/inline.mjs` inlines the bundle with a replacer **function**, not a
replacement string. A replacement string treats `$&` as the matched substring,
and minified JavaScript is full of `$&` where a variable named `$` meets `&&`;
with a string the bundle silently rewrites itself and the page dies with a syntax
error nowhere near the cause. That cost a debugging cycle.

    execviz serve run.db --port 8900 --ui path/to/dist/index.html

## What changed, and why it had to

The renderer this replaces was one 1,400-line HTML file that the Python tooling
patched by string substitution. That arrangement failed twice in ways nothing
reported: a helper inserted on the wrong side of a splice boundary was silently
deleted from the served copy, and a failed patch left a stale file that measured
well for the wrong reason.

- **The page fetches its data.** Nothing about a capture is compiled into the
  artifact, so the same build serves any store and changing one cannot quietly
  break the other. The string splicing is gone.
- **Layout is separated from drawing.** `model.ts` derives everything once per
  ingest: cluster and host placement, family bundles, routes, and sorted
  start/end arrays so "how many spans are open at t" is two binary searches
  rather than a scan. The old renderer recomputed most of that per frame, per
  cluster, and again per host.
- **Budgets are explicit and in one file.** `lod.ts` holds the tier thresholds
  and the per-frame limits: how many routes, how many tokens on a route, how
  many rails in a family, how many cluster interiors resolve at once. They are
  stated rather than emergent, so tuning is a number in one place.

## Analytic layers

Three overlays over the same model, each answering a question the map does not,
and none of them re-capturing anything:

| layer | question |
|---|---|
| density | where are the hotspots? |
| tree-rings | how is depth distributed? |
| wedges | the fingerprint as geometry, for comparison |

Measured as ink added to the centre patch, against the same view with layers
off: tree-rings +373%, wedges +57%, density +8%, and switching back returns the
canvas to baseline.

The canopy stays primary and the layers draw over it. Two notes on what they
deliberately are not. Density uses one hue at varying intensity, because a heat
map with several hues invites a reader to see categories that are not there. And
the wedges are offered as a comparison mode rather than the primary fingerprint
reading precisely because a wedge's *area* implies a magnitude the invariants do
not have; because the profile, not this, is the settled form for section
4.2.

## Overview mode: drawing from the rollup

`overview only` switches the map to draw from the rollup instead of from spans.
A reader looking at a whole system is not looking at spans, and every tier above
the individual span is exactly what the rollup already carries.

Measured on the 50,006-span capture:

| | spans mode | overview mode |
|---|---|---|
| spans held by the client | 50,006 | **0** |
| bytes pulled from the span feed | ~17 MB | **0** |
| payload |; | 52,762 bytes of rollup |
| frame time | ~20 ms | ~16 ms |

Switching modes clears what is held rather than mixing the two, and closes the
span stream outright. That last part is not a detail: the first version left the
stream open, so the client happily downloaded all 17 MB while claiming to hold
nothing, and the saving was imaginary. A test that only counted `spans held`
would have passed.

Two things the mode refuses to do. A cluster standing for thirty thousand
operations draws what the summary supports and says how many it stands for; it
does not invent individuals. And openness from a summary reports the state *when
the summary was taken* rather than pretending to vary with the playhead, because
a summary has no per-span times and claiming otherwise would be fabrication.

## Replay and settings

`⤓ save replay` writes the capture **as delivered**: raw times and the window
travel with the spans, so the file opens on the same clock without the instance
that produced it. `⤒ load replay` replaces what is held rather than merging into
it, because two captures on one map would be a graph of something that never ran,
and it pauses the live feed, because what is on screen is then a recording rather
than a system.

Verified by saving from one capture and loading it over a different live one: 127
spans on screen became the replay's 19, and stayed 19 through three more seconds
of polling.

Settings expose the budgets that `lod.ts` holds rather than hiding them: labels,
canopy, rails per family, routes drawn. Changing the route budget invalidates the
canopy layer, since the budget changed what that layer would draw.

## The flipbook

Arrow keys or the ◀ ▶ buttons step through the rows of the largest resolved
family, one at a time. The selected row is held at the centre of its wedge and
brightened; the rest bunch toward the edges and dim. The family's direction never
changes, only the spacing inside it, so the map still reads the same at a glance
while an individual row is isolated.

It is driven by keys and buttons, never by a drag, because a drag would compete
with panning and two gestures on one surface is how a map becomes unusable.
Escape clears it. Verified stepping through a 53-row family by name.

This is also why the rail sample is safe: capping a family at 60 drawn rails is a
way of seeing the shape, and the flipbook is how any individual row is reached.

## The waterfall

`w` opens it. One row per span, nested by causality, ordered by start, position
and width on the same clock as the map. The chain that set the total is marked,
and self time is drawn as a lighter section inside each bar, so a wide bar that
is mostly its children reads differently from one that is mostly itself.

    [critical] checkout, ok, 56ms, on the critical path
               charge, ok, 10ms
    [critical] charge, error, 34ms, on the critical path

Building the map first and treating the waterfall as redundant was taste over
evidence. Every tracing tool has one because every reader reaches for one: the
map answers what shape a system has, the waterfall answers what happened and in
what order.

It is also the view a screen reader can read. Each row carries a label with the
same facts as the bar, holds keyboard focus, and responds to Enter.

## Access

Status was carried by colour alone, and measurement showed what that cost: under
deuteranopia the `ok` and `err` colours sat **0.027 apart**; for the most common
colour vision deficiency they were the same colour, in the channel that matters
most.

The ramp was rechosen by simulating deuteranopia, protanopia and tritanopia and
searching for one that also holds contrast:

| | before | after |
|---|---|---|
| worst separation under simulation | 0.027 | **0.349** |
| minimum contrast against background |; | **4.91:1** |

The palette is the smaller half of the fix. Colour no longer carries status
alone: a cluster holding an error is marked `▲`, running work is ringed with a
dashed stroke, and every waterfall row states its status as a word. The picture
survives greyscale, a screenshot and a printout.

## Language

The interface is available in English, Spanish and German, with numbers and
dates formatted by the reader's convention rather than the author's. A missing
string falls back to English rather than to a blank, because a half-translated
interface is usable and one of empty labels is not.

**The chrome is translated; the record is quoted.** Span names, log lines,
domains and error messages come from the observed program; they are evidence,
and translating evidence would make two people looking at one capture see
different text. Verified in German:

    41 Zeilen · info 40 · meta 1        ← chrome
    batch  retrying connection          ← evidence, untouched

## A window in time

Drag on the strip above the scrubber. One range restricts **every** view at once,
map, waterfall, console, canopy; because a person investigating an incident is
investigating a period, and making them re-filter each view separately is making
them do the join by hand.

    window 250-600 of 1000
    waterfall rows: 61 in the window, 127 without

A span that straddles the boundary is inside the window: work spanning the
incident is the work most likely to explain it. Clusters keep their positions,
a window filters time, not space, and moving the map would lose the reader.

## From a span to the code

Frames already carried file and line and nothing did anything with them. Choose
an editor, or write a template with `{file}` and `{line}`, and waterfall rows
gain a source link:

    vscode://file/_weakrefset.py:37

The editor is a matter of preference, so it is a template the person sets once
rather than a guess the tool makes and gets wrong. A link appears only when both
the template and the location exist; an inert link differs from none, because
it looks like it should work.

## Keeping a finding

`save this view` and `add a note` write **beside the capture**, not into the
browser, because a finding that lives in one person's tab is not a finding anyone
else has. A note records who wrote it: an unattributed annotation on shared
evidence invites an opinion to be read as part of the record.

    execviz note run.db          # what people found
    execviz view run.db --list   # where they were looking
    execviz report run.db        # the investigation as text

The report assembles the window, the record's own trustworthiness (sound, seal,
sampling, clock agreement), what took the time, the critical path, and the notes.
It ends by saying every figure was measured and that what it means is not stated
there; the writing up is a person's job, and a tool that supplies the conclusion
invites it to be believed.

## Smaller screens

Below 820px the layout stacks rather than shrinks: panels become full-width,
controls stay finger-sized. Verified at 430px; menubar, console, waterfall,
scrubber, side panel and fingerprint all still present, all nine menu groups
reachable. Nothing is removed, because a capability that disappears on a narrow
screen is one the reader will assume is missing everywhere.

## Menus, keys, and finding things

Every capability is in the menu bar: **view, layers, logs, time, capture**. Each
action shows its own key beside it, and `?` opens a sheet listing all 24 of them.
Toggles show their state where they are toggled, and radio groups show which
member is on.

This was a real gap rather than a polish pass. The map had accumulated a log
console, a flipbook, replay, overview mode, three analytic layers and a
fingerprint panel, each behind a small control placed wherever there was room.
Every one worked; none announced itself. A capability no reader can find is not
built, and that failure is harder to notice than a missing feature because the
code passes its tests.

Keys are defined in exactly one place, so the shortcut sheet cannot drift from
the behaviour. A key pressed inside a text field is text, not a shortcut.

## The log console

The ▤ logs button opens a console that shares the map's clock and its selection.
Lines appear as the playhead reaches them, so scrubbing the trace scrubs the log:
reading the two on separate clocks is the correlation problem the design exists
to remove. Clicking a span node scopes the console to that span and everything
causally beneath it, which is the `--under` query with a mouse instead of a span
id. Filters for warnings and errors sit in the header.

The console carries the same operations as the command line: free-text narrowing
across message, level, span, domain and host; ordering by time, level, span,
domain or host; folding repeated lines into one row marked `×n`; and a live tally
of each level so the shape of the noise is visible before any of it is read.
`/` focuses the narrow box, `f` folds, `g` opens the console.

    unfolded: 41 lines · info 40 · meta 1
    folded:    2 rows · 41 lines · info 40 · meta 1
               58 info batch retrying connection ×40

Verified against the same capture as the legacy renderer: 12 lines at the end of
the trace, 2 with the error filter, and 2 when scrubbed back to t=120.

The rows are rebuilt only when what they would say has changed, which is the same
rule as the canopy layer and the fingerprint panel.

## The fingerprint panel

    http://host:8900/                                  # the signature alone
    http://host:8900/?baseline=a.db,b.db               # read against earlier runs

A profile across fixed axes, one line per reading, with the stability band drawn
from the baseline captures. The form was settled by measurement rather than
taste: a radial glyph makes a memorable shape whose outline depends on the
arbitrary order of the axes and whose area means nothing, and a waveform needs an
axis with intrinsic order that these quantities do not have. A profile matches
the question a reader is asking, and it says *which* axis moved.

Verified on real repeated captures:

| | result |
|---|---|
| same program against its own baseline | matches, no axis outside the band |
| a different program against that baseline | departs on branching, concentration, jitter, io share and depth; largest move `depth` |

The band is narrow because repeated runs of one program agree to within 0.01 on
every axis, which makes a departure from it worth looking at. The panel
refreshes when the capture changes rather than every frame, because a signature
is a property of the whole capture and does not vary with the playhead.

## Measured on a 50,006-span capture

| | before | after |
|---|---|---|
| fit (whole system) | ~1 fps | ~9 fps |
| zoomed to clusters | ~1 fps | ~23 fps |
| zoomed deep | ~5 fps | ~21 fps |

The canopy is drawn to its own layer and composited, rebuilt only when the
camera or the model changes, since route geometry does not vary with the
playhead. That exposed a rule worth keeping: an eased camera converges
asymptotically and never arrives, so anything keyed on camera state must snap to
its target when close enough or it never gets to reuse its work.

## Analytic layers

Three overlays over the same model, each answering a question the map does not,
and none of them re-capturing anything:

| layer | question |
|---|---|
| density | where are the hotspots? |
| tree-rings | how is depth distributed? |
| wedges | the fingerprint as geometry, for comparison |

Measured as ink added to the centre patch, against the same view with layers
off: tree-rings +373%, wedges +57%, density +8%, and switching back returns the
canvas to baseline.

The canopy stays primary and the layers draw over it. Two notes on what they
deliberately are not. Density uses one hue at varying intensity, because a heat
map with several hues invites a reader to see categories that are not there. And
the wedges are offered as a comparison mode rather than the primary fingerprint
reading precisely because a wedge's *area* implies a magnitude the invariants do
not have; because the profile, not this, is the settled form for section
4.2.

## Overview mode: drawing from the rollup

`overview only` switches the map to draw from the rollup instead of from spans.
A reader looking at a whole system is not looking at spans, and every tier above
the individual span is exactly what the rollup already carries.

Measured on the 50,006-span capture:

| | spans mode | overview mode |
|---|---|---|
| spans held by the client | 50,006 | **0** |
| bytes pulled from the span feed | ~17 MB | **0** |
| payload |; | 52,762 bytes of rollup |
| frame time | ~20 ms | ~16 ms |

Switching modes clears what is held rather than mixing the two, and closes the
span stream outright. That last part is not a detail: the first version left the
stream open, so the client happily downloaded all 17 MB while claiming to hold
nothing, and the saving was imaginary. A test that only counted `spans held`
would have passed.

Two things the mode refuses to do. A cluster standing for thirty thousand
operations draws what the summary supports and says how many it stands for; it
does not invent individuals. And openness from a summary reports the state *when
the summary was taken* rather than pretending to vary with the playhead, because
a summary has no per-span times and claiming otherwise would be fabrication.

## Replay and settings

`⤓ save replay` writes the capture **as delivered**: raw times and the window
travel with the spans, so the file opens on the same clock without the instance
that produced it. `⤒ load replay` replaces what is held rather than merging into
it, because two captures on one map would be a graph of something that never ran,
and it pauses the live feed, because what is on screen is then a recording rather
than a system.

Verified by saving from one capture and loading it over a different live one: 127
spans on screen became the replay's 19, and stayed 19 through three more seconds
of polling.

Settings expose the budgets that `lod.ts` holds rather than hiding them: labels,
canopy, rails per family, routes drawn. Changing the route budget invalidates the
canopy layer, since the budget changed what that layer would draw.

## The flipbook

Arrow keys or the ◀ ▶ buttons step through the rows of the largest resolved
family, one at a time. The selected row is held at the centre of its wedge and
brightened; the rest bunch toward the edges and dim. The family's direction never
changes, only the spacing inside it, so the map still reads the same at a glance
while an individual row is isolated.

It is driven by keys and buttons, never by a drag, because a drag would compete
with panning and two gestures on one surface is how a map becomes unusable.
Escape clears it. Verified stepping through a 53-row family by name.

This is also why the rail sample is safe: capping a family at 60 drawn rails is a
way of seeing the shape, and the flipbook is how any individual row is reached.

## The waterfall

`w` opens it. One row per span, nested by causality, ordered by start, position
and width on the same clock as the map. The chain that set the total is marked,
and self time is drawn as a lighter section inside each bar, so a wide bar that
is mostly its children reads differently from one that is mostly itself.

    [critical] checkout, ok, 56ms, on the critical path
               charge, ok, 10ms
    [critical] charge, error, 34ms, on the critical path

Building the map first and treating the waterfall as redundant was taste over
evidence. Every tracing tool has one because every reader reaches for one: the
map answers what shape a system has, the waterfall answers what happened and in
what order.

It is also the view a screen reader can read. Each row carries a label with the
same facts as the bar, holds keyboard focus, and responds to Enter.

## Access

Status was carried by colour alone, and measurement showed what that cost: under
deuteranopia the `ok` and `err` colours sat **0.027 apart**; for the most common
colour vision deficiency they were the same colour, in the channel that matters
most.

The ramp was rechosen by simulating deuteranopia, protanopia and tritanopia and
searching for one that also holds contrast:

| | before | after |
|---|---|---|
| worst separation under simulation | 0.027 | **0.349** |
| minimum contrast against background |; | **4.91:1** |

The palette is the smaller half of the fix. Colour no longer carries status
alone: a cluster holding an error is marked `▲`, running work is ringed with a
dashed stroke, and every waterfall row states its status as a word. The picture
survives greyscale, a screenshot and a printout.

## Menus, keys, and finding things

Every capability is in the menu bar: **view, layers, logs, time, capture**. Each
action shows its own key beside it, and `?` opens a sheet listing all 24 of them.
Toggles show their state where they are toggled, and radio groups show which
member is on.

This was a real gap rather than a polish pass. The map had accumulated a log
console, a flipbook, replay, overview mode, three analytic layers and a
fingerprint panel, each behind a small control placed wherever there was room.
Every one worked; none announced itself. A capability no reader can find is not
built, and that failure is harder to notice than a missing feature because the
code passes its tests.

Keys are defined in exactly one place, so the shortcut sheet cannot drift from
the behaviour. A key pressed inside a text field is text, not a shortcut.

## The log console

The ▤ logs button opens a console that shares the map's clock and its selection.
Lines appear as the playhead reaches them, so scrubbing the trace scrubs the log:
reading the two on separate clocks is the correlation problem the design exists
to remove. Clicking a span node scopes the console to that span and everything
causally beneath it, which is the `--under` query with a mouse instead of a span
id. Filters for warnings and errors sit in the header.

The console carries the same operations as the command line: free-text narrowing
across message, level, span, domain and host; ordering by time, level, span,
domain or host; folding repeated lines into one row marked `×n`; and a live tally
of each level so the shape of the noise is visible before any of it is read.
`/` focuses the narrow box, `f` folds, `g` opens the console.

    unfolded: 41 lines · info 40 · meta 1
    folded:    2 rows · 41 lines · info 40 · meta 1
               58 info batch retrying connection ×40

Verified against the same capture as the legacy renderer: 12 lines at the end of
the trace, 2 with the error filter, and 2 when scrubbed back to t=120.

The rows are rebuilt only when what they would say has changed, which is the same
rule as the canopy layer and the fingerprint panel.

## The fingerprint panel

    http://host:8900/                                  # the signature alone
    http://host:8900/?baseline=a.db,b.db               # read against earlier runs

A profile across fixed axes, one line per reading, with the stability band drawn
from the baseline captures. The form was settled by measurement rather than
taste: a radial glyph makes a memorable shape whose outline depends on the
arbitrary order of the axes and whose area means nothing, and a waveform needs an
axis with intrinsic order that these quantities do not have. A profile matches
the question a reader is asking, and it says *which* axis moved.

Verified on real repeated captures:

| | result |
|---|---|
| same program against its own baseline | matches, no axis outside the band |
| a different program against that baseline | departs on branching, concentration, jitter, io share and depth; largest move `depth` |

The band is narrow because repeated runs of one program agree to within 0.01 on
every axis, which makes a departure from it worth looking at. The panel
refreshes when the capture changes rather than every frame, because a signature
is a property of the whole capture and does not vary with the playhead.

## Measured on a 50,006-span capture

| | before | after |
|---|---|---|
| ingest | tab died before drawing | 50,006 spans in 6.9s |
| fit |; | ~48 fps |
| zoomed to clusters | ~1 fps (old renderer) | ~41 fps |
| zoomed deep | ~5 fps (old renderer) | ~55 fps |

Three things got it there, and two of them are the same rule at different
levels. The feed carries a cursor, so a client that already holds most of a
capture receives what changed rather than the whole store; a delivery that
carries nothing does not trigger a rebuild; and the canopy, which depends on the
model and camera but not on the playhead, is drawn once to its own layer and
composited.

Times arrive raw with the window the store covers, and the client places them on
the shared clock. Normalising at the server would compute the scale from
whichever subset was being sent, so a delta would land on a different clock than
what the client already held.
