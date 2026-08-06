# Decisions: Fix All Ssh Problems Preventing Connected Remote Hosts From Starting

Record concise, externally reviewable evidence and choices here. Do not store
private chain-of-thought, prompts, credentials, secrets, or scratchpad text.

## D-001: Select the implementation approach

Status: Accepted

### Evidence

- The saved host accepts SSH and has tmux 3.7b, git, Codex 0.146.0, and the
  requested workdir.
- Remote Codex argv contains `mcp_servers.agentum.bearer_token=...`; Codex
  rejects that key before its UI starts. Its CLI documents
  `bearer_token_env_var` instead.
- The detached process disappears before `pipe-pane`, so a secondary missing
  target error replaces the real Codex error in the returned 500.
- Auto-resume currently invokes a local-only launch helper for SSH-owned rows,
  and remote terminal sessions inherit the daemon Mac's shell path.

### Options

1. Disable authenticated Agentum MCP for remote Codex.
2. Embed an Authorization header/token directly in Codex argv.
3. Use Codex's supported bearer-token env reference, supply the secret only in
   pane env, and harden the full host-aware startup/resume transaction.

### Chosen approach

Choose option 3. Extend the launch contract so Codex emits a stable environment
variable reference and supplies its value through launch environment entries.
Route every lifecycle path through host-aware server logic and preserve the
first bounded, redacted startup diagnostic.

### Trade-offs and risks

- Environment values remain readable by the same remote Unix user, matching the
  existing askpass boundary, but stop leaking through argv.
- A bounded launch check adds small startup latency; keep it short and only on
  session start.
- Remote programs fail arbitrarily; cap and redact returned pane output.
- Cover local and non-Codex adapter behavior with regressions.

### Verification

- Assert Codex argv uses `bearer_token_env_var` and contains no literal token.
- Assert the referenced env var carries the token.
- Exercise fast exit, missing workdir/tool, incompatible tmux, SSH failure,
  remote auto-resume, and successful startup.
- Reproduce old/new Codex config shapes locally and on the saved host.
