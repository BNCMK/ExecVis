// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: store.rs
//  script_path: execviz-rs/src/store.rs
//  module_name: store
//  version: 0.53.1
//  description: The span store. Two-phase writes: a span is inserted with status=running at start and updated with end plus final status at completion. A span that never receives its second phase stays
//  kind: module
//  spec: internal
//  internal_dependencies: json
//  external_dependencies: rusqlite
//  features: store
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

//! The span store. Two-phase writes: a span is inserted with
//! status=running at start and updated with end plus final status at
//! completion. A span that never receives its second phase stays
//! running with end NULL, which is the stale-running death signal held
//! as a stored fact rather than inferred from absence.
use crate::json::J;
use rusqlite::{params, Connection};

// ========================================================================
// CONSTANTS
// ========================================================================

pub const COLS: &str = "span_id,trace_id,parent_span_id,links,name,kind,start,end,\
status,lifecycle,origin,host_id,clock_source,domain,attributes,events,\
inputs,output,error,run";

/// The column set before spec 3.2 added state, failure and run identity.
pub const COLS_LEGACY: &str = "span_id,trace_id,parent_span_id,links,name,kind,start,end,\
status,lifecycle,origin,host_id,clock_source,domain,attributes,events";

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS spans (
  span_id        TEXT PRIMARY KEY,
  trace_id       TEXT NOT NULL,
  parent_span_id TEXT,
  links          TEXT NOT NULL DEFAULT '[]',
  name           TEXT NOT NULL,
  kind           TEXT NOT NULL,
  start          REAL NOT NULL,
  end            REAL,
  status         TEXT NOT NULL,
  lifecycle      TEXT NOT NULL DEFAULT '[]',
  origin         TEXT NOT NULL DEFAULT 'semantic',
  host_id        TEXT NOT NULL DEFAULT 'local',
  clock_source   TEXT,
  domain         TEXT,
  attributes     TEXT NOT NULL DEFAULT '{}',
  events         TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS idx_trace ON spans(trace_id);
CREATE INDEX IF NOT EXISTS idx_host ON spans(host_id);
CREATE INDEX IF NOT EXISTS idx_domain ON spans(domain);
";

// ========================================================================
// TYPES
// ========================================================================

#[derive(Clone, Debug)]
pub struct Span {
    pub span_id: String,
    pub trace_id: String,
    pub parent_span_id: Option<String>,
    pub links: Vec<String>,
    pub name: String,
    pub kind: String,
    pub start: f64,
    pub end: Option<f64>,
    pub status: String,
    pub lifecycle: J,
    pub origin: String,
    pub host_id: String,
    pub clock_source: Option<String>,
    pub domain: Option<String>,
    pub attributes: J,
    pub events: J,
    /// Values as rendered at the moment of the call. Rendered, not
    /// referenced: an object that mutates later must not rewrite history.
    pub inputs: J,
    pub output: J,
    /// Type, message, frames and the cause chain beneath. The chain
    /// matters most; the top exception is usually the least informative.
    pub error: J,
    /// What produced this capture: commit, build, environment.
    /// Without it, comparing two runs compares two unknowns.
    pub run: J,
}

// ========================================================================
// IMPLEMENTATIONS
// ========================================================================

impl Span {
    /// The family a primitive belongs to.
    ///
    /// Derived, never recorded. An adapter states the primitive it observed and
    /// nothing more; the family is a function of that. If adapters could send a
    /// family, two of them could classify the same primitive differently and the
    /// map would show a difference the program never had.
    ///
    /// The mapping is total: an unrecognised kind still gets a family rather
    /// than a gap, and the conformance checker reports the unknown kind
    /// separately, so the reader learns of it from the check instead of from a
    /// hole in the picture.
    pub fn family(&self) -> &'static str { family_of(&self.kind) }
    pub fn duration_ms(&self) -> Option<f64> {
        self.end.map(|e| ((e - self.start) * 1000.0 * 1000.0).round() / 1000.0)
    }
    pub fn to_json(&self) -> J {
        let mut o = J::obj();
        o.set("span_id", J::s(&self.span_id));
        o.set("trace_id", J::s(&self.trace_id));
        o.set("parent_span_id", match &self.parent_span_id {
            Some(p) => J::s(p), None => J::Null });
        o.set("links", J::Arr(self.links.iter().map(|l| J::s(l)).collect()));
        o.set("name", J::s(&self.name));
        o.set("kind", J::s(&self.kind));
        o.set("family", J::s(self.family()));          // derived, not recorded
        o.set("start", J::n(self.start));
        o.set("end", match self.end { Some(e) => J::n(e), None => J::Null });
        o.set("status", J::s(&self.status));
        o.set("lifecycle", self.lifecycle.clone());
        o.set("origin", J::s(&self.origin));
        o.set("host_id", J::s(&self.host_id));
        o.set("clock_source", match &self.clock_source {
            Some(c) => J::s(c), None => J::Null });
        o.set("domain", match &self.domain { Some(d) => J::s(d), None => J::Null });
        o.set("attributes", self.attributes.clone());
        o.set("events", self.events.clone());
        if !matches!(self.inputs, J::Null) { o.set("inputs", self.inputs.clone()); }
        if !matches!(self.output, J::Null) { o.set("output", self.output.clone()); }
        if !matches!(self.error, J::Null) { o.set("error", self.error.clone()); }
        if !matches!(self.run, J::Null) { o.set("run", self.run.clone()); }
        o.set("duration_ms", match self.duration_ms() { Some(d) => J::n(d), None => J::Null });
        o
    }
    /// Note what is *not* read here: `family`. It is derived, so accepting one
    /// from the wire would let a sender contradict its own `kind`.
    /// The most a single text field may carry.
    ///
    /// Ingest already bounds a batch by span count (spec 5.6, gap 33), which
    /// bounds nothing about size: one span with a hundred-kilobyte name passes
    /// that check. Fields are bounded too, and over-long text is truncated with
    /// a marker rather than rejected, because losing a whole span to a verbose
    /// name would discard evidence over presentation.
    pub const MAX_FIELD: usize = 4096;

    fn bound(s: String) -> String {
        if s.len() <= Self::MAX_FIELD { return s; }
        let mut cut = Self::MAX_FIELD;
        while cut > 0 && !s.is_char_boundary(cut) { cut -= 1; }   // never split a char
        format!("{}…<truncated {} bytes>", &s[..cut], s.len() - cut)
    }

    /// What must hold before a span is written down.
    ///
    /// Everything here is a property the rest of the system already assumes.
    /// Accepting a span that breaks one of them does not fail loudly; it
    /// produces a capture that quietly disagrees with itself, which is the
    /// expensive kind of wrong. A rejection names the span and the reason, so a
    /// sender can fix the adapter rather than guess.
    pub fn validate(&self) -> Result<(), String> {
        if self.span_id.trim().is_empty() {
            return Err("span_id is empty: a span with no identity cannot be referred to".into());
        }
        // An identifier is not free text. Ids are interpolated into markup by
        // readers and into queries by tools, and a capture crosses a trust
        // boundary every time it is exchanged with a peer; an id carrying a
        // quote put a live event handler into the page of whoever opened it.
        // Escaping at each sink is still done; this makes the class impossible
        // rather than merely handled.
        if let Some(bad) = self.span_id.chars().find(|c| {
            !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        }) {
            return Err(format!("span_id contains {:?}: an identifier may hold only letters, digits, and - _ . :", bad));
        }
        if let Some(p) = &self.parent_span_id {
            if let Some(bad) = p.chars().find(|c| {
                !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
            }) {
                return Err(format!("parent_span_id contains {:?}: an identifier may hold only letters, digits, and - _ . :", bad));
            }
        }
        if self.name.trim().is_empty() {
            return Err(format!("{}: name is empty, and a nameless span cannot be read", self.span_id));
        }
        if !self.start.is_finite() {
            return Err(format!("{}: start is not a finite time", self.span_id));
        }
        if let Some(e) = self.end {
            if !e.is_finite() {
                return Err(format!("{}: end is not a finite time", self.span_id));
            }
            // an end before a start is not a slow span, it is a broken clock or
            // a broken adapter, and storing it would make every duration lie
            if e < self.start {
                return Err(format!("{}: ends {:.6}s before it starts", self.span_id, self.start - e));
            }
        }
        if self.parent_span_id.as_deref() == Some(self.span_id.as_str()) {
            return Err(format!("{}: is its own parent", self.span_id));
        }
        Ok(())
    }

    pub fn from_json(v: &J, host_override: Option<&str>) -> Option<Span> {
        let gs = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|s| s.to_string());
        let gn = |k: &str| v.get(k).and_then(|x| x.as_f64());
        Some(Span {
            span_id: gs("span_id")?,
            trace_id: gs("trace_id").unwrap_or_else(|| "t".into()),
            parent_span_id: gs("parent_span_id"),
            links: v.get("links").and_then(|l| l.as_arr())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default(),
            name: Span::bound(gs("name").unwrap_or_else(|| "?".into())),
            kind: Span::bound(gs("kind").unwrap_or_else(|| "call".into())),
            start: gn("start").unwrap_or(0.0),
            end: v.get("end").and_then(|e| if e.is_null() { None } else { e.as_f64() }),
            status: Span::bound(gs("status").unwrap_or_else(|| "running".into())),
            lifecycle: v.get("lifecycle").cloned().unwrap_or(J::Arr(vec![])),
            origin: gs("origin").unwrap_or_else(|| "semantic".into()),
            host_id: host_override.map(|h| h.to_string())
                .or_else(|| gs("host_id")).unwrap_or_else(|| "local".into()),
            clock_source: gs("clock_source"),
            domain: gs("domain").map(Span::bound),
            attributes: v.get("attributes").cloned().unwrap_or(J::obj()),
            events: v.get("events").cloned().unwrap_or(J::Arr(vec![])),
            inputs: v.get("inputs").cloned().unwrap_or(J::Null),
            output: v.get("output").cloned().unwrap_or(J::Null),
            error: v.get("error").cloned().unwrap_or(J::Null),
            run: v.get("run").cloned().unwrap_or(J::Null),
        })
    }
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/// The one place the primitive-to-family mapping lives, so no consumer has to
/// reinvent it and none of them can disagree.
pub fn family_of(kind: &str) -> &'static str {
    match kind {
        "io" | "external" => "io",
        "wait" => "wait",
        "queue" => "boundary",
        "error" => "fault",
        // control is the family carrying the least specific claim, which is the
        // right home for a primitive this build does not recognise
        _ => "control",
    }
}

// ========================================================================
// CONSTANTS
// ========================================================================

/// Columns added after the first schema. SQLite has no ADD COLUMN IF NOT
/// EXISTS, so each is attempted and a failure means it is already there. This
/// is the migration story the design lacked: a store written by an older build
/// stays readable and gains the new fields empty.
pub const MIGRATIONS: &[&str] = &[
    "ALTER TABLE spans ADD COLUMN inputs TEXT",
    "ALTER TABLE spans ADD COLUMN output TEXT",
    "ALTER TABLE spans ADD COLUMN error TEXT",
    "ALTER TABLE spans ADD COLUMN run TEXT",
];

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

pub fn migrate(conn: &Connection) {
    for m in MIGRATIONS { let _ = conn.execute(m, []); }
}

// ========================================================================
// CONSTANTS
// ========================================================================

/// Spans a sender could not deliver.
///
/// Recorded per host rather than aggregated: one adapter losing rows says
/// something quite different from every adapter losing them, and averaging the
/// two would hide the case that matters.
pub const LOSS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS delivery_loss (
  host        TEXT PRIMARY KEY,
  spans_lost  INTEGER NOT NULL DEFAULT 0,
  traces_lost INTEGER NOT NULL DEFAULT 0,
  abnormal_lost INTEGER NOT NULL DEFAULT 0,
  last_seen   REAL NOT NULL
);
";

// ========================================================================
// TYPES
// ========================================================================

pub struct Store { pub conn: Connection }

// ========================================================================
// IMPLEMENTATIONS
// ========================================================================

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Store> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        let _ = conn.execute_batch(LOSS_SCHEMA);
        migrate(&conn);
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        Ok(Store { conn })
    }
    pub fn open_ro(path: &str) -> rusqlite::Result<Store> {
        let conn = Connection::open_with_flags(path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Store { conn })
    }

    /// Phase one and phase two in a single upsert: a span arriving with an end
    /// updates the open row rather than duplicating it.
    /// Writes a span, or refuses it.
    ///
    /// Validation lives here rather than at each caller because there are three
    /// write doors; local ingest, a peer exchange, and the syscall enrichment
    /// merge; and an invariant enforced at two of three is not an invariant.
    /// Putting it at the single point every write passes through means a future
    /// caller cannot bypass it by not knowing it exists.
    pub fn upsert(&self, s: &Span) -> rusqlite::Result<()> {
        if let Err(why) = s.validate() {
            return Err(rusqlite::Error::InvalidParameterName(why));
        }
        self.upsert_unchecked(s)
    }

    fn upsert_unchecked(&self, s: &Span) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO spans (span_id,trace_id,parent_span_id,links,name,kind,start,end,\
             status,lifecycle,origin,host_id,clock_source,domain,attributes,events,\
             inputs,output,error,run) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20) \
             ON CONFLICT(span_id) DO UPDATE SET end=excluded.end, status=excluded.status, \
             lifecycle=excluded.lifecycle, links=excluded.links, attributes=excluded.attributes, \
             output=excluded.output, error=excluded.error",
            params![s.span_id, s.trace_id, s.parent_span_id,
                J::Arr(s.links.iter().map(|l| J::s(l)).collect()).dump(),
                s.name, s.kind, s.start, s.end, s.status, s.lifecycle.dump(),
                s.origin, s.host_id, s.clock_source, s.domain,
                s.attributes.dump(), s.events.dump(),
                s.inputs.dump(), s.output.dump(), s.error.dump(), s.run.dump()],
        )?;
        Ok(())
    }

    /// True when a store already has a column added by a later build.
    ///
    /// A reader must not require a writable file, so an old store opened
    /// read-only cannot be migrated on the spot. The query adapts instead.
    fn has_column(&self, name: &str) -> bool {
        self.conn.prepare("SELECT 1 FROM pragma_table_info('spans') WHERE name=?1")
            .and_then(|mut st| st.exists([name])).unwrap_or(false)
    }

    /// Records that a sender dropped spans it could not deliver.
    pub fn record_loss(&self, host: &str, lost: i64, traces_lost: i64, abnormal_lost: i64)
        -> rusqlite::Result<()> {
        self.conn.execute_batch(LOSS_SCHEMA)?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64()).unwrap_or(0.0);
        self.conn.execute(
            "INSERT INTO delivery_loss (host,spans_lost,traces_lost,abnormal_lost,last_seen)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(host) DO UPDATE SET
                 spans_lost    = delivery_loss.spans_lost + excluded.spans_lost,
                 traces_lost   = delivery_loss.traces_lost + excluded.traces_lost,
                 abnormal_lost = delivery_loss.abnormal_lost + excluded.abnormal_lost,
                 last_seen     = excluded.last_seen",
            rusqlite::params![host, lost, traces_lost, abnormal_lost, now])?;
        Ok(())
    }

    /// What this capture is known to be missing, per host.
    pub fn losses(&self) -> Vec<(String, i64, i64, i64)> {
        let _ = self.conn.execute_batch(LOSS_SCHEMA);
        self.conn.prepare("SELECT host,spans_lost,traces_lost,abnormal_lost FROM delivery_loss WHERE spans_lost > 0 ORDER BY spans_lost DESC")
            .and_then(|mut st| {
                let it = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
                Ok(it.filter_map(|x| x.ok()).collect())
            }).unwrap_or_default()
    }

    pub fn all(&self) -> rusqlite::Result<Vec<Span>> {
        let modern = self.has_column("inputs");
        let sql = if modern {
            format!("SELECT {} FROM spans ORDER BY start", COLS)
        } else {
            // an older store: the four later columns read as absent
            format!("SELECT {}, NULL, NULL, NULL, NULL FROM spans ORDER BY start", COLS_LEGACY)
        };
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map([], |r| {
            let pj = |s: String| crate::json::parse(&s).unwrap_or(J::Null);
            Ok(Span {
                span_id: r.get(0)?, trace_id: r.get(1)?, parent_span_id: r.get(2)?,
                links: crate::json::parse(&r.get::<_, String>(3)?).ok()
                    .and_then(|j| j.as_arr().map(|a| a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string())).collect()))
                    .unwrap_or_default(),
                name: r.get(4)?, kind: r.get(5)?, start: r.get(6)?, end: r.get(7)?,
                status: r.get(8)?, lifecycle: pj(r.get(9)?), origin: r.get(10)?,
                host_id: r.get(11)?, clock_source: r.get(12)?, domain: r.get(13)?,
                attributes: pj(r.get(14)?), events: pj(r.get(15)?),
                // a row written before these columns existed reads as absent
                inputs: r.get::<_, Option<String>>(16).ok().flatten().map(pj).unwrap_or(J::Null),
                output: r.get::<_, Option<String>>(17).ok().flatten().map(pj).unwrap_or(J::Null),
                error: r.get::<_, Option<String>>(18).ok().flatten().map(pj).unwrap_or(J::Null),
                run: r.get::<_, Option<String>>(19).ok().flatten().map(pj).unwrap_or(J::Null),
            })
        })?;
        let mut out = Vec::new();
        for x in rows { out.push(x?); }
        Ok(out)
    }
}
