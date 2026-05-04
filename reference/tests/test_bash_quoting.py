"""Verify the bash quote_flags / quote_argv helpers produce correct output.

Tests the functions as defined in `amux` (the bash CLI), not a copy. Uses
plain `source amux` — enabled by the source-guard at the bottom of the
amux script that skips CLI dispatch when sourced.

The bash functions delegate to python3's shlex, so semantically these
tests should mirror test_shell_quote_flags.py exactly. The parity test
imports the REAL python helper via importlib (no copy-pasted logic) so
drift is impossible.
"""

import importlib.util
import shlex
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).parent.parent
AMUX_SCRIPT = REPO_ROOT / "amux"
SERVER_PATH = REPO_ROOT / "amux-server.py"


@pytest.fixture(scope="module")
def amux_server():
    """Load amux-server.py via importlib so we can call the real
    _shell_quote_flags for parity comparisons. The __main__ guard at the
    bottom of the file prevents the HTTP server from starting on import."""
    spec = importlib.util.spec_from_file_location("amux_server", SERVER_PATH)
    assert spec is not None and spec.loader is not None, f"could not load {SERVER_PATH}"
    mod = importlib.util.module_from_spec(spec)
    sys.modules["amux_server"] = mod
    spec.loader.exec_module(mod)
    return mod


def quote_flags(s: str) -> str:
    """Invoke quote_flags by sourcing the live amux script in a subshell."""
    if not AMUX_SCRIPT.exists():
        pytest.skip(f"amux script not found at {AMUX_SCRIPT}")
    result = subprocess.run(
        ["bash", "-c", 'source "$1" && quote_flags "$2"', "_", str(AMUX_SCRIPT), s],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"quote_flags failed (exit {result.returncode}): {result.stderr}"
        )
    return result.stdout.rstrip("\n")


def quote_argv(*args: str) -> str:
    """Invoke quote_argv (multi-arg form) from the live amux script."""
    if not AMUX_SCRIPT.exists():
        pytest.skip(f"amux script not found at {AMUX_SCRIPT}")
    result = subprocess.run(
        ["bash", "-c", 'source "$1"; shift; quote_argv "$@"', "_", str(AMUX_SCRIPT), *args],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"quote_argv failed (exit {result.returncode}): {result.stderr}"
        )
    return result.stdout.rstrip("\n")


# ── quote_flags: regression case ─────────────────────────────────────────────

def test_one_megacontext_model_bash():
    assert quote_flags("--model claude-opus-4-6[1m]") == "--model 'claude-opus-4-6[1m]'"


def test_one_megacontext_in_full_flag_list_bash():
    s = "--model claude-opus-4-6[1m] --effort high --dangerously-skip-permissions"
    expected = "--model 'claude-opus-4-6[1m]' --effort high --dangerously-skip-permissions"
    assert quote_flags(s) == expected


# ── quote_flags: pass-through cases ──────────────────────────────────────────

def test_empty_string_bash():
    assert quote_flags("") == ""


def test_plain_flags_unchanged_bash():
    s = "--model opus --effort high"
    assert quote_flags(s) == s


# ── quote_flags: globbing and word-splitting hazards ────────────────────────

def test_no_globbing_bash(tmp_path, monkeypatch):
    """Bash's native word-splitting would expand '*' against the filesystem
    BEFORE quoting. Verify quote_flags does not."""
    (tmp_path / "file1").touch()
    (tmp_path / "file2").touch()
    (tmp_path / "file3").touch()
    monkeypatch.chdir(tmp_path)
    result = quote_flags("--model *")
    assert result == "--model '*'"


def test_multiword_quoted_flag_preserved_bash():
    """Bash's native word-splitting would break this into 3 tokens. shlex
    correctly parses the quoted multi-word value as a single token."""
    s = '--system-prompt "hello world"'
    result = quote_flags(s)
    assert shlex.split(result) == ["--system-prompt", "hello world"]


# ── quote_flags: metachar safety ────────────────────────────────────────────

def test_semicolon_does_not_execute_bash():
    result = quote_flags("--model evil;rm")
    assert shlex.split(result) == ["--model", "evil;rm"]


def test_dollar_sign_quoted_bash():
    result = quote_flags("--model $HOME")
    assert shlex.split(result) == ["--model", "$HOME"]


def test_backtick_quoted_bash():
    result = quote_flags("--model `whoami`")
    assert shlex.split(result) == ["--model", "`whoami`"]


# ── Source-guard regression ─────────────────────────────────────────────────

def test_source_does_not_run_dispatch():
    """Sourcing amux must not trigger any CLI dispatch behavior. The
    source-guard at the bottom of amux is what makes this test possible."""
    result = subprocess.run(
        ["bash", "-c", 'source "$1"; echo OK', "_", str(AMUX_SCRIPT)],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"source failed: {result.stderr}"
    assert result.stdout.strip() == "OK", f"unexpected output: {result.stdout!r}"


# ── Parity with python helper ────────────────────────────────────────────────

def test_parity_with_python_helper(amux_server):
    """Outputs of bash quote_flags must match the REAL python
    _shell_quote_flags for the same input. We import the production
    helper via importlib (not a copy) so divergence is impossible."""
    py_quote = amux_server._shell_quote_flags
    cases = [
        "--model opus",
        "--model claude-opus-4-6[1m]",
        "--model opus --effort high --yolo",
        "--model evil;rm",
        "--model $HOME",
        "--model `whoami`",
        "--model *",
        "",
    ]
    for s in cases:
        bash_result = quote_flags(s)
        py_result = py_quote(s)
        assert bash_result == py_result, f"parity broken for {s!r}: bash={bash_result!r} py={py_result!r}"


# ── quote_argv direct tests (multi-arg form) ────────────────────────────────

def test_quote_argv_empty():
    assert quote_argv() == ""


def test_quote_argv_single_plain():
    assert quote_argv("--yolo") == "--yolo"


def test_quote_argv_two_plain():
    assert quote_argv("--model", "opus") == "--model opus"


def test_quote_argv_bracket_in_value():
    """The regression case via the multi-arg form."""
    result = quote_argv("--model", "claude-opus-4-6[1m]")
    assert result == "--model 'claude-opus-4-6[1m]'"


def test_quote_argv_multiword_value():
    """User passed `--system-prompt "hello world"` to amux start."""
    result = quote_argv("--system-prompt", "hello world")
    assert shlex.split(result) == ["--system-prompt", "hello world"]


def test_quote_argv_metachar_value():
    """A semicolon in a value must be quoted, not interpreted."""
    result = quote_argv("--system-prompt", "wait; echo BOOM")
    assert shlex.split(result) == ["--system-prompt", "wait; echo BOOM"]


def test_quote_argv_dollar_var_in_value():
    result = quote_argv("--system-prompt", "$HOME mine")
    assert shlex.split(result) == ["--system-prompt", "$HOME mine"]


def test_quote_argv_glob_in_value():
    """A literal glob char in a value must not expand."""
    result = quote_argv("--model", "*")
    assert shlex.split(result) == ["--model", "*"]


def test_quote_argv_single_quote_in_value():
    """shlex.quote handles single quotes via the '\\''-escape pattern."""
    result = quote_argv("--system-prompt", "it's working")
    assert shlex.split(result) == ["--system-prompt", "it's working"]
