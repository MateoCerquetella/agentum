# Handoff — Developer to Tester (Reviewer race closure)

- **Spec:** 028-bound-transcript-observers
- **From:** Developer
- **To:** Tester
- **Date:** 2026-07-23
- **Gate:** PASS

## Delivered

- Monotonic observer generations captured by each consumer; generation-aware refresh verifies,
  mutates, and broadcasts within one mutex-held retirement boundary.
- Deterministic synchronous post-receive gate proving already-awake consumers cannot mutate or emit
  after SnapshotOnly, stop, reconcile retirement, or forget.
- Final stop/kill retirement after durable Stopped commit and final delete forget after durable row
  deletion, while retaining the early cleanup for slow/error paths.
- Controlled successful route interleavings that deliberately reattach/recreate during teardown
  and prove the final production boundaries remove that work.

## Acceptance-criteria evidence

- **AC 3, 8:** An already-received wake is parked through each retirement operation, then released;
  generation mismatch prevents mutation/event emission and consumer completion is observed.
- **AC 6:** Stop and kill finish with zero observers/cache retained after mid-teardown reattachment;
  forced running delete finishes with zero observers/cache after mid-teardown recreation.
- **AC 1–2, 4–5, 7:** Prior verified behavior remains unchanged.

## Verification

- Transcript store — PASS (10 tests).
- Agent-task routes — PASS (2 tests).
- Session transcript lifecycle routes — PASS (4 tests).
- Server-wired and generic watchdog tests — PASS (1 + 1).
- Isolated Spec 028 QA — PASS (17 tests).
- `cargo test --workspace --lib --exclude agentum-desktop` — PASS (835 passed, 2 ignored).
- `cargo check -p agentum-server -p agentum-watchdog`, `cargo fmt --all -- --check`, JSON/shell
  validation, blocking-receiver source guard, and `git diff --check` — PASS.
- Full workspace/UI gates retain the known Sherpa dylib and missing Vite dependency blockers.

## Decisions and invariants

- Observer authority is removed from the slot before abort, so cooperative cancellation is no
  longer relied on for quiescence.
- Event emission for consumer refresh stays inside the same generation/mutex boundary as mutation.
- Early route cleanup handles slow/error paths; final durable-boundary cleanup closes concurrent
  reads without changing teardown behavior.

## Remaining risks / next action

- Fresh Tester must force the same post-receive and teardown-window schedules independently, then
  update `verification.md` before Reviewer iteration 2.
