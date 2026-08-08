# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: pytest_execviz.py
#  script_path: execviz/pytest_execviz.py
#  module_name: pytest_execviz
#  version: 0.53.1
#  description: explicit units only: a test runner that traces its own internals buries the tests among thousands of frames from pytest itself
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, os
#  features: pytest execviz
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""Attach a capture to a test run (spec 5.6, gap 42).

    pytest -p pytest_execviz --execviz-db run.db

Each test becomes a span, so a failing test carries the execution that produced
it rather than only a traceback. Across runs (`execviz across`) the same tests
become a flakiness report, which is the thing a single run cannot show.

The plugin is deliberately inert unless asked for: a test tool that changes
behaviour when merely installed is a test tool nobody trusts.
"""
import os, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture


def pytest_addoption(parser):
    g = parser.getgroup("execviz")
    g.addoption("--execviz-db", action="store", default=None,
                help="record this test run into a capture")
    g.addoption("--execviz-collector", action="store", default=None,
                help="push this test run to a running instance instead")
    g.addoption("--execviz-values", action="store_true",
                help="record test arguments; off by default because values can leak")


def pytest_configure(config):
    db = config.getoption("--execviz-db")
    coll = config.getoption("--execviz-collector")
    if not db and not coll:
        return                      # inert unless asked for
    # explicit units only: a test runner that traces its own internals buries
    # the tests among thousands of frames from pytest itself
    if coll:
        capture.install_push(coll, host_id="tests", autotrace=False)
    else:
        capture.install(db, autotrace=False)
    capture.set_trace()
    capture.set_domain("tests")
    if config.getoption("--execviz-values"):
        capture.capture_values(True)
    # what produced this run, so two runs can be compared rather than guessed at
    capture.declare_run(
        build=os.environ.get("BUILD_ID") or os.environ.get("GITHUB_RUN_ID"),
        commit=os.environ.get("GIT_COMMIT") or os.environ.get("GITHUB_SHA"),
        environment=os.environ.get("EXECVIZ_ENV", "ci" if os.environ.get("CI") else "local"),
    )
    config._execviz_session = capture.span_start("test session", "call")
    config._execviz_spans = {}


def pytest_runtest_setup(item):
    cfg = item.config
    if not hasattr(cfg, "_execviz_session"):
        return
    sid = capture.span_start(item.nodeid, "call",
                             attributes={"file": str(item.fspath), "test": item.name})
    cfg._execviz_spans[item.nodeid] = {"span": sid, "status": "ok", "error": None}


def pytest_runtest_makereport(item, call):
    """Records the outcome while the item is in hand.

    Hooking the report alone loses the config, and guessing it from module state
    is the kind of shortcut that works until two runs overlap.
    """
    cfg = item.config
    if not hasattr(cfg, "_execviz_session"):
        return
    rec = cfg._execviz_spans.get(item.nodeid)
    if rec is None:
        return
    if call.excinfo is not None:
        skipped = call.excinfo.typename in ("Skipped", "XFailed")
        rec["status"] = "skipped" if skipped else "error"
        rec["error"] = str(call.excinfo.getrepr(style="short"))[:1500]


def pytest_runtest_teardown(item, nextitem):
    cfg = item.config
    if not hasattr(cfg, "_execviz_session"):
        return
    rec = cfg._execviz_spans.pop(item.nodeid, None)
    if rec is None:
        return
    capture.span_end(rec["span"], rec["status"],
                     attributes={"detail": rec["error"]} if rec["error"] else None)


def pytest_unconfigure(config):
    if not hasattr(config, "_execviz_session"):
        return
    # close anything still open rather than leaving it looking hung
    for rec in list(config._execviz_spans.values()):
        capture.span_end(rec["span"], rec["status"])
    capture.span_end(config._execviz_session, "ok")
    capture.uninstall()
