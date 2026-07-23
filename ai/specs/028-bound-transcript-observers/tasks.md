# Spec 028 — Tasks

## F1 — Passive transcript state and bounded live observer

- Refactor `TranscriptStore` around `ObservationMode` and atomic `read`.
- Add passive slot creation, shared synchronous refresh, bounded coalescing notify transport,
  abort-on-drop consumer ownership, session-aware reset, and injected factory tests.
- Add `stop_observing`, `retain_observers`, and `forget`.
- Covers AC 2–5, 7–8.
- Gate: focused `transcript_store` library tests and `cargo fmt --all -- --check`.

## F2 — Route-selected mode and immediate lifecycle retirement

- Remove transcript side effects from session listing.
- Select live versus snapshot-only mode in the agent-task route.
- Pass session identity into reset and retire observation on stop/kill/delete/manual tool change.
- Add the 500-session list and route lifecycle regressions.
- Covers AC 1–6.
- Gate: focused `routes::agent_tasks` and `routes::sessions` library tests.

## F3 — Reconcile retirement and performance harness

- Add the optional watchdog running-session hook and inject `retain_observers` from server boot.
- Verify crash/stopped/deleted/tool-changed observers retire without new observations.
- Add the three Spec 028 harness entries and isolated runtime QA route.
- Covers AC 6 and the full verification contract.
- Gate: focused watchdog/server tests, then all required workspace/build/diff commands.
