# Architecture — Spec 016 · Board lives inside each project

- **Spec:** `ai/specs/016-board-per-project/spec.md` (Status: PM, issue #360)
- **Author:** Claude (Architect role, autonomous SDD run)
- **Date:** 2026-07-13
- **Base:** worktree rebased onto `origin/develop` v0.75.1 — every `path:line` below was re-verified on this HEAD. All paths are relative to `crates/agentum-desktop/ui/src/` unless prefixed.
- **Surface:** UI-only. Zero Rust edits; the binding API is already host-aware (`getProjectBinding` accepts `hostId` → `host_id`, `runtime/github-projects-client.ts:124-152`).

## Shape of the change (one paragraph)

Today one global settings slot (`settings.githubProjects.activeProject`, read at `components/github-project/ProjectViewWrapper.tsx:87`, written at `components/github-project/ProjectPicker.tsx:212-217` and by the hub copy-hack at `components/project-hub/ProjectHubPage.tsx:113-115`) decides what every board surface shows. We add a per-repo map `activeProjectByRepo` to settings, a pure resolver (`pick → binding → legacy global`) in `lib/`, a session-only per-repo binding cache in the github store slice (populated by the retargeted hub effect, hostId-aware), and thread `repoId` into `ProjectViewWrapper`/`ProjectPicker` so the embedded hub board reads/writes per-repo while the standalone `activeView === 'tasks'` surface (kept alive per D3) behaves exactly as before. The sidebar Board entry is deleted and every bare board opener re-routes to `openProjectHub(repoId, 'tasks')` with an `openProjectsPage()` fallback (D2).

---

## 1. Settings-shape change

### 1.1 Type

`shared/github-project-types.ts` — `GitHubProjectSettings` (verified `:224-234`):

```ts
export type GitHubProjectSettings = {
  pinned: { owner: string; ownerType: GitHubProjectOwnerType; number: number }[]
  recent: { owner: string; ownerType: GitHubProjectOwnerType; number: number; lastOpenedAt: string }[]
  lastViewByProject: Record<string, { viewId: string }>
  /** LEGACY global slot. Read-only for new code (migration fallback, AC 4);
   *  still written ONLY by the standalone (repoId=null) picker path — see §3.4. */
  activeProject: { owner: string; ownerType: GitHubProjectOwnerType; number: number } | null
  /** Spec 016: the per-repo board pick, keyed by Repo.id. OPTIONAL on the wire —
   *  upgraded profiles carry a stored `githubProjects` object without this key
   *  (see §1.3), so every read must be `?? {}`. */
  activeProjectByRepo?: Record<
    string,
    { owner: string; ownerType: GitHubProjectOwnerType; number: number }
  >
}
```

The field is **optional by design**, not an oversight — see migration semantics below.

### 1.2 Stable-shape default

`shared/constants.ts` — the `githubProjects` default (verified `:320-325`) gains one line:

```ts
githubProjects: {
  pinned: [],
  recent: [],
  lastViewByProject: {},
  activeProject: null,
  activeProjectByRepo: {}
},
```

### 1.3 Persistence path + migration semantics

- **Write path:** unchanged — `useAppStore.getState().updateSettings({ githubProjects: next })`, which flows through `store/slices/settings.ts:257-302` → `api.settings.set(...)`. Settings deep-merges **only** notifications, so every writer must spread the **whole** `githubProjects` object (the existing discipline documented at `ProjectPicker.tsx:183-186` and exercised by `handleSwitchView` at `ProjectViewWrapper.tsx:277-292`). All new writes go through the pure `applyBoardPick`/`clearBoardPick` helpers (§2.3) so sibling preservation is testable, not conventional.
- **Migration is lazy, read-side, and write-through — no migration pass.** The settings hydrate merge at `settings.ts:250-251` (`{ ...getDefaultSettings('~'), ...stored }`) is **top-level shallow**: an upgraded profile's stored `githubProjects` object *replaces* the default wholesale, so `activeProjectByRepo` is `undefined` until the first per-repo pick writes it. Consequences, all mandatory:
  1. every read is `settings?.githubProjects?.activeProjectByRepo ?? {}`;
  2. `applyBoardPick` spreads `...(prev.activeProjectByRepo ?? {})` before adding the entry;
  3. the first per-repo pick persists the map; older keys are never rewritten.
- **Legacy `activeProject` fallback — where exactly it is read by new code:** in exactly **one** place, step 3 of the pure resolver (§2.2). `ProjectViewWrapper.tsx:87`'s direct read is *replaced* by the resolver call; `CreateWorkspaceWizard.tsx:140` keeps its own existing read (untouched, §6 must-not-touch). New code **never writes** `activeProject`; the only surviving writer is the preserved standalone-picker path (§3.4), which is the pre-016 behavior verbatim.

---

## 2. The pure resolver

### 2.1 Module

**`lib/board-project-resolution.ts`** + **`lib/board-project-resolution.test.ts`** — colocated, following the repo's pure-model convention (header comment explaining *why pure*, exported structural input types, zero React/store/DOM imports; model: `lib/start-gated-run-precondition.ts`, `components/new-workspace/work-item-picker-model.ts:120-137`).

It intentionally **generalizes** `resolvePickerProject` (spec 011 F2, `work-item-picker-model.ts:120`) — same binding-identity normalization rules (complete identity only: `projectOwner` truthy AND `projectNumber != null`; `ownerType === 'organization'` exact-match else `'user'`) with two additions: the per-repo pick tier on top, and pending/divergence semantics. We do **not** modify `resolvePickerProject` itself (wizard stays byte-untouched).

### 2.2 Exported signatures

```ts
import type { GitHubProjectOwnerType, GitHubProjectSettings } from '../../../shared/github-project-types'

export type BoardProjectRef = {
  owner: string
  ownerType: GitHubProjectOwnerType
  number: number
}

/** The identity fields read off ProjectBindingDto (free-string ownerType on the wire). */
export type BoardBindingIdentity = {
  projectOwner: string | null
  projectOwnerType: string | null
  projectNumber: number | null
  projectTitle?: string | null
}

/** The session cache entry the hub effect writes (§4) and consumers read. */
export type BoardBindingState =
  | { status: 'loading' }
  | { status: 'loaded'; binding: BoardBindingIdentity | null }

export type BoardProjectResolution =
  /** Explicit per-repo pick wins (D1). `divergesFromBinding` is the BOUND
   *  project when the pick differs from a complete, loaded binding — drives
   *  the non-blocking hint; null otherwise (including while binding loads). */
  | { source: 'pick'; project: BoardProjectRef; divergesFromBinding: BoardProjectRef | null }
  | { source: 'binding'; project: BoardProjectRef }
  | { source: 'legacy'; project: BoardProjectRef }
  /** Nothing resolved → plain issue Kanban; NEVER force githubMode 'project' (PM risks 2/7). */
  | { source: 'none'; project: null }
  /** No pick and the binding fetch is in flight → hold (skeleton), do NOT
   *  flash the legacy project then swap (AC 2's A→X/B→Y case). */
  | { source: 'pending'; project: null }

export function resolveBoardProject(input: {
  /** null = the standalone (non-hub) surface: pick map and binding are skipped. */
  repoId: string | null
  settings:
    | Pick<GitHubProjectSettings, 'activeProject' | 'activeProjectByRepo'>
    | null
    | undefined
  bindingState: BoardBindingState
}): BoardProjectResolution
```

Precedence, exactly:

1. **Pick** — `settings?.activeProjectByRepo?.[repoId]` (only when `repoId != null`). Short-circuits even while `bindingState.status === 'loading'` (a pick needs no fetch → synchronous render); `divergesFromBinding` is computed only once the binding is loaded and complete, and only when it differs by `(owner, ownerType, number)` triple.
2. **Binding** — a complete loaded binding identity, normalized as in `resolvePickerProject` (partial identities ignored, never half-resolved).
3. **Legacy global** — `settings?.activeProject` (the ONLY new-code read of the legacy slot). Applies for `repoId == null` too (standalone surface).
4. **`none`** — no forcing of `'project'` mode; consumer renders the plain issue list/Kanban (§3.3).
5. **`pending`** — special-cased *before* 2–4: no pick AND `status === 'loading'` → hold. Binding fetch *failure* is written as `{status:'loaded', binding:null}` by the hub effect (§4), so `pending` can never wedge.

### 2.3 The settings-write helpers (PM risk 3 made testable)

Same module — the commitSelection retarget is expressed as pure settings transforms so verify.sh asserts sibling preservation instead of trusting review:

```ts
/** ProjectPicker's commit, retargeted. repoId != null → write the pick to
 *  activeProjectByRepo[repoId]; recent + lastViewByProject stay GLOBAL
 *  (project-keyed, repo-agnostic — PM risk 3); legacy activeProject is
 *  byte-untouched. repoId == null → the pre-016 standalone behavior verbatim
 *  (writes activeProject; the preserved legacy write path, §3.4). */
export function applyBoardPick(
  prev: GitHubProjectSettings,
  repoId: string | null,
  selection: { owner: string; ownerType: GitHubProjectOwnerType; number: number; viewId?: string }
): GitHubProjectSettings

/** The hint's one-click "Use bound project": delete the per-repo entry, all
 *  siblings (incl. legacy activeProject and every OTHER repo's entry) untouched. */
export function clearBoardPick(prev: GitHubProjectSettings, repoId: string): GitHubProjectSettings
```

`applyBoardPick` reproduces `commitSelection`'s current body (`ProjectPicker.tsx:194-217`: recent-dedupe-slice(10), conditional `lastViewByProject[key]`) with only the active-slot destination switched on `repoId`.

### 2.4 Test file + the cases verify.sh asserts

**`lib/board-project-resolution.test.ts`** (`bunx vitest run src/lib/board-project-resolution.test.ts` — never bare `tsc`; `shared/*` is a vite alias). Cases, matching the spec's verify.sh contract plus the D1 mechanics:

1. **pick beats binding beats legacy** — all three present → `source:'pick'` with the pick's ref.
2. **binding beats legacy** — no pick, loaded complete binding + legacy set → `source:'binding'`.
3. **legacy fallback (AC 4)** — no pick, loaded `binding:null`, legacy set → `source:'legacy'`.
4. **unknown repo falls through** — `repoId` absent from the map → binding/legacy tiers as above.
5. **no result** — nothing set, loaded null binding → `{source:'none', project:null}`.
6. **pending holds, never legacy-flashes** — no pick + `{status:'loading'}` → `source:'pending'` even with legacy set.
7. **pick short-circuits loading** — pick + loading → `source:'pick'`, `divergesFromBinding:null`.
8. **divergence hint derivation** — pick ≠ loaded binding → `divergesFromBinding` = bound ref; pick == binding (triple-equal) → `null`.
9. **partial binding ignored** — `projectOwner:null` or `projectNumber:null` → falls to legacy.
10. **ownerType normalization** — `'organization'` → organization; `'USER'`/garbage/null → `'user'` (mirrors `resolvePickerProject`).
11. **repoId null skips the pick map** — a map entry for some repo never leaks into the standalone resolution.
12. **no write-path returns the legacy slot** — `applyBoardPick(prev, 'repo-A', sel)`: `next.activeProject` is reference/byte-equal to `prev.activeProject`, `next.activeProjectByRepo['repo-B']` unchanged, `recent`/`lastViewByProject` updated globally; with `repoId:null` it reproduces today's shape (writes `activeProject`, no map entry). `clearBoardPick` deletes exactly one key.
13. **missing-map tolerance** — `applyBoardPick` on a settings object without `activeProjectByRepo` (upgraded profile) produces `{ [repoId]: pick }` without throwing.

---

## 3. Repo-context threading

### 3.1 Decision: **prop for `repoId`, store for everything else**

`ProjectViewWrapper` gains `repoId?: string | null` (default `null`) and passes it to `ProjectPicker`. Justification over a store selector (`useActiveRepo`, `store/selectors.ts:102-104`):

- `activeRepoId` is **global app state** — exactly the coupling this spec removes. The embedded board must be keyed by *the hub that mounted it*, not by whatever repo is globally active (a background `setActiveRepo` from the sidebar would silently re-key the board — the same class of bug as the copy-hack).
- The embedded TaskPage **already owns the repo context**: the hub seeds `taskPageData.preselectedRepoId` and gates mount on it (`ProjectHubPage.tsx:65-73`, render gate `:237`), and TaskPage reads it at `TaskPage.tsx:251-253`. One prop hop (`TaskPage → ProjectViewWrapper → ProjectPicker`) is explicit and remount-safe (`key={repo.id}` at `ProjectHubPage.tsx:237` already remounts per project).
- `repoId == null` cleanly encodes "standalone/global context" for the D3-preserved `activeView === 'tasks'` surface (`App.tsx:1754`).

Concretely:

- `TaskPage.tsx:4062`: `<ProjectViewWrapper />` → `<ProjectViewWrapper repoId={embedded ? (pageData.preselectedRepoId ?? null) : null} />`.
- `ProjectViewWrapper.tsx:58/74`: `type Props = { repoId?: string | null }` (replaces `Record<string, never>`).
- `ProjectViewWrapper.tsx:87`: `const activeProject = ...activeProject ?? null` → `const resolution = useMemo(() => resolveBoardProject({ repoId, settings: settings?.githubProjects, bindingState }), [...]); const activeProject = resolution.project`. Everything downstream of `activeProject` (`lastViewByProject` keys, view-list cache, `projectViewCacheKey`, live-refresh at `:199-220`) is already project-keyed and needs **no change**.
- `bindingState` comes from the store map (§4): `useAppStore((s) => (repoId ? s.projectBindingByRepo[repoId] : undefined)) ?? { status: 'loaded', binding: null }` — a missing entry (hub effect not run yet for a pick-less repo renders one skeleton frame; standalone always `loaded/null`).

### 3.2 Where the binding fetch lives: **the hub effect, cached in the github slice** (not the wrapper)

Per the handoff ("hub effect… retargeted, not rewritten") and because the wrapper must stay mountable in the standalone context with zero fetching. New session-only state in `store/slices/github.ts` (adjacent to `projectViewCache`, `:1269-1270/:1315`):

```ts
// GithubSlice additions
projectBindingByRepo: Record<string, BoardBindingState>        // init: {}
setProjectBindingState: (repoId: string, state: BoardBindingState) => void
```

Not persisted; repopulated on each hub Tasks-tab visit (the fetch is one cheap loopback GET). The wrapper and the embedded TaskPage both read it; only the hub effect writes it. Refetch policy in §4.

### 3.3 PM risk 2 — where `githubMode: 'project'` forcing lives now

**Inside the embedded TaskPage, as local state derived from the resolver — the global `taskResumeState` slot is never touched from the hub.** Mechanics (all in `TaskPage.tsx`):

- TaskPage (embedded only) computes the same `resolveBoardProject` memo (it already subscribes to `settings`; add the `projectBindingByRepo` read for `pageData.preselectedRepoId`).
- An effect applies the resolution to the **local** `githubMode` state (`:412`): resolution source `pick|binding|legacy` → `setGithubMode('project')`; source `none` → `setGithubMode('items')` (plain issue Kanban, PM risk 7); source `pending` → no-op (initial state `'project'` shows the wrapper skeleton, no flash). The effect tracks the last-applied resolution identity in a ref so it re-fires only when the resolution *changes* — a user's manual Projects/Issues toggle inside the hub is not fought.
- The resume effect (`:1047`) skips the global slot when embedded: `const nextGithubMode = embedded ? 'project' : (taskResumeState?.githubMode ?? 'project')` — a stale global `'items'` (from a standalone visit) can no longer leak into a bound repo's hub, which is the entire reason the copy-hack's forcing existed.
- The mode buttons (`:3328-3336`) gate their `setTaskResumeState({ githubMode })` calls on `!embedded` (local `setGithubMode` always runs). This extends the existing embedded discipline already documented at `:3377-3382` ("no updateSettings writes … from inside a per-project view") to the resume slot.

Result: the forcing fires for pick OR binding OR legacy (PM risk 2's "whenever the resolver yields a project"), never persists globally, and never leaks into an unbound repo (source `none` actively selects `'items'`).

### 3.4 PM risk 3 — the surgical `commitSelection` split (ProjectPicker)

`ProjectPicker.tsx` changes:

- **Props** (`:36-44`): add `repoId?: string | null`. The `activeProject` display prop (`:37-42`) is **fed the resolved per-repo value** by the wrapper — the wrapper already passes its `activeProject` local (built at `:614-631`), which after §3.1 *is* `resolution.project`, so the picker button label follows pick/binding/legacy automatically. No picker-internal read changes.
- **`commitSelection`** (`:191-229`): the `updateProjectSettings(prev => …)` mutate body (`:194-218`) is replaced by `applyBoardPick(prev, repoId ?? null, selection)`. That is the whole split: active-slot write per-repo when `repoId` set; `recent` + `lastViewByProject` global in both branches; `repoId == null` (standalone surface) keeps writing the legacy `activeProject` **verbatim** — this is the one surviving legacy write, deliberately preserved so the D3 standalone board isn't left with a dead picker (see Decisions log #4).
- Everything else in the picker (pinned/recent/browse/paste, `updateProjectSettings`'s whole-object spread at `:179-189`) is untouched.

### 3.5 Divergence hint (D1)

Rendered by `ProjectViewWrapper` in its header bar (the `:613-726` toolbar region), only when `resolution.source === 'pick' && resolution.divergesFromBinding != null`:

- Copy: non-blocking, names the bound project — title from the store binding entry's `projectTitle` when present, else `owner/#number` — e.g. *"This project's tracker binding is **{title}** — status automation writes there."*
- One action: **"Use bound project"** → `updateSettings({ githubProjects: clearBoardPick(prev, repoId) })` (same fresh-`getState()` read discipline as `handleSwitchView`, `:277-283`), after which the resolver yields `source:'binding'` and the board swaps.
- Never rendered when `repoId == null` (standalone), when binding is loading, or when pick == binding.

### 3.6 `hostId` threading

One derivation, reused: `deriveTrackerBindingTarget({ repo, isGit })` (`components/new-workspace/create-workspace-wizard-model.ts:204-214`, the v0.75.1/#356 host-aware pattern, vitest-covered) maps `Repo.connectionId` (`shared/types.ts:92`) → `{ workdir, hostId? }`. The hub effect imports it as-is (import-only; the wizard model file is not edited). The wrapper/picker never see `hostId` — binding fetch is the hub's job (§3.2), and all board *data* fetches (`fetchProjectViewTable`, `listProjectViews`) already route through the runtime-RPC seam untouched.

---

## 4. Hub effect retarget (`ProjectHubPage.tsx:81-123`)

The effect keeps its trigger (`repo`, `tab === 'tasks'`, `isGitRepoKind(repo)` — verified `:82`) and its cancellation pattern; its **body** changes from copy-into-global to write-per-repo:

```
Old (:84-118)                                  New
──────────────────────────────                 ──────────────────────────────
getProjectBinding({ workdir: repo.path })      const target = deriveTrackerBindingTarget({ repo, isGit: true })
  // no hostId → SSH repos always blind        if (!target) → setProjectBindingState(repo.id, {status:'loaded', binding:null}); return
                                               if no entry yet for repo.id → setProjectBindingState(repo.id, {status:'loading'})
                                               //  (an existing 'loaded' entry is kept while refetching — no flicker)
                                               getProjectBinding({ workdir: target.workdir, hostId: target.hostId })
s.setTaskResumeState({githubMode:'project'})     .then(res => setProjectBindingState(repo.id,
  // GLOBAL persisted slot — GONE (§3.3)                 { status:'loaded', binding: res.binding /* identity+title subset */ }))
s.updateSettings({ githubProjects:               .catch(() => setProjectBindingState(repo.id, {status:'loaded', binding:null}))
   {...gh, activeProject: {...}} })              //  fail-closed → resolver falls to legacy (AC 4), never wedges 'pending'
  // GLOBAL settings write — GONE (AC 2)
```

Invariants:

- **No `updateSettings` call and no `setTaskResumeState` call remain anywhere in `ProjectHubPage.tsx`** (AC 2's "emits no settings write"; qa.sh flips hubs A/B and asserts persisted `activeProject` unchanged).
- The incomplete-identity guard (old `:92-94`) moves into the resolver's normalization — the effect stores the raw identity; the resolver decides completeness.
- `'project'`-mode forcing moved to the embedded TaskPage (§3.3).
- The `taskDataSeeded` re-assert effect (`:65-73`) and the render gate (`:237`) are untouched — they are what makes `repoId` threading trustworthy.
- SSH (AC 5): `target.hostId` = `repo.connectionId` → server resolves the slug on the remote host. Pure client threading, exactly PM risk 6's diagnosis.

---

## 5. Sidebar removal + D2 re-route table

### 5.1 The route helper

**`lib/board-route.ts`** + **`lib/board-route.test.ts`**:

```ts
/** D2: where a bare "open the board" gesture lands. Pure. */
export function resolveBoardRoute(input: {
  repos: ReadonlyArray<Pick<Repo, 'id' | 'kind'>>   // whatever isGitRepoKind needs
  preferredRepoId?: string | null                    // e.g. ChatPage's filedRepoId
  activeRepoId: string | null                        // null on cold start (settings.ts:42 reset shape)
}): { kind: 'hub'; repoId: string } | { kind: 'projects' }
```

Order (D2 verbatim): `preferredRepoId` if it resolves to a **live git repo** in `repos` → else `activeRepoId` under the same guard (PM risk 7's stale-id check) → else `{kind:'projects'}`. **No** first-git-repo fallback — D2 enumerates exactly these two. A thin non-pure dispatcher in the same file keeps call sites one-line:

```ts
export function openBoardSurface(seed?: { preferredRepoId?: string | null; taskSource?: TaskProvider }): void
// reads useAppStore.getState(); dispatches openProjectHub(repoId, 'tasks', {taskSource}) or openProjectsPage()
```

Tests: preferred wins; stale preferred falls to active; stale active falls to projects; non-git repos excluded; empty repos → projects.

### 5.2 `openProjectHub` seed extension (small, typed)

`store/slices/ui.ts:508/:929-942`: `openProjectHub(repoId, tab?, seed?: { taskSource?: TaskProvider })`; `:938` becomes `taskPageData: { preselectedRepoId: repoId, ...(seed?.taskSource ? { taskSource: seed.taskSource } : {}) }`. This is **not** detail-payload threading (PM risk 1 respected — `openGitHubWorkItem`/`openLinearIssue` still never pass through here); it preserves ChatPage's Linear filed-card landing on the Linear tab (TaskPage already consumes `pageData.taskSource` in its resume effect, `TaskPage.tsx:1041`). The hub's re-assert effect merges (`{...s.taskPageData, preselectedRepoId}`, `ProjectHubPage.tsx:70-72`), so the seed survives.

Also here: extend the `projectHubTab` union at `ui.ts:503` with `'tracker'` to match `ProjectHubPage.tsx:30`'s `HubTab` (PM risk 5 — extend, don't fork; this latent drift only survives because vite build doesn't typecheck).

### 5.3 Re-route table (every caller, old → new)

| # | Call site (verified) | Today | After 016 |
|---|---|---|---|
| 1 | `components/sidebar/SidebarNav.tsx:226-255` Board button (+ selector `:94`, `showTasksButton` read `:105`, prefetch handler `:149-182` and its now-unused imports) | `openTaskPage()` gated on `canBrowseTasks` | **Deleted** (AC 1). No re-route — Projects rail entry (`:217-222`) is the path. Also delete the dead "Show Tasks Button" toggle in `components/settings/AppearancePane.tsx:290-301`; keep the `showTasksButton` field in `shared/types.ts:2033`/`constants.ts:246` (persisted-settings compat, no churn). |
| 2 | `components/CommandPalette.tsx:103-109` (`view-board`, selector `:66`) | `go(() => openTaskPage())`, no gate | `go(() => openBoardSurface())` — repo resolves → hub Tasks; else Projects page (the fallback *is* the no-git-repo handling; palette had no gate to preserve). Keep id/label/icon. |
| 3 | `App.tsx:1280-1288` `view.tasks` shortcut | gate `store.repos.some(isGitRepoKind)` (`:1282`) then `store.openTaskPage()` | **Keep the gate + preventDefault exactly** (D2), body → `openBoardSurface()`. |
| 4 | `hooks/useIpcEvents.ts:817-825` `onOpenTasks` (native menu) | gate `activeView !== 'settings' && has git repo` (`:820`) then `store.openTaskPage()` | **Keep the gate exactly**, body → `openBoardSurface()`. |
| 5 | `components/harness/ChatPage.tsx:503-506` (filed-card, Linear or no issue number) | `openTaskPage({ preselectedRepoId: filedRepoId, taskSource })` | `openBoardSurface({ preferredRepoId: filedRepoId, taskSource: filed.provider === 'linear' ? 'linear' : 'github' })`. |
| 6 | `components/harness/ChatPage.tsx:523-526` ("Open Board" header — standalone only, `pinnedRepo` is null in that branch per `:515`) | `openTaskPage({ preselectedRepoId: …, taskSource: 'github' })` | `openBoardSurface({ preferredRepoId: workspaceId ?? undefined, taskSource: 'github' })`. |
| 7 | `components/harness/ChatPage.tsx:480-500` (filed-card **detail**: `openGitHubWorkItem`) | `openTaskPage({ …detail payload })` | **Unchanged** (D3; PM risk 1). |
| 8 | `components/sidebar/WorktreeCard.tsx:530` / `:540` (issue / Linear **detail**) | `openTaskPage({ …openGitHubWorkItem / openLinearIssue })` | **Unchanged** (D3). |
| 9 | `TaskPage.tsx:576/798/853/3245` (internal nav + `routedOpenBoardPage` sync-toast) | `openTaskPage(...)` (embedded-merge wrapper `:194-203`) | **Unchanged** — inside `TaskPage.tsx`, exempt from the grep gate by the spec's own wording. |
| 10 | `store/slices/worktree-nav-history.ts:223-238` history replay (`viewActivator('tasks')`) | `setActiveView('tasks')` direct | **Unchanged** (D3): old `'tasks'` entries still replay into the standalone view. New hub-routed opens record no history entry — identical to every existing `openProjectHub` navigation (verified: only `openTaskPage` calls `recordViewVisit`, `ui.ts:979/:982`). PM risk 4 accepted as parity, documented in Decisions #6. |

**Post-state grep gate** (verify.sh): every `openTaskPage(` outside `TaskPage.tsx` and `*.test.ts` is exactly ChatPage `:480` and WorktreeCard `:530/:540` — all with detail payloads; zero bare calls. `App.tsx:1754`'s `{activeView === 'tasks' ? <TaskPage /> : null}` **stays** (D3 — live for rows 7–10).

---

## 6. Build plan — F1 / F2 / F3 (matching spec harness wiring)

Ordering rule: each slice leaves the app fully working — F1 alone keeps the old hub copy-hack (so bound repos render via the legacy tier the hack still populates); F2 swaps the data source; F3 removes the entrances.

### F1 — `per-repo-board-resolution`

| File | Change |
|---|---|
| `shared/github-project-types.ts:224-234` | Add optional `activeProjectByRepo` (§1.1). |
| `shared/constants.ts:320-325` | Add `activeProjectByRepo: {}` to the stable default (§1.2). |
| `lib/board-project-resolution.ts` **(new)** | `resolveBoardProject`, `applyBoardPick`, `clearBoardPick`, types (§2.2–2.3). |
| `lib/board-project-resolution.test.ts` **(new)** | Cases 1–13 (§2.4). |
| `store/slices/github.ts` | `projectBindingByRepo: {}` + `setProjectBindingState` (state + one setter; nothing writes it yet) (§3.2). |
| `components/github-project/ProjectViewWrapper.tsx` | `repoId` prop; `:87` read → resolver memo; divergence-hint bar + "Use bound project" (`clearBoardPick`); pass `repoId` + resolved display value to the picker (§3.1, §3.5). |
| `components/github-project/ProjectPicker.tsx` | `repoId` prop; `commitSelection` mutate body → `applyBoardPick` (§3.4). |
| `components/TaskPage.tsx` | Pass `repoId` at `:4062`; embedded resolver memo + local-mode effect; resume effect `:1047` embedded branch; gate mode-button `setTaskResumeState` on `!embedded` (§3.3). |

**Gate:** `bun run build` (from `crates/agentum-desktop/ui`) green + `bunx vitest run src/lib/board-project-resolution.test.ts` green + `bunx vitest run src/components/new-workspace/work-item-picker-model.test.ts src/components/new-workspace/create-workspace-wizard-model.test.ts` green (proves wizard models untouched). *(Full-suite failures must be proven pre-existing — orchestrator note.)*

### F2 — `hub-binding-retarget`

| File | Change |
|---|---|
| `components/project-hub/ProjectHubPage.tsx:81-123` | Retarget per §4: `deriveTrackerBindingTarget` import, `hostId` threading, write `projectBindingByRepo`, delete the `setTaskResumeState({githubMode:'project'})` (`:97`) and `updateSettings` (`:113-115`) calls. |

**Gate:** `bun run build` green + the F1 vitest set green + `git grep -n 'updateSettings\|setTaskResumeState' -- crates/agentum-desktop/ui/src/components/project-hub/ProjectHubPage.tsx` returns nothing.

### F3 — `sidebar-board-removal`

| File | Change |
|---|---|
| `lib/board-route.ts` + `lib/board-route.test.ts` **(new)** | `resolveBoardRoute` + `openBoardSurface` (§5.1). |
| `store/slices/ui.ts` | `openProjectHub` seed param (`:508`, `:929-942`); `projectHubTab` union + `'tracker'` (`:503`) (§5.2). |
| `components/sidebar/SidebarNav.tsx` | Delete Board button + selector + prefetch machinery (table row 1). |
| `components/settings/AppearancePane.tsx:290-301` | Delete the dead toggle. |
| `components/CommandPalette.tsx`, `App.tsx:1280-1288`, `hooks/useIpcEvents.ts:817-825`, `components/harness/ChatPage.tsx:503-506/:523-526` | Re-route per table rows 2–6. |

**Gate:** `bun run build` green + `bunx vitest run src/lib/board-route.test.ts src/lib/board-project-resolution.test.ts` green + the spec's grep gate: `git grep -n 'openTaskPage(' -- crates/agentum-desktop/ui/src` → outside `TaskPage.tsx`/tests only detail-payload callers, zero bare calls; plus `git grep -n '"Board"' crates/agentum-desktop/ui/src/components/sidebar/SidebarNav.tsx` empty.

### Must NOT touch (all slices)

- `hooks/useComposerState*` (handoff constraint).
- `components/github-projects/ProjectBindingEditor.tsx` (context only — D1 explicitly rejects coupling the browse pick to `handleSave`'s discover+mapping gate at `:224-254`).
- Anything under `crates/agentum-server/` or `crates/agentum-desktop/src/` (UI-only).
- `components/new-workspace/CreateWorkspaceWizard.tsx`, `create-workspace-wizard-model.ts`, `work-item-picker-model.ts` — spec-012/013 wizard behavior preserved byte-identically; `deriveTrackerBindingTarget` is **imported**, never edited (see Deviation D-1).
- Tracker-transition machinery (spec 010's zero-call-site Projects write) and `task_sink` paths — bindings still drive automation.
- `TaskPage` detail navigation internals (`:576/798/853/3245`) and `store/slices/worktree-nav-history.ts` (D3).
- The `taskDataSeeded` gate + re-assert effect (`ProjectHubPage.tsx:59-73`) — load-bearing for §3.1.

---

## Architectural risks & decisions log

1. **D: `repoId` prop, not `useActiveRepo`, keys the board** (§3.1). R if violated: background `setActiveRepo` re-keys a mounted hub board — the copy-hack bug in a new coat.
2. **D: binding cache is session-only store state, not settings.** The server's `bindings.json` is the durable source (slug-keyed); persisting a UI mirror would invite drift. R: one extra loopback GET per hub Tasks visit — negligible (local).
3. **D: `pending` resolution state exists** so a pick-less bound repo never flashes the legacy project before the binding lands (AC 2's A→X/B→Y flip test would intermittently fail without it). R: a repo whose binding fetch hangs shows a skeleton until the client timeout (10 s, `github-projects-client.ts:139`) resolves it to `loaded/null` → legacy/none. Bounded, fail-closed.
4. **D: the standalone (`repoId == null`) picker keeps writing legacy `activeProject`** — preserved pre-016 path, not "new code" in the AC-3/4 sense; the alternative (a dead picker on the D3-preserved surface) is worse UX and D1 explicitly rejects dead-picker outcomes. The hub path can never write the legacy slot. qa.sh's flip test is unaffected (it exercises hubs only).
5. **D: `'project'`-mode forcing = embedded-local state derived from the resolver; embedded never writes `taskResumeState`** (§3.3). R mitigated: stale-mode leaks in either direction (hub→global, global→hub) are structurally impossible rather than ordered-effect-dependent.
6. **D (PM risk 4 accepted): hub-routed board opens record no nav-history entry** — parity with every existing `openProjectHub` call (Projects page `:103`, WorktreeList `:3050/:3077`, ChatPage `:594` — none record). Old `'tasks'` entries replay fine via `viewActivator` (`worktree-nav-history.ts:233-238`). Adding hub entries to history is a separate spec (same boundary as D3's detail re-homing).
7. **D: `openProjectHub` gains an optional `taskSource` seed** (§5.2) — without it, a Linear filed-card click would land the hub on the GitHub tab (a regression vs. today's `openTaskPage({taskSource:'linear'})`). Not detail threading; PM risk 1 intact.
8. **R: vite build doesn't typecheck** (the `projectHubTab`/`HubTab` drift proves it), so type-only mistakes can pass the build gate. Mitigation: everything decision-bearing is in pure vitest-covered modules (`board-project-resolution`, `board-route`); component wiring is thin.
9. **R: upgraded profiles lack `activeProjectByRepo`** (top-level settings merge, §1.3). Mitigation is structural: the field is optional in the type, forcing `?? {}` at compile-use sites; test case 13 pins the write path.
10. **D: hint copy lives in the wrapper, data in the binding entry's `projectTitle`** — no extra fetch for the hint; when title is absent (older binding writers) fall back to `owner/#number`.

## Deviations from PM guidance

- **D-1 (spec "Risks & invariants": wizard "ideally switched to the new resolver").** Not done — the wizard keeps `resolvePickerProject` untouched. Justification: the handoff hardens this to "may adopt, must not regress", and the wizard's resolution is *binding → legacy* by design (it feeds status **automation**, where D1 itself says the binding must win; a browse-pick tier would make the wizard's issue list diverge from where transitions write). Unifying them would change spec-012/013 behavior — exactly what the must-not-touch list forbids. The new resolver *generalizes* the same normalization rules, so a future unification is mechanical.
- **No other deviations.** PM risks 1–7 are each addressed at: 1 → §5.2 + table rows 7–8; 2 → §3.3; 3 → §2.3 + §3.4; 4 → Decisions #6; 5 → §5.2; 6 → §3.6 + §4; 7 → §5.1 (stale-id guard) + §2.2 (`none` → items, forcing never leaks).
