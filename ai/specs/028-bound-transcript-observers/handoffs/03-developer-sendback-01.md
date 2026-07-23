# Handoff — Developer retry

- **Spec:** 028-bound-transcript-observers
- **From:** Developer gate
- **To:** Developer
- **Date:** 2026-07-23
- **Gate:** SEND-BACK (iteration 1 of 2)

## Delivered

- F1–F3 implementation with focused store, route, watchdog, and isolated-QA evidence green.
- Gate review of the actual mode-transition behavior before advancing to Tester.

## Acceptance-criteria evidence

- **AC 1–2, 5–8:** Implemented with focused green evidence, subject to retry verification.
- **AC 3:** BLOCKED. A `SnapshotOnly` read does not detach an observer previously attached by a
  live read. The route test manually calls `stop_observing`, so it does not verify the promised
  running-to-historical transition.
- **AC 4:** BLOCKED for a tool transition. The non-Claude early return creates nothing for a fresh
  ID but leaves a prior Claude slot/observer cached when that same session changes tools.

## Verification

- Focused Spec 028 Rust suites — PASS (10 server + 1 watchdog test).
- Isolated Spec 028 QA route — PASS, but currently masks AC 3 with a manual lifecycle call.
- `cargo test --workspace --lib --exclude agentum-desktop` — PASS.
- `cargo test --workspace --lib` — ENVIRONMENT BLOCKED by the known missing
  `target/release/libsherpa-onnx-c-api.dylib` prerequisite.
- Desktop UI build — ENVIRONMENT BLOCKED because dependencies are absent (`vite: command not
  found`).
- `cargo fmt --all -- --check`, harness syntax, and `git diff --check` — PASS.

## Decisions and invariants

- `ObservationMode` must describe the post-read observer state, not only whether a new observer may
  be attached.
- A non-Claude read must leave no entry, including after a Claude-to-other-tool transition.
- Do not rely on the five-second watchdog reconcile to make an explicit snapshot read correct.

## Remaining risks / next action

- In `TranscriptStore::read`, make `SnapshotOnly` take/drop any existing observer and make the
  non-Claude branch forget any prior slot before returning empty.
- Remove the manual `stop_observing` call from the route transition test and assert the factory drop
  count. Add a Claude-live → non-Claude read regression proving cache and observation counts are
  zero.
- Rerun focused tests, isolated QA, formatting, and diff checks; preserve the documented
  environment-only umbrella-gate blockers.
