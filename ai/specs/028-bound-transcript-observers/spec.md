# Spec 028 — Bound Transcript Observers

- **Number:** 028
- **Status:** PM
- **Surface:** `crates/agentum-server`, `crates/agentum-watchdog`
- **Author:** Codex
- **Date:** 2026-07-23

## Problem

An engineer operating Agentum with hundreds of historical workspaces pays one filesystem watcher
and one permanently blocked consumer task for every Claude session merely because the session list
is polled. The resulting thread and observer growth makes the desktop progressively less responsive
even though those historical sessions are not running.

## Goal

An operator can retain and browse hundreds of historical sessions while Agentum observes
transcripts only for currently running Claude sessions.

## Users / personas

An engineer supervising 20 or more workspaces feels this while switching projects or leaving the
desktop open after accumulating many completed Claude sessions.

## Acceptance criteria

1. `GET /api/sessions` returns the existing session response without creating transcript
   directories, cached transcript entries, filesystem observers, or consumer tasks for any row; a
   fixture with 500 sessions, including 250 Claude sessions, records zero observer creations.
2. Reading agent tasks for a running Claude session synchronously returns current transcript state
   and attaches one live observer; repeated and concurrent reads for that session create exactly
   one observer and preserve the existing `agent_tasks.updated` payload.
3. Reading agent tasks for an idle, stopped, or crashed Claude session synchronously incorporates
   newly appended complete transcript lines without attaching an observer.
4. Reading agent tasks for a non-Claude session returns the existing empty state without creating
   a cached entry, transcript directory, observer, or task.
5. Resetting before any prior agent-task read clears the returned state and advances the selected
   transcript cursor to EOF, so later reads never resurrect pre-reset plan, todo, or task data.
6. Stopping or killing a session removes its live observation promptly; deleting a session removes
   both observation and cached transcript state; the watchdog's existing five-second reconcile
   retires observers for crashed, stopped, deleted, or tool-changed sessions without starting any.
7. UUID-pinned transcript isolation, legacy fallback selection, fallback promotion to the pinned
   file, complete-line parsing, and the agent-task HTTP response schemas remain unchanged.
8. Dropping an observer terminates its asynchronous consumer promptly; notify callbacks use a
   bounded coalescing Tokio channel and no permanent `spawn_blocking` receiver thread remains.

## Scope & non-goals (YAGNI)

- **In:** separate passive cached transcript state from optional live observation; explicit live
  versus snapshot-only reads; deterministic injected observer creation/drop accounting; lifecycle
  retirement from session stop, kill, delete, and watchdog reconciliation.
- **Out:** database or HTTP schema changes; transcript retention/deletion; parsers for Codex,
  Gemini, or other agent formats; watchdog pane-sampling batching; desktop renderer changes.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `TranscriptStore` (`crates/agentum-server/src/transcript_store.rs`) already owns per-session
  parser state, cursor, pinned path, legacy fallback, incremental refresh, and
  `agent_tasks.updated` emission; retain those semantics.
- `get_agent_tasks` and `reset_agent_tasks`
  (`crates/agentum-server/src/routes/agent_tasks.rs`) already resolve the durable session and return
  the public `AgentTaskState` schema.
- Session stop, kill, and delete lifecycle routes
  (`crates/agentum-server/src/routes/sessions.rs`) already establish intentional retirement points.
- `Watchdog::reconcile` (`crates/agentum-watchdog/src/lib.rs`) already computes the running-session
  set every five seconds; expose an optional server-owned hook rather than adding another timer.

### Build new

- Internal `ObservationMode::{Live, SnapshotOnly}` and atomic `TranscriptStore::read`, with passive
  entries and an optional live-observer handle.
- Bounded/coalescing notify delivery and an abortable Tokio consumer whose lifetime is owned by the
  observer handle.
- `stop_observing`, `retain_observers`, and `forget` lifecycle operations plus an injected observer
  factory used by deterministic tests.
- A server-to-watchdog optional running-session reconcile hook that can retire observers without
  introducing the reverse crate dependency or starting observation.

## Risks & invariants

- Concurrent first reads must not race into duplicate watchers; entry creation and observer attach
  require one atomic store decision.
- Reset must load enough passive metadata to choose the pinned/legacy file and move the cursor to
  EOF before clearing, including when reset is the first interaction.
- A late notify callback or consumer wake after stop/delete must not recreate state or emit stale
  events.
- Preserve per-session Claude UUID pinning, the one launch path, push-based terminal streaming, and
  all existing event names/payloads.

## Harness wiring (the gate)

- **feature_list.json entries:** `side-effect-free-session-list`,
  `mode-aware-transcript-read`, `transcript-observer-lifecycle`.
- **`verify.sh` asserts:** focused `agentum-server` transcript and route tests cover the 500-session
  fixture, exactly-once live observation, passive appended reads, reset-first semantics, pinned
  promotion/legacy fallback, non-Claude behavior, and stop/crash/delete retirement; then
  `cargo fmt --all -- --check`, `cargo test --workspace --lib`, and `git diff --check` pass.
- **`qa.sh` asserts:** with isolated temporary `AGENTUM_HOME`, `HOME`, and `TMUX_TMPDIR`, listing a
  production-shaped historical fixture does not change observer or thread counts, while a running
  Claude session still streams agent-task updates and stopping it retires observation.

## Open questions

- None. The staged performance plan fixes lifecycle and compatibility behavior for this slice.
