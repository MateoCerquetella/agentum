# Continue Enhancing And Optimizing Agentum Especially The Ssh Backed Inte

## Request

> Continue enhancing and optimizing Agentum, especially the SSH-backed interactive session path after the recently integrated stall recovery and host-aware image paste fixes.
>
> Approved Socratic discovery:
> - Primary user and problem: The user operates SSH-backed Agentum sessions. Typing still has noticeable latency, output rendering stalls, and image paste is not working in the version currently being run. These failures make remote interactive sessions unreliable and slow for normal agent work.
> - Smallest observable outcome: For the full lifetime of every SSH-backed session, typing feels immediate, output continues rendering without requiring a keypress or reconnect, and image paste consistently uploads to the remote workdir and inserts a usable path into the exact pane.
> - Scope, non-goals, and constraints: Include the SSH-backed interactive input path, remote output stream/rendering path, and remote image-paste transaction. Preserve local-session behavior, exact saved-host/session/tmux identity, clipboard compatibility, security constraints, and existing public protocols. Avoid new dependencies, unrelated UI redesign, release publication, and broad architectural changes. Follow-up decision: Non-goals: redesigning the TUI, changing supported SSH authentication, modifying agent CLI behavior, adding dependencies, or publishing a release.
> - Failure cases and risks: Protect against stale saved-host revisions, reused tmux names, dead or saturated SSH masters, silent tail children, blocked input writers, duplicate or lost input, output gaps, unsafe upload paths, symlink/hardlink attacks, partial uploads, missing remote permissions, leaked image bytes, and success events emitted before remote completion. Recovery must be bounded, cancellation-aware, and must not disrupt healthy sessions sharing a host.
> - Required verification: This is a terminal/SSH path rather than a browser UI. Prove it with deterministic transport tests that simulate sustained typing, silent tail stalls, recovery, exact byte ordering, and host-aware binary uploads; focused server tests; the configured full workspace test command; code review; and independent-worktree integration. If a configured live SSH target is available, add credential-redacted latency and long-idle interaction evidence, but do not weaken deterministic gates when one is unavailable.

## Goal

Keep SSH-backed Agentum sessions continuously responsive: terminal input and
output must make progress without another user event, and image paste must use
the saved remote host from clipboard acquisition through pane injection.

## Acceptance Criteria

- [ ] [AC-1] Terminal WebSockets disable Nagle buffering and fairly service
  inbound pane output while outbound key events are continuously available.
- [ ] [AC-2] Interactive SSH commands avoid bulk-stream compression latency,
  while pane-output streams retain compression for throughput.
- [ ] [AC-3] Pane liveness/title probes use an SSH pool independent from both
  keystroke delivery and pane streaming, so a wedged stream remains observable
  and cannot stall input.
- [ ] [AC-4] A remote log that remains ahead of locally consumed output triggers
  bounded autonomous tail recovery without a keypress, reconnect, duplicate
  input, or disruption of healthy sessions on the same host.
- [ ] [AC-5] Ctrl-V image paste validates the saved remote tmux target, writes
  private binary bytes atomically under the remote workdir, and injects the
  returned relative path once with no Enter or local shadow file.
- [ ] [AC-6] Local sessions, clipboard request correlation, MIME mapping, public
  HTTP/WebSocket shapes, exact host/session/tmux identity, and upload failure
  ordering remain compatible.
- [ ] [AC-7] Deterministic tests cover pool isolation/cleanup, low-latency SSH
  and WebSocket settings, fair pump behavior, stalled-output recovery state,
  binary upload integrity, and local/remote routing invariants.
- [ ] [AC-8] The configured full workspace verification and independent review
  pass, and the newly built executable contains the completed fixes.

## Scope

- SSH pool roles and command options.
- Bidirectional terminal WebSocket scheduling.
- Remote pane liveness and recovery.
- Existing host-aware upload and Ctrl-V paths.
- Deterministic tests and local executable installation after verification.

## Non-goals

- TUI redesign, speculative local echo, or agent CLI changes.
- New SSH authentication methods, dependencies, or wire protocols.
- Release publication or source-control delivery.

## Verification

- Focused `agentum-tmux`, `agentum-server`, and `agentum-tui` tests.
- Configured full workspace package/test checkpoints.
- Independent code-review evidence and capability-delta integration.
- Compare the installed executable timestamp after a successful rebuild.

## Capability Deltas

- `deltas/remote-session-reliability.md`
