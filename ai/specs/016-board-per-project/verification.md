# Verification — Spec 016 · Board lives inside each project

- **Spec:** `ai/specs/016-board-per-project/spec.md` (issue #360)
- **Role:** Tester (independent re-verification, autonomous SDD run)
- **Date:** 2026-07-14
- **Worktree HEAD:** `86f2b405` (`board-lives-inside-each-project-remove-sidebar-b`)
- **Product commits verified:** `f5eda0ee` (F1) · `ae4b44d8` (F2) · `4b98dd73` (F3), diffed against base `a26ba769`
- **Method:** every gate re-run from scratch; every AC checked against the code; full-suite failures compared against a clean baseline run of `a26ba769` in a throwaway worktree (removed after use). No numbers taken from the developer log.

## 1. Gate reproduction (all independently re-run)

| Gate | Claimed | Reproduced | Verdict |
|---|---|---|---|
| `bun run build` (crates/agentum-desktop/ui) | green | **exit 0, built in 1m 19s** (chunk-size warnings only, pre-existing) | ✅ |
| Targeted vitest (4 files) | 78/78 | **4 files passed, 78/78 passed** | ✅ |
| — `src/lib/board-project-resolution.test.ts` | 15 | **15 ✓** | ✅ |
| — `src/lib/board-route.test.ts` | 6 | **6 ✓** | ✅ |
| — `src/components/new-workspace/work-item-picker-model.test.ts` | — | **16 ✓** | ✅ |
| — `src/components/new-workspace/create-workspace-wizard-model.test.ts` | — | **41 ✓** | ✅ |
| Hub grep: `git grep -n 'updateSettings\|setTaskResumeState' -- …/ProjectHubPage.tsx` | empty | **empty (exit 1)** | ✅ |
| SidebarNav grep: `git grep -n '"Board"' -- …/SidebarNav.tsx` | empty | **empty (exit 1)**; case-insensitive `board` matches only 3 comments (`:17`, `:104`, `:123`) | ✅ |
| `openTaskPage(` audit outside `TaskPage.tsx`/tests | 3 detail callers | **exactly** `ChatPage.tsx:481` (openGitHubWorkItem payload), `WorktreeCard.tsx:530` (openGitHubWorkItem), `WorktreeCard.tsx:540` (openLinearIssue); the 4th grep hit (`board-route.ts:42`) is a doc comment, not a call. **Zero bare calls.** | ✅ |
| Must-not-touch: `git diff --name-only a26ba769..4b98dd73` | clean | **19 files, all in-scope.** Absent: `useComposerState*`, `ProjectBindingEditor.tsx`, `CreateWorkspaceWizard.tsx`, `create-workspace-wizard-model.ts`, `work-item-picker-model.ts`, `worktree-nav-history.ts`, `shared/types.ts`, anything under `crates/agentum-server/` or `crates/agentum-desktop/src/` | ✅ |

Note: the orchestrator's task message named the wizard test file `create-workspaceWizard-model.test.ts`; the actual file is `create-workspace-wizard-model.test.ts` (as in the developer handoff) — that is what was run.

### Full-suite new-failure hunt (baseline-proven, not assumed)

Ran the **entire** vitest suite twice: at HEAD and at base `a26ba769` (temp `git worktree`, same node_modules).

| | Base `a26ba769` | HEAD `4b98dd73`+ |
|---|---|---|
| Test files | 40 failed / 718 passed (758) | 39 failed / 721 passed (760) |
| Tests | 139 failed / 5984 passed (6123) | 138 failed / 6006 passed (6144) |

- **Failing test names at HEAD are a strict subset of the baseline: zero new failures** (compared at `file > test-name` granularity, `comm -23` empty).
- Total-test delta **+21 = exactly** the new spec-016 tests (15 resolver + 6 board-route).
- One file failed at base but passed at HEAD (`TabGroupPanel.sdd-bar.test.tsx`) — flaky-at-baseline; not a regression concern.
- The visible `ui.test.ts` failures (`closeTaskPage` nav-history expecting `'terminal'`) fail identically at base; the spec-016 `ui.ts` diff touches only the `projectHubTab` union and the `openProjectHub` seed param — nowhere near `closeTaskPage`.

## 2. Acceptance criteria

| AC | Verdict | Evidence |
|---|---|---|
| 1 — no sidebar Board; board inside hub Tasks | **PASS** | F3 diff deletes the Board button + all machinery (openTaskPage selector, canBrowseTasks, showTasksButton read, prefetch handler, provider derivation, warm-up effect, `tasksActive`, unused imports); only comments mention "Board"; Projects rail entry intact; AppearancePane "Show Tasks Button" toggle deleted, `showTasksButton` settings field kept (`shared/types.ts`/`constants.ts` field untouched). |
| 2 — A→X / B→Y, no settings write from hub | **PASS (deferred — runtime browser leg for qa.sh/staging)** | Code structure verified: `ProjectHubPage.tsx` contains zero `updateSettings`/`setTaskResumeState` (grep empty); binding lands in session-only `projectBindingByRepo[repo.id]` (`github.ts:1310-1327`); board resolves per `repo.id` through the resolver; `<TaskPage key={repo.id} embedded />` remount at `:238`. Actual hub-flip rendering is the qa.sh browser leg. |
| 3 — per-repo pick persists, siblings byte-unchanged, divergence hint | **PASS (deferred — runtime persistence leg)** | `commitSelection` body is `applyBoardPick(prev, repoId, …)` (`ProjectPicker.tsx:200-210`); tests assert `next.activeProject`/`repo-B` entry/`pinned` **reference-equal** to prev (test file `:190-193`); hint renders only on `source === 'pick' && divergesFromBinding` (`ProjectViewWrapper.tsx:776`), names bound title else `owner/#number`, "Use bound project" → `clearBoardPick` via fresh `getState()` (`:121-135`). |
| 4 — legacy-only settings still render; legacy never written by new code | **PASS (deferred — runtime render leg)** | Legacy read exists in exactly one new-code place: resolver step 3 (`board-project-resolution.ts:96`); test case 3 pins it. Audit of every `activeProject` writer: the only writer is the `repoId == null` standalone path (`:133`, pre-016 verbatim, decisions log #4); the two `activeProject: null` hits in `ProjectViewWrapper` (`:130`, `:326`) are null-settings fallback shapes, not writes. |
| 5 — SSH repo (hostId) resolves binding | **PASS (deferred — runtime SSH leg)** | Hub effect calls `getProjectBinding({ workdir: target.workdir, hostId: target.hostId })` (`ProjectHubPage.tsx:99`) with `target` from `deriveTrackerBindingTarget` (import-only, `:23`); `connectionId → hostId` covered by the green wizard-model test "SSH git repo → workdir + hostId". |
| 6 — bare openers re-route; detail openers unchanged | **PASS** | Palette `view-board` → `go(() => openBoardSurface())` (`CommandPalette.tsx:111`); `App.tsx:1281-1288` keeps gate + `preventDefault` + `notifyTerminalCapture`, body → `openBoardSurface()`; `useIpcEvents.ts:819-826` keeps the settings+has-git-repo gate; ChatPage filed-card fallback + "Open Board" header → `openBoardSurface({preferredRepoId, taskSource})`; detail openers intact (ChatPage:481, WorktreeCard:530/540); grep gate exact; `App.tsx:1756` `activeView === 'tasks'` route stays (D3); `worktree-nav-history.ts` untouched (not in diff). |
| 7 — build + vitest green | **PASS** | Build exit 0 (1m 19s); 78/78 across the 4 targeted files; run via vitest, not tsc. |

## 3. Architecture invariant spot-checks

- **Resolver ≡ §2.2** — read line-by-line: pick short-circuits `loading` with `divergesFromBinding: null` until loaded+complete; `pending` returned before binding/legacy tiers only when no pick; partial identities normalized away (`projectOwner` truthy AND `projectNumber != null`; `'organization'` exact-match else `'user'`); `repoId == null` skips pick map and binding entirely; failure written as `loaded/null` by the hub so `pending` cannot wedge. **All 13 §2.4 cases map to the 15 `it`s** (cases 1–11 → one `it` each, cases 4/8 carrying two scenarios in one `it`; case 12 → three `it`s incl. reference-equality sibling assertions; case 13 → missing-map tolerance for both helpers).
- **Embedded mode forcing ≡ §3.3** — `TaskPage.tsx:429-464`: `embeddedRepoId`-keyed resolver memo, `EMBEDDED_BINDING_ABSENT` fallback, ref-tracked resolution identity (manual toggle not fought), `pending` → no-op, `none` → `'items'`; resume effect `:1099` forces `'project'` locally when embedded; mode buttons gate `setTaskResumeState({githubMode})` on `!embedded` (`:3387`, `:3393`). Embedded TaskPage never writes `taskResumeState`.
- **Hub effect ≡ §4** — trigger + cancellation kept; no-target → `loaded/null`; loading state only written on first visit (loaded entry kept while refetching); raw identity+title stored; `.catch` fail-closed to `loaded/null`; `taskDataSeeded` gate/effect (`:68-76`) and render gate (`:238`) untouched.
- **Re-route table §5.3** — rows 1–6 verified in code (above); rows 7–10 untouched, proven by diff scope (WorktreeCard, worktree-nav-history, TaskPage internal nav not in the product diff except TaskPage's own §3.3 changes).
- **`openProjectHub` seed** — additive param only; `taskPageData` seed carries `taskSource`, never a detail payload (PM risk 1 respected); `projectHubTab` union now includes `'tracker'` (PM risk 5).
- **Wrapper feeds the picker the resolved value** — `activeProject = resolution.project` (`ProjectViewWrapper.tsx:110`) → picker display prop; `repoId` threaded (`:659`); `pending` renders `ProjectTableSkeleton` (`:812-815`), never the legacy flash or the choose-prompt.

## 4. Deviation audit (tasks.md)

| Deviation | Verdict |
|---|---|
| D-1: `@/shared/...` import alias instead of the architecture sketch's relative path | **Accurate.** The `lib/` convention is `@/shared` (`github-work-item-state.ts:1`, `browser-project.ts:1`); vite + vitest both resolve it (build + tests green). |
| D-2: SidebarNav preflight/linear warm-up effect removed with the prefetch machinery | **Accurate.** The F3 diff shows the effect fed only the removed prefetch chain (`visibleTaskProviders` → `resolvedDefaultTaskSource` → `handlePrefetch`). Every other consumer self-triggers the same checks on mount: `TaskPage.tsx:2750/2753`, `use-feature-wall-task-source-presentation.ts:33/36`, `SmartWorkspaceNameField.tsx:287/290`, onboarding (`IntegrationsStep.tsx:181`, `use-onboarding-flow.ts:357`), `IntegrationsPane.tsx:427-428`. |
| (architecture.md D-1: wizard not switched to the new resolver) | **Consistent** — wizard files byte-untouched (absent from diff), 41/41 wizard-model tests green. |

## 5. Defects found

- **Blocker:** none.
- **Should-fix:** none.
- **Info:**
  1. Removing SidebarNav's warm-up effect means `preflightStatus`/`linearStatus` are no longer warmed at app boot (SidebarNav was always mounted); the first consumer surface now pays the check latency on mount. Behavior-correct, minor perceived-latency shift only.
  2. `TabGroupPanel.sdd-bar.test.tsx` failed in the baseline run but passed at HEAD — flaky at baseline, worth an eye if it flaps in CI.
  3. Pre-existing full-suite baseline at this spec's own base is 40 failing files / 139 failing tests — unrelated to 016 (memory's "~38 files" baseline has drifted upstream, not here).

## 6. Verdict

**PASS-WITH-DEFERRALS.**

All seven ACs verified at the code level; all developer-claimed gate numbers independently reproduced exactly (build green, 78/78 targeted, grep gates exact, must-not-touch clean); zero new full-suite failures proven against a same-environment baseline run of the base commit. The runtime browser legs of AC 2/3/4/5 (hub A/B flip with persisted-settings assert, pick persistence across reload, legacy-only render, live SSH binding render) are **deferred to qa.sh/staging by design** — their code-level structure is verified here. Nothing blocks promotion to the QA gate.
