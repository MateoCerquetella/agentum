# Handoff — Architect to Developer

- **Spec:** 025-operational-sidebar-triage
- **From:** Architect
- **To:** Developer
- **Date:** 2026-07-22
- **Gate:** PASS

## Delivered

- `architecture.md` with verified current-state seams, seven implementation decisions, exact
  files, data/control flow, race/error handling, build order, and complete AC-to-test mapping.
- `tasks.md` with three ordered implementation tasks that deliver the single harness feature.

## Acceptance-criteria evidence

- **AC 1–2:** standard virtual rows are built by a pure operational model from the shared
  `resolveWorktreeStatus` truth path.
- **AC 3–5:** lifted transient query, existing persisted repo filters, measured chip overflow,
  shared relative-time clock, rich/compact variants, and disclosure have named seams/tests.
- **AC 6–7:** `WorktreeCard` remains the only interaction owner; a real `operational` preference
  value permits a safe absent-only default without overwriting explicit groupings.
- **AC 8–9:** semantic control/card contracts, virtual size tests, browser QA matrix, focused
  Vitest suites, and the production build are enumerated.

## Verification

- Architecture gate AC map — PASS (9/9 criteria have implementation and named verification).
- Source seam audit — PASS (every cited module/function exists in this worktree).
- `git diff --check` — PASS.

## Decisions and invariants

- Status classification reuses `resolveWorktreeStatus`; no parallel detector or polling.
- Operational items remain standard `Row` + `VirtualizedWorktreeViewport` entries.
- Rich and settled bodies live inside `WorktreeCard`; interactions are not copied.
- Search is transient, project/group filters remain on their existing persistence path.
- Explicit persisted grouping choices win; only absent/invalid state defaults operational.

## Remaining risks / next action

- Implement Task 1 first and keep its pure model independent of React; do not begin visual
  integration until status precedence, persistence normalization, ordering, and disclosure tests
  are green.
