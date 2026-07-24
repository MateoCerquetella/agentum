# Handoff — Reviewer to Developer

- **Spec:** 028-bound-transcript-observers
- **From:** Reviewer
- **To:** Developer
- **Date:** 2026-07-23
- **Gate:** SEND-BACK (Reviewer iteration 1 of 2)

## Delivered

- Fresh final review of `4f3c030c..ff43ef40` with three reproducible lifecycle race findings.
- `review.md` traces the exact schedules, acceptance-criterion impact, and minimal correction/test
  boundaries.

## Acceptance-criteria evidence

- **AC 1–2, 4–5, 7:** PASS.
- **AC 3, 8:** BLOCKED because an already-awake consumer can outlive observer abort across the
  synchronous store mutex and mutate/emit after SnapshotOnly or other retirement.
- **AC 6:** BLOCKED because stop/kill can reattach during teardown before durable Stopped status,
  and delete can recreate cache/observation before durable row deletion.

## Verification

- Reviewer transcript-store spot check — PASS (9 tests).
- `git diff --check 4f3c030c..ff43ef40` and blocking-receiver source guard — PASS.
- Existing green gates do not force the three adversarial schedules.

## Decisions and invariants

- Add a monotonically unique observer generation/liveness token. Consumer refresh and event emit
  must share the same current-generation boundary; an unlocked check-then-send is insufficient.
- Keep early route retirement, then add final `stop_observing` after durable Stopped commit and
  final `forget` after durable deletion.
- Deterministic barriers/interleavings, not timing-only silence, are authoritative for these races.

## Remaining risks / next action

- Add the generation-aware refresh path and a barrier regression for SnapshotOnly,
  `stop_observing`, `retain_observers`, and `forget` while a wake is already in flight.
- Add controlled successful stop/kill and forced-running delete interleavings proving the final
  route boundaries remove any observer/cache recreated during teardown.
- Update verification/QA claims, rerun all gates, and return to a fresh Tester before Reviewer
  iteration 2.
