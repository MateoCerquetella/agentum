# Architecture Blueprint — Spec 009 Per-repo task projects

**Validated against this worktree @ `d957eefd`.** The spec is sound; one spec
framing is **imprecise** (⚠️-A): the GitHub-project binding does **not** live in
`TaskResumeState` — it lives in `settings.githubProjects.activeProject`. The fix
is therefore two small scoped maps, one per existing persistence blob, not one.

## 1. Citation audit (all read on this tree)

| Seam | Citation | Status |
|---|---|---|
| GitHub binding **read** | `ProjectViewWrapper.tsx:85` (`settings?.githubProjects?.activeProject`), auto-fetch `:159-191` | ✅ |
| GitHub binding **write (select)** | `ProjectPicker.tsx:212` (inside `updateProjectSettings`, also `recent`/`lastViewByProject` `:200-216`) | ✅ |
| GitHub binding **write (clear on `not_found`)** | `ProjectViewWrapper.tsx:~253` (`activeProject: null`) | ✅ |
| `GitHubProjectSettings` shape | `shared/github-project-types.ts:219-229` | ✅ |
| Settings default | `shared/constants.ts:318-325` (`githubProjects` default, `activeProject: null`) | ✅ |
| Settings persistence | `api.settings.set` → Tauri `settings_set` (`crates/agentum-desktop/src/commands/settings.rs:78`, returns full merged settings) | ✅ |
| Linear binding **read (restore)** | `TaskPage.tsx:1082-1168` resume effect, guarded by `linearContextResumeAttemptedRef` (`:948`) | ✅ |
| Linear binding **write (select/clear)** | `TaskPage.tsx:995-997`, `:1018-1028` (select); `:970`, `:1106/1119/1143/1153` (clear) | ✅ |
| `TaskResumeState` shape | `shared/types.ts:2452-2465` | ✅ |
| UI-state write path | `setTaskResumeState` (`ui.ts:1006-1011`) → `api.ui.set` → Tauri `ui_set` (`commands/ui.rs:112`) | ✅ |
| Sanitizer / hydrate seam | `sanitizeTaskResumeState` (`ui.ts:363-415`); hydrate at `ui.ts:1569` | ✅ |
| **Scope key** | `activeRepoId: string \| null` (`repos.ts:135`; setter `repos.ts:680`; called on worktree activation `lib/worktree-activation.ts:172` + `ui.ts:881`; revalidated on load `repos.ts:192`) | ✅ |
| Persisted fallback key | `lastActiveRepoId` (`shared/types.ts:2471`) | ✅ |
| Hydrate-test precedent | `ui.test.ts:460` `hydratePersistedUI` describe + `makePersistedUI` | ✅ |
| validRepoIds prune precedent | `filterTrustedAgentumHooksToValidRepos` (`ui.ts:1572-1575`) | ✅ |

⚠️ **-A correction to the spec's "Reuse vs build":** scoping `TaskResumeState`
alone would fix only Linear. The plan below scopes both real homes.


## 2. Components

**Touch (desktop UI only — no server, no Tauri command changes):**

1. `shared/github-project-types.ts` — add optional `activeProjectByRepo?: Record<string, { owner; ownerType; number }>` to `GitHubProjectSettings` (:219).
2. `shared/types.ts` — add optional `linearContextByRepo?: Record<string, LinearContext>` to `TaskResumeState` (:2452); keep `GLOBAL_TASK_PROJECT_SCOPE = 'global'` const next to it.
3. **New** `shared/task-project-scope.ts` — pure: `scopeKeyFor(repoId | null)` → `repoId ?? 'global'`; `resolveActiveProjectForRepo(settings, repoId)`; `resolveLinearContextForRepo(resume, repoId)`. Resolvers read the scoped map **only** — never the legacy global (D2).
4. `store/slices/ui.ts` — extend `sanitizeTaskResumeState` (:363; extract the existing linearContext validation into a per-entry helper, reuse for map values); hydrate passes the map through (:1569) pruned to `validRepoIds` (precedent :1572); new action `setLinearContextForRepo(context | undefined)` beside `setTaskResumeState` (:1006) — reads `get().activeRepoId`, spreads, same `api.ui.set` pipeline.
5. `components/github-project/ProjectPicker.tsx` — `updateProjectSettings` (:200-216) also writes `activeProjectByRepo[scope]` and nulls legacy `activeProject` once.
6. `components/github-project/ProjectViewWrapper.tsx` — read seam :85 → `resolveActiveProjectForRepo(settings?.githubProjects, activeRepoId)` + subscribe `activeRepoId` (the existing auto-fetch effect :159-191 re-runs on the resolved change, so a repo switch re-fetches or shows the picker for free); clear-on-`not_found` (:253) deletes the scoped key.
7. `components/TaskPage.tsx` — swap `setTaskResumeState({ linearContext … })` at :995/:1018/:1106/:1119/:1143/:1153 for `setLinearContextForRepo`; restore effect (:1082-1168) resolves the scoped context for `activeRepoId` (added to deps).
8. Tests: new `shared/task-project-scope.test.ts`; extend `store/slices/ui.test.ts` hydrate describe (:460 precedent).

**Untouched (boundaries):** all of `crates/agentum-server` (`/api/board` stays global — spec non-goal); Tauri commands `settings.rs`/`ui.rs` (blobs are opaque to them); `github.ts` slice fetch/cache (`projectViewCacheKey` is already per-project); `pinned`/`recent`/`lastViewByProject` (project-keyed, not repo bindings — stay global); `githubMode`/`linearMode`/presets/queries (taste, not bindings — D4).

## 3. APIs

- `scopeKeyFor(repoId: string | null): string` — `repoId ?? GLOBAL_TASK_PROJECT_SCOPE`.
- `resolveActiveProjectForRepo(gp: GitHubProjectSettings | undefined, repoId: string | null): ActiveProjectRef | null` — `gp?.activeProjectByRepo?.[scopeKeyFor(repoId)] ?? null`.
- `resolveLinearContextForRepo(r: TaskResumeState | undefined, repoId: string | null): LinearContext | undefined`.
- `setLinearContextForRepo(context: LinearContext | undefined): void` — ui-slice action; `undefined` deletes only this repo's key.
- Data flow: select → writer spreads map (`{ ...prev, [scope]: ref }`) → `api.settings.set` / `api.ui.set` → renderer store → resolver per current `activeRepoId` → view. Switch repo → `activeRepoId` change → subscribed readers re-resolve.

## 4. Important decisions

- **D1 — Two scoped maps in the existing blobs** over one new unified bindings store: each binding stays next to its existing global, both persist through pipelines that already exist. New store = speculative abstraction.
- **D2 — Hard-cut migration; legacy `activeProject`/`linearContext` are never read for resolution** (chose over legacy-fallback): any fallback shows repo X's project on repo Y's first post-upgrade view — which *is* the bug. Cost: one re-pick per repo, one click via `recent`. Old blobs still hydrate (AC-4) since sanitizers stay tolerant and every new field is optional.
- **D3 — Scope key = live `activeRepoId`** (repos.ts:135) with a reserved `'global'` bucket for the no-repo case, over `lastActiveRepoId` (stale until repos load) and over worktree granularity (two worktrees of one repo share tracker context — resolves the spec's open question to **repo**).
- **D4 — Scope only the bindings.** Mode/preset/query/view-memory stay global: the ACs concern the binding; widening changes behavior beyond the ask.
- **D5 — Zero server changes.** Both blobs are desktop-persisted (`settings.rs:78`, `ui.rs:112`); `cargo test -p agentum-server --lib` stays green by construction.

## 5. Risks

- **R1 — Stale-closure clobber on settings writes** (the `handleSwitchView` "Why:" comment, ProjectViewWrapper.tsx:246-251). Mitigation: scoped writers read fresh `useAppStore.getState().settings` before spreading — same precedent.
- **R2 — `activeRepoId` null early / no repos.** Bindings resolve under the `'global'` bucket, then re-resolve on repo load (subscribed). Accepted: matches today's behavior for repo-less profiles; no leak possible with ≤1 context.
- **R3 — Linear restore effect loops on repo switch.** Mitigation: extend `linearContextResumeAttemptedRef` (:948) from `boolean` to the last attempted `(repoId, contextId)` pair — re-entry per repo works, repeat-within-repo doesn't loop. Covered by the AC-2 test.
- **R4 — Older build reads a new profile.** Legacy fields are null → picker shown; all new fields optional → no crash. Accepted.
- **R5 — Map growth across deleted repos.** UI-state map pruned to `validRepoIds` at hydrate (:1572 precedent); settings map entries for unknown repos are inert (resolver only looks up the current key). Accepted.

## 6. AC → plan → test

| AC | Plan part | Test |
|---|---|---|
| 1 persists scoped + reload | Writers (§2.5/§2.4 action) + hydrate passthrough (§2.4) | `task-project-scope.test.ts` round-trip: write under X → resolver(X) returns it after simulated hydrate; `ui.test.ts` hydrate keeps `linearContextByRepo` |
| 2 switch renders Y / picker, never X | Resolvers (§2.3) + read seams (§2.6/§2.7) + `activeRepoId` subscription | Resolver unit: X-bound, view Y → `null`; R3 ref-key test; `qa.sh` browser pass (spec wiring) |
| 3 clear removes only X | Clear paths delete scoped key (§2.4 action, §2.6) | Action test: seed X+Y, clear X, Y intact |
| 4 legacy hydrates, writes scoped | Sanitizer tolerance + optional fields + D2 | `ui.test.ts:460` describe with legacy-shaped blob via `makePersistedUI` → no crash, resolver → null, then write lands under repo key |

**Gate:** `npx vitest run` (vitest 4.1.5; no `test` script in package.json) +
`npm run build --prefix crates/agentum-desktop/ui`. `qa.sh`: bind on X → switch
to Y (picker) → switch back (X returns) → reload (persists).

