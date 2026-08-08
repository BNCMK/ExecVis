# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: test_state.py
#  script_path: execviz/test_state.py
#  module_name: test_state
#  version: 0.53.1
#  description: silent removal makes an absent field ambiguous between "nothing there" and "not shown"
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: io, json, push_store, redact, sys
#  features: test state
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""Tests for state, failure and redaction."""
import json
import sys, unittest
sys.path.insert(0, '.')
import redact, capture


class Redaction(unittest.TestCase):
    def setUp(self):
        self.p = redact.Policy()

    def test_a_secret_key_is_never_recorded(self):
        out, why = self.p.scrub('password', 'hunter2')
        self.assertNotIn('hunter2', out)
        self.assertEqual(why, 'key')

    def test_a_redacted_value_is_marked_not_deleted(self):
        # silent removal makes an absent field ambiguous between "nothing there"
        # and "not shown"
        out, _ = self.p.scrub('token', 'abc')
        self.assertTrue(out.startswith('<redacted:'), out)

    def test_a_secret_inside_a_container_is_caught_by_its_own_key(self):
        text = self.p.render({'sku': 'A1', 'password': 'hunter2'})
        self.assertIn('sku=A1', text)
        self.assertNotIn('hunter2', text)

    def test_a_pattern_catches_what_the_program_named_innocently(self):
        out, why = self.p.scrub('note', 'Authorization: Bearer abcdefghijklmnopq123')
        self.assertNotIn('abcdefghijklmnopq123', out)
        self.assertTrue(why)

    def test_it_fails_closed_when_a_rule_cannot_be_evaluated(self):
        class Hostile:
            def __repr__(self): raise RuntimeError('no')
        out, why = self.p.scrub('x', Hostile())
        self.assertTrue(out.startswith('<'), out)

    def test_a_long_value_is_truncated_and_says_so(self):
        out = redact.Policy(patterns=[]).render('z' * 5000)
        self.assertLess(len(out), 400)
        self.assertIn('truncated', out)

    def test_containers_render_elements_not_shapes(self):
        # "tuple(len=1)" answers nothing: the question is which input caused this
        self.assertIn('A1', self.p.render(('A1',)))


class State(unittest.TestCase):
    def test_absence_is_stated_rather_than_implied(self):
        capture.capture_values(False)
        v = capture.render_values({'a': 1})
        self.assertFalse(v['recorded'])
        self.assertIn('why', v)
        capture.capture_values(True)

    def test_values_are_rendered_at_the_moment_of_the_call(self):
        # an object that mutates afterwards must not rewrite history
        capture.capture_values(True)
        box = ['before']
        v = capture.render_values({'box': box})
        box[0] = 'after'
        self.assertIn('before', str(v['values']))
        self.assertNotIn('after', str(v['values']))


class Failure(unittest.TestCase):
    def test_the_cause_chain_is_recorded_beneath_the_top_exception(self):
        try:
            try:
                raise ValueError('the real problem')
            except ValueError as inner:
                raise RuntimeError('the visible symptom') from inner
        except RuntimeError as e:
            err = capture.describe_error(e)
        self.assertEqual(err['type'], 'RuntimeError')
        self.assertTrue(err['caused_by'])
        self.assertEqual(err['caused_by'][0]['type'], 'ValueError')
        self.assertIn('real problem', err['caused_by'][0]['message'])

    def test_frames_are_recorded(self):
        try:
            raise KeyError('k')
        except KeyError as e:
            err = capture.describe_error(e)
        self.assertTrue(err['frames'])
        self.assertIn('line', err['frames'][-1])

    def test_an_error_message_is_scrubbed_like_any_other_value(self):
        capture.capture_values(True)
        try:
            raise RuntimeError('failed for Bearer abcdefghijklmnopq12345')
        except RuntimeError as e:
            err = capture.describe_error(e)
        self.assertNotIn('abcdefghijklmnopq12345', err['message'])




class TailSampling(unittest.TestCase):
    """Spec 4.5.2: decide once the outcome is known."""

    def setUp(self):
        capture.tail_sample(True, rate=0.1, slow_ms=500)

    def tearDown(self):
        capture.tail_sample(False)

    def test_a_failed_trace_is_always_kept(self):
        keep, why, w = capture.decide_trace('x', [{'status': 'error', 'start': 0, 'end': 0.01}])
        self.assertTrue(keep)
        self.assertEqual(w, 1.0, 'a trace kept because it is interesting stands for itself alone')

    def test_an_open_trace_is_always_kept(self):
        # an unfinished span is the finding; sampling it away would discard
        # exactly what someone came looking for
        keep, _, _ = capture.decide_trace('x', [{'status': 'running', 'start': 0, 'end': None}])
        self.assertTrue(keep)

    def test_a_slow_trace_is_always_kept(self):
        keep, _, _ = capture.decide_trace('x', [{'status': 'ok', 'start': 0, 'end': 0.9}])
        self.assertTrue(keep)

    def test_a_drawn_trace_carries_the_weight_it_stands_for(self):
        # so a count can be weighted accurately rather than read as a total
        found = None
        for i in range(500):
            keep, why, w = capture.decide_trace('t%d' % i, [{'status': 'ok', 'start': 0, 'end': 0.01}])
            if keep and 'drawn' in why:
                found = w
                break
        self.assertIsNotNone(found, 'some trace should be drawn at rate 0.1')
        self.assertAlmostEqual(found, 10.0, places=4)

    def test_the_decision_is_deterministic_across_replays(self):
        spans = [{'status': 'ok', 'start': 0, 'end': 0.01}]
        self.assertEqual(capture.decide_trace('same', spans), capture.decide_trace('same', spans))

    def test_sampling_off_keeps_everything(self):
        capture.tail_sample(False)
        keep, _, w = capture.decide_trace('x', [{'status': 'ok', 'start': 0, 'end': 0.01}])
        self.assertTrue(keep)
        self.assertEqual(w, 1.0)



class HostileValues(unittest.TestCase):
    """A recorded value is not guaranteed to be a finite tree, or renderable."""

    def setUp(self):
        self.p = redact.Policy()

    def test_a_cycle_is_named_a_cycle_not_a_redaction(self):
        # it used to be absorbed by the fail-closed handler and reported as
        # <redacted:...>, which reads as "a secret was withheld here" and is untrue
        loop = {}
        loop["self"] = loop
        out = self.p.render(loop)
        self.assertIn("cycle", out)
        self.assertNotIn("redacted", out)

    def test_a_self_referencing_list_is_also_caught(self):
        lst = []
        lst.append(lst)
        self.assertIn("cycle", self.p.render(lst))

    def test_depth_is_bounded_and_says_so(self):
        deep = {}
        cur = deep
        for _ in range(400):
            cur["n"] = {}
            cur = cur["n"]
        out = self.p.render(deep)
        self.assertIn("too deep", out)
        self.assertLess(len(out), 400)

    def test_two_equal_containers_are_not_a_cycle(self):
        # identity, not equality: repeating a shape is ordinary data
        out = self.p.render([{"a": 1}, {"a": 1}])
        self.assertNotIn("cycle", out)

    def test_a_secret_nested_deep_is_still_caught(self):
        out = self.p.render({"a": {"b": {"password": "hunter2"}}})
        self.assertNotIn("hunter2", out)
        self.assertIn("redacted", out)

    def test_an_object_that_refuses_to_render_does_not_escape(self):
        class Hostile:
            def __repr__(self): raise RuntimeError("no")
            def __str__(self): raise RuntimeError("no")
        out = self.p.render(Hostile())
        self.assertTrue(out.startswith("<"), out)



class BoundedDelivery(unittest.TestCase):
    """A recorder must never be what kills the program it is observing."""

    def _store(self):
        import push_store
        return push_store.PushStore('http://127.0.0.1:9', 'test', 999)

    def test_the_buffer_is_bounded_when_the_collector_is_unreachable(self):
        st = self._store()
        for i in range(st.MAX_PENDING + 2000):
            st.begin({"span_id": f"s{i}", "trace_id": "t", "name": "w",
                      "kind": "call", "start": float(i)})
            st.finish(f"s{i}", float(i) + 1, "ok")
        self.assertLessEqual(len(st._spans), st.MAX_PENDING)
        self.assertGreater(st.dropped, 0)

    def test_whole_traces_are_dropped_never_loose_spans(self):
        # spec 2.5: sampling is trace-level, because dropping a span whose
        # siblings remain punches a hole in that trace's graph
        st = self._store()
        for i in range(st.MAX_PENDING + 3000):
            t = "trace%d" % (i // 4)
            st.begin({"span_id": f"s{i}", "trace_id": t, "name": "w",
                      "kind": "call", "start": 1.0 + i})
            st.finish(f"s{i}", 2.0 + i, "ok")
        by = {}
        for sid, s in st._spans.items():
            by.setdefault(s["trace_id"], []).append(sid)
        partial = [t for t, ids in by.items() if len(ids) != 4]
        self.assertEqual(partial, [], "a surviving trace must be causally complete")
        self.assertGreater(st.dropped_traces, 0)

    def test_a_trace_holding_an_error_or_a_stuck_span_is_kept(self):
        # spec 2.5: bias retention toward the abnormal; those are the traces
        # someone came looking for
        st = self._store()
        st.begin({"span_id": "E1", "trace_id": "failed", "name": "boom",
                  "kind": "call", "start": 0.0})
        st.finish("E1", 1.0, "error")
        st.begin({"span_id": "H1", "trace_id": "hung", "name": "never_returns",
                  "kind": "wait", "start": 0.0})
        for i in range(st.MAX_PENDING + 3000):
            st.begin({"span_id": f"s{i}", "trace_id": "ord%d" % (i // 4),
                      "name": "w", "kind": "call", "start": 1.0 + i})
            st.finish(f"s{i}", 2.0 + i, "ok")
        self.assertIn("E1", st._spans, "a failed trace outlives ordinary ones")
        self.assertIn("H1", st._spans, "so does one still running")
        self.assertEqual(st.dropped_abnormal, 0)

    def test_a_loss_is_counted_rather_than_hidden(self):
        # a capture missing rows is a fact about the record; dropping silently
        # makes every count taken from it quietly wrong
        st = self._store()
        for i in range(st.MAX_PENDING + 40):
            st.begin({"span_id": f"s{i}", "trace_id": "trace%d" % (i // 4),
                      "name": "w", "kind": "call", "start": float(i)})
            st.finish(f"s{i}", float(i) + 1, "ok")
        self.assertGreaterEqual(st.dropped, 40)
        self.assertGreater(st.dropped_traces, 0)
        # whole traces, so the loss is a multiple of the trace size
        self.assertEqual(st.dropped % 4, 0)

    def test_one_trace_larger_than_the_buffer_is_dropped_and_named(self):
        # holding it anyway is the unbounded growth the bound exists to prevent,
        # and an operator needs to know the buffer is too small for this trace
        st = self._store()
        for i in range(st.MAX_PENDING + 100):
            st.begin({"span_id": f"s{i}", "trace_id": "one-huge-trace",
                      "name": "w", "kind": "call", "start": float(i)})
            st.finish(f"s{i}", float(i) + 1, "ok")
        self.assertLessEqual(len(st._spans), st.MAX_PENDING)
        self.assertEqual(st.oversized_traces, 1)



class SenderHearsTheCollector(unittest.TestCase):
    """The collector explains every refusal; an adapter that discards the reply
    leaves its author with nothing to fix."""

    def _store(self):
        import push_store
        return push_store.PushStore('http://127.0.0.1:9', 'test', 999)

    def test_a_refusal_is_reported_once_not_once_per_batch(self):
        # a bug in an adapter repeats every second, and a message that repeats
        # with it is one nobody reads
        import io, sys as _sys
        st = self._store()
        captured = io.StringIO()
        real, _sys.stderr = _sys.stderr, captured
        try:
            reply = json.dumps({"ok": False, "rejected": 1,
                                "reasons": ["a1: name is empty, and a nameless span cannot be read"]})
            st._read_reply(reply.encode())
            reply2 = json.dumps({"ok": False, "rejected": 1,
                                 "reasons": ["a2: name is empty, and a nameless span cannot be read"]})
            st._read_reply(reply2.encode())
        finally:
            _sys.stderr = real
        self.assertEqual(captured.getvalue().count("refused a span"), 1,
                         "the same fault must not be reported twice")
        self.assertEqual(st.refused_by_collector, 2, "but the count keeps accruing")

    def test_a_clean_reply_says_nothing(self):
        import io, sys as _sys
        st = self._store()
        captured = io.StringIO()
        real, _sys.stderr = _sys.stderr, captured
        try:
            st._read_reply(json.dumps({"ok": True, "ingested": 5, "rejected": 0}).encode())
        finally:
            _sys.stderr = real
        self.assertEqual(captured.getvalue(), "")

    def test_an_unreadable_reply_never_breaks_the_sender(self):
        st = self._store()
        st._read_reply(b"not json at all")      # must not raise
        st._read_reply(b"")
        self.assertEqual(st.refused_by_collector, 0)


if __name__ == '__main__':
    unittest.main(verbosity=2)
