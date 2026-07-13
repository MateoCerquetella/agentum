# Spec 015 — Architecture Blueprint: Host-aware start-work + Tracker intake

**Self-check passed.** Every load-bearing cite below re-verified line-by-line on
this worktree (= `origin/develop` @ `4f98453f`, 2026-07-13). D1–D6 honored.
Three architect calls resolved (§1). **One spec premise found stale during
grounding** (§1.3 — spec 013 F3 is NOT unbuilt; it changes F3's seam shape but
no product scope). No PM send-back required: the PM handoff pre-authorized the
reuse ("whichever lands first, the other reuses").

- **Status:** Architect → ready for Developer.
- **Order:** F1 → F2 → F3 (D5). F1 is the keystone; F2 depends on F1; F3 is
  severable (and, post-grounding, touches **zero Rust**).

---

## 0. TL;DR — three slices, one sentence each

1. **F1 (root cause):** `append_repo`'s dedupe key becomes
   **(path, connection_id)** via a pure, unit-testable `register_repo` core in
   `routes/repos.rs`; `update()` additionally refuses `connectionId` edits
   (identity is now two fields, both immutable); the full `read_repos()`
   consumer audit (§2.3) found every server consumer id-keyed except two
   deterministic-but-order-dependent path fallbacks (cdp_browser scope, and a
   UI store family in `hosted-review.ts`/`github.ts`) which get a one-line
   locals-first hardening each.
2. **F2 (asks where):** a pure classifier
   `classifyStartWorkRepoMatches(matches: Repo[])` (none / direct / choose)
   replaces the `matches.length === 1 ? matches[0] : null` collapse in
   `ProjectViewWrapper` — direct keeps today's `launchWorkItemDirect` path
   byte-equivalent (AC 6); choose hops through the existing
   `openModal('new-workspace-composer', …)` front door seeded with
   `{ linkedWorkItem, prefilledName, initialRepoId: seed, telemetrySource }`,
   landing the operator on a live Host step (no new create/spawn code, AC 8).
3. **F3 (intake):** a new sibling `TrackerIntakePanel` in the Tracker tab
   (`ProjectBindingEditor` untouched) drives intent → draft
   (`draftGithubIssueBody`) → file (GitHub `createGithubIssue` / Linear
   `linearCreateIssue` — the **already-landed** 013 F3 client, so **no new
   server route**) → optional "Start gated run" = the spec-008 pre-armed
   composer hop (`startGatedRun: true`), which reaches
   `POST /api/harness/start-work` through the wizard's existing
   `maybeStartGatedRun` — one gated-run entry path, same precondition set.

---

## 1. Architect calls resolved

### 1.1 D6 — the dedupe key is **(path, connection_id)**, `None == None`

- `Repo` carries both `connection_id` (desktop SSH target) and `host_id`
  (server host, client-resolved at add time) — `repos.rs:56-64`. The UI's
  entire host model buckets by `connection_id`
  (`hostKeyForRepo` → `ssh:<connectionId>`, `worktree-list-groups.ts:246-248`;
  `filterReposForHost`/`resolveDefaultHostKey`, `composer-host-scoping.ts`).
  An entry per (path, connection) is exactly one repo per host bucket — the
  UX contract AC 1/3 needs.
- **Why not `host_id`:** two desktop connections → one server host would
  collapse into one entry labeled with the *first* connection's id; the second
  connection's bucket would render nothing and its `POST /api/repos` would
  return an entry belonging to another bucket — violating AC 1 from that
  caller's view. Keying by `connection_id` yields two entries that route to
  the same server host: harmless duplication, honest per-connection UX (the
  PM's finding 4 subtlety, resolved in favor of the UI's identity).
- **Why not both:** `host_id` is client-resolved from `connection_id`; if the
  hosts registry changes between two adds of the *same* connection, a
  composite key would mint duplicates — breaking AC 2 idempotency.
- Comparison is exact string equality on `connection_id` (`Option<String>`;
  `None == None` covers local), exact string on `path` (today's semantics —
  no normalization added).
- **Corollary:** `update()` (`repos.rs:199-225`) must add `connectionId` to
  its immutable list (`id`/`path`/`addedAt`) — identity is now two fields and
  PATCH must not be able to collide two entries onto one key. `hostId` stays
  editable (routing metadata, repairable). Developer: grep UI PATCH callers to
  confirm nobody sends `connectionId` today (none found in this grounding).

### 1.2 F2 — the hop payload and where detection lives

- Detection lives in a **pure module**
  `ui/src/components/github-project/start-work-repo-match.ts` (sibling of the
  already-tested `project-dialog-state.ts`), consumed by
  `ProjectViewWrapper.tsx`. The slug index already returns `Repo[]` per slug
  (`repo-slug-index.ts:27,113` — it was built multi-match-aware; only the call
  sites collapse it).
- The hop carries `{ linkedWorkItem, prefilledName, initialRepoId: seed,
  telemetrySource: 'sidebar' }` — and deliberately **not** `startGatedRun`
  (board Start-work is an ungated direct launch today; arming the gate would
  be new product behavior) and **not** `initialBaseBranch` (the wizard owns
  PR-head resolution on its own path). Full rationale §3.3.
- Seed = the local match when present, else the first match — guaranteeing the
  wizard opens on a host that actually holds the item's repo (the wizard's
  default `resolveDefaultHostKey(eligibleRepos, initialRepoId ?? activeRepoId, …)`
  at `useComposerState.ts:453-455` would otherwise seed from the unrelated
  active project).

### 1.3 F3 — Linear seam: **reuse the landed 013 F3 client; no new route**

**Stale spec premise found during grounding (loud):** spec 015 and the PM
handoff describe 013 F3 (wizard Linear create) as *unbuilt* and prescribe a
"thin HTTP seam" over `linear.rs:159`/`TaskSink`. In fact **013 F3 is on
develop at this exact base**: `CreateWorkspaceWizard.tsx:1676-1830`
(`CreateIssuePanel`, doc comment "Spec 013 F2/F3") already files Linear issues
via `runtime-linear-client.ts:153` `linearCreateIssue` → desktop-native
`api.linear.createIssue` (Tauri shell, `crates/agentum-desktop/src/commands/linear.rs`;
remote runtime environments route the same call over RPC). The webview never
talks GraphQL — the *shell* does, which is what the spec's "UI never talks
GraphQL" invariant actually protects.

Adding `POST /api/linear/issues` now would *create* the fork the spec forbids:
two UI-facing Linear-create seams. Ruling:

- The Tracker panel files Linear through the **same**
  `linearStatus`/`linearListTeams`/`linearCreateIssue` client functions the
  wizard panel uses. One seam for both — satisfied by reuse, exactly as the PM
  handoff pre-authorized ("whichever lands first, the other reuses").
- The server-side `linear.rs::create_issue` + `TaskSink::Linear`
  (`task_sink.rs:200-211`) remains the *server-driven* path (harness planner,
  board goals) — untouched.
- **Verify-plan deviation:** the spec's `verify.sh` line "Linear-create route
  test with a stubbed sink" is dropped (there is no new route); replaced by
  model-level vitest (§4.3). Noted for PM visibility; no scope change.

### 1.4 F3 — "Start gated run" is the pre-armed composer hop, not a direct call

`POST /api/harness/start-work` requires "the freshly created worktree"
(`StartWorkRequest.workdir`, `routes/harness.rs:460-462`); the wizard calls it
*after* `createWorktree` via `maybeStartGatedRun`
(`useComposerState.ts:2291-2320`), gated by `deriveIssueSideEffectGate`. A
panel that called `startGatedWork` with the repo's main checkout would point a
gated run at the main checkout — a second, semantically different entry path.
So the panel's button performs the **spec-008 pre-armed hop**
(`openModal('new-workspace-composer', { …, startGatedRun: true })`, precedent
`TaskPage.tsx:2346-2385` + `composer-modal-props.ts`): the wizard creates the
worktree and fires the same `startGatedWork` with the same precondition set.
AC 11's "invokes the same one-click seam" is satisfied *through* the wizard —
zero new gated-run code.

---

## 2. F1 — remote-repo-identity (server)

### 2.1 Exact change

`crates/agentum-server/src/routes/repos.rs`:

```rust
/// Pure core of registration (spec 015 D6): a repo's identity is WHERE it
/// lives (its desktop connection, None = local) plus its path there. Returns
/// the existing entry for (path, connection_id) or appends a new one;
/// `true` = appended (the caller persists).
fn register_repo(
    repos: &mut Vec<Repo>,
    path: String,
    kind: Option<String>,
    connection_id: Option<String>,
    host_id: Option<Uuid>,
) -> (Repo, bool)
```

- The dedupe inside: `repos.iter().find(|r| r.path == path &&
  r.connection_id == connection_id)` (replaces `:134`'s find-by-path-alone).
- `append_repo` (`:127-161`) becomes the thin I/O wrapper:
  `read_repos()` → `register_repo(…)` → `if added { write_repos(&repos)? }` →
  `Ok(repo)`. Kind/badge/basename logic moves into `register_repo` unchanged.
  Doc comment `:125` updates to "idempotent by (path, connection)".
- `update()` (`:214-216`): add `"connectionId"` to the skipped keys.
- `create`/`clone` (`:263`, `:297`) pass `None, None` — local by construction,
  unchanged behavior through the new key.
- Storage shape check: `write_repos` persists a JSON **array** (`Vec<Repo>`,
  `:100-108`) — no path-keyed map anywhere in the file or its consumers; two
  same-path entries round-trip losslessly (the `extra` flatten is per-entry).

### 2.2 Unit tests (write FIRST, in `repos.rs::tests` — pure, no fs/env)

`register_repo` operates on `&mut Vec<Repo>`, so tests need no `HOME`
override (house rule: never mutate env in tests):

1. `same_path_local_then_remote_registers_two_entries` — add `(p, None)` then
   `(p, Some("ssh-1"), Some(host))`: 2 entries; the second returned value
   carries `connection_id`+`host_id` (AC 1 — the remote add must not return
   the local entry); ids differ; entry 0 unchanged.
2. `same_path_same_connection_is_idempotent` — `(p, Some("ssh-1"))` twice →
   1 entry, same id, `added == false` (AC 2 remote).
3. `local_readd_stays_idempotent` — `(p, None)` twice → 1 entry (AC 2 local).
4. `two_connections_same_path_are_two_entries` — `ssh-1` + `ssh-2` (same
   `host_id`) → 2 entries (pins the D6 two-connections-one-host choice).
5. `remote_register_defaults_kind_git` — preserves the `:137-146` behavior
   through the refactor.
6. `update_refuses_connection_id_edit` — serialize-roundtrip an update map
   containing `connectionId` → unchanged (extend the existing test module's
   style; `update` itself needs fs, so pin the skip-list via a small pure
   extraction if needed — developer's call, the invariant is the test).

### 2.3 `read_repos()` / find-by-path consumer audit (exhaustive)

Grep basis: every `read_repos`, `write_repos`, `scope_repo_pairs`,
`resolve_repo_*`, `load_host_for_repo`, `repos.json`, and every
`repos.find(...path...)` in the workspace.

| Consumer | Keying | Can two same-path entries confuse it? | Action |
|---|---|---|---|
| `repos.rs::list` (`:164`) | returns all | No — UI buckets by `connectionId`; both render, which is the F1 goal | none |
| `repos.rs::update`/`remove`/`reorder` (`:200/:306/:321`) | repo **id** | No. But `update` could PATCH `connectionId` into a key collision | add `connectionId` to immutable list (§2.1) |
| `repos.rs::all_repo_ids` (`:341`) → worktrees prune scan | ids | No — per-id host resolution downstream | none |
| `repos.rs::resolve_repo_path` (`:347`) / `resolve_repo_host_id` (`:358`) / `load_host_for_repo` (`:371`) | repo **id** | No — two entries have distinct ids; each resolves to its own path/host. This is precisely why F1 fixes AC 4 with zero worktree edits | none |
| `worktrees.rs` (`:430-431`, `:542-543`, `:708-709`, `:836-837`, `:859`, `:955`) | repo **id** via the three helpers | No | none |
| `wiki.rs:186-187` | repo **id** | No | none |
| `cdp_browser.rs::resolve_scope_from_tables:322` + `resolve_path_via_git:359` via `scope_repo_pairs` (`repos.rs:84`) | **first match by path** (registry order) | Mildly: a bare-path browser context resolves to whichever entry is first; isolation stays correct (deterministic single id) but `POST /api/repos/reorder` can flip which id keys the profile, silently migrating browser state. A local Chromium can only ever serve local checkouts | one-line hardening: `scope_repo_pairs` returns local entries (`connection_id.is_none()`) first (stable partition) + a unit test |
| `cdp_browser.rs:299` (bare-UUID → project scope) | repo **id** | No | none |
| Desktop shell `commands/hooks.rs::repo_path_for` (`:9-24`) | repo **id** | No | none |
| Desktop shell `commands/project_groups.rs` (`:128-160`) | repo **id** (membership fields written per matching id) | No (developer: eyeball the id match at `:136-156` while in there) | none |
| UI `store/slices/hosted-review.ts` `:177/:191/:202/:215/:231` | `repos.find(r => r.path === repoPath)` fallback | **Yes — sharpest UI case:** the match's `connectionId` is forwarded to `api.hostedReview.*`; a remote-first registry order routes a local hosted-review call with the remote connection id | swap to the new `findRepoByPathPreferLocal` helper |
| UI `store/slices/github.ts` `:107` (runtime-env repo resolve), `:1986` (`fetchPRForBranch`), `:2149` (`fetchIssue`) | path fallback when no `repoId` option | Mild: wrong id/connectionId only mislabels cache keys / RPC repo ref | same helper swap |
| UI `repo-slug-index.ts` | per-entry by `repo.id`; index value is `Repo[]` | No — already multi-entry aware. Limitation: a remote repo's slug resolves via the **local** `.git/config` at `repo.path` (`:77`), so a VPS-only path that doesn't exist locally drops out of the index (honest; F2 note §3.5) | none |
| UI `composer-host-scoping.ts`, `worktree-list-groups.ts`, wizard badge `selectedRepoBadge` (`CreateWorkspaceWizard.tsx:1128-1134`) | `connectionId` / id | No — these become *truthful* post-F1 (spec's claim verified) | none |

**New UI helper (the only F1 UI edit — the spec expected zero; the audit found
these, "verify don't assume" honored):**

```ts
// ui/src/lib/find-repo-by-path.ts
/** Exact-path lookup over the repo registry that tolerates spec-015 dual
 *  entries (same path, local + remote): prefers the local entry
 *  (no connectionId), else the first match — deterministic regardless of
 *  registry reorder. */
export function findRepoByPathPreferLocal(
  repos: Repo[] | undefined,
  path: string
): Repo | undefined
```

Swap the eight fallback sites in `hosted-review.ts`/`github.ts` mechanically
(the `options?.repoId ? by-id : by-path` branches keep their by-id arm).

### 2.4 F1 end-to-end (AC 3–4) — zero further UI edits, verified

- Badge: `selectedRepoBadge` derives from `repo.connectionId`
  (`CreateWorkspaceWizard.tsx:1128-1134`) — truthful once the entry exists.
- Selection survival: keep-selection effect `useComposerState.ts:1012-1020`
  keeps `repoId` when it belongs to the selected host
  (`resolveRepoIdForHost`, `composer-host-scoping.ts:89-94`); `submit`
  (`:2349`, `createWorktree` call `:2471`) and `submitQuick` (`:2602`, call
  `:2681` — **line drift** vs spec's ":2602 call") pass that exact id.
- Landing host: `worktrees.rs::create:430-431` resolves path+host from the
  picked repo id; the create path is already host-aware (PM finding 2).

### 2.5 F1 build/test plan

1. Write §2.2 tests (red) → refactor `append_repo` → green.
2. `update` connectionId-immutability + `scope_repo_pairs` locals-first (+ its
   unit test asserting partition order).
3. `find-repo-by-path.ts` + `find-repo-by-path.test.ts` (dual-entry: prefers
   local; single/no match; remote-only) → swap call sites.
4. Gates: `cargo test -p agentum-server --lib` · `cargo fmt` ·
   `cargo clippy -p agentum-server` ·
   `bunx vitest run src/lib/find-repo-by-path.test.ts` ·
   `bun run build` (in `crates/agentum-desktop/ui`). Never the full vitest
   suite or bare tsc (pre-broken baselines).

---

## 3. F2 — start-work-asks-where (UI)

### 3.1 Grounded behavior today (and the post-F1 twist — loud)

`ProjectViewWrapper.tsx::handleStartWork` (`:503-543`):
`lookupSlug(owner/repo)` → `matched = matches.length === 1 ? matches[0] : null`
→ `!matched` shows the **"isn't added to Agentum"** dialog; matched →
`launchWorkItemDirect({ item, repoId: matched.id, … })`. The same collapse
exists in `handleOpenDialog` (`:480-499`) and in the auto-dismiss logic
`project-dialog-state.ts::resolveMissingRepoProjectDialogState`.

**Twist:** pre-F1 the bug is "silently lands local" (the collapsed registry
made `matches.length === 1`). **Post-F1, without F2, it becomes a false
"Repository isn't added to Agentum" dialog** (two matches → `matched = null`)
— a regression in kind, which is why F2 must land in the same release train
as F1.

### 3.2 Pure classifier (new, vitest FIRST)

`ui/src/components/github-project/start-work-repo-match.ts`:

```ts
export type StartWorkRepoMatch =
  | { kind: 'none' }
  | { kind: 'direct'; repo: Repo }
  | { kind: 'choose'; repos: Repo[]; seedRepoId: string }

/** Classify a slug's registered matches for board Start-work. Exactly one →
 *  the direct path (AC 6, byte-equivalent friction). Multiple → the wizard
 *  hop; seed = the local copy when present, else the first match, so the
 *  composer opens on a host that actually holds this repo. */
export function classifyStartWorkRepoMatches(matches: Repo[]): StartWorkRepoMatch
```

Tests (`start-work-repo-match.test.ts`, colocated like
`project-dialog-state.test.ts`): empty → none; one local → direct; one remote
→ direct (VPS-only repo starts on the VPS — AC 6's second half); local+remote
→ choose with local seed; two remotes → choose with first seed; determinism.

### 3.3 Wiring in `ProjectViewWrapper`

Extract one shared callback `startWorkForItem(row, origin, matches)` used by
`handleStartWork` **and** `GitHubItemDialog.onUse` (`:805-816`) so the two
start gestures on the board behave identically:

- `none` → `setRepoNotInAgentum` (unchanged).
- `direct` → `launchWorkItemDirect` exactly as today (`:526-542`), including
  the open-URL `openModalFallback`.
- `choose` → the hop (async, mirrors `TaskPage.tsx::openComposerForItem`
  `:2346-2385`): `buildWorkItem(row, seedRepoId)`; best-effort issue-body
  fetch via the same helpers TaskPage uses (`fetchGithubIssueBody`,
  `runtime/github-issue-client.ts:26`, workdir = the **local** seed repo's
  path when available, slug from `origin`; `buildGithubIssueLinkedWorkItem`,
  `lib/github-linked-work-item.ts:57`); then
  `useAppStore.getState().openModal('new-workspace-composer', {
    linkedWorkItem, prefilledName: getLinkedWorkItemSuggestedName(item),
    initialRepoId: seedRepoId, telemetrySource: 'sidebar' })`.

The modal is app-mounted (`App.tsx:1801`; `NewWorkspaceComposerModal.tsx:16`
renders on `activeModal === 'new-workspace-composer'` →
`CreateWorkspaceWizard` with `deriveWizardComposerSeed`,
`create-workspace-wizard-model.ts:220-237`), so Project mode *can* open it —
the `:531-541` fallback comment predates the 013 F4 front door; revise it
where touched.

**Hop payload — carried / deliberately omitted:**

| Field | Carried? | Why |
|---|---|---|
| `linkedWorkItem` (+ fetched body in `linkedContext`) | yes | parity with TaskPage's hop; without it the agent gets only a URL |
| `prefilledName` | yes | same suggested-name derivation as the direct path (`launch-work-item-direct.ts:218-223`) |
| `initialRepoId` = seed | yes | pins the wizard's opening host to a real candidate (`useComposerState.ts:453-455`); host switch then auto-selects that host's copy (`handleHostChange:459-476` → `resolveRepoIdForHost`) |
| `startGatedRun` | **no** | board Start-work is an ungated direct launch today; arming the gate is new product behavior, not disambiguation |
| `initialBaseBranch` | **no** | only the direct PR path needs `resolveDirectPrStartPoint` (`launch-work-item-direct.ts:227-240`); the wizard owns its own PR-head resolution |
| `telemetrySource: 'sidebar'` | yes | matches `openComposerForItem`; `launchSource` rides only the direct path's queued startup |

Also route `handleOpenDialog`'s `choose` case to the repo-backed
`GitHubItemDialog` with the seed candidate (dialog mutations are
slug-addressed — `:790-793` comment — so any same-slug candidate is safe);
without this, F1 degrades the item dialog to slug-mode for dual-entry repos.
Developer: re-check `resolveMissingRepoProjectDialogState` for the same
`length === 1` assumption and extend its existing test file if its contract
shifts.

### 3.4 Other `launchWorkItemDirect` callers — stay as-is (justified)

- `github-item-checks-tab.tsx:189` and `pull-request-checks-tab.tsx:189`: the
  repo is already determined by the surface (`repoId ?? item.repoId` from an
  explicit repo-bound checks tab) — no slug matching, no ambiguity to
  resolve. Untouched.
- `TaskPage` list "Use": already routes through `openComposerForItem` (the
  single front door) with `item.repoId` known. Untouched.

### 3.5 Honest limitation (documented, not fixed here)

The slug index resolves a remote repo's slug from the **local** filesystem at
`repo.path` (`repo-slug-index.ts:77`; runtime-env targets use the RPC arm).
The multi-host `choose` case therefore materializes exactly in the spec's
collision scenario (same absolute path exists locally); a VPS-only repo whose
path is absent locally is excluded from the index and stays a single/zero
match. Host-aware slug resolution is a named follow-up, out of 015.

### 3.6 F2 build/test plan

1. `start-work-repo-match.test.ts` (red) → helper → green.
2. Wire `ProjectViewWrapper` (shared `startWorkForItem`, dialog `choose` arm,
   comment fix).
3. Gates: `bunx vitest run
   src/components/github-project/start-work-repo-match.test.ts
   src/components/github-project/project-dialog-state.test.ts` ·
   `bun run build`. End-to-end host landing is F1's server behavior + the
   wizard (qa.sh asserts it in-browser).

---

## 4. F3 — tracker-intent-intake (UI only, post-ruling)

### 4.1 Component boundary

- `ProjectHubPage.tsx::ProjectTrackerConfig` (`:253-277`) widens its prop from
  `{ path }` to the hub's `repo` (it already holds `repo` from
  `useActiveRepo()`, `:54`) and mounts **two siblings** in the tab:
  1. `ProjectBindingEditor` (`components/github-projects/ProjectBindingEditor.tsx`)
     — **untouched** except wiring its existing `onBound` prop (`:63-66`) to a
     `bindingVersion` bump in `ProjectTrackerConfig`, so the panel refreshes
     when a binding is saved.
  2. **NEW** `components/project-hub/TrackerIntakePanel.tsx` — props
     `{ repo: Repo; bindingVersion: number }`.
- The panel does its **own** binding read (`getProjectBinding({ workdir })`,
  `runtime/github-projects-client.ts:122`, fail-closed null — the exact
  `WorkItemsField` precedent, `CreateWorkspaceWizard.tsx:1517-1541`) keyed on
  `[workdir, bindingVersion]`, then
  `resolved = resolvePickerProject({ binding, activeProject })`
  (`work-item-picker-model.ts`) and the Linear probe
  (`linearStatus`/`linearListTeams`, the `CreateIssuePanel` precedent
  `:1706-1737`). `ProjectBindingEditor` is not bloated; the tab stays the
  config half + the intake half.
- Render policy: the panel renders whenever the tab has a workdir; provider
  resolution (`resolveCreateIssueProvider`,
  `create-issue-intent-model.ts:69-77`) decides the file target, `ambiguous`
  shows the toggle (AC 10), and an unresolvable GitHub arm surfaces the
  server's typed `no_github_repo` / no-credential message inline (AC 12) —
  a superset of AC 9's "whenever a binding/tracker resolves".

### 4.2 Pure state model (extend `create-issue-intent-model.ts` — additive only)

Spec 013 owns the existing exports (`deriveCreateIssueIntentPhase`,
`canDraftIssue`, `canFileIssue`, `deriveIntentTitle`,
`resolveCreateIssueProvider`) — **reused unchanged, none edited**. Add:

```ts
export type TrackerIntakePhase =
  | 'idle' | 'drafting' | 'review' | 'filing' | 'filed' | 'error'

export type FiledIssue =
  | { provider: 'github'; number: number; url: string; slug: string; title: string }
  | { provider: 'linear'; identifier: string; url: string | null; title: string }

/** Precedence: filing > drafting > error > filed > review(hasBody) > idle.
 *  `filed` must beat `review` (the drafted body is still in hand after a
 *  successful file); a new Draft resets `filed` (hook contract). */
export function deriveTrackerIntakePhase(s: {
  generating: boolean; submitting: boolean; error: string | null
  hasBody: boolean; filed: FiledIssue | null
}): TrackerIntakePhase

/** Gated-run eligibility for a filed issue: composes the SAME
 *  `deriveIssueSideEffectGate` the wizard submits through — a filed GitHub
 *  issue on a local repo is eligible; Linear (D3) and remote repos are
 *  ineligible with the gate's honest reason. */
export function deriveFiledGatedRunGate(
  filed: FiledIssue | null,
  repoConnectionId: string | null | undefined
): IssueSideEffectGate
```

`deriveFiledGatedRunGate` = `deriveIssueSideEffectGate(filed && { type:
'issue', url: filed.url ?? '' }, repoConnectionId)` — a Linear identifier URL
fails the `parseGitHubIssueOrPRLink` check and returns `not-github-url`
(honest by construction, `lib/issue-side-effect-gate.ts:27-44`); render D3's
"gated run: GitHub issues only" copy off that reason.

Vitest (extend `create-issue-intent-model.test.ts`, write FIRST): full phase
precedence matrix incl. `filed`-beats-`review` and busy-beats-`filed`
degenerate inputs; gate matrix (github+local → eligible with slug+number;
github+remote → `remote-repo`; linear → `not-github-url`; null →
`no-linked-item`).

### 4.3 Thin hook + wiring (the `useComposerState:1519/:1615` precedent, not a reuse of the hook)

**NEW** `components/project-hub/use-tracker-intake.ts` — owns
`intent/title/body/generating/submitting/error/filed/teamId/providerChoice`:

- **Draft:** `createIssueTitle = deriveIntentTitle(intent)` →
  `draftGithubIssueBody({ workdir, title, slug? })`
  (`github-issue-client.ts:189`; server `routes/github.rs:302` — the body is
  provider-agnostic markdown, 013's established contract). Errors inline,
  form stays usable (mirrors `handleGenerateIssueBody`,
  `useComposerState.ts:1615-1650`).
- **File GitHub:** `createGithubIssue({ title, body, workdir })`
  (`github-issue-client.ts:115`; server `routes/github.rs:212` →
  `TaskSink::Github`) → `filed = { provider:'github', number, url, slug,
  title }`. Labels omitted in v1 (YAGNI; the wire supports them later).
- **File Linear:** `linearCreateIssue(settings, { teamId, title,
  description })` (`runtime-linear-client.ts:153`) with the
  `CreateIssuePanel` team handling (sole team auto-selected, else a select;
  no team → inline "Pick a Linear team", `CreateWorkspaceWizard.tsx:1770-1780`)
  → `filed = { provider:'linear', identifier, url }`. `result.ok === false` →
  inline error, **no state change to `filed`** — inconclusive never shows
  "filed" (AC 12; both arms are single POSTs, so no half-issue is possible).
  Error strings come from the native command / server envelope — no token
  material passes through the UI (the Linear key lives in the shell's creds;
  developer: keep error rendering to `result.error` / `Error.message`,
  never interpolate settings).
- **Start gated run** (renders only when `deriveFiledGatedRunGate(...).eligible`):
  `openModal('new-workspace-composer', { linkedWorkItem: { type:'issue',
  number, title, url: filed.url }, prefilledName:
  getLinkedWorkItemSuggestedName(...), initialRepoId: repo.id,
  startGatedRun: true, telemetrySource: 'sidebar' })` — §1.4. Ineligible +
  Linear → the D3 note.
- **Board visibility (AC 11):** the filed GitHub issue reaches the Tasks tab's
  project-mode board on its normal refresh — no code; the panel's filed state
  shows the issue link.

### 4.4 F3 build/test plan

1. Model tests in `create-issue-intent-model.test.ts` (red) → model fns →
   green.
2. `use-tracker-intake.ts` + `TrackerIntakePanel.tsx` + the
   `ProjectTrackerConfig` prop widening + `onBound` wiring.
3. Gates: `bunx vitest run
   src/components/new-workspace/create-issue-intent-model.test.ts` ·
   `bun run build`. **No Rust gates — F3 touches no server code** (deviation
   from the spec's verify.sh sketch, §1.3).

---

## 5. Risks & coordination

- **Post-F1-without-F2 regression window (§3.1):** two matches → the false
  "not in Agentum" dialog. F1 and F2 must ride the same release; the harness
  order (F1 then F2, one feature list) already enforces this — do not ship F1
  alone to users.
- **In-flight specs 012/013 share the wizard surface.** F2/F3 deliberately
  touch only the *stable modal-data contract*
  (`CreateWorkspaceWizardData`, `create-workspace-wizard-model.ts:201-210`)
  and *add-only* exports to `create-issue-intent-model.ts` — never
  `useComposerState` internals or `CreateIssuePanel`. Re-ground every UI line
  number at build time (known drift already: `submitQuick` createWorktree call
  is `:2681`, not the spec's `:2602`; `handleCreateIssueSubmit` is `:1519`,
  `handleGenerateIssueBody` `:1615`).
- **Serde-alias hazard:** F1 adds no fields and no aliases; the `Repo` struct
  is untouched except behavioral dedupe. No payload shape changes anywhere in
  015 — linked-work-item and worktree wire shapes are reused as-is.
- **One launch path:** F2's hop ends in the wizard's existing
  `createWorktree` → `POST /api/worktrees/create`; F3's gated run ends in the
  wizard's existing `startGatedWork` → engine `drive`. Zero new spawn/create
  code anywhere (AC 8, invariant 1). Push-based streaming untouched.
- **`unwrap_or(LOCAL_HOST_ID)` stays** (`repos.rs:372`) — F1 fixes identity at
  registration, not the resolver's default (spec invariant).
- **Old collapsed entries:** untouched by design (D4). The operator re-adds
  the remote copy once; release notes carry it.
- **cdp_browser hardening is behavior-adjacent:** locals-first partition
  changes which id labels a dual-entry bare-path browser scope when the remote
  was registered first — acceptable (isolation semantics unchanged, and the
  local id is the correct label for a local Chromium); pinned by its unit
  test.

---

## 6. Gate summary (per increment)

| Gate | F1 | F2 | F3 |
|---|---|---|---|
| `cargo test -p agentum-server --lib` | ✅ (new dedupe tests) | — | — |
| `cargo fmt` + `cargo clippy -p agentum-server` | ✅ | — | — |
| `bunx vitest run <targeted>` | `lib/find-repo-by-path.test.ts` | `github-project/start-work-repo-match.test.ts` + `github-project/project-dialog-state.test.ts` | `new-workspace/create-issue-intent-model.test.ts` |
| `bun run build` (ui) | ✅ | ✅ | ✅ |

Never gate on the full vitest suite (~139 pre-broken) or bare `tsc` (~1650
pre-broken); ui uses **bun**.
