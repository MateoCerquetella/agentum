"""Tests for graceful session restart: name validation, PID helpers, stop/start logic.

Tests replicate the relevant functions from amux-server.py and verify
behavior in isolation — no tmux or running Claude required.
"""

import json
import re
import tempfile
import threading
from pathlib import Path


# ── Replicated helpers from amux-server.py ────────────────────────────────

_VALID_SESSION_NAME_RE = re.compile(r'^[a-zA-Z0-9_.\-]+$')
_VALID_CC_SESSION_NAME = re.compile(r'^[a-zA-Z0-9][a-zA-Z0-9_.\-]*$')


def _validate_cc_session_name(name: str) -> bool:
    return bool(name and len(name) <= 64 and _VALID_CC_SESSION_NAME.match(name))


def _read_claude_session_name(claude_pid: int, sessions_dir: Path | None = None) -> str:
    if claude_pid <= 1:
        return ""
    if sessions_dir is None:
        sessions_dir = Path.home() / ".claude" / "sessions"
    sf = sessions_dir / f"{claude_pid}.json"
    try:
        if not sf.is_file() or sf.stat().st_size > 1_000_000:
            return ""
        data = json.loads(sf.read_text(errors="replace"))
        return data.get("name", "")
    except Exception:
        return ""


_session_locks: dict = {}
_session_locks_init = threading.Lock()


def _get_session_lock(name: str) -> threading.RLock:
    with _session_locks_init:
        if name not in _session_locks:
            _session_locks[name] = threading.RLock()
        return _session_locks[name]


# ── Session name validation ──────────────────────────────────────────────

class TestSessionNameValidation:
    def test_valid_alphanumeric(self):
        assert _validate_cc_session_name("scripts-2")

    def test_valid_with_dots(self):
        assert _validate_cc_session_name("my.session.name")

    def test_valid_with_underscores(self):
        assert _validate_cc_session_name("college_advisor_1")

    def test_valid_single_char(self):
        assert _validate_cc_session_name("a")

    def test_rejects_empty(self):
        assert not _validate_cc_session_name("")

    def test_rejects_leading_dash(self):
        assert not _validate_cc_session_name("-rf")

    def test_rejects_leading_dot(self):
        assert not _validate_cc_session_name(".hidden")

    def test_rejects_spaces(self):
        assert not _validate_cc_session_name("my session")

    def test_rejects_semicolon(self):
        assert not _validate_cc_session_name("test;rm -rf /")

    def test_rejects_backtick(self):
        assert not _validate_cc_session_name("test`whoami`")

    def test_rejects_dollar(self):
        assert not _validate_cc_session_name("test$(id)")

    def test_rejects_newline(self):
        assert not _validate_cc_session_name("test\nmalicious")

    def test_rejects_pipe(self):
        assert not _validate_cc_session_name("test|cat /etc/passwd")

    def test_rejects_too_long(self):
        assert not _validate_cc_session_name("a" * 65)

    def test_accepts_max_length(self):
        assert _validate_cc_session_name("a" * 64)

    def test_rejects_html_xss(self):
        assert not _validate_cc_session_name("<script>alert(1)</script>")

    def test_rejects_quotes(self):
        assert not _validate_cc_session_name('test"injection')

    def test_amux_name_regex_compat(self):
        """Amux session names should also pass CC validation."""
        for name in ["scripts-1", "blog", "college-advisor-1", "scripts_test.2"]:
            assert _VALID_SESSION_NAME_RE.match(name), f"{name} should match amux regex"
            assert _validate_cc_session_name(name), f"{name} should pass CC validation"


# ── PID file reading ────────────────────────────────────────────────────

class TestReadClaudeSessionName:
    def test_reads_name_from_pid_file(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            d = Path(tmpdir)
            (d / "12345.json").write_text(json.dumps({
                "pid": 12345,
                "sessionId": "abc-123",
                "name": "vault-stuff",
                "cwd": "/Users/test"
            }))
            assert _read_claude_session_name(12345, d) == "vault-stuff"

    def test_returns_empty_for_missing_file(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            assert _read_claude_session_name(99999, Path(tmpdir)) == ""

    def test_returns_empty_for_no_name(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            d = Path(tmpdir)
            (d / "12345.json").write_text(json.dumps({
                "pid": 12345,
                "sessionId": "abc-123",
                "cwd": "/Users/test"
            }))
            assert _read_claude_session_name(12345, d) == ""

    def test_returns_empty_for_empty_name(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            d = Path(tmpdir)
            (d / "12345.json").write_text(json.dumps({
                "pid": 12345,
                "name": "",
            }))
            assert _read_claude_session_name(12345, d) == ""

    def test_returns_empty_for_invalid_pid(self):
        assert _read_claude_session_name(0) == ""
        assert _read_claude_session_name(-1) == ""
        assert _read_claude_session_name(1) == ""

    def test_handles_corrupted_json(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            d = Path(tmpdir)
            (d / "12345.json").write_text("{broken json")
            assert _read_claude_session_name(12345, d) == ""

    def test_rejects_oversized_file(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            d = Path(tmpdir)
            f = d / "12345.json"
            f.write_text("x" * 1_100_000)
            assert _read_claude_session_name(12345, d) == ""


# ── Session locks ────────────────────────────────────────────────────────

class TestSessionLocks:
    def test_returns_same_lock_for_same_name(self):
        lock1 = _get_session_lock("test-lock-a")
        lock2 = _get_session_lock("test-lock-a")
        assert lock1 is lock2

    def test_returns_different_locks_for_different_names(self):
        lock1 = _get_session_lock("test-lock-b")
        lock2 = _get_session_lock("test-lock-c")
        assert lock1 is not lock2

    def test_lock_is_reentrant(self):
        lock = _get_session_lock("test-lock-reentrant")
        with lock:
            with lock:
                pass  # should not deadlock

    def test_concurrent_access(self):
        results = []
        lock = _get_session_lock("test-lock-concurrent")

        def worker(val):
            with lock:
                results.append(val)

        threads = [threading.Thread(target=worker, args=(i,)) for i in range(10)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        assert sorted(results) == list(range(10))


# ── UUID validation (used in migration path) ────────────────────────────

class TestUUIDValidation:
    _uuid_re = re.compile(r'^[0-9a-fA-F-]{36}$')

    def test_valid_uuid(self):
        assert self._uuid_re.match("f5dd568b-77b7-489b-9cc5-fc2ec98710d6")

    def test_rejects_short(self):
        assert not self._uuid_re.match("f5dd568b-77b7")

    def test_rejects_semicolons(self):
        assert not self._uuid_re.match("f5dd568b;rm -rf /;fc2ec98710d6")

    def test_rejects_spaces(self):
        assert not self._uuid_re.match("f5dd568b 77b7 489b 9cc5 fc2ec98710d6")


# ── Resume command construction ──────────────────────────────────────────

class TestResumeCommandConstruction:
    """Verify the session flag logic from start_session."""

    def _build_session_flag(self, cc_session_name="", cc_conversation_id="", amux_name="test"):
        import shlex
        _uuid_re = re.compile(r'^[0-9a-fA-F-]{36}$')
        if cc_session_name and _validate_cc_session_name(cc_session_name):
            return f'--resume {shlex.quote(cc_session_name)}'
        elif cc_conversation_id and _uuid_re.match(cc_conversation_id):
            return f"--resume {cc_conversation_id}"
        else:
            return f'--name {shlex.quote(amux_name)}'

    def test_resume_by_name(self):
        flag = self._build_session_flag(cc_session_name="vault-stuff")
        assert flag == "--resume vault-stuff"

    def test_resume_by_name_quoted(self):
        flag = self._build_session_flag(cc_session_name="my-session.v2")
        assert flag == "--resume my-session.v2"

    def test_migration_uuid(self):
        flag = self._build_session_flag(
            cc_conversation_id="f5dd568b-77b7-489b-9cc5-fc2ec98710d6"
        )
        assert flag == "--resume f5dd568b-77b7-489b-9cc5-fc2ec98710d6"

    def test_fresh_start(self):
        flag = self._build_session_flag(amux_name="scripts-2")
        assert flag == "--name scripts-2"

    def test_invalid_name_falls_through(self):
        flag = self._build_session_flag(
            cc_session_name="-rf",
            amux_name="scripts-2"
        )
        assert flag == "--name scripts-2"

    def test_invalid_uuid_falls_through(self):
        flag = self._build_session_flag(
            cc_conversation_id="not-a-uuid",
            amux_name="scripts-2"
        )
        assert flag == "--name scripts-2"

    def test_name_preferred_over_uuid(self):
        flag = self._build_session_flag(
            cc_session_name="my-session",
            cc_conversation_id="f5dd568b-77b7-489b-9cc5-fc2ec98710d6"
        )
        assert flag == "--resume my-session"
