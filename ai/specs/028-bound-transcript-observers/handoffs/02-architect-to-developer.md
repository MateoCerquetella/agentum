# Handoff — Architect to Developer

- **Spec:** 028-bound-transcript-observers
- **From:** Architect
- **To:** Developer
- **Date:** 2026-07-23
- **Gate:** PASS

## Delivered

- `architecture.md` pins one atomic transcript read seam, passive cached entries, optional
  abort-on-drop observers, bounded notify delivery, route lifecycle calls, and a one-way watchdog
  retirement hook.
- `tasks.md` sequences store refactoring before routes and watchdog/harness wiring.

## Acceptance-criteria evidence

- **AC 1–5:** Every list/read/reset/non-Claude outcome maps to one store or route seam and a
  deterministic test.
- **AC 6:** Immediate stop/kill/delete retirement and five-second crash/tool reconcile map to
  explicit lifecycle methods and hook coverage.
- **AC 7–8:** One shared incremental refresh preserves pin/fallback semantics; observer ownership
  and channel capacity provide a concrete shutdown/bounding proof.

## Verification

- Architecture traceability — PASS (8/8 acceptance criteria have implementation and test seams).
- Architecture invariants — PASS (no crate cycle, schema/event change, new poller, or transcript
  parser expansion).
- `git diff --check` — PASS.

## Decisions and invariants

- The store mutex is the exactly-once boundary for live observer attachment.
- Snapshot reads never create directories; non-Claude reads never create entries.
- Watchdog exposes a policy-free callback and the server owns transcript retirement policy.
- Retirement paths may drop state/observation early on a later operation failure because the next
  authoritative read can recreate it; they may never leave an unbounded observer behind.

## Remaining risks / next action

- Implement F1 through F3 in order. Keep fake-factory tests authoritative for observer counts and
  keep real notify/tmux checks optional runtime evidence rather than timing-sensitive unit gates.
