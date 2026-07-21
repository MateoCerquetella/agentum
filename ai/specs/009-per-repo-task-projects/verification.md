# Spec 009 — Verification

- **Date:** 2026-07-20
- **Verdict:** PASS with project-baseline/tooling caveats

## Acceptance coverage

- After integrating current `develop`, GitHub Project selection uses its newer
  Spec 016 `applyBoardPick` / `resolveBoardProject` path. Embedded repo boards
  write `activeProjectByRepo[repoId]`, never fall back to another repo's legacy
  slot, and clear exactly one repo key. The standalone no-repo surface retains
  its intentional global behavior.
- Linear project/view writes through the existing resume-state API are migrated
  into `linearContextByRepo[activeRepoId]`; clearing deletes only that key.
- Switching repos re-resolves both providers. An unbound Linear repo explicitly
  clears the previously rendered context.
- Legacy Linear global fields still sanitize/hydrate but are intentionally
  ignored by scoped resolution, preventing cross-repo leakage after upgrade.
- Hydration validates every scoped Linear value and prunes deleted repo IDs while
  retaining the reserved no-repo scope.

## Gates run

- Focused Vitest after integrating current `develop`: **21 passed, 0 failed**
  (18 GitHub board-resolution/scope tests plus 3 scoped Linear UI-slice cases).
- Full `ui.test.ts` plus resolver tests: new tests passed; overall **69 passed,
  4 failed**. The failures are unrelated existing expectations in legacy-pet
  hydration and three task-navigation-history cases.
- `git diff --check`: **pass**.
- Production Vite build and bare `tsc --noEmit`: dependencies were absent and
  installed with legacy peer resolution; both commands then remained in their
  transform/type-analysis phases without producing diagnostics and were stopped.
  This is recorded as a tooling caveat rather than claimed green.
