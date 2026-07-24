# Handoff — Developer to Tester (final autonomous retry)

- **Spec:** 028-bound-transcript-observers
- **From:** Developer
- **To:** Tester
- **Date:** 2026-07-23
- **Gate:** PASS

## Delivered

- A weak, ref-counted per-session Tokio mutex registry in `TranscriptStore`; concurrent holders
  and waiters share one UUID lock, while dead UUID keys are pruned opportunistically.
- Agent-task GET/reset now hold that boundary across durable session load and transcript work.
- Stop/kill, forced delete, and tool PATCH hold the same boundary across authoritative mutation and
  final observer/cache retirement, without nested acquisition in HTTP or MCP wrappers.
- Deterministic actual-handler regressions park a Running/Claude GET after load, prove each
  lifecycle transition waits, and then prove no stale observer or deleted cache survives.

## Acceptance-criteria evidence

- The remaining AC 6 race is linearized: an agent-task request either completes before the
  lifecycle mutation and is retired by it, or loads the authoritative post-mutation state.
- Existing generation fencing and teardown-window regressions remain intact, preserving the prior
  AC 1–5 and AC 7–8 evidence.
- The lifecycle registry itself has a concurrency/cleanup regression proving same-key exclusion and
  opportunistic dead-key pruning.

## Verification

- Transcript store: **11/11 PASS**.
- Transcript lifecycle routes: **7/7 PASS**.
- Agent-task routes: **2/2 PASS**.
- Server-wired watchdog and generic watchdog: **1/1 PASS** each.
- Isolated QA: **21/21 PASS**.
- Non-desktop backend workspace: **839 passed, 0 failed, 2 ignored**.
- `cargo check -p agentum-server -p agentum-watchdog`, `cargo fmt --all -- --check`, JSON/shell
  validation, the blocking-receiver source guard, and `git diff --check`: **PASS**.

## Known environment blockers

- `cargo test --workspace --lib` remains unavailable because
  `target/release/libsherpa-onnx-c-api.dylib` is absent; the documented non-desktop workspace gate
  passes.
- The UI build remains unavailable because dependencies are not installed (`vite` not found); this
  backend-only change has no browser surface.

## Tester directive

- This is the final Tester attempt after failure 2/2. Re-run the focused and isolated suites and
  specifically audit the preloaded Running/Claude GET interleavings for stop, kill, delete, and
  tool patch. Any further Tester failure must route to HITL; advance only on a green gate.
