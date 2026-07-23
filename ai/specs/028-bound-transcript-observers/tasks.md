# Spec 028 — Tasks

## F1 — Passive transcript state and bounded live observer — COMPLETE

- Refactor `TranscriptStore` around `ObservationMode` and atomic `read`.
- Add passive slot creation, shared synchronous refresh, bounded coalescing notify transport,
  abort-on-drop consumer ownership, session-aware reset, and injected factory tests.
- Add `stop_observing`, `retain_observers`, and `forget`.
- Covers AC 2–5, 7–8.
- Gate: focused `transcript_store` library tests and `cargo fmt --all -- --check`.
- Evidence: 7 focused transcript-store tests pass, including live-to-snapshot observer retirement
  and live-Claude-to-non-Claude cache/observer removal; formatting passes.

## F2 — Route-selected mode and immediate lifecycle retirement — COMPLETE

- Remove transcript side effects from session listing.
- Select live versus snapshot-only mode in the agent-task route.
- Pass session identity into reset and retire observation on stop/kill/delete/manual tool change.
- Add the 500-session list and route lifecycle regressions.
- Covers AC 1–6.
- Gate: focused `routes::agent_tasks` and `routes::sessions` library tests.
- Evidence: 2 agent-task route tests and 2 transcript-lifecycle session route tests pass; the
  stopped-session regression contains no manual `stop_observing` call and observes one guard drop.

## F3 — Reconcile retirement and performance harness — COMPLETE

- Add the optional watchdog running-session hook and inject `retain_observers` from server boot.
- Verify crash/stopped/deleted/tool-changed observers retire without new observations.
- Add the three Spec 028 harness entries and isolated runtime QA route.
- Covers AC 6 and the full verification contract.
- Gate: focused watchdog/server tests, then all required workspace/build/diff commands.
- Evidence: the watchdog hook test and isolated Spec 028 QA pass; `cargo test --workspace --lib
  --exclude agentum-desktop`, `cargo fmt --all -- --check`, and `git diff --check` pass. The full
  workspace command remains environment-blocked by the missing release Sherpa dylib, and the UI
  build remains environment-blocked because Vite dependencies are not installed.

## Tester send-back evidence closure — COMPLETE

- Production stop, kill, and delete route seams now execute against the counting transcript store:
  stop/kill drop observation while retaining cache, delete drops observation and cache, and none
  starts an observer.
- The production server watchdog builder is exercised through one authoritative reconcile pass;
  it keeps the running Claude observer, retires stopped/crashed/deleted/tool-changed observers,
  preserves passive caches, and starts none.
- The injected observer seam retains callbacks and records queued/coalesced/closed delivery plus
  consumer start/finish. Its regression proves capacity-one burst coalescing, a consumed queued
  wake, prompt consumer completion after stop and forget, and no stale update/cache recreation.
- Isolated QA now includes a real `RecommendedWatcher` append → `agent_tasks.updated` bus event →
  retirement-silence check. It explicitly makes no portable OS-thread-count or WebSocket claim;
  `verify.sh` rejects `spawn_blocking`/`std::sync::mpsc` receiver paths.
- Gate evidence: transcript store **9/9**, agent-task routes **2/2**, transcript lifecycle routes
  **3/3**, server-wired watchdog retirement **1/1**, watchdog generic hook **1/1**, isolated QA
  **15/15**; `cargo fmt --all -- --check` and `git diff --check` pass.
