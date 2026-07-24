# Spec 015 — Reviewer sign-off

- **Verdict:** **SIGN-OFF** — no blockers. 3 should-fixes (all follow-up
  material), 6 leave-as-is nits.
- **Reviewer:** final sign-off role (did not write or test the code).
- **Date:** 2026-07-13
- **Commits reviewed:** F1 `ff7290ee`, F2 `d7d64f33`, F3 `3ec6f028` on
  `fixes-new-workspace`, base `4f98453f`. Complete diff (14 files) read
  hunk-by-hunk; every ruling below is against code at HEAD, not reports.
- **Builds on:** the tester's PASS-WITH-DEFERRALS (`verification.md`) — gates
  and AC evidence are NOT re-proven here; this review is correctness, security,
  invariant protection, and design fidelity.

---

## Per-focus-item rulings

### 1. Tester focus 1 — `onUse` zero-match behavior shift — **PASS (accepted; release-note it)**

Old `onUse` launched blind off the dialog's seed:
`launchWorkItemDirect({ item, repoId: current.workItem.repoId, … })`, whose
zero-match arm is a silent URL-open (`launch-work-item-direct.ts:197-201`:
`const repo = store.repos.find((r) => r.id === repoId); if (!repo) { openModalFallback(); return }`).
New `onUse` routes through `startWorkForItem`, whose `none` arm is
`setRepoNotInAgentum({ owner, repo, url })` — the honest missing-repo dialog.

Three reasons this is acceptable, not a regression:

1. **Practically unreachable.** The dialog only opens off a non-`none`
   classification (`handleOpenDialog`: `if (match.kind !== 'none')` →
   `setDialogRepoItem`), and `resolveRepoBackedProjectDialogState`
   (`project-dialog-state.ts:27`: `if (dialog && !liveRepoIds.has(dialog.repoId)) return null`)
   closes it the moment the seed repo leaves the registry. Crucially the slug
   index never regresses to empty mid-session: `useRepoSlugIndex` keeps serving
   the previous index during a rebuild (`setReady(false)` but `index` is only
   replaced when the new build resolves, `repo-slug-index.ts:140-150`), so a
   mounted dialog cannot see a transient `[]` for a live repo.
2. **The residual edge is now MORE honest.** The one reachable case (repo still
   registered but its slug newly unresolvable — remote removed / negative-cached
   IPC failure) previously opened a browser tab with no explanation; it now
   shows a dialog. The row-level Start-work had this exact exposure pre-015
   (`lookupSlug` → `setRepoNotInAgentum`); F2 merely unifies the two gestures.
3. **The two gestures behaving identically is the design intent** (architecture
   §3.3: one shared `startWorkForItem` "so the two start gestures on the board
   behave identically").

Ruling: acceptable product behavior. Carry one release-note line (see
checklist) — it is the only product-visible shift outside the spec's letter.

### 2. Tester focus 2 — choose-arm dialog seeding — **PASS**

Seed choice is correct and cannot diverge from what `startWorkForItem` would
choose:

- Both arms classify the same input: `classifyStartWorkRepoMatches(lookupSlug(`${origin.owner}/${origin.repo}`))`
  in `handleOpenDialog` and in `startWorkForItem`. The seed is deterministic
  (`start-work-repo-match.ts:24`: `matches.find((repo) => repo.connectionId == null) ?? matches[0]`).
- **`onUse` re-classifies at click time** — `buildItem: (repoId) => ({ ...item, repoId })`
  with the in-code comment "the dialog's repoId was only the seed candidate" —
  so the launch repo never depends on the (possibly stale) seed. If the registry
  changed while the dialog sat open, fresh classification is the correct answer.
- **Every mutation the dialog exposes is slug-addressed when `projectOrigin` is
  set** (and `setDialogRepoItem({ …, origin })` always sets it). Verified per
  surface: `runPullRequestStateUpdate` branches
  `if (args.projectOrigin) { … api.gh.updatePullRequestBySlug(…owner, repo, number…) }`
  (`GitHubItemDialog.tsx:261-277`); `GHEditSection`
  (`github-item-edit-section.tsx`) reads labels/assignees via
  `useRepoLabelsBySlug`/`useRepoAssigneesBySlug` when `projectOrigin` is set
  (`:144-152`) and threads `projectOrigin` into every write call (`:183-358`);
  the prop contract is explicit (`GitHubItemDialog.tsx:57-61`: "edits in the
  dialog are routed via slug-addressed mutation IPCs … slug routing wins for
  writes"). The seed's `repoPath`/`repoId` feed only reads and cache keys —
  and any same-slug candidate's checkout is a checkout of the same repo.
- Nit: the `?? match.repos[0]` fallback in `handleOpenDialog` is unreachable
  (`seedRepoId` is always drawn from `match.repos`) — harmless defensive code.

### 3. Tester focus 3 — AC 9 render-policy superset — **PASS (superset accepted; fail-closed)**

`ProjectTrackerConfig` mounts the panel whenever the tab has a workdir
(`ProjectHubPage.tsx`: `{repo.path ? … <TrackerIntakePanel repo={repo} …/> … : null}`)
— a superset of AC 9's "whenever a binding/tracker resolves", explicitly
sanctioned by architecture §4.1 ("Render policy: the panel renders whenever the
tab has a workdir; … a superset of AC 9's 'whenever a binding/tracker
resolves'"). The no-tracker-at-all case is fail-closed by the landed 013
contract the panel reuses: `resolveCreateIssueProvider` — "neither ⇒ `github` —
the default create path; the GitHub arm surfaces the honest no-repo /
no-credential error inline (never silently misfiles)"
(`create-issue-intent-model.ts:67-69` doc) — and `createGithubIssue` **throws on
any non-2xx** (`github-issue-client.ts`: "Throws on any non-2xx so the caller
can render an inline error"), so `filed` cannot be set without a real issue.
The panel renders **nowhere the spec forbids**: its render condition
(`repo.path` truthy) is byte-identical to the pre-existing
`ProjectBindingEditor` condition in the same tab. A remote-repo hub is also
safe: drafts error inline (server-side path miss) and
`deriveFiledGatedRunGate` refuses with `remote-repo`.

### 4. Tester focus 4 — the 2 residual path-fallback sites — **PASS (benign; ruled FOLLOW-UP, not blocker)**

Both sites confirmed at HEAD (`GitHubItemDialog.tsx:361-366`,
`PullRequestPage.tsx:340-345`), both this exact shape:

```ts
return s.repos.find((r) => (effectiveRepoId ? r.id === effectiveRepoId : r.path === repoPath))
  ?.issueSourcePreference
```

The path arm fires only when `effectiveRepoId` is falsy, and the selector's
**sole** consumer in each file is `detailsCacheKey`, which returns `null` in
exactly that case (`if (!workItem || !repoPath || !effectiveRepoId) return null`
— GitHubItemDialog.tsx:369-370, PullRequestPage.tsx:348-349; grep confirms no
other `issueSourcePreference` reader in either file). A dual entry therefore
cannot mislabel a cache key today. Ruling: **follow-up** — fold both swaps into
the spec's doctor-check follow-up ticket so `findRepoByPathPreferLocal` becomes
the single lookup idiom (should-fix S1). Not a blocker: no observable defect.

### 5. F1 data-shape safety — **PASS**

- **No ghost/duplicate path through any caller.** All three registration
  callers funnel through `append_repo` → `register_repo`. `add` passes
  `body.connection_id`/`body.host_id` (repos.rs:226); `create` and `clone` pass
  `None, None` (repos.rs:302-307, :336-341) — local by construction. A
  `create`/`clone` at a path holding a remote entry mints a local **sibling**
  (the intended dual-entry shape, distinct key); a re-add of either key returns
  the existing entry with `added == false` and **no registry rewrite**
  (`if added { write_repos(&repos)?; }`). The seven unit tests pin every arm,
  including `two_connections_same_path_are_two_entries` (D6) and byte-untouched
  entry 0 on a remote add.
- **Read-modify-write race: pre-existing, marginally NARROWED.** The old
  `append_repo` was already read → find/push → write; `update`/`remove`/
  `reorder` share the same pattern at base. F1's only change to the window is
  that a re-add no longer rewrites the file at all. Nothing worsened. (The
  non-atomic `std::fs::write` in `write_repos` and the corrupt-file→empty
  tolerance in `read_repos` are pre-existing shapes, out of 015's scope —
  nit N5.)

### 6. F1 `apply_repo_updates` case-variation smuggle — **PASS (no smuggle possible)**

The update map arrives **verbatim** — `Json(updates): Json<Map<String, Value>>`
(repos.rs:253) does no key renaming — and the skip list matches the exact wire
key: `if key == "id" || key == "path" || key == "addedAt" || key == "connectionId" { continue; }`.
A variant-case key (`"connection_id"`, `"connectionid"`) is inserted into the
serialized object, but `Repo` is `#[serde(rename_all = "camelCase")]` with
**zero `alias` attributes** (struct read at HEAD, repos.rs:46-67; diff grep for
`serde(alias` is empty), so `serde_json::from_value` binds the
`connection_id` field **only** from the exact key `"connectionId"` — the
variant key falls into the `#[serde(flatten)] extra` map and never touches the
field. Explicit `Value::Null` is also refused (skip is on the key,
value-independent; pinned by the second half of
`update_refuses_connection_id_edit`). A local entry can't be made remote
either: its serialized object has no `connectionId` key
(`skip_serializing_if = "Option::is_none"`) and the update can't add one.
Residual effect of a smuggle attempt: the variant key round-trips harmlessly in
`extra` (documented tolerance) — nit N3.

### 7. F2 hop payload — **PASS**

The choose-arm `openModal` payload is exactly

```ts
useAppStore.getState().openModal('new-workspace-composer', {
  linkedWorkItem,
  prefilledName: getLinkedWorkItemSuggestedName(item),
  initialRepoId: match.seedRepoId,
  telemetrySource: 'sidebar'
})
```

— no `startGatedRun`, no `initialBaseBranch` (architect ruling §3.3; diff-wide
grep finds those tokens in F2 only inside explanatory comments). The
`linkedWorkItem` body fetch cannot block the hop: it is inside
`try { … } catch (err) { console.warn(…) }` with the pre-built title+URL
fallback already assigned (`let linkedWorkItem = { type, number, title, url }`),
and the `openModal` call sits **after** the try/catch unconditionally. Fetch
only attempted for `item.type === 'issue' && workdir` where `workdir` is the
seed's path only when local (`seedRepo.connectionId == null`) — faithful to
§3.3's "workdir = the local seed repo's path when available".

### 8. F3 invariants — **PASS**

- **`filed` only from provider-confirmed responses.** GitHub:
  `createGithubIssue` throws on non-2xx (client doc: "Throws on any non-2xx"),
  so `setFiled({ provider:'github', number: created.number, url: created.url,
  slug: created.slug, … })` runs only on a parsed 2xx; the catch arm only
  `setError`. Linear: `if (!result.ok) { setError(…); return }` with the
  in-code comment "Inconclusive/failed never shows 'filed' (AC 12) — `filed`
  unchanged"; the confirmed arm sets identifier/url/title from `result`. A new
  draft resets `filed` (`setFiled(null)` in `draft`, per the model contract).
- **No direct `startGatedWork`.** `startGatedRun` is the spec-008 pre-armed hop
  (`openModal('new-workspace-composer', { …, startGatedRun: true, … })`) guarded
  by `if (!gate.eligible || !filed || filed.provider !== 'github') return`;
  `startGatedWork` appears in the F3 diff only in comments. The modal-data
  contract fields all exist (`create-workspace-wizard-model.ts:202-208`:
  `prefilledName` / `initialRepoId` / `linkedWorkItem` / `startGatedRun`).
- **ProjectBindingEditor untouched** — zero diff lines in that file; F3 only
  passes its pre-existing `onBound` prop at the ProjectHubPage call site.
- **Add-only model.** The `create-issue-intent-model.ts` diff is two import
  lines plus appended exports; every 013 export
  (`deriveCreateIssueIntentPhase`/`canDraftIssue`/`canFileIssue`/
  `deriveIntentTitle`/`resolveCreateIssueProvider`) is byte-identical, and the
  test diff is 111 insertions / 0 deletions.
- **No token leakage in inline errors.** Rendered strings are exclusively
  `err instanceof Error ? err.message : '<fixed copy>'` and
  `result.error || '<fixed copy>'` — no settings interpolation anywhere in the
  hook, and the webview never holds the Linear key (it lives in the shell's
  creds; architecture §4.3). The GitHub arm's server messages flow through
  `extractServerErrorMessage` (typed envelope / plain text) — server-authored
  text, same trust level as every pre-existing panel. A provider echoing a
  credential in an error body would be a shell/server concern outside this
  diff; the UI adds no new exposure.

### 9. Serde-alias hazard (spec 012 memory) — **PASS**

Diff-wide grep for `serde(alias` and `rename =` yields **zero** hits in code
(only doc prose). The `Repo` struct and every request body
(`AddBody`/`CreateBody`/`CloneBody`/`ReorderBody`) are untouched; F1 adds no
fields and no wire-shape changes; F2/F3 reuse `LinkedWorkItemSummary` and
`CreateWorkspaceWizardData` as-is. No new aliases or renamed wire fields
anywhere in `4f98453f..3ec6f028`.

### 10. Architecture-principles sweep — **PASS**

- **One launch path:** diff-wide grep for `createWorktree` / `worktrees/create`
  / `spawn` / `startGatedWork` in code hits only comments and tasks.md prose;
  both F2 arms end in `launchWorkItemDirect` (existing) or
  `openModal('new-workspace-composer', …)` (existing front door), F3's gated
  run in the same modal hop → the wizard's existing `maybeStartGatedRun`.
- **No polling:** zero `setInterval`/`setTimeout` in the diff's product code
  (the only `setTimeout`s are the pre-existing client abort budgets, untouched);
  `use-tracker-intake`'s binding read is keyed `[repo.path, bindingVersion]`
  (event: `onBound` bump) and the Linear probe `[linearSettings]` (mount /
  runtime-target change) — both cancel-guarded effects, no loops.
- **No new routes / auth holes:** `git diff ff7290ee..3ec6f028 -- crates/agentum-server`
  is **empty** (F2/F3 are zero Rust, re-verified); F1's `router()` and
  `auth.rs::is_public` are untouched — same 8 `/api/repos*` routes behind the
  same middleware.
- **YOLO translation untouched:** no diff anywhere under `agentum-executor` or
  any flags/marker code.

### 11. Comment/code quality — **PASS**

Comments are why-shaped throughout ("Why: the body fetch runs `gh` against a
local checkout…", "sort_by_key is stable: locals keep registry order…",
"`filed` only from the provider-confirmed response — never before (AC 12)");
no AI attribution anywhere in the diff (grep: zero hits); naming matches
surroundings (kebab-case UI modules colocated with their tests like
`project-dialog-state.ts`; Rust pure-core extraction mirrors the file's
existing `basename`/`detect_kind` style; doc comments follow the crate's
`/// Pure core of …` convention).

---

## Should-fix (non-blocking, follow-up ticket material)

- **S1** — Swap the two residual path-fallback selectors
  (`GitHubItemDialog.tsx:365`, `PullRequestPage.tsx:344`) to
  `findRepoByPathPreferLocal`; fold into the spec's doctor-check follow-up
  ticket so the helper is the single lookup idiom (benign today — see focus 4).
- **S2** — `server-repo-client.ts:38` doc comment "(id/path/addedAt are
  ignored)" is now stale: `connectionId` is also ignored by PATCH. One-line doc
  fix next time the file is touched.
- **S3** — `POST /api/repos` accepts `connectionId: ""`, which now keys a
  distinct `Some("")` "remote" entry at a local path (pre-015 the storage was
  possible but the dedupe masked it). No UI caller can produce it
  (`reposAddRemote` requires a selected SSH target; `reposAdd` omits the
  field), but the server could normalize empty-string → `None` (or reject) —
  same doctor follow-up.

## Leave-as-is nits

- **N1** — The Linear probe never resets `linearConnected` to false when the
  runtime environment switches to a non-connected one (stale `true` until
  remount). This is **byte-parity** with the wizard's 013 `CreateIssuePanel`
  probe (`CreateWorkspaceWizard.tsx:1718-1722` has the identical
  `if (cancelled || !status.connected) return` shape) and fail-closed at file
  time (inline Linear error). Inherited precedent, not 015's to fix.
- **N2** — `handleOpenDialog`'s `?? match.repos[0]` is unreachable defensive
  code (`seedRepoId` always comes from `match.repos`).
- **N3** — A variant-case PATCH key (e.g. `"connection_id"`) round-trips into
  the flattened `extra` map — cosmetic registry pollution, no identity effect
  (the `extra` flatten is the struct's documented tolerance).
- **N4** — `draft`/`file` double-invocation inside one frame could start twice
  before React re-renders `busy` — same shape as the wizard's handlers, and the
  buttons are `disabled` on `!canDraft`/`!canFile`; practically unreachable.
- **N5** — `write_repos` is not atomic (no temp+rename) and `read_repos` treats
  a corrupt file as empty — pre-existing shape, unchanged by 015.
- **N6** — (tester's finding) "8 new `routes::repos` tests" is 7 new test
  functions; substance intact.

## Release checklist (for the human)

1. **F1 and F2 must ride the same release train** (architecture §5: F1 alone
   turns bug 1 into a false "isn't added to Agentum" dialog). Both are on this
   branch — keep them together through develop → staging → main.
2. **Release notes:** (a) previously collapsed remote repo entries are NOT
   migrated — the operator re-adds the VPS copy once (spec non-goal, D4);
   (b) the board item dialog's "Use" on a repo that can no longer be resolved
   now shows the missing-repo dialog instead of opening the item URL (focus 1).
3. **Stays deferred to qa.sh/staging (per 008/010 precedent; tester §7):**
   - Live VPS leg: real host add → ssh badge → selection survives → worktree +
     session land on `dyaus` (AC 3/4/7).
   - Board Start-work choose-hop in a real browser (AC 5).
   - Real GitHub/Linear filing with credentials (AC 10) and board-refresh
     visibility + a real gated run from the panel's hop (AC 11).
4. **Promote flow:** merge to `develop` (integration) → `staging`
   (label `status/qa`, run the qa.sh legs above) → on QA pass, `status/qa-pass`,
   promote to `main` + tag per the repo convention. Never close the issue at
   the develop/staging merge.
