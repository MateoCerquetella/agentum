# Handoff — Developer to Developer

- **Spec:** 028-bound-transcript-observers
- **From:** Developer (paused after gate send-back)
- **To:** Developer (continuation session)
- **Date:** 2026-07-23
- **Gate:** SEND-BACK ACTIVE (iteration 1 of 2)

## Summary

Spec 028 F1–F3 is implemented but uncommitted. Focused tests and isolated QA were green before
review. The Developer gate found two mode-transition defects; the retry was requested but did not
land before the session was paused.

## Completed Work

- Added passive transcript slots, `ObservationMode::{Live, SnapshotOnly}`, atomic reads,
  session-aware reset, bounded/coalescing Tokio notify delivery, abort-on-drop consumers, injected
  observer counting, and `stop_observing`/`retain_observers`/`forget`.
- Removed transcript side effects from `GET /api/sessions`.
- Wired agent-task mode selection, immediate route retirement, and the optional watchdog
  running-session hook.
- Added Spec 028 harness entries and isolated temporary-HOME QA routing.
- Green evidence before send-back: 10 focused server tests, 1 watchdog test, isolated Spec 028 QA,
  `cargo test --workspace --lib --exclude agentum-desktop`, formatting, harness syntax, and diff
  checks.

## Pending Work

1. Fix `TranscriptStore::read` so `SnapshotOnly` drops an existing observer. Current evidence:
   `crates/agentum-server/src/transcript_store.rs:132` only handles
   `mode == ObservationMode::Live`; no snapshot branch retires `slot.observer`.
2. Fix the non-Claude branch so it forgets any prior Claude slot/observer before returning empty.
   Current evidence: `crates/agentum-server/src/transcript_store.rs:106-108` returns immediately.
3. Remove the masking manual call at
   `crates/agentum-server/src/routes/agent_tasks.rs:129` and assert that the stopped-session route
   read itself drops observation. Add a live-Claude → non-Claude read regression proving cache and
   observer counts become zero.
4. Rerun focused tests, isolated QA, fmt, and diff checks. Update `tasks.md`, write the proper
   Developer-to-Tester handoff, commit the Developer phase, then continue fresh Tester and Reviewer
   roles.
5. After Spec 028 signs off, create and run Spec 029 from the staged plan. Spec 029 has not started.

## Important Decisions

- `ObservationMode` describes the observer state after the read, not merely whether attachment is
  allowed.
- A non-Claude session owns no transcript cache entry even if the same ID previously used Claude.
- The five-second watchdog hook is a backstop for crash/tool drift, not a substitute for an
  explicit snapshot/non-Claude read transition.
- Do not create a tool-managed Goal: `get_goal` returned no active Goal in this environment, while
  the repository SDD state is authoritative for continuation.

## Risks

- The implementation files are intentionally uncommitted; preserve them and commit only after the
  Developer retry gate passes.
- `cargo test --workspace --lib` is blocked before desktop compilation by missing
  `target/release/libsherpa-onnx-c-api.dylib` (known worktree prerequisite). The same command with
  `--exclude agentum-desktop` is green.
- `npm run build --prefix crates/agentum-desktop/ui` is blocked because UI dependencies are absent
  (`vite: command not found`). Installing dependencies (the repo has `bun.lock`) is an environment
  step, not a source fix.

## Questions

- None. The send-back specifies the required behavior and tests.

## Recommended Next Step

Resume the Developer retry from `handoffs/03-developer-sendback-01.md`, implement the two small
`TranscriptStore::read` transitions and unmask the route regression, then run the focused Spec 028
gate before advancing the SDD cursor.
