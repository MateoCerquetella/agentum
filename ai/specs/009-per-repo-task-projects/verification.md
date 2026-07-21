# Spec 009 — Verification

- **Date:** 2026-07-20
- **Verdict:** PASS with project-baseline/tooling caveats

## Acceptance coverage

- GitHub Project selection writes `activeProjectByRepo[activeRepoId]`; the reader
  resolves only that key, and the picker can clear only the current repo.
- Linear project/view writes through the existing resume-state API are migrated
  into `linearContextByRepo[activeRepoId]`; clearing deletes only that key.
- Switching repos re-resolves both providers. An unbound Linear repo explicitly
  clears the previously rendered context.
- Legacy global fields still sanitize/hydrate but are intentionally ignored by
  both resolvers, preventing cross-repo leakage after upgrade.
- Hydration validates every scoped Linear value and prunes deleted repo IDs while
  retaining the reserved no-repo scope.

## Gates run

- Focused Vitest (`task-project-scope.test.ts` plus three scoped UI-slice cases):
  **6 passed, 0 failed**.
- Full `ui.test.ts` plus resolver tests: new tests passed; overall **69 passed,
  4 failed**. The failures are unrelated existing expectations in legacy-pet
  hydration and three task-navigation-history cases.
- `git diff --check`: **pass**.
- Production Vite build and bare `tsc --noEmit`: dependencies were absent and
  installed with legacy peer resolution; both commands then remained in their
  transform/type-analysis phases without producing diagnostics and were stopped.
  This is recorded as a tooling caveat rather than claimed green.
