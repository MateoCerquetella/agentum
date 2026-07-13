# Spec 015 — Developer task log

## F1 — remote-repo-identity (2026-07-13, branch `fixes-new-workspace`)

### What was built

**Server (`crates/agentum-server/src/routes/repos.rs`):**

- `register_repo(&mut Vec<Repo>, path, kind, connection_id, host_id) -> (Repo, bool)`
  — the pure registration core (architecture §2.1). Dedupe key is
  **(path, connection_id)** with `None == None` for local (D6); `true` =
  appended. Kind/badge/basename logic moved in unchanged (remote defaults to
  `git`, local probes `detect_kind`).
- `append_repo` is now the thin I/O wrapper: `read_repos()` → `register_repo`
  → `write_repos` only when appended. Doc comment updated to
  "idempotent by (path, connection)".
- `apply_repo_updates(&Repo, Map) -> Result<Repo>` — pure PATCH merge
  extracted from `update()`; the immutable skip list gains `connectionId`
  (identity is now two fields; `hostId` stays editable per D6 corollary).
  `update()` delegates to it.
- `scope_repo_pairs()` → new pure `scope_pairs_locals_first(Vec<Repo>)`:
  stable locals-first partition so a bare-path browser scope on a dual entry
  always resolves to the local id regardless of registry order (audit §2.3,
  cdp_browser row).
- `create`/`clone` still pass `None, None` — local by construction, behavior
  unchanged through the new key.

**UI (`crates/agentum-desktop/ui`):**

- NEW `src/lib/find-repo-by-path.ts` — `findRepoByPathPreferLocal(repos, path)`:
  exact-path lookup preferring the local entry (`connectionId == null`), else
  the first match.
- NEW `src/lib/find-repo-by-path.test.ts` — 7 cases (empty/undefined, no
  match, sole local, sole remote, dual-entry prefers local in both orders,
  explicit-null connectionId is local, all-remote falls back to first).
- `src/store/slices/hosted-review.ts` — all 5 path-fallback sites swapped
  (:177/:191/:202/:215 plain finds; :231's ternary keeps its by-id arm).
- `src/store/slices/github.ts` — all 9 path-fallback sites swapped (see
  deviation 1): `:107` (`getRuntimeRepoTarget`), `:1986` (`fetchPRForBranch`),
  `:2149` (`fetchIssue`), plus the identical ternaries in `fetchPRChecks`,
  `fetchPRCheckDetails`, `fetchPRComments`, `addPRConversationComment`,
  `addPRReviewCommentReply`, `resolveReviewThread`. Every
  `options?.repoId ? by-id : by-path` branch keeps its by-id arm.

### Test-first evidence

- Rust: the 8 new tests (§2.2's six + `scope_pairs_lists_locals_first_stably`
  + the hostId-editable assertion folded into `update_refuses_connection_id_edit`)
  were written first; `cargo test -p agentum-server --lib repos` failed to
  compile with E0425 (`register_repo`, `apply_repo_updates`,
  `scope_pairs_locals_first` not found) — red — then green after the
  implementation (10 passed in `routes::repos::tests`).
- UI: `find-repo-by-path.test.ts` written first; `bunx vitest run` failed with
  module-not-found — red — then 7/7 green after creating the helper.

### Gate outputs

| Gate | Result |
|---|---|
| `cargo test -p agentum-server --lib` | 687 passed, 0 failed, 5 ignored |
| `cargo fmt --all` | applied (reformatted repos.rs only) |
| `cargo clippy -p agentum-server --lib --tests -- -D warnings` | clean |
| `bunx vitest run src/lib/find-repo-by-path.test.ts` + the 5 pre-existing suites for both touched slices | 6 files, 119 passed, 0 failed |
| `bun run build` (crates/agentum-desktop/ui) | green (39.5s; pre-existing chunk-size warning only) |

### PATCH-caller audit (architecture §1.1 developer note)

Confirmed no UI caller sends `connectionId` through `PATCH /api/repos/{id}`:
`updateRepo` sends `sanitizeRepoUpdate` over the `RepoUpdate` Pick (no
identity fields); the hostId backfill (`store/slices/repos.ts:51`) sends
`{ hostId }` — still editable, as required; issue-source persistence sends
`{ issueSourcePreference }`.

### Deviations from architecture.md (numbered)

1. **github.ts has 9 path-fallback sites, not 3.** At the exact grounding base
   (`4f98453f`), the same `options?.repoId ? by-id : by-path` shape the
   architecture flagged at `:1986`/`:2149` also exists in `fetchPRChecks`
   (:2199), `fetchPRCheckDetails` (:2320), `fetchPRComments` (:2352),
   `addPRConversationComment` (:2416), `addPRReviewCommentReply` (:2472), and
   `resolveReviewThread` (:2540) — each feeds `repo?.connectionId` into cache
   keys and/or the native call, the exact drift the audit describes. All were
   swapped to the helper (identical mechanical change; behavior unchanged
   absent dual entries). Leaving them would have split cache-key derivation
   for the same repoPath across sibling functions.
2. **Test 6 shape.** `update()` itself needs fs, so the connectionId
   immutability is pinned via the pure extraction the architecture explicitly
   allowed ("developer's call"): `apply_repo_updates` + the
   `update_refuses_connection_id_edit` test (refuses a new value AND an
   explicit null; asserts `displayName`/`hostId` still apply).
3. **Vitest scope widened (defensively).** Besides the mandated
   `find-repo-by-path.test.ts`, the 5 pre-existing test files covering the two
   touched slices (`github.test.ts`, `github-checks.test.ts`,
   `hosted-review*.test.ts`) were run — all green — to guard the swaps.

No other deviations: `worktrees.rs::CreateBody` untouched (D2),
`unwrap_or(LOCAL_HOST_ID)` untouched, no serde changes to `Repo`, collapsed
legacy entries never rewritten (D4), `useComposerState` untouched.

### Note for F2

Per handoff: **do not ship F1 without F2** — post-F1 a two-host repo turns the
board's Start-work into a false "isn't added to Agentum" dialog until the
classifier lands (architecture §3.1).

## F2 — start-work-asks-where (2026-07-13, branch `fixes-new-workspace`)

### What was built

**UI only (`crates/agentum-desktop/ui`); zero Rust.**

- NEW `src/components/github-project/start-work-repo-match.ts` —
  `classifyStartWorkRepoMatches(matches: Repo[]) -> StartWorkRepoMatch`
  (exact name/shape from architecture §3.2): 0 → `{kind:'none'}`, 1 →
  `{kind:'direct', repo}`, >1 → `{kind:'choose', repos, seedRepoId}` with the
  seed = the local match (`connectionId == null`, explicit null included)
  else the first match.
- NEW `src/components/github-project/start-work-repo-match.test.ts` — 7 cases
  (empty → none; sole local → direct; sole remote → direct, AC 6's VPS-only
  half; local+remote → choose with local seed in both orders; explicit-null
  connectionId seeds as local; two remotes → choose with first seed;
  determinism + no input mutation).
- `src/components/github-project/ProjectViewWrapper.tsx`:
  - NEW shared `startWorkForItem({buildItem, origin, url})` — the single
    classification point both board start gestures use. `none` →
    `setRepoNotInAgentum` (unchanged dialog); `direct` →
    `launchWorkItemDirect` exactly as today (same args, same open-URL
    `openModalFallback`; AC 6 byte-equivalent); `choose` → the wizard hop.
  - NEW `openComposerForChoice(item, match, origin)` — mirrors
    `TaskPage.openComposerForItem`: best-effort `fetchGithubIssueBody`
    (workdir = the seed repo's path only when the seed is local; slug from
    `origin`; failure keeps the title+URL fallback) →
    `buildGithubIssueLinkedWorkItem` →
    `openModal('new-workspace-composer', { linkedWorkItem, prefilledName,
    initialRepoId: seedRepoId, telemetrySource: 'sidebar' })`. Deliberately
    NO `startGatedRun` and NO `initialBaseBranch` (architect ruling §1.2).
  - `handleStartWork` → thin wrapper over `startWorkForItem`.
  - `GitHubItemDialog.onUse` → re-classifies via `startWorkForItem` (stamps
    the classified repoId onto the dialog's item), so both gestures behave
    identically.
  - `handleOpenDialog`'s multi arm → the repo-backed `GitHubItemDialog`
    seeded with the seed candidate (dialog mutations are slug-addressed, any
    same-slug candidate is safe) instead of degrading to the slug-mode
    dialog.
  - Revised the stale "Project mode does not own the composer modal" comment
    in the direct arm's `openModalFallback` (the modal is app-mounted; the
    fallback stays URL-open to keep the direct gesture byte-equivalent).
- `resolveMissingRepoProjectDialogState` **re-checked per handoff**: it keys
  on `length > 0` (`hasRepoMatch`), not `length === 1` — a dual-entry repo
  correctly auto-dismisses the missing-repo dialogs. Contract unchanged; no
  edit, no test extension needed (its suite runs in the gate regardless).
- Untouched by ruling: `github-item-checks-tab.tsx` /
  `pull-request-checks-tab.tsx` (repo predetermined by surface),
  `launch-work-item-direct.ts`, `useComposerState` internals,
  `project-dialog-state.ts`.

### Test-first evidence

`start-work-repo-match.test.ts` written first; `bunx vitest run` failed with
"Failed to resolve import ./start-work-repo-match" — red — then 7/7 green
after creating the classifier.

### Gate outputs

| Gate | Result |
|---|---|
| `bunx vitest run src/components/github-project/start-work-repo-match.test.ts src/components/github-project/project-dialog-state.test.ts` | 2 files, 12 passed, 0 failed |
| F1 regression suites (`find-repo-by-path.test.ts` + the 5 slice suites from the F1 log) | 6 files, 119 passed, 0 failed |
| `npm run build --prefix crates/agentum-desktop/ui` (vite) | green (38.6s; pre-existing chunk-size warning only) |
| `cargo test -p agentum-server --lib` | 687 passed, 0 failed, 5 ignored (F2 touches no Rust — run to prove no breakage) |

### Deviations from architecture.md (numbered)

1. **`onUse` zero-match behavior shifts (shared-callback corollary).** Today's
   dialog "Use" never consulted the slug index; on a repo that vanished
   mid-dialog it fell into `launchWorkItemDirect`'s `!repo` guard and opened
   the URL. Routing through the shared `startWorkForItem` (as §3.3 directs)
   makes a zero-match show the repo-not-in-agentum dialog instead. Practically
   unreachable (`resolveRepoBackedProjectDialogState` closes the dialog when
   its repo leaves the registry), and the new surface is the honest one; noted
   because the single-match "byte-equivalent" guarantee is scoped to the
   direct path, which is preserved.
2. **Choose-arm body fetch requires a local seed.** Architecture §3.3 says
   "workdir = the local seed repo's path when available"; implemented as: fetch
   only when the seed repo itself is local (seed = local-first, so a local
   match is always the seed when one exists). A remote-only choose set skips
   the fetch — `gh` runs against local checkouts, and a remote path may not
   exist locally (§3.5's honest limitation makes this case rare: remote-only
   paths usually drop out of the slug index entirely).

No other deviations: hop payload exactly as ruled (no `startGatedRun`, no
`initialBaseBranch`), no new create/spawn code (AC 8), checks-tabs untouched,
classifier name/shape/location as pinned.

### Note for F3

Nothing in F2 blocks F3. F3 is UI-only per the architect ruling (no new
server route; reuse `linearCreateIssue` et al.) and touches a disjoint
surface (`ProjectHubPage`/`TrackerIntakePanel`/`create-issue-intent-model`).
Re-ground `useComposerState` line numbers before editing (012/013 in-flight).
