<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-db/README.md
  module_name: README
  version: 0.53.1
  description: execviz for the database interior
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz for the database interior

    db = execviz_db.trace(sqlite3.connect(...), plans=True)

`db_query 140ms` was where the trail went cold and the person opened another
tool. A query span now carries the statement, the plan, and what the engine
did:

    db select orders
       statement: SELECT * FROM orders WHERE id = ?
       plan     : SEARCH orders USING INTEGER PRIMARY KEY (rowid=?)

    db select orders
       statement: SELECT * FROM orders WHERE customer = ? AND total > ?
       plan     : SCAN orders
       ⚠ scans a whole table

Three rules, because a database is the easiest place both to leak and to lie.

**The statement is recorded parameterised, never interpolated.** Literals are
replaced before anything is stored, so the shape a reader needs survives and the
customer's data does not travel. Verified: a query containing
`'bob@example.com'` produced a capture in which that string appears zero times.

**Asking for a plan must not change the run.** `EXPLAIN` is a second statement
with its own cost, so `plans` is off by default; a tracing tool that silently
doubles the query count is not one anybody should run in production without
choosing to. The plan is taken on a separate cursor and any failure is silent:
breaking a query because the tool wanted a plan would be worse than having no
plans.

**Rows examined against rows returned explains the time.** A query returning ten
rows after examining a million is the specific fault nobody sees from the
duration alone, and the plan is what names it.

Wraps any DB-API connection; everything not overridden passes through.
