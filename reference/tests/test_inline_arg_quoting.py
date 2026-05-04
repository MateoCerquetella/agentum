"""Verify shlex.quote-based inline-arg escaping survives nested-shell re-eval.

This is the v3/v4 fix for the bash CLI inline-argument bug. The amux:cmd_start
function builds a shell command string via:

    q=$(quote_argv "$@")
    cmd="$cmd $q"

Then passes the string to `tmux new-session ... "$cmd"` which is run by the
user's default shell (bash, zsh, dash, etc). This test verifies that
shlex.quote output, when re-evaluated by ANY POSIX shell, yields the
original argv.
"""

import shlex
import shutil
import subprocess

import pytest


def cmd_string_from_argv(argv: list[str]) -> str:
    """Replicate cmd_start's inline-arg quoting: shlex.quote each token."""
    parts = ["claude"]
    parts.extend(shlex.quote(a) for a in argv)
    return " ".join(parts)


def reparse_via_shell(cmd: str, shell: str) -> list[str]:
    """Pass cmd through a specific shell and recover the argv it parsed.

    We replace `claude` with `printf '%s\\0'` so each argument is
    null-terminated. Null-separation is required because some test cases
    (e.g. test_newline_in_value) use literal newlines inside argument
    values; using newline as the record separator would conflate content
    and structure."""
    test_cmd = cmd.replace("claude", "printf '%s\\0'", 1)
    result = subprocess.run(
        [shell, "-c", test_cmd],
        capture_output=True, check=True,  # bytes mode (no text=True)
    )
    if not result.stdout:
        return []
    # printf '%s\0' produces N nulls for N args, so split gives N+1 entries
    # with an empty trailing element. Drop it.
    parts = result.stdout.split(b"\x00")
    if parts and parts[-1] == b"":
        parts = parts[:-1]
    return [p.decode("utf-8") for p in parts]


# Discover available POSIX shells on this system. bash and sh are guaranteed.
# dash and zsh are tested if present so we catch portability regressions.
_SHELLS = [s for s in ["bash", "sh", "dash", "zsh"] if shutil.which(s)]


@pytest.mark.parametrize("shell", _SHELLS)
def test_quoted_multiword_preserved(shell):
    """--system-prompt 'hello world' must reach claude as 2 args, not 3."""
    argv = ["--system-prompt", "hello world"]
    cmd = cmd_string_from_argv(argv)
    parsed = reparse_via_shell(cmd, shell)
    assert parsed == argv, f"shell={shell}: {parsed!r}"


@pytest.mark.parametrize("shell", _SHELLS)
def test_glob_metachar_not_expanded(shell):
    """--model claude-opus-4-6[1m] must reach claude as the literal string,
    not trigger zsh's nomatch."""
    argv = ["--model", "claude-opus-4-6[1m]"]
    cmd = cmd_string_from_argv(argv)
    parsed = reparse_via_shell(cmd, shell)
    assert parsed == argv, f"shell={shell}: {parsed!r}"


@pytest.mark.parametrize("shell", _SHELLS)
def test_semicolon_not_executed(shell):
    """--system-prompt 'wait; echo BOOM' must NOT spawn an extra command."""
    argv = ["--system-prompt", "wait; echo BOOM"]
    cmd = cmd_string_from_argv(argv)
    parsed = reparse_via_shell(cmd, shell)
    assert parsed == argv, f"shell={shell}: {parsed!r}"


@pytest.mark.parametrize("shell", _SHELLS)
def test_dollar_var_not_expanded(shell):
    """--system-prompt '$HOME is mine' must NOT have $HOME expanded."""
    argv = ["--system-prompt", "$HOME is mine"]
    cmd = cmd_string_from_argv(argv)
    parsed = reparse_via_shell(cmd, shell)
    assert parsed == argv, f"shell={shell}: {parsed!r}"


@pytest.mark.parametrize("shell", _SHELLS)
def test_backtick_not_executed(shell):
    """--system-prompt 'who is `whoami`' must NOT execute whoami."""
    argv = ["--system-prompt", "who is `whoami`"]
    cmd = cmd_string_from_argv(argv)
    parsed = reparse_via_shell(cmd, shell)
    assert parsed == argv, f"shell={shell}: {parsed!r}"


@pytest.mark.parametrize("shell", _SHELLS)
def test_single_quote_in_value(shell):
    """A literal single quote in an arg must round-trip correctly. shlex.quote
    handles this with the '\\''-escape pattern."""
    argv = ["--system-prompt", "it's working"]
    cmd = cmd_string_from_argv(argv)
    parsed = reparse_via_shell(cmd, shell)
    assert parsed == argv, f"shell={shell}: {parsed!r}"


@pytest.mark.parametrize("shell", _SHELLS)
def test_newline_in_value(shell):
    """A newline inside an arg value must be preserved as part of the same
    argument, not treated as a statement separator. shlex.quote produces
    literal-newline-inside-single-quotes which all POSIX shells handle."""
    argv = ["--system-prompt", "first line\nsecond line", "--effort", "high"]
    cmd = cmd_string_from_argv(argv)
    parsed = reparse_via_shell(cmd, shell)
    assert parsed == argv, f"shell={shell}: {parsed!r}"
