# Plan: SSH Remote Session Reliability

## 1. Correct the executor launch contract

- Add MCP environment generation to `ToolAdapter`.
- Emit Codex `bearer_token_env_var` overrides with stable per-server names.
- Keep token values out of Codex argv and add focused regression tests.
- Add host-supplied remote launch choices for terminal shell and Claude resume.

Evidence: `cargo test -p agentum-executor` and assertions that a sentinel token is
absent from every argv element.

## 2. Harden the canonical SSH transport

- Add no-TTY machine execution and keyboard-interactive password support.
- Kill/reap timed-out SSH children.
- Make missing-session classification narrow and typed.
- Expand stale-ControlMaster detection and add bounded master-exit helpers.
- Add command-construction and fake-SSH timeout regressions.

Evidence: focused `agentum-tmux` SSH tests.

## 3. Build the remote launch transaction

- Resolve remote home/workdir and selected executable before mutation.
- Detect remote Claude transcript existence and select remote terminal shell.
- Serialize validated launch environment and feed it over SSH stdin to a private
  per-session file.
- In one remote script: create dormant pane, enable `remain-on-exit`, create a
  private log, arm pipe, and respawn the real wrapped command.
- Inspect early liveness; capture/redact a dead pane and clean partial state.
- Remove reliance on tmux `new-session -e` and enforce 0700/0600 remote modes.
- Bound and serialize reverse-tunnel control operations.

Evidence: host-runtime pure/script tests, early-exit tests, and focused server
library tests.

## 4. Route every lifecycle through the saved host

- Factor the HTTP start body into a reusable host-aware server operation.
- Boot the embedded server before resume and resume only idle rows through that
  operation; remove use of local-only `commands::up` for boot resume.
- Preserve the owning profile/host in the plain-terminal shortcut.
- Invalidate old interactive/streaming masters after host edit/removal.
- Map prerequisite failures and remote runtime failures to actionable HTTP
  responses.

Evidence: mixed local/SSH resume tests, terminal routing tests, host invalidation
tests, and start-route error mapping tests.

## 5. Make SSH failures readable

- Decode `{"error":"..."}` API bodies before storing an error.
- Wrap each error entry into visual lines at the overlay width.
- Treat error scroll as visual-row scroll and retain all existing controls.
- Cover narrow width, explicit newlines, long tokens, ordering, and end-scroll.

Evidence: focused TUI API/UI tests and a rendered buffer snapshot/assertion.

## 6. Verify the complete workflow

- Run `cargo fmt --all -- --check`.
- Run focused crate suites, `cargo check --workspace --all-targets`, then
  `cargo test --workspace`; classify any pre-existing unrelated fixture failure.
- Run an independent review against all acceptance criteria and fix findings.
- Run credential-redacted live saved-host checks: accepted Codex MCP env config,
  remote start/settle, pane/log stream, input, stop, and cleanup.
- Confirm no sentinel secret appears in argv, tmux pane command, logs, or errors.
- Mark acceptance evidence in the specification and complete the Empirical
  verify/review/archive steps.
