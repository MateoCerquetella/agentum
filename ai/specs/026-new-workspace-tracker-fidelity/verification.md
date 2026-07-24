# Spec 026 — Tester verification

- **Date:** 2026-07-21
- **Role:** Tester
- **Iteration:** Reviewer retry 2/2
- **Verdict:** PASS — eligible for final Reviewer

## Summary

The Reviewer B1 blocker is resolved. `ProjectBindingEditor` invokes the typed
`onUnbound` callback only after the repo-owned DELETE succeeds. The mounted
TrackerSection immediately nulls the old scope ref, projects the current target
to `absent`, clears table/status/query state, and closes the editor. Late
completions carrying the deleted scope are rejected.

The new production-seam regression passes and proves the resulting UI model is
Configure tracker, status `none`, zero eligible rows, and no acceptance of an
old-scope completion. Both cumulative Spec 026 harness routes remain green.

## Reviewer blocker verification

### B1 — Inline unbind stale projection: FIXED

- `ProjectBindingEditor.handleUnbind` awaits `deleteProjectBinding`; callback
  dispatch occurs only on success and after the mounted guard.
- TrackerSection's callback uses the current `bindingTargetKey`, writes
  `latestScopeKeyRef.current = null` synchronously, stores an `absent` binding,
  clears table/status/query state, and closes the popover.
- `tracker-section-scope.test.ts` uses the same helpers imported by production
  TrackerSection and proves Configure tracker, no connected state, zero rows,
  and rejection of the deleted scope.
- DELETE failure never invokes `onUnbound`; editor error handling retains the
  prior parent projection.

## Acceptance-criteria evidence

| AC | Automated and inspected evidence | Verdict |
|---|---|---|
| 1 | Repo-aware binding route resolves selected origin before canonical repo-key projection; normalized slug matching and two-repo isolation pass focused Rust tests. | PASS |
| 2 | Selected-repo absent/failed/loading states reject global fallback. Successful inline unbind now projects absent immediately, renders Configure tracker, and exposes no connected rows. | PASS; live desktop evidence pending |
| 3 | Migrated mismatch repair and configured mismatch preservation/typed error pass Rust and UI model evidence. | PASS |
| 4 | Full repo+slug+Project scope rejects deferred A after B and rejects late deleted-scope completion after unbind. | PASS |
| 5 | Exact normalized repository filtering precedes grouping, counting, searching, and selection; invalid item kinds/states remain excluded. | PASS |
| 6 | Repo-keyed writes preserve task preferences and isolate other repo rows; inline unbind now updates the current repo projection without global refresh or cross-repo mutation. | PASS; live configure/unbind evidence pending |
| 7 | Local/SSH target derivation and registered-host routing remain fail-closed; unknown repo identity cannot use a valid hint or local fallback. | PASS at executable gate; live SSH matrix pending |
| 8 | Repo switch clearing and exact linked-versus-unlinked worktree coordinates remain covered by production-seam tests. | PASS; persisted runtime inspection pending |

## Independent commands and results

- `bunx vitest run src/components/new-workspace/tracker-section-scope.test.ts` from the UI package — **PASS** (1 file, 2 tests).
- `jq empty .harness/feature_list.json` — **PASS**.
- `bash -n .harness/verify.sh` — **PASS**.
- `bash -n .harness/qa.sh` — **PASS**.
- `git diff --check` — **PASS**.
- `HARNESS_FEATURE_ID=binding-identity-fidelity bash .harness/verify.sh` — **PASS** (5 project-tracker tests, 4 resolver tests, diff check).
- `HARNESS_FEATURE_ID=wizard-closed-tracker-scope bash .harness/verify.sh` — **PASS** (4 files/71 focused tests; exact worktree test 1 passed/81 skipped; fresh Vite production build in 1m16s; diff check).
- `HARNESS_FEATURE_ID=binding-identity-fidelity bash .harness/qa.sh` — **PENDING as designed**, exit 2.
- `HARNESS_FEATURE_ID=wizard-closed-tracker-scope bash .harness/qa.sh` — **PENDING as designed**, exit 2.

The duplicate Rust test-attribute warning and Vite dynamic-import/chunk-size
warnings remain non-fatal and outside Spec 026.

## Handoff validation

`ai/skills/validate_handoff.md` — **PASS, 9/9**:

1. One shippable tracker-fidelity slice.
2. User-felt wrong-project issue leakage precedes the solution.
3. Multi-project and local/SSH operators are named.
4. All eight ACs have observable verify/QA assertions.
5. Non-goals are explicit.
6. Reuse/build seams cite concrete routes, models, editor, and store paths.
7. Launch, host, streaming, and configured-data invariants remain protected.
8. Both harness feature IDs and green-gate routes are present.
9. `ai/STATE.md` points to Spec 026 at Tester after Reviewer retry 2/2 and has a decision entry.

## Runtime QA and remaining risk

Real Agentum/xcode-theme, repeated/in-flight switching, inline unbind, SSH, and
linked/unlinked persistence QA was not run: this worktree still lacks a verified
current-build desktop plus named safe fixtures. Both QA routes correctly refuse
to pass. No tracker, repository, host, SSH fixture, or external state was
created or mutated.

Non-blocking documentation note for Reviewer: one comment above the selected
repo binding effect still says failure falls back to global `activeProject`,
while the implementation and tests correctly fail closed. The behavior is
green; the comment should be corrected opportunistically without changing the
gate verdict.
