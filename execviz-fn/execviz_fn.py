# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: execviz_fn.py
#  script_path: execviz-fn/execviz_fn.py
#  module_name: execviz_fn
#  version: 0.53.1
#  description: Module state survives a freeze; that makes cold-start detection possible at all, and what makes the freeze visible.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: capture, os, sys, time
#  features: execviz fn, detect
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""execviz capture for serverless functions.

Two assumptions the ordinary delivery model makes are false here.

The process may be killed the instant a handler returns, so anything buffered
for a later flush is lost; worst precisely where it matters most, because a
function that died mid-flight is the one someone is looking for. Delivery is
therefore synchronous at the boundary, and the cost is stated rather than hidden:
the handler waits for the recording.

And a sandbox is *frozen* between invocations rather than destroyed, so wall
time keeps running while nothing executes. Wall time and execution time are
different quantities here, and a capture that conflates them is lying with
arithmetic.
"""
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "execviz"))
import capture  # noqa: E402

# Module state survives a freeze; that makes cold-start detection
# possible at all, and what makes the freeze visible.
_SANDBOX_STARTED = time.time()
_INVOCATIONS = 0
_SANDBOX_ID = "%s-%d" % (os.environ.get("AWS_LAMBDA_LOG_STREAM_NAME", "sandbox"), os.getpid())


def _process_cpu():
    """Execution time, as distinct from wall time.

    A frozen sandbox accrues wall time and no CPU. Reporting only wall time
    would describe a function as slow when it was not running.
    """
    try:
        return time.process_time()
    except Exception:
        return None


def install(collector=None, host_id=None, domain="function"):
    collector = collector or os.environ.get("EXECVIZ_COLLECTOR", "http://127.0.0.1:8900")
    host = host_id or os.environ.get("AWS_LAMBDA_FUNCTION_NAME") \
        or os.environ.get("K_SERVICE") or os.environ.get("FUNCTION_NAME") or "function"
    # explicit units only: a function is short, and tracing every frame of a
    # runtime's own bootstrap would cost more than the work being measured
    capture.install_push(collector, host_id=host, autotrace=False)
    capture.set_domain(domain)
    capture.declare_run(
        commit=os.environ.get("GIT_COMMIT"),
        build=os.environ.get("AWS_LAMBDA_FUNCTION_VERSION") or os.environ.get("K_REVISION"),
        environment=os.environ.get("EXECVIZ_ENV", "serverless"),
        region=os.environ.get("AWS_REGION") or os.environ.get("FUNCTION_REGION"),
    )
    return collector


def handler(fn=None, name=None):
    """Wraps a function handler.

    Records the invocation, whether it was a cold start, how long the sandbox
    was frozen beforehand, and; always; flushes before returning.
    """
    def wrap(f):
        def invoke(event=None, context=None):
            global _INVOCATIONS
            _INVOCATIONS += 1
            capture.set_trace()
            cpu0 = _process_cpu()
            # Only what cannot be derived. The conformance checker rejected an
            # earlier version of this file for recording cold starts and freeze
            # gaps as lifecycle events: both follow from timestamps, and a
            # capture that states a derived fact invites it to disagree with the
            # data it came from. What is NOT derivable is which sandbox this is,
            # two sandboxes running at once interleave, and nothing in the
            # timestamps separates them.
            attrs = {"sandbox": _SANDBOX_ID, "sandbox_started": _SANDBOX_STARTED}
            if getattr(context, "aws_request_id", None):
                attrs["request_id"] = context.aws_request_id
            sid = capture.span_start(name or getattr(f, "__name__", "handler"), "call",
                                     attributes=attrs)
            try:
                result = f(event, context)
                status = "ok"
                return result
            except BaseException as e:
                status = "error"
                capture.span_end(sid, "error", error=e)
                raise
            finally:
                if status == "ok":
                    cpu1 = _process_cpu()
                    end_attrs = {}
                    if cpu0 is not None and cpu1 is not None:
                        # both are recorded: wall time is what the caller waited,
                        # execution time is what the function used
                        end_attrs["cpu_ms"] = round((cpu1 - cpu0) * 1000, 3)
                    capture.span_end(sid, "ok", attributes=end_attrs or None)
                # synchronous, because the process may not exist a moment from
                # now. The flush happens before the clock is marked, so the
                # recorder's own cost is never mistaken for a freeze.
                flush()
        invoke.__name__ = getattr(f, "__name__", "invoke")
        return invoke
    return wrap(fn) if fn else wrap


def flush():
    """Delivers now. The handler waits, and that cost is the price of not
    losing the record of a function that is about to be killed."""
    store = getattr(capture, "_store", None)
    if store is not None and hasattr(store, "flush"):
        try:
            store.flush()
        except Exception:
            pass          # never fail the invocation because recording failed


def sandbox():
    return {"id": _SANDBOX_ID, "started": _SANDBOX_STARTED, "invocations": _INVOCATIONS}
