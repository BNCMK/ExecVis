# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: demo.py
#  script_path: execviz-db/demo.py
#  module_name: demo
#  version: 0.53.1
#  description: an indexed lookup
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, os
#  features: demo
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

import os, sqlite3, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "execviz"))
import capture, execviz_db

capture.install_push(os.environ.get("EXECVIZ_COLLECTOR", "http://127.0.0.1:8900"),
                     host_id="api-1", autotrace=False)
capture.set_trace(); capture.set_domain("orders")

raw = sqlite3.connect(":memory:")
raw.executescript("""
CREATE TABLE orders (id INTEGER PRIMARY KEY, sku TEXT, customer TEXT, total REAL);
CREATE TABLE customers (id INTEGER PRIMARY KEY, email TEXT);
""")
for i in range(4000):
    raw.execute("INSERT INTO orders VALUES (?,?,?,?)", (i, "SKU%d" % (i % 50), "cust%d" % (i % 200), i * 1.5))
raw.commit()

db = execviz_db.trace(raw, plans=True)
root = capture.span_start("GET /orders", "call")

# an indexed lookup
c = db.cursor(); c.execute("SELECT * FROM orders WHERE id = ?", (42,)); c.fetchall()
# and one that examines everything to return a handful
c = db.cursor(); c.execute("SELECT * FROM orders WHERE customer = 'cust7' AND total > 100"); c.fetchall()
# a statement carrying a secret in a literal
c = db.cursor(); c.execute("SELECT * FROM customers WHERE email = 'bob@example.com'"); c.fetchall()

capture.span_end(root, "ok")
capture.uninstall()
print("db demo complete")
