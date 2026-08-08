# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: redact.py
#  script_path: execviz/redact.py
#  module_name: redact
#  version: 0.53.1
#  description: Keys catch what a program named accurately.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: re
#  features: redact
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

"""Redaction. Runs at capture, before anything is written down.

Redacting at display leaves the secret in the file; redacting at export leaves it
in the local store. The only place that removes a secret is reported before it
is recorded.
"""
import re

# Keys catch what a program named accurately.
SECRET_KEYS = {
    'password', 'passwd', 'pwd', 'secret', 'token', 'api_key', 'apikey',
    'authorization', 'auth', 'cookie', 'session', 'credential', 'credentials',
    'private_key', 'privatekey', 'access_key', 'secret_key', 'refresh_token',
    'ssn', 'social_security', 'card', 'card_number', 'cvv', 'pin',
}

# Patterns catch what it did not.
PATTERNS = [
    ('bearer token', re.compile(r'\bBearer\s+[A-Za-z0-9._\-]{12,}', re.I)),
    ('jwt', re.compile(r'\beyJ[A-Za-z0-9._\-]{20,}')),
    ('aws key', re.compile(r'\b(AKIA|ASIA)[0-9A-Z]{16}\b')),
    ('private key block', re.compile(r'-----BEGIN [A-Z ]*PRIVATE KEY-----')),
    ('card number', re.compile(r'\b(?:\d[ -]*?){13,19}\b')),
    ('email', re.compile(r'\b[\w.+-]+@[\w-]+\.[\w.-]+\b')),
    ('long hex secret', re.compile(r'\b[0-9a-f]{32,}\b', re.I)),
]

MARK = '<redacted:%s>'


class Policy:
    """Rules for what never gets written down.

    Fails closed: if a rule cannot be evaluated, the value is withheld rather
    than recorded. Over-redacting costs a missing string; under-redacting costs a
    credential in a file that gets copied between machines.
    """

    def __init__(self, keys=None, patterns=None, enabled=True, max_len=200):
        self.keys = {k.lower() for k in (keys if keys is not None else SECRET_KEYS)}
        self.patterns = patterns if patterns is not None else PATTERNS
        self.enabled = enabled
        self.max_len = max_len

    def key_is_secret(self, key):
        k = str(key).lower()
        return any(s in k for s in self.keys)

    def _scrub_member(self, key, value, depth, seen):
        """Scrubs a value nested inside a container, carrying the depth down.

        Without this a nested member restarted the depth count, so a cycle one
        level in was still unbounded.
        """
        if not self.enabled:
            return self.render(value, depth, seen)
        try:
            if key is not None and self.key_is_secret(key):
                return MARK % 'key'
            text = self.render(value, depth, seen)
            for why, rx in self.patterns:
                if rx.search(text):
                    return MARK % why.replace(' ', '_')
            return text
        except Exception:
            return MARK % 'unevaluable'

    def scrub(self, key, value):
        """Returns (rendered_value, redacted_reason_or_None)."""
        if not self.enabled:
            return self.render(value), None
        try:
            if key is not None and self.key_is_secret(key):
                return MARK % 'key', 'key'
            text = self.render(value)
            for why, rx in self.patterns:
                if rx.search(text):
                    return MARK % why.replace(' ', '_'), why
            return text, None
        except Exception:
            # fail closed: a rule that cannot be evaluated withholds the value
            return MARK % 'unevaluable', 'unevaluable'

    #: How deep a recorded value is followed.
    #:
    #: A recorded value is not guaranteed to be a finite tree. A cycle used to be
    #: absorbed by the fail-closed handler and reported as
    #: ``<redacted:unevaluable>``; which reads as "a secret was withheld here"
    #: and is  untrue. Depth is bounded explicitly so the reason is
    #: accurate.
    MAX_DEPTH = 6

    def render(self, value, _depth=0, _seen=None):
        """A short textual rendering taken now, never a reference to the object.

        An object that mutates after the call must not silently rewrite history,
        so the string is produced at the moment of capture.
        """
        if _depth > self.MAX_DEPTH:
            return '<too deep>'
        # identity, not equality: two equal dicts are not a cycle, the same dict
        # appearing inside itself is
        if _seen is None:
            _seen = set()
        if isinstance(value, (dict, list, tuple, set)):
            if id(value) in _seen:
                return '<cycle>'
            _seen = _seen | {id(value)}
        try:
            if isinstance(value, (str, bytes)):
                text = value.decode('utf-8', 'replace') if isinstance(value, bytes) else value
            elif isinstance(value, (int, float, bool)) or value is None:
                text = repr(value)
            elif isinstance(value, (list, tuple, set)):
                # Render the elements, not the shape. "tuple(len=1)" answers
                # nothing: the question is which input caused this, and a
                # container's length is not an input.
                items = list(value)[:8]
                rendered = [self.render(v, _depth + 1, _seen) for v in items]
                more = '' if len(value) <= 8 else ', …%d more' % (len(value) - 8)
                text = '[%s%s]' % (', '.join(rendered), more)
            elif isinstance(value, dict):
                # each member is scrubbed by its own key, so a secret inside a
                # container is caught by the same rule as one at the top level
                parts = []
                for k in list(value)[:8]:
                    sub = (self._scrub_member(k, value[k], _depth + 1, _seen)
                           if self.enabled else self.render(value[k], _depth + 1, _seen))
                    parts.append('%s=%s' % (k, sub))
                more = '' if len(value) <= 8 else ', …%d more' % (len(value) - 8)
                text = '{%s%s}' % (', '.join(parts), more)
            else:
                text = repr(value)
        except Exception:
            text = '<unrenderable %s>' % type(value).__name__
        if len(text) > self.max_len:
            return text[:self.max_len] + '…<truncated>'
        return text


DEFAULT = Policy()
