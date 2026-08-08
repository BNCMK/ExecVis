# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: sitecustomize.py
#  script_path: execviz-attach/sitecustomize.py
#  module_name: sitecustomize
#  version: 0.53.1
#  description: Auto-tracing is off unless asked for. Attaching to a program nobody asked to instrument should not silently change its performance profile; log capture is nearly free, a profile hook is not.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: atexit, capture, os, sys
#  features: sitecustomize, capture, profile
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""Attaches the Python adapter with no change to the program.

`sitecustomize` is imported automatically by CPython at startup if it is
anywhere on the path, which makes it the supported way to attach to a program
you are not editing:

    PYTHONPATH=/path/to/execviz-attach EXECVIZ_COLLECTOR=http://host:8900 python3 app.py

The program is not modified, not rebuilt, and does not import anything.
"""
import os
import sys
# =========================================================================
# INBOUND: ONE SPAN PER REQUEST SERVED
# =========================================================================
def _patch_wsgi(capture):
    """Wrap the WSGI entry point every Python web server goes through.

    Flask, Django, Bottle and anything else conforming call the application
    with (environ, start_response), so wrapping the server's handler catches a
    request whatever framework produced it. The socket descriptor is recorded
    with the span where the server exposes it, because that is what ties the
    read before the handler and the write after it back to this request.
    """
    try:
        import wsgiref.handlers as _h
    except Exception:
        return

    if getattr(_h.BaseHandler, "_execviz", False):
        return

    _orig = _h.BaseHandler.run

    def run(self, application):
        wrapped = _wrap_app(capture, application, self)
        return _orig(self, wrapped)

    run._execviz = True
    _h.BaseHandler.run = run
    _h.BaseHandler._execviz = True


def _wrap_app(capture, application, handler):
    def app(environ, start_response):
        name = "%s %s" % (environ.get("REQUEST_METHOD", "?"),
                          str(environ.get("PATH_INFO", "/"))[:120])
        attrs = {"method": environ.get("REQUEST_METHOD", ""),
                 "path": str(environ.get("PATH_INFO", ""))[:200]}
        fd = _fd_of(environ)
        if fd >= 0:
            attrs["fd"] = fd
        span = capture.span_start(name, "io", attributes=attrs)
        status_holder = {}

        def sr(status, headers, exc_info=None):
            status_holder["code"] = int(str(status).split(" ")[0] or 0)
            return start_response(status, headers, exc_info)

        try:
            result = application(environ, sr)
            code = status_holder.get("code", 0)
            capture.span_end(span, "error" if code >= 500 else "ok",
                             attributes={"status_code": code})
            return result
        except Exception:
            capture.span_end(span, "error")
            raise
    return app


def _fd_of(environ):
    """The connection's descriptor, when the server hands one over."""
    for key in ("gunicorn.socket", "werkzeug.socket", "wsgi.input"):
        obj = environ.get(key)
        for attr in ("fileno", "_sock"):
            try:
                target = getattr(obj, attr, None)
                if callable(target):
                    return int(target())
            except Exception:
                pass
    return -1


# =========================================================================
# OUTBOUND: ONE SPAN PER REQUEST MADE
# =========================================================================
def _patch_http_client(capture):
    """Wrap the standard library's HTTP client.

    `requests`, `urllib` and most of what is built on them end in
    `http.client.HTTPConnection.getresponse`, so wrapping it catches an
    outbound call without knowing which library asked for it.
    """
    try:
        import http.client as _c
    except Exception:
        return
    if getattr(_c.HTTPConnection, "_execviz", False):
        return

    _orig = _c.HTTPConnection.getresponse

    def getresponse(self, *a, **kw):
        target = "%s%s" % (getattr(self, "host", ""), getattr(self, "_execviz_path", ""))
        span = capture.span_start(("http out %s" % target)[:120], "external",
                                  attributes={"target": target[:200]})
        try:
            resp = _orig(self, *a, **kw)
            code = getattr(resp, "status", 0)
            capture.span_end(span, "error" if code >= 500 else "ok",
                             attributes={"status_code": code})
            return resp
        except Exception:
            capture.span_end(span, "error")
            raise

    getresponse._execviz = True
    _c.HTTPConnection.getresponse = getresponse
    _c.HTTPConnection._execviz = True


if os.environ.get("EXECVIZ_COLLECTOR") or os.environ.get("EXECVIZ_DB"):
    try:
        here = os.path.dirname(os.path.abspath(__file__))
        sys.path.insert(0, os.path.join(here, "..", "execviz"))
        import capture

        collector = os.environ.get("EXECVIZ_COLLECTOR")
        host = os.environ.get("EXECVIZ_HOST") or "python"
        # Auto-tracing is off unless asked for. Attaching to a program nobody
        # asked to instrument should not silently change its performance
        # profile; log capture is nearly free, a profile hook is not.
        autotrace = os.environ.get("EXECVIZ_TRACE") == "1"
        if collector:
            capture.install_push(collector, host_id=host, autotrace=autotrace)
        else:
            capture.install(os.environ["EXECVIZ_DB"], autotrace=autotrace)
        capture.set_trace()
        capture.set_domain(os.environ.get("EXECVIZ_DOMAIN", "app"))

        # A process is a unit of execution, so attaching with no source change
        # opens a span for the run itself. Without it every captured line is
        # dropped for having no parent; the "attribute, don't invent" rule
        # refusing lines that do have a true parent, just not a declared one.
        # This is not an invented parent: the program really did run.
        import atexit
        _root = capture.span_start(
            os.path.basename(sys.argv[0]) or "python", "call",
            attributes={"argv": " ".join(sys.argv[:8])[:400], "pid": os.getpid()})

        def _close():
            try:
                capture.span_end(_root, "ok")
                capture.uninstall()
            except Exception:
                pass

        atexit.register(_close)

        # =========================================================================
        # REQUESTS BECOME SPANS
        # =========================================================================
        # A span for the process says a program ran and nothing about what it
        # did. Every request served and every request made is a unit of work
        # with its own timing and status, so each becomes a span. Without this
        # the map holds one span per process and `witness` has no claim to
        # check against what the kernel saw.
        _patch_wsgi(capture)
        _patch_http_client(capture)

        if os.environ.get("EXECVIZ_VERBOSE") == "1":
            sys.stderr.write("execviz: attached to this process, no source change\n")
    except Exception as e:
        # A program must never fail to start because a recorder could not
        # attach. It reports and steps aside.
        sys.stderr.write("execviz: could not attach (%s); the program runs unchanged\n" % e)

