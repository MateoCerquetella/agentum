# Handoff 03 — Developer → Tester (spec 016, 2026-07-14, autonomous)

**Developer gate:** PASS (orchestrator spot-checked). Slices:
- F1 `f5eda0ee` — per-repo board resolution (9 files, +638/−37). Gates: build 1m01s, resolver 15/15, F1 set 72/72.
- F2 `ae4b44d8` — hub binding retarget (2 files, +66/−39). Gates: build 1m47s, 72/72, hub grep empty.
- F3 `4b98dd73` — sidebar removal + re-routes (10 files, +209/−162). Gates: build 1m11s, 21/21, grep gates exact.
- Final combined: 4 test files 78/78; must-not-touch diff over `a26ba769..HEAD` empty.

## Tester must (independently — do NOT trust the numbers above)

1. Re-run every gate: `bun run build` (from `crates/agentum-desktop/ui`);
   `bunx vitest run src/lib/board-project-resolution.test.ts src/lib/board-route.test.ts
   src/components/new-workspace/work-item-picker-model.test.ts
   src/components/new-workspace/create-workspace-wizard-model.test.ts`.
2. Verify each AC in `spec.md` against the code (AC 1–7); classify each
   PASS / PASS(deferred qa.sh/staging — runtime browser legs) / FAIL.
3. Verify the architecture invariants line-by-line where cheap:
   - resolver precedence + `pending` semantics vs §2.2 (read the module);
   - `applyBoardPick` sibling preservation (test 12/13 exist and assert it);
   - hub effect: no `updateSettings`/`setTaskResumeState` anywhere in
     `ProjectHubPage.tsx`; `deriveTrackerBindingTarget` is import-only
     (wizard model file byte-untouched — `git diff a26ba769..HEAD` scope);
   - embedded TaskPage never writes `taskResumeState` (grep the `!embedded`
     gates); standalone picker path still writes legacy `activeProject`;
   - re-route table rows 2–6 gates + preventDefault preserved; rows 7–10
     untouched (WorktreeCard/ChatPage:481 detail payloads intact,
     worktree-nav-history untouched).
4. Audit the 2 logged deviations (tasks.md) for accuracy: `@/shared` alias
   import; SidebarNav warm-up effect removal scope.
5. Check the full vitest suite ONLY to prove failures pre-existing (known
   baseline) — new-failure hunt, not a green requirement.

Verdict + evidence → `verification.md` in the spec dir (return content in
your final message; you are read-only except Bash for running tests).
