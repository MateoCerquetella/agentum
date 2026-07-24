# Review — Spec 016 · Board lives inside each project

- **Spec:** `ai/specs/016-board-per-project/spec.md` (issue #360)
- **Role:** Reviewer (adversarial final gate, autonomous SDD run)
- **Date:** 2026-07-14
- **Worktree HEAD:** `278f8bcc` (`board-lives-inside-each-project-remove-sidebar-b`)
- **Product commits reviewed:** `f5eda0ee` (F1) · `ae4b44d8` (F2) · `4b98dd73` (F3) against base `a26ba769`
- **Inputs:** spec.md, architecture.md, verification.md (tester: PASS-WITH-DEFERRALS), tasks.md, and every touched file read at HEAD. Method note: this review ran without a shell, so the diff was reviewed by reading the touched files at HEAD against the architecture's documented pre-state and the tester's independently-reproduced diff scope (19 files, must-not-touch clean); the one claim that strictly needs git history ("byte-identical to pre-016 `commitSelection`") was verified semantically against the architecture's quoted old body plus the test that pins the `repoId:null` shape, not by re-diffing history.

---

## 1. What worked well

- **Decision-bearing logic is pure and pinned.** `resolveBoardProject` / `applyBoardPick` / `clearBoardPick` (`crates/agentum-desktop/ui/src/lib/board-project-resolution.ts`) have zero React/store imports; the 15 tests pin precedence, pending/divergence, sibling reference-equality, and missing-map tolerance. This is exactly the right response to the "vite build doesn't typecheck" risk (architecture #8).
- **Per-repo keying makes the race surface almost trivially safe** — see §2.1 below: even if the hub effect's cancellation guard were removed, a stale write would land under its own repo's key with a convergent value. The design is race-tolerant by construction, not by ordering.
- **Sibling-write discipline is now enforced, not conventional.** All three `githubProjects` writers in the codebase (`ProjectPicker.tsx:195`, `ProjectViewWrapper.tsx:133`, `ProjectViewWrapper.tsx:329`) spread the whole object; the only writers of `activeProjectByRepo` are the two pure helpers.
- **The re-routes are surgical** — every gate, `preventDefault`, and `notifyTerminalCapture` preserved (details in §2.4).

## 2. Focus-area findings

### 2.1 Correctness under races / edge cases — **PASS**

- **Cancelled fetch writing `projectBindingByRepo`:** cannot happen — both `.then` and `.catch` check `cancelled` (`ProjectHubPage.tsx:98-120`). More importantly, even a hypothetical stale write would be harmless: the write key is the closure-captured `repo.id`, so repo A's late result can only land under A's key with A's correct binding — per-repo keying makes stale writes convergent, not corrupting.
- **Rapid A→B hub flips:** flipping away mid-fetch leaves A's entry stuck at `{status:'loading'}` while unmounted, but nothing reads it (A's TaskPage is unmounted), and revisiting A re-fires the fetch — the first-visit guard `if (!useAppStore.getState().projectBindingByRepo[repo.id])` (`ProjectHubPage.tsx:95`) correctly skips re-writing `loading` and the new fetch repairs the entry. Subsequent visits keep the `loaded` entry while refetching (no flicker). Correct.
- **Repo removed while its hub is open:** `useActiveRepo()` goes null → the "This project is no longer available" bail screen (`ProjectHubPage.tsx:156-171`); effect cleanup cancels the in-flight fetch. Correct.
- **"Use bound project" during a refetch:** `clearBoardPick` drops the pick; the resolver falls to the kept `loaded` binding; a refetch landing afterwards just overwrites the entry (worst case: binding was deleted server-side concurrently → falls to legacy/none, which is the truthful outcome). No wedge, no crash.

### 2.2 Resolver's untested seams — **PASS**

- **`applyBoardPick` repoId=null vs pre-016:** the legacy branch (`board-project-resolution.ts:132-133`) reproduces the documented old `commitSelection` body — recent unshift + triple-key dedupe + `slice(0,10)`, conditional `lastViewByProject[key]` only when `viewId` supplied, `activeProject` written as the bare `{owner, ownerType, number}` triple. `{...prev, recent, lastViewByProject, activeProject: pick}` preserves key insertion order (spread-first), so persisted bytes match pre-016 modulo the `lastOpenedAt` timestamp. Test `board-project-resolution.test.ts:207-215` pins the shape including `activeProjectByRepo` reference-equality. Caveat recorded above: verified semantically, not against a `git show` of the old body.
- **`clearBoardPick` on a missing key:** safe — spread of `?? {}` then no-op delete; test `:234-235` covers it. It does persist `activeProjectByRepo: {}` where the key was absent (harmless normalization — nit N4).
- **Settings persistence semantics:** `updateSettings` (`store/slices/settings.ts:257-302`) passes the update through `api.settings.set` with top-level keys stored wholesale and merges the response over complete in-memory settings — so a partial `githubProjects` write *would* clobber siblings, and none exists: all three write sites spread the whole object via the pure helpers or the pre-existing `handleSwitchView` pattern. The architecture's "whole-object spread required" claim is honored at every site.

### 2.3 Leak-proofing ("structurally impossible") — **PASS, with one first-frame gap (issue S1) and one docs overclaim (nit N2)**

- **Counterexample attempt (standalone `items` → hub → standalone):** fails to leak in either direction. Embedded resume forces local `'project'` and never reads the global slot (`TaskPage.tsx:1099`); the mode buttons gate `setTaskResumeState({githubMode})` on `!embedded` (`:3387`, `:3393`); the standalone visit afterwards restores its own `'items'` from the untouched global slot. The `key={repo.id}` remount (`ProjectHubPage.tsx:238`) resets `lastAppliedEmbeddedResolutionRef`, so no cross-repo identity carryover. Within a mount, the ref correctly lets a manual toggle survive re-renders and yields only when the resolution identity actually changes (`TaskPage.tsx:446-463`). The *mode* claim holds.
- **However, one real gap the tester's structural checks missed — S1 (Should-fix):** the "one skeleton frame" claim in architecture §3.1 is wrong for the first visit. A **missing** store entry maps to `BINDING_ABSENT = {status:'loaded', binding:null}` (`ProjectViewWrapper.tsx:74`, `TaskPage.tsx:183`), which resolves to **legacy** (or `none`) — *not* `pending`. Because React runs child effects before parent effects, on the first hub-Tasks visit of a session the wrapper paints and its auto-fetch + view-list effects (`ProjectViewWrapper.tsx:206-237`, `:269-304`) fire **for the legacy project** before `ProjectHubPage`'s binding effect (`:86-124`) ever writes `loading`. Consequences: (a) one paint of the wrong surface — the "Choose a project" prompt, or, if the legacy project's table is already in `projectViewCache` from an earlier standalone visit, a visible one-frame flash of the *wrong project's board*; (b) wasted `gh`/RPC fetches (`fetchProjectViewTable` + `listProjectViews`) for the legacy project. It converges correctly on the next frame (`loading` → `pending` skeleton → binding board), `activeProject` is never written, and AC 2's settings assertion holds — but this is exactly the flash decision #3 invented `pending` to prevent, so it should be tracked. Fix is small: treat a missing entry as `{status:'loading'}` when `repoId != null` (embedded), or seed the `loading` entry synchronously in `openProjectHub`.
- **Docs overclaim (nit N2):** architecture decision #5 and verification §3 say "embedded TaskPage never writes `taskResumeState`". Not literally true — embedded still writes non-mode keys (`githubItemsPreset`/`githubItemsQuery` at `TaskPage.tsx:2036-2039`, Linear context at `:1046/:1069` etc.), all **pre-existing pre-016 behavior** outside this spec's scope. The true (and sufficient) invariant is "embedded never writes `taskResumeState.githubMode`".

### 2.4 The six re-routes — **PASS**

- Palette `view-board` → `go(() => openBoardSurface())`, id/label/icon kept, no gate ever existed to lose (`CommandPalette.tsx:104-112`).
- `view.tasks` shortcut: gate, `e.preventDefault()`, and `notifyTerminalCapture('view.tasks')` all kept, body swapped (`App.tsx:1281-1288`).
- Native-menu `onOpenTasks`: settings + has-git-repo gate kept verbatim (`useIpcEvents.ts:819-826`).
- ChatPage filed-card fallback and "Open Board" header both route through `openBoardSurface` with `preferredRepoId` + `taskSource` (`ChatPage.tsx:507-510`, `:528-530`); the GitHub-with-number detail path is unchanged (`:481`).
- **Linear seed lands correctly:** `openProjectHub` writes `taskPageData: {preselectedRepoId, taskSource}` wholesale (`ui.ts:944-947`); because `preselectedRepoId` is already the hub's repo, `taskDataSeeded` is true immediately and the fresh-mounted embedded TaskPage's once-only resume effect reads `pageData.taskSource` (`TaskPage.tsx:1087-1092`) → Linear tab. Verified end-to-end.
- Nav-history parity (PM risk 4): hub-routed opens record no entry, identical to every existing `openProjectHub` call; old `'tasks'` entries still replay into the preserved standalone view. Ratified as accepted (architecture decision #6).

### 2.5 Security / robustness — **PASS (one growth nit)**

- Map keys are `Repo.id` — app-generated identifiers, not user free-text; no injection surface into the settings map.
- Stored values are public GitHub identifiers (`owner`, `ownerType`, `number`; session-only `projectTitle`). **No token or secret** enters `projectBindingByRepo` or settings.
- **Nit N3:** `activeProjectByRepo` has no GC — entries for removed repos persist in settings forever. Three scalar fields per repo; negligible size; acceptable, noted for a someday-cleanup.

### 2.6 D3 integrity (standalone board) — **PASS**

- The `activeView === 'tasks'` route stays (`App.tsx:1756` region, tester-verified); wrapper defaults `repoId=null` → resolver reads only legacy (`board-project-resolution.ts:75-95`, test case 11); the standalone picker writes legacy verbatim (`applyBoardPick:132-133`) — **no dead picker**; the divergence hint requires `source === 'pick'` so it can never render standalone (`ProjectViewWrapper.tsx:776`); detail openers (`ChatPage.tsx:481`, `WorktreeCard.tsx:530/:540`) and TaskPage-internal nav (`:624/:846/:3297`, embedded-merge wrapper `:201-210`) unchanged. Note for completeness: a standalone TaskPage opened with a `preselectedRepoId` (WorktreeCard detail) still keys the board by `null`/legacy — deliberate parity with pre-016 (§3.1's "null = standalone/global"), not a defect.

### 2.7 Consistency / stale docs / tester nits — **two findings + ratifications**

- **S2 (Should-fix):** the settings-search index still advertises the deleted toggle — `components/settings/appearance-search.ts:120-125` (`SIDEBAR_ENTRIES`: `title: 'Show Tasks Button', description: 'Show the Tasks button at the top of the left sidebar.'`) while the AppearancePane control was removed in F3. Settings search for "tasks"/"sidebar" now surfaces a ghost entry pointing at nothing. One-line deletion.
- **N1 (Nit):** stale comments in `ProjectHubPage.tsx` — the header (`:7-8`) still says "The rail's Chat / Wiki / Board entries stay the global, cross-project views" (the rail Board was removed by this very spec; rail Wiki by spec 009), and `:63-64`'s "a detour through the global Board (rail click, palette) wipes that data" cites entrances that no longer exist (the effect itself is still load-bearing for detail opens — keep it, fix the examples).
- **Tester's 3 info nits — all ratified as info, none escalate:** (1) warm-up latency shift after removing SidebarNav's preflight effect — behavior-correct, every consumer self-triggers on mount; accept. (2) `TabGroupPanel.sdd-bar.test.tsx` flaky at baseline — pre-existing; watch CI, no action here. (3) 40-file/139-test pre-existing full-suite baseline — upstream drift, unrelated to 016.

## 3. Issue classification (complete list)

| ID | Finding | Class |
|---|---|---|
| S1 | First-frame legacy/none render + spurious legacy-project fetches on the first hub-Tasks visit per session (missing binding entry → `BINDING_ABSENT` resolves past `pending`; child effects fire before the hub's `loading` write). Contradicts architecture §3.1's "skeleton frame" claim. Converges in one frame; no settings write; no wrong writes. | **Should-fix** — follow-up ticket (map missing entry → `loading` when embedded, or seed `loading` in `openProjectHub`) |
| S2 | Ghost "Show Tasks Button" entry in `appearance-search.ts:120-125` after the toggle's deletion. | **Should-fix** — fold into the same follow-up ticket |
| N1 | Stale comments `ProjectHubPage.tsx:7-8` and `:63-64`. | Nit — leave-as-is (fix opportunistically) |
| N2 | Docs overclaim "embedded never writes `taskResumeState`" (true only for `.githubMode`; preset/query/linear writes are pre-existing). | Nit — docs-only |
| N3 | `activeProjectByRepo` never GC'd on repo removal. | Nit — accepted, noted |
| N4 | `clearBoardPick` persists `activeProjectByRepo: {}` on upgraded profiles where the key was absent. | Nit — harmless |

**Blockers: none.**

## 4. Locked decisions & PM risks — explicit statement

- **D1 (pick-wins + divergence hint): HONORED.** Precedence implemented exactly (`board-project-resolution.ts:75-103`, tests 1–3); hint renders only on `pick && divergesFromBinding` with bound title else `owner/#number` and one-click `clearBoardPick` (`ProjectViewWrapper.tsx:776-795`, `:121-135`); transitions untouched (task_sink/tracker machinery absent from the diff); no dead picker anywhere.
- **D2 (bare openers repo-first, Projects fallback): HONORED.** `resolveBoardRoute` is exactly preferred→active→projects with the live-git-repo guard and no first-repo fallback (`board-route.ts:13-36`, 6 tests); all four bare openers + two ChatPage sites re-routed with gates preserved (§2.4).
- **D3 (standalone `'tasks'` surface stays live): HONORED.** §2.6; detail openers, internal nav, and nav-history replay untouched.
- **PM risks 1–7:** 1 — honored (`openProjectHub` seed carries only `taskSource`, never a detail payload, `ui.ts:944-947`); 2 — honored with the S1 first-frame caveat (forcing fires for pick/binding/legacy, `none` actively selects `'items'`, never persists globally); 3 — honored and test-pinned (§2.2); 4 — honored-as-accepted (decision #6 parity, ratified); 5 — honored (`'tracker'` in the union, `ui.ts:503`); 6 — honored (`deriveTrackerBindingTarget` import-only, `hostId` threaded, `ProjectHubPage.tsx:88/:99`); 7 — honored (live-repo guard in `resolveBoardRoute`; `none` → items so the forcing cannot leak into the fallback Kanban).
- **Deviation D-1** (wizard not switched to the new resolver): ratified — the wizard's binding-first order feeds automation, and the must-not-touch list forbids changing it; files verified byte-untouched (absent from the diff, 41/41 wizard tests green).

## 5. Recommendations

1. File one follow-up ticket carrying S1 + S2 (both are small; S1's fix is a few lines in `ProjectViewWrapper.tsx`/`TaskPage.tsx` or `ui.ts`, S2 is a deletion). Neither blocks release; S1 should land before the board surface gets heavier per-frame fetch costs.
2. During qa.sh's hub-flip leg, explicitly watch the **first** Tasks-tab open of a bound repo after visiting the standalone board — that is the S1 reproduction recipe (cached legacy table → visible one-frame wrong-board flash).
3. When touching `ProjectHubPage.tsx` next, refresh the two stale comments (N1) and reword the "never writes taskResumeState" claims to "never writes `taskResumeState.githubMode`" (N2).

## 6. Final verdict

**SIGN-OFF — ship-ready.** Zero blockers; two should-fix items to a follow-up ticket (S1, S2); four nits recorded. All seven ACs stand as verified by the tester, D1/D2/D3 and PM risks 1–7 are honored in code, and the adversarial passes (races, leak counterexamples, write-path byte-discipline, re-route gate order, secret hygiene) found nothing that changes the outcome.

Remaining steps (human/orchestrator):
1. Move `ai/specs/016-board-per-project/spec.md` **Status → Done**.
2. Run the deferred qa.sh browser legs per verification.md §6 (hub A/B flip with persisted-settings assert, pick persistence across reload, legacy-only render, live SSH binding render) — plus the S1 watch above.
3. Open the PR into `develop` with `Closes #360` in body and commit message; file the S1+S2 follow-up issue and link it from the PR.
4. Promote develop → staging (QA, `status/qa`) → main + tag per the release convention; #360 closes when the merge reaches `main`.
