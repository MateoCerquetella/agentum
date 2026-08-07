# Tui Session Ux And Observability Fixes For The Agentum Terminal

## Request

> TUI session UX and observability fixes for the agentum terminal app (crates/agentum-tui): 1) In-session agent management: from inside a session view, let the user add more agents (spawn a new plain terminal pane or another agent such as claude/codex) and add keyboard shortcuts to move focus between sessions/panes and to resize panes. 2) Keybind change: make C the key for create (change the current create/new-session keybind). 3) Error copy: improve error messages so the full error can be pasted into an LLM chat for debugging — include operation, session id, daemon/host state, version, and log file location where relevant. 4) Bug: the per-session/per-agent usage readout (bottom-left usage panel, /api/usage/claude) is not working, and agent tasks are not working either. 5) Bug: the Plan / Todos / Agents right-side panels (agent-tasks panel backed by transcript_store and /api/sessions/{id}/agent-tasks) are not working — investigate why and fix.

## Goal

Make a running session a complete, diagnosable workspace: the user can add a
shell or another agent without abandoning the current session, move and resize
terminal panes from the keyboard, create sessions with the requested `C`
binding, copy a self-contained diagnostic report when an operation fails, and
trust the Usage and Plan / Todos / Agents panels to show current data or an
explicit actionable unavailable state.

## Acceptance Criteria

- [ ] [AC-1] [UI] With a running session focused, the user can invoke a
  discoverable add-pane action and choose either a plain terminal or an
  installed agent, including Claude and Codex. The new session inherits the
  active session's daemon, host, and working directory by default, is created
  through the normal session create/start path, and appears in an additional
  terminal pane without replacing or stopping the original session.
- [ ] [AC-2] [UI] Agent choices that are unavailable on the target host are
  visibly disabled or rejected before launch with an actionable diagnostic.
  If create succeeds but start fails, the created session remains visible in a
  recoverable non-running state and the error identifies that partial outcome.
- [ ] [AC-3] [UI] `F5` and `F6` move focus forward and backward through every
  visible focusable surface, including both terminal panes, and the focused
  pane is visually unambiguous. Closing a pane moves focus to a surviving
  pane. Keystrokes not reserved as global TUI shortcuts continue to reach the
  focused terminal unchanged.
- [ ] [AC-4] [UI] When two terminal panes are visible,
  `Ctrl-Shift-Left` and `Ctrl-Shift-Right` resize their boundary in bounded
  steps without collapsing either pane. The chosen ratio persists, both
  terminal streams receive their new dimensions, and using the shortcuts
  without a split produces a clear no-op message.
- [ ] [AC-5] [UI] Uppercase `C` opens the new-session/create-agent form from
  tree focus and all create hints and command-palette copy use `C`. The old
  `n` binding no longer opens the form. Lowercase `c` retains its existing
  bound-card behavior, and a literal `C` typed while a terminal has focus is
  forwarded to that terminal.
- [ ] [AC-6] [UI] Every user-visible operational failure recorded by the TUI
  has a full, non-truncated detail view and a discoverable copy action. The
  copied report contains the operation, original error chain, TUI version,
  daemon endpoint/reachability/version, and, when the operation is
  session-scoped, the session id plus target host identity/readiness. It also
  contains the resolved TUI or pane log path when one exists. Unknown values
  are labeled as unknown rather than omitted, and credentials/tokens are
  redacted.
- [ ] [AC-7] [UI] The bottom-left Usage panel performs an immediate
  authenticated fetch from the active daemon and refreshes at the configured
  interval. It shows account-wide Claude usage from `/api/usage/claude` plus
  per-tool and per-running-session metrics from session snapshots. Loading,
  live, stale, Claude-not-installed, OAuth-unavailable, daemon-unreachable,
  and incompatible-daemon states are distinguishable; a failed refresh keeps
  the last good snapshot, marks it stale, clears the in-flight gate, and
  exposes a copyable diagnostic instead of silently leaving the panel blank.
- [ ] [AC-8] The authenticated `/api/usage/claude` response is derived from
  the daemon host, tolerates absent or unreadable transcript/credential
  stores, never exposes OAuth credentials, and returns enough source and
  freshness metadata for the TUI to render the states in AC-7. Valid local
  transcript and OAuth fixtures produce non-empty token/window fields; an
  unavailable upstream degrades to an explicit local-scan result rather than
  failing the route.
- [ ] [AC-9] [UI] For a supported Claude session, the Plan / Todos / Agents
  panel populates on first selection without waiting for a later filesystem
  event, then converges after `ExitPlanMode`, `TodoWrite`, `TaskCreate`,
  `TaskUpdate`, task completion/deletion, and supported subagent-dispatch
  records. Switching either terminal pane changes the panel to the session
  associated with the focused/selected pane and never leaks another session's
  transcript state.
- [ ] [AC-10] The agent-task API and transcript store resolve the transcript
  on the session's actual local or SSH host, pin modern Claude sessions to the
  Agentum session id, handle transcript creation, append, replacement,
  truncation, daemon restart, and `/clear`/`/compact`, and emit or recover from
  missed `agent_tasks.updated` events. Two Claude sessions sharing a workdir
  remain isolated.
- [ ] [AC-11] [UI] A session whose tool has no supported task parser (including
  Codex until a Codex parser exists), a missing transcript, a host read error,
  an incompatible daemon, and a genuinely empty Claude task state render as
  distinct explicit states. They do not all appear as the same empty panel.
  Fetch failures retain the last good state with a stale marker and a copyable
  diagnostic.
- [ ] [AC-12] Existing single-pane session creation, lifecycle actions,
  terminal input/stream reconnect, multi-host routing, card hints, error-log
  clearing, and authenticated API behavior continue to pass their tests.

## Scope

- `crates/agentum-tui`: pane/add-agent interaction, focus and resize key
  dispatch, create binding and hints, diagnostic capture/copy, Usage panel,
  agent-task panel state, and client response models.
- `crates/agentum-server`: only the routes, usage collection, transcript
  ingestion, host-aware file access, events, and response metadata needed by
  the TUI behavior above.
- `crates/agentum-core`, `crates/agentum-executor`, `crates/agentum-tmux`, or
  `crates/agentum-store` where shared contracts, launch identity, remote-host
  access, pane logs, or persisted session metrics must change.
- The existing primary-plus-one-auxiliary terminal layout is sufficient for
  this change, provided both a plain terminal and a supported agent can be
  added to that auxiliary pane and the original session remains available.

## Non-goals

- An unbounded tiling-window manager, arbitrary nested pane trees, pane drag
  and drop, or more than the current two-terminal layout.
- Combining multiple agents into one Agentum session record or one tmux pane;
  each added shell/agent remains an independently addressable session.
- Implementing transcript parsers for every supported agent. Claude task
  parsing must work; unsupported tools must report that status honestly.
- Inventing plan-limit percentages, costs, task data, or host readiness when
  the source is unavailable.
- Redesigning the web dashboard, board, orchestration DAG, authentication
  model, agent CLIs, or upstream Claude/Codex transcript formats.
- Sending diagnostics, logs, or telemetry to a remote service.

## Risks

- Global shortcuts can steal literal input from interactive agents; scope
  `C` to tree focus and keep focus/resize chords ahead of pane forwarding.
- Session creation is a two-step create/start flow; partial failure must not
  hide an orphaned row or falsely report success.
- Transcript identity and paths differ between local and SSH hosts and across
  Claude versions; stale fallback selection can cross-pollinate sessions.
- Filesystem notifications can coalesce or be lost, and remote files may not
  support local watchers; periodic reconciliation must complement events.
- OAuth credential stores rotate and contain secrets; source selection and
  diagnostics must never log or serialize bearer tokens.
- Cached usage/task data can look current after transport failure unless
  freshness and failure state are modeled explicitly.
- Multiline diagnostics and terminal clipboard protocols can corrupt the
  alternate screen if copying is performed during drawing; reuse the TUI's
  deferred clipboard/output mechanism.

## Verification

- Unit-test key dispatch and focus-cycle helpers for tree, primary terminal,
  auxiliary terminal, pane close, no-split resize, bounds, `C`, `c`, and `n`.
- Exercise create/start with fake clients for terminal, Claude, Codex,
  unavailable-tool, remote-host, and create-success/start-failure cases; assert
  session ownership, inherited defaults, focus, and visible recovery state.
- Unit-test diagnostic construction and redaction with complete, partial,
  multiline, remote-host, incompatible-version, and secret-bearing errors;
  assert the copied bytes contain the full report while the list preview may
  remain compact.
- Test `/api/usage/claude` with isolated HOME/credential/transcript fixtures,
  successful OAuth fixtures, absent credentials, upstream failure, malformed
  files, and auth enforcement. Test initial fetch, refresh coalescing, stale
  retention, retry after failure, and the rendered state labels.
- Test transcript parsing and storage using real representative Claude JSONL
  records for plan, legacy/current todos, tasks, subagents, clear/compact,
  partial final lines, file creation/replacement/truncation, two same-workdir
  sessions, and daemon restart. Cover local and mocked SSH host reads plus
  missed-event periodic reconciliation.
- Run focused crate tests, then `cargo test --workspace` and `cargo clippy
  --workspace --all-targets -- -D warnings` (or document pre-existing failures
  separately).
- Run the TUI in a real PTY at normal and narrow widths. Capture visual
  evidence for adding a pane/agent, focus styling, resize bounds, Usage states,
  populated Plan / Todos / Agents content, unsupported/error states, and the
  full diagnostic copy flow. Confirm copied reports can be pasted intact into
  a separate terminal/editor.

## Capability Deltas

- `deltas/terminal-session-workspace.md`
- `deltas/diagnostics.md`
- `deltas/usage-observability.md`
- `deltas/agent-task-observability.md`
