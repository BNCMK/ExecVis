# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: parity_check.py
#  script_path: execviz-rs/parity_check.py
#  module_name: parity_check
#  version: 0.53.1
#  description: parity check.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: json, subprocess, sys
#  features: parity check
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""Checks that the two implementations of the same views still agree.

`api.py` is a documented Python reader of the store (execviz/README.md) and the
Rust core answers the same questions. Two implementations of one contract drift
apart silently: a change to one is a change to the documented behaviour of the
other, and nothing noticed. This turns that into a loud failure.
"""
import json
import subprocess
import sys

GREEN = "\033[32mPASS\033[0m"
RED = "\033[31mFAIL\033[0m"

binary, api_py, db = sys.argv[1], sys.argv[2], sys.argv[3]
failed = False

for view in ("system", "field"):
    py = json.loads(subprocess.run(
        [sys.executable, api_py, db, "--view", view],
        capture_output=True, text=True, check=True).stdout)
    rs = json.loads(subprocess.run(
        [binary, "view", db, "--lod", view],
        capture_output=True, text=True, check=True).stdout)

    def shape(d):
        if "hosts" in d:
            return {h["host"]: (h.get("spans"), h.get("errors"), h.get("running"))
                    for h in d["hosts"]}
        if "clusters" in d:
            return {c.get("id") or c.get("cluster"): c.get("spans") for c in d["clusters"]}
        return sorted(d)

    a, b = shape(py), shape(rs)
    if a == b:
        print(f"  {GREEN} the python reader and the rust core agree on the {view} view")
    else:
        failed = True
        print(f"  {RED} the two implementations disagree on the {view} view")
        if isinstance(a, dict) and isinstance(b, dict):
            for k in sorted(set(a) | set(b)):
                if a.get(k) != b.get(k):
                    print(f"        {k}: python={a.get(k)} rust={b.get(k)}")

sys.exit(1 if failed else 0)
