# Handoff — Tester to Developer (final autonomous Tester retry)

- **Spec:** 028-bound-transcript-observers
- **From:** Tester
- **To:** Developer
- **Date:** 2026-07-23
- **Gate:** SEND-BACK (Tester failure 2 of 2)

## Delivered

- Fresh verification of the generation and final-route-boundary fixes.
- Two temporary adversarial regressions that reproduced one remaining stale-request race, then
  were removed so the report is the only Tester change.

## Acceptance-criteria evidence

- **AC 1–5, 7–8:** PASS; observer-generation fencing closes the Reviewer B1 race.
- **AC 6:** BLOCKED. A GET that loads a Running Claude session before final stop/delete cleanup can
  delay its `TranscriptStore::read(Live)` until afterward, recreating observation/cache.

## Verification

- Committed focused suites, isolated QA, check/fmt/diff/source guard — PASS.
- Backend workspace — PASS (835 passed, 2 ignored).
- Temporary stale-request regressions — FAIL as expected (0/2): post-stop observer count 1;
  post-delete cache count 1 with a live observer.

## Decisions and invariants

- Use a scoped per-session async lifecycle boundary shared by durable load + transcript read/reset
  and by stop/kill/delete/tool-patch mutation + final retirement.
- Prefer a weak/ref-counted keyed-lock registry with opportunistic cleanup; do not add permanent
  UUID tombstones that grow with deleted history.
- The operation must linearize so a transcript request completes before lifecycle mutation (and is
  caught by final cleanup) or loads the authoritative post-mutation row afterward.

## Remaining risks / next action

- Implement the keyed lifecycle boundary and deterministic route tests that park after agent-task
  durable load, prove stop/delete cannot cross it, then verify zero observer/cache after completion.
- This is Tester failure 2 of 2. A third Tester gate failure requires HITL under policy; do not
  weaken AC 6 or advance without a green final Tester run.
