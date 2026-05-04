"""Unit tests for _shell_quote_flags — the spawn-side flag quoting helper.

This is the load-bearing fix for the spawn-time shell-injection bug where
zsh interprets '[1m]' in model IDs like 'claude-opus-4-6[1m]' as a glob
character class and aborts the entire spawn command with 'no matches found'.

We import the helper from amux-server.py via importlib rather than copying
it. The server's __main__ guard prevents the HTTP server from actually
starting during import. No drift is possible.
"""

import importlib.util
import shlex
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).parent.parent
SERVER_PATH = REPO_ROOT / "amux-server.py"


@pytest.fixture(scope="module")
def amux_server():
    """Load amux-server.py as a module without running its HTTP server.

    The server file has `if __name__ == "__main__":` at module bottom that
    contains the actual server startup; importlib sets __name__ to the spec
    name ('amux_server') so the guard skips startup.
    """
    spec = importlib.util.spec_from_file_location("amux_server", SERVER_PATH)
    assert spec is not None and spec.loader is not None, f"could not load {SERVER_PATH}"
    mod = importlib.util.module_from_spec(spec)
    sys.modules["amux_server"] = mod
    spec.loader.exec_module(mod)
    return mod


@pytest.fixture
def quote_flags(amux_server):
    """Return the production _shell_quote_flags function."""
    return amux_server._shell_quote_flags


@pytest.fixture
def model_id_re(amux_server):
    """Return the production _MODEL_ID_RE compiled regex."""
    return amux_server._MODEL_ID_RE


# ── The regression case ──────────────────────────────────────────────────────

def test_one_megacontext_model_gets_quoted(quote_flags):
    """The exact case from PR #18 that broke session spawn."""
    result = quote_flags("--model claude-opus-4-6[1m]")
    assert result == "--model 'claude-opus-4-6[1m]'"


def test_one_megacontext_in_full_flag_list(quote_flags):
    """Realistic CC_FLAGS string with [1m] model alongside other flags."""
    s = "--model claude-opus-4-6[1m] --effort high --dangerously-skip-permissions"
    result = quote_flags(s)
    assert result == "--model 'claude-opus-4-6[1m]' --effort high --dangerously-skip-permissions"


# ── Pass-through cases ───────────────────────────────────────────────────────

def test_empty_string(quote_flags):
    assert quote_flags("") == ""


def test_plain_flags_unchanged(quote_flags):
    """Flags with no shell metacharacters round-trip identically."""
    s = "--model opus --effort high"
    assert quote_flags(s) == s


def test_single_flag(quote_flags):
    assert quote_flags("--yolo") == "--yolo"


def test_short_model_names_unchanged(quote_flags):
    for name in ["opus", "sonnet", "haiku", "claude-opus-4-6", "claude-sonnet-4-6"]:
        s = f"--model {name}"
        assert quote_flags(s) == s, f"unchanged expected for {name}"


# ── Other shell metacharacters that would also break ────────────────────────

def test_semicolon_does_not_execute(quote_flags):
    """Shell command separators get quoted, not executed."""
    result = quote_flags("--model evil;rm")
    assert shlex.split(result) == ["--model", "evil;rm"]


def test_dollar_sign_quoted(quote_flags):
    result = quote_flags("--model $HOME")
    assert shlex.split(result) == ["--model", "$HOME"]


def test_backtick_quoted(quote_flags):
    result = quote_flags("--model `whoami`")
    assert shlex.split(result) == ["--model", "`whoami`"]


def test_pipe_quoted(quote_flags):
    result = quote_flags("--model a|b")
    assert shlex.split(result) == ["--model", "a|b"]


def test_glob_star_quoted(quote_flags):
    """The glob '*' must not expand at spawn time."""
    result = quote_flags("--model *")
    assert shlex.split(result) == ["--model", "*"]


def test_newline_in_value_quoted(quote_flags):
    """A newline inside a stored flag value must be preserved as part of
    the same token, not split into multiple. shlex.quote handles this
    correctly with literal-newline-inside-single-quotes."""
    s = "--system-prompt 'first\nsecond'"
    result = quote_flags(s)
    assert shlex.split(result) == ["--system-prompt", "first\nsecond"]


# ── Round-trip property ──────────────────────────────────────────────────────

def test_round_trip_preserves_tokens(quote_flags):
    """For any input, shlex.split(quote_flags(input)) == shlex.split(input)."""
    cases = [
        "--model opus",
        "--model claude-opus-4-6[1m]",
        "--model opus --effort high --yolo",
        "--system-prompt hello",
        "",
    ]
    for s in cases:
        original_tokens = shlex.split(s) if s else []
        quoted = quote_flags(s)
        roundtrip_tokens = shlex.split(quoted) if quoted else []
        assert roundtrip_tokens == original_tokens, f"round-trip failed for {s!r}"


# ── Malformed input fallback (no longer raw passthrough) ────────────────────

def test_unbalanced_quote_quoted_as_literal(quote_flags):
    """Malformed input (unbalanced quote) gets quoted as a single literal
    token rather than passed through raw — preserving the security invariant
    that the shell never interprets stored data."""
    s = '--model "unbalanced'
    result = quote_flags(s)
    assert result == shlex.quote(s)
    reparsed = shlex.split(result)
    assert reparsed == [s], f"expected [{s!r}], got {reparsed}"


def test_malformed_with_metachars_still_safe(quote_flags):
    """Even with shell metacharacters in malformed input, the result is safe."""
    s = '--model "evil; rm -rf /'
    result = quote_flags(s)
    reparsed = shlex.split(result)
    assert reparsed == [s], "malformed-with-metachars must round-trip as single token"


# ── Defense-in-depth: PATCH endpoint validator ──────────────────────────────

def test_validator_accepts_known_anthropic_models(model_id_re):
    for m in ["opus", "sonnet", "haiku",
              "claude-opus-4-6", "claude-opus-4-6[1m]",
              "claude-sonnet-4-6", "claude-sonnet-4-6[1m]",
              "claude-haiku-4-5-20251001"]:
        assert model_id_re.match(m), f"{m} should be accepted"


def test_validator_accepts_third_party_models(model_id_re):
    """Bedrock, Vertex, OpenRouter, HuggingFace use slashes, at-signs, plus."""
    for m in ["anthropic/claude-3-opus",
              "claude-3-opus@20240229",
              "anthropic.claude-3-sonnet-20240229-v1:0",
              "openrouter/anthropic/claude-3.5-sonnet",
              "meta-llama/Llama-3+chat"]:
        assert model_id_re.match(m), f"{m} should be accepted"


def test_validator_rejects_metachars(model_id_re):
    for bad in ["opus; rm", "opus|cat", "opus$VAR", "opus`x`",
                "opus>file", "opus<file", "opus&", "opus(x)",
                "opus{x}", "opus x", "opus\nhostile", "opus\\evil",
                "opus'quote", 'opus"quote']:
        assert not model_id_re.match(bad), f"{bad} should be rejected"


def test_validator_rejects_leading_hyphen(model_id_re):
    """A model name starting with '-' would be reparsed as a flag by claude
    after shell-quoting, enabling application-level argument injection."""
    for bad in ["-p", "-malicious", "--api-key", "-injection",
                "-rm", "-yolo", "--dangerously-skip-permissions"]:
        assert not model_id_re.match(bad), f"{bad} should be rejected (leading hyphen)"


def test_validator_accepts_local_paths(model_id_re):
    """Leading '.', '/', and other harmless characters are allowed for
    users driving non-Anthropic LLM CLIs through amux that accept local
    model paths."""
    for ok in ["./model", "/opt/local-model", "/home/me/llama-2-7b",
               "[bracketed]", "@scoped", ".hidden"]:
        assert model_id_re.match(ok), f"{ok} should be accepted (harmless leading char)"


def test_validator_rejects_empty(model_id_re):
    assert not model_id_re.match("")


@pytest.fixture
def validate_model_name(amux_server):
    """Return the production _validate_model_name helper."""
    return amux_server._validate_model_name


def test_validate_model_name_accepts_known_models(validate_model_name):
    for m in ["opus", "claude-opus-4-6", "claude-opus-4-6[1m]",
              "anthropic/claude-3-opus"]:
        ok, normalized, err = validate_model_name(m)
        assert ok, f"{m} should be accepted: {err}"
        assert normalized == m
        assert err == ""


def test_validate_model_name_strips_whitespace(validate_model_name):
    ok, normalized, _ = validate_model_name("  opus  ")
    assert ok
    assert normalized == "opus"


def test_validate_model_name_allows_empty(validate_model_name):
    """Empty string is valid at this level — caller decides what empty means."""
    ok, normalized, _err = validate_model_name("")
    assert ok
    assert normalized == ""


def test_validate_model_name_rejects_non_string(validate_model_name):
    for bad in [None, 42, ["opus"], {"model": "opus"}, True]:
        ok, _, err = validate_model_name(bad)
        assert not ok, f"{bad!r} should be rejected"
        assert "string" in err.lower()


def test_validate_model_name_rejects_too_long(validate_model_name):
    """255-char limit prevents DoS via huge JSON payloads."""
    ok, _, err = validate_model_name("a" * 256)
    assert not ok
    assert "too long" in err


def test_validate_model_name_rejects_metachars(validate_model_name):
    for bad in ["opus; rm", "opus|cat", "opus$VAR", "-injection"]:
        ok, _, err = validate_model_name(bad)
        assert not ok, f"{bad} should be rejected"
        assert "invalid" in err


def test_validator_does_not_choke_on_long_input(model_id_re):
    """The regex itself is bounded — no catastrophic backtracking on long
    inputs. The PATCH endpoint enforces a 255-char length limit before
    calling this regex, but verify it would still match a long valid name
    quickly (no ReDoS)."""
    long_valid = "a" * 250
    assert model_id_re.match(long_valid)
    long_invalid = "a" * 200 + " " + "b" * 50
    assert not model_id_re.match(long_invalid)


# ── Regression: PATCH /api/settings/default-model preserves non-model flags ──

@pytest.fixture
def strip_model_from_flags(amux_server):
    """Return the production _strip_model_from_flags helper."""
    return amux_server._strip_model_from_flags


@pytest.fixture
def extract_model_from_flags(amux_server):
    """Return the production _extract_model_from_flags helper."""
    return amux_server._extract_model_from_flags


def test_strip_model_basic(strip_model_from_flags):
    """The original PR #18 data-loss bug: changing the default model wiped
    --max-tokens. The strip helper must remove only --model X, not the rest."""
    assert strip_model_from_flags("--model opus --max-tokens 4000") == "--max-tokens 4000"


def test_strip_model_preserves_multiple_other_flags(strip_model_from_flags):
    assert strip_model_from_flags(
        "--model sonnet --max-tokens 4000 --effort high"
    ) == "--max-tokens 4000 --effort high"


def test_strip_model_handles_equals_form(strip_model_from_flags):
    """The --model=X form must also be stripped, not just --model X."""
    assert strip_model_from_flags(
        "--model=opus --max-tokens 4000"
    ) == "--max-tokens 4000"


def test_strip_model_handles_quoted_bracketed(strip_model_from_flags):
    """A quoted bracketed model name (round-trip from .env) must be stripped
    correctly without breaking the rest of the flags."""
    assert strip_model_from_flags(
        "--model 'claude-opus-4-6[1m]' --effort high"
    ) == "--effort high"


def test_strip_model_no_model_present(strip_model_from_flags):
    """Flags with no --model are returned unchanged (modulo shlex re-quoting)."""
    assert strip_model_from_flags("--max-tokens 4000") == "--max-tokens 4000"


def test_strip_model_empty(strip_model_from_flags):
    assert strip_model_from_flags("") == ""


def test_strip_model_only(strip_model_from_flags):
    """Just a --model flag with no other flags returns empty string."""
    assert strip_model_from_flags("--model opus") == ""


def test_strip_model_malformed_raises(strip_model_from_flags):
    """Malformed input (unbalanced quotes) MUST raise ValueError so the
    PATCH endpoint can return 400 to the user. Silently returning empty
    would wipe all the user's other flags during a routine model update,
    which is a data-loss bug. The PATCH endpoints catch this exception
    and surface a clear error message."""
    with pytest.raises(ValueError):
        strip_model_from_flags('--model "unbalanced')


def test_extract_model_basic(extract_model_from_flags):
    assert extract_model_from_flags("--model opus") == "opus"


def test_extract_model_equals_form(extract_model_from_flags):
    assert extract_model_from_flags("--model=opus") == "opus"


def test_extract_model_bracketed(extract_model_from_flags):
    assert extract_model_from_flags(
        "--model claude-opus-4-6[1m] --effort high"
    ) == "claude-opus-4-6[1m]"


def test_extract_model_absent(extract_model_from_flags):
    assert extract_model_from_flags("--max-tokens 4000") == ""


def test_extract_model_empty(extract_model_from_flags):
    assert extract_model_from_flags("") == ""
