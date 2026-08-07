# Design: TUI Session UX and Observability Fixes

## Overview

This change extends the existing primary-plus-right-split terminal model rather
than introducing a new pane manager. It adds an explicit create destination to
the new-session workflow, introduces structured diagnostic and observable-data
states in the TUI, and makes agent-task transcript access aware of the session's
actual host.

The server remains the authority for session launch, host access, usage
collection, and transcript parsing. The TUI remains responsible for focus,
layout, clipboard behavior, polling state, and presentation.

## Current Evidence

- `App` already owns independent left and right terminal streams through the
  flat primary fields plus `TermSlot`, with `F5`/`F6` focus cycling and
  `Ctrl-Shift-Left`/`Ctrl-Shift-Right` persisted split resizing.
- `spawn_plain_terminal` and the new-session form currently select the target
  by `App::target_side`, but opening an auxiliary pane and preserving the
  original selection is not an atomic user flow.
- Tree focus currently binds `n` to new session and lowercase `c` to the card
  hint. Terminal-focused printable keys are forwarded after global chord
  handling.
- `ErrorEntry` stores only a timestamp and string. Its list renderer flattens
  newlines and truncates to one row, and the errors overlay has no copy action.
- The TUI log is append-only at the resolved Agentum cache directory's
  `tui.log`; session pane logs are already derived from the session id.
- The Usage fetch converts every error to `None`. The receive path keeps a last
  good value but records neither failure nor stale state, so initial failures
  render no account header and later failures look current.
- `/api/usage/claude` already separates OAuth enrichment from local scanning
  and degrades to `source = "scan"`, but its wire data has no generated-at or
  collection-status metadata.
- `TranscriptStore` eagerly parses on first local read and pins modern Claude
  transcripts by Agentum UUID. It returns only `AgentTaskState`, silently
  short-circuits unsupported tools, and treats every workdir as local even when
  the session belongs to an SSH host.
- The right panel reads `app.selected`, so a right-pane focus can display the
  primary/last tree selection rather than the right slot's selected session.

## Component Changes

### 1. Add-session destination and pane flow

Add a `CreateDestination` value to `NewSessionForm`:

- `ReplaceFocused`: existing tree-driven single-pane behavior.
- `Auxiliary`: add from an active terminal workspace.

Create one helper that opens the form with defaults resolved from an explicit
source session id and destination. A command-palette action available from any
terminal focus opens `Auxiliary`; uppercase `C` from tree focus opens
`ReplaceFocused`. The existing plain-terminal action receives the same
destination instead of maintaining a separate selection policy.

On successful create/start for `Auxiliary`:

1. Preserve the primary pane and its stream.
2. Open `split_right` if necessary.
3. Put the new session id in the right slot and open its stream.
4. Focus `TermRight` and set `last_term_side = Right`.
5. Refresh the session tree without retargeting the preserved left slot.

If the split is already open, the action replaces only the auxiliary pane's
view; it does not stop the session that was previously shown there. If create
succeeds and start fails, refresh/select the created row, leave it recoverable,
and emit a structured partial-failure diagnostic.

Host availability is resolved with `list_agents_on(source.host_id)` so the
picker is gated for the actual target host, not only the daemon's local PATH.

### 2. Key dispatch and layout

Keep the existing focus and split implementation. Change only the new-session
tree match from `n` to `C`, update palette/help/status copy, and add direct unit
tests around the dispatch predicate. Because the terminal-forwarding branch
runs before the tree-only match, literal uppercase `C` continues to reach a
focused terminal. Lowercase `c` remains the card hint.

The existing `next_focus`/`prev_focus`, bounded percentage, persisted prefs,
and per-stream resize frames remain the implementation for AC-3/AC-4. Add
regression tests and ensure the new auxiliary-create flow uses those paths.

### 3. Structured diagnostics

Replace `ErrorEntry { at, text }` with an additive structured record:

- stable entry id and timestamp;
- `operation`;
- full `message`/cause text;
- optional session id;
- TUI version;
- daemon endpoint, reachability, and reported version;
- optional target host id, label, kind, and readiness summary;
- resolved TUI log path and optional pane log path;
- a pre-rendered redacted report string.

Introduce `App::push_diagnostic(operation, session_id, error)` as the explicit
path for network, lifecycle, usage, task, stream, and host operations.
`push_error(text)` remains as a compatibility wrapper during the conversion and
builds a diagnostic with operation `tui` plus current runtime context, ensuring
no existing user-visible error falls outside the richer model.

A central redactor runs before storage, tracing, display, and copy. It removes
known bearer/header, OAuth token, password, and hook-token shapes without
removing the surrounding error chain.

The errors overlay maintains a selected entry. Its compact list can stay
one-line, while a wrapped detail region renders the selected report. A `y`
action queues the complete report through the existing deferred OSC-52
clipboard path; `c` remains clear and is visually distinct.

Expose a pure `tui_log_path()` resolver from the TUI library initialization and
use the existing local pane-log resolver. For SSH sessions, report the known
remote `$HOME/.agentum/panes/<session-id>.log` path.

### 4. Usage state and wire metadata

Extend the Claude usage response additively with:

- `generated_at_ms`;
- `collection_status` (`live`, `local_scan`, `not_installed`,
  `transcript_unreadable`);
- optional redacted `status_detail` suitable for UI diagnostics.

Do not expose the OAuth token or raw credential-store contents. Preserve the
existing optional utilization fields and `source` compatibility.

Replace `UnboundedSender<Option<ClaudeUsage>>` with a typed outcome carrying
either a snapshot or a classified client error. Track:

- initial loading;
- last good snapshot and fetch time;
- last failure and failure time;
- in-flight state.

Success clears the failure. Failure always clears in-flight, retains any last
good snapshot, marks it stale immediately, and creates a diagnostic. The Usage
renderer always reserves a header state when the panel is visible, so initial
loading/unavailable cannot appear as an unexplained blank.

Account data stays tied to the active daemon. Per-tool and per-session rows
continue to use the merged session snapshots and are labeled separately.

### 5. Agent-task envelope and host-aware transcript source

Add an additive API envelope in `agentum-core`:

```text
AgentTaskSnapshot {
  state: AgentTaskState,
  status: current | empty | waiting_for_transcript | unsupported |
          stale | host_unavailable | read_error,
  tool,
  source_host_id,
  transcript_path,
  updated_at_ms,
  detail
}
```

The TUI client first parses the new envelope and may accept the legacy bare
`AgentTaskState` shape as `current/empty` for compatibility with an older
daemon. A 404 is classified as incompatible rather than collapsed into empty.

Refactor transcript access behind a source abstraction:

- local source: current `notify` watcher plus filesystem reads;
- SSH source: host-runtime commands that stat and read only the deterministic
  Claude project transcript path on the session's host.

The store slot records session id, tool, host id, source path, cursor, state,
status, last success, and last error. Local notifications remain a fast path.
A bounded periodic reconciliation scans active slots, which covers coalesced
events, daemon restart, and remote sources where local `notify` cannot work.
All reads preserve the partial-final-line cursor rule. File replacement or
shrink resets parser state safely.

For modern Claude sessions, only the deterministic Agentum-UUID path is used
once it exists. Legacy latest-file fallback is local-only, explicitly marked,
and must never be used for an SSH read error. Unsupported tools return an
`unsupported` snapshot without creating Claude directories.

The TUI caches `AgentTaskSnapshot`, not only `AgentTaskState`. A failed refresh
updates status/detail while retaining the last good state. The panel's effective
session id is derived from focused pane (`split_right.selected` for
`TermRight`, otherwise primary selection), with tree focus using
`last_term_side` consistently.

## Data Flows

### Add agent

`terminal focus -> palette add action -> NewSessionForm(Auxiliary) ->
POST /api/sessions -> POST /start -> refresh -> split_right selection ->
terminal WebSocket`

### Usage

`run-loop start/timer -> authenticated /api/usage/claude -> daemon scan +
optional OAuth enrichment -> typed outcome -> ready/stale/unavailable state ->
Usage renderer + diagnostic on failure`

### Agent tasks

`focused pane session -> /agent-tasks -> session + host lookup -> local watcher
or SSH transcript source -> incremental parser -> AgentTaskSnapshot -> TUI cache
-> Plan / Todos / Agents renderer`

### Error copy

`operation failure -> contextual diagnostic builder -> redactor -> error log ->
selected wrapped detail -> deferred OSC-52 bytes -> external clipboard`

## Compatibility and Migration

- New usage fields are optional/defaulted so older clients ignore them.
- The TUI accepts both the new agent-task envelope and the legacy bare state.
- No database migration is required; transient freshness/error metadata remains
  in memory.
- Existing session records and tmux targets remain unchanged.
- Unsupported agent task parsers remain unsupported but become explicit.

## Verification Mapping

| Criteria | Primary verification |
| --- | --- |
| AC-1, AC-2 | create/start unit integration with fake client plus real PTY flow |
| AC-3, AC-4, AC-5 | key/focus/layout unit tests and PTY screenshots |
| AC-6 | diagnostic/redaction/copy byte tests plus pasted-report PTY check |
| AC-7, AC-8 | usage route fixtures, typed polling-state tests, panel screenshots |
| AC-9, AC-10, AC-11 | parser/store/API tests for local and mocked SSH sources plus panel screenshots |
| AC-12 | workspace tests and clippy |

## Implementation Order

1. Add shared usage/task response models and typed client outcomes.
2. Make transcript storage host-aware and add reconciliation.
3. Fix task-panel session selection and observable states.
4. Add usage state metadata and rendering.
5. Add structured diagnostics and copy/detail UI.
6. Add auxiliary create destination and change the `C` binding/hints.
7. Run focused, workspace, lint, and real-PTY verification; repair regressions.
