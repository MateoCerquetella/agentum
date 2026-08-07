# Plan: TUI Session UX and Observability Fixes

## Execution Rules

- Preserve unrelated worktree changes and the untracked Empirical integration
  files.
- Add backward-compatible wire fields or dual-shape parsing before changing a
  consumer.
- Keep each task compiling and run its focused tests before advancing.
- Do not mark a UI criterion complete from unit tests alone; collect real-PTY
  and screenshot evidence in the verification task.
- Never put credential contents into fixtures, tracing, errors, or evidence.

## Task 1: Add observable usage and agent-task contracts

Files:

- `crates/agentum-core/src/transcript.rs`
- `crates/agentum-server/src/usage.rs`
- `crates/agentum-server/src/routes/usage.rs`
- `crates/agentum-server/src/routes/agent_tasks.rs`
- `crates/agentum-tui/src/commands/terminal/api.rs`

Changes:

1. Add serializable agent-task snapshot/status/source metadata around the
   existing `AgentTaskState`.
2. Add Claude usage generated-at and collection-status metadata while retaining
   every existing optional field.
3. Return the new task envelope from the server and parse both envelope and
   legacy bare-state responses in the TUI.
4. Classify missing routes, authentication/transport failure, and malformed
   payloads instead of converting them to empty data.

Tests:

- Serde round trips and legacy response compatibility.
- Usage response status for OAuth, local scan, not installed, and unreadable
  fixture states.
- Agent-task route response for supported empty, unsupported, missing
  transcript, and current data.

Acceptance: AC-7, AC-8, AC-11, AC-12.

## Task 2: Make transcript ingestion host-aware and recoverable

Files:

- `crates/agentum-server/src/transcript_store.rs`
- `crates/agentum-server/src/routes/agent_tasks.rs`
- `crates/agentum-server/src/host_runtime.rs`
- `crates/agentum-server/src/lib.rs`
- `crates/agentum-core/src/transcript.rs`

Changes:

1. Pass session tool and resolved host identity/kind into transcript slot
   creation.
2. Isolate local and SSH transcript stat/read implementations behind one
   source interface; never interpret an SSH workdir locally.
3. Preserve local `notify` as a fast path and add a bounded reconciliation task
   for requested slots.
4. Maintain cursor correctness for partial lines, creation, replacement,
   truncation, reset, and daemon restart.
5. Pin modern Claude sessions to their Agentum UUID and explicitly label any
   legacy local fallback.
6. Emit `agent_tasks.updated` only after a meaningful status/data change.

Tests:

- Initial full parse and append.
- Partial line, replacement, truncation, `/clear`, `/compact`, and reset.
- Two UUID-pinned sessions in the same workdir.
- Reconciliation after a deliberately missed notification.
- Mocked SSH success/failure and proof that remote failure does not read a local
  same-shaped path.

Acceptance: AC-9, AC-10, AC-11, AC-12.

## Task 3: Fix task-panel session selection and visible states

Files:

- `crates/agentum-tui/src/commands/terminal/app.rs`
- `crates/agentum-tui/src/commands/terminal/ui.rs`

Changes:

1. Cache agent-task envelopes and retain last good task state on refresh error.
2. Derive the observed session from the focused terminal side; use
   `last_term_side` while the tree owns focus.
3. Fetch on initial load, selection/focus change, server event, and periodic
   catch-up with existing coalescing.
4. Render distinct current-empty, waiting, unsupported, stale, host failure,
   incompatible, and loading states.
5. Route task fetch/reset failures into structured diagnostics once Task 5 is
   available; use the compatibility wrapper until then.

Tests:

- Left/right focus selects the correct cache key.
- Last-good state survives refresh failure and becomes stale.
- Every status has distinct renderer text.
- Missed event converges on the slow path.

Acceptance: AC-9, AC-11, AC-12.

## Task 4: Make usage polling and rendering observable

Files:

- `crates/agentum-tui/src/commands/terminal/app.rs`
- `crates/agentum-tui/src/commands/terminal/ui.rs`
- `crates/agentum-tui/src/commands/terminal/api.rs`

Changes:

1. Replace optional usage channel payloads with typed outcomes.
2. Track loading, last success, last failure, freshness, and in-flight state.
3. Start the first authenticated request immediately, retry at the configured
   interval, and clear in-flight on every outcome.
4. Always render an account header state and clearly separate account-wide
   Claude values from per-tool/per-session metrics.
5. Mark retained values stale after failure and surface classified diagnostics.

Tests:

- Immediate first fetch, coalescing, configured refresh, failure/retry, and
  recovery.
- Rendering of loading/live/local-scan/not-installed/stale/unreachable/
  incompatible states.
- Account versus per-session labels and estimated-value labeling.

Acceptance: AC-7, AC-8, AC-12.

## Task 5: Add structured, redacted, copyable diagnostics

Files:

- `crates/agentum-tui/src/lib.rs`
- `crates/agentum-tui/src/commands/terminal/app.rs`
- `crates/agentum-tui/src/commands/terminal/ui.rs`
- `crates/agentum-tui/src/commands/terminal/api.rs`
- `crates/agentum-store/src/paths.rs` (only if an existing resolver cannot be
  reused without duplication)

Changes:

1. Expose the resolved TUI log path used during tracing initialization.
2. Add the structured diagnostic record, contextual builder, central redactor,
   stable selection, and full report formatting.
3. Convert lifecycle, stream, host, usage, and agent-task errors to explicit
   operation/session diagnostics; keep `push_error` as a contextual fallback.
4. Render compact error rows plus wrapped selected detail.
5. Bind `y` to copy the full selected report through deferred OSC-52; retain
   `c` for clearing.

Tests:

- Full context for session and non-session operations, including daemon/host
  unknown states and local/SSH log paths.
- Multiline/nested error preservation.
- Bearer, authorization, OAuth, password, and hook-token redaction.
- Error selection, clear behavior, and exact copied bytes.

Acceptance: AC-2, AC-6, AC-7, AC-11, AC-12.

## Task 6: Add auxiliary agent creation and change the create key

Files:

- `crates/agentum-tui/src/commands/terminal/app.rs`
- `crates/agentum-tui/src/commands/terminal/palette.rs`
- `crates/agentum-tui/src/commands/terminal/ui.rs`

Changes:

1. Add create destination/source session state to the new-session form.
2. Add command-palette actions for adding a terminal or agent beside the active
   session, with defaults from the source daemon/host/workdir.
3. Query availability for the target host and gate tool submission.
4. On auxiliary success, preserve/open the primary pane, attach the new session
   to the right slot, focus it, and open its stream.
5. On start failure, retain/select the created row and emit a partial-failure
   diagnostic.
6. Change the tree create binding and every hint from `n` to uppercase `C`;
   preserve lowercase `c` and terminal-focused literal `C`.
7. Retain and regression-test existing `F5`/`F6`, close behavior, and persisted
   split resize.

Tests:

- Plain terminal, Claude, Codex, unavailable tool, inherited remote host, and
  create-success/start-failure flows.
- Primary stream/selection preservation and right-slot attachment.
- `C`, `c`, `n`, literal terminal input, focus cycle, close focus, resize
  bounds, persistence, and both resize frames.
- Help, pane titles, status copy, and palette hints.

Acceptance: AC-1, AC-2, AC-3, AC-4, AC-5, AC-12.

## Task 7: Integration and evidence

Commands:

1. `cargo fmt --all -- --check`
2. Focused tests for each touched crate.
3. `cargo test --workspace`
4. `cargo clippy --workspace --all-targets -- -D warnings`

Real-PTY scenarios:

1. Start with one session, add a plain terminal, focus both directions, resize
   to both bounds, close/reopen, and confirm the original session remains.
2. Add Claude and Codex where installed; capture unavailable-tool behavior where
   not installed.
3. Exercise `C`, `c`, `n`, and literal terminal `C`.
4. Populate representative Claude plan/todo/task/subagent records and confirm
   the right panel follows left/right focus without cross-session leakage.
5. Exercise usage live/degraded/stale states using controlled daemon fixtures.
6. Trigger a session operation failure and a usage/task failure, copy each full
   report, and paste into an external editor to confirm completeness/redaction.
7. Repeat layout checks at normal and narrow terminal sizes and save screenshots
   for all UI-tagged acceptance criteria.

Completion:

- Create immutable evidence receipts for every acceptance criterion using test,
  screenshot, PTY/browser-equivalent, review, or human evidence as required by
  Empirical.
- Run a final code review focused on credential leakage, SSH quoting/path
  correctness, event/poll races, and terminal input regressions.
- Complete the exact Empirical revision with receipt ids and continue through
  verification/archive.

Acceptance: AC-1 through AC-12.
