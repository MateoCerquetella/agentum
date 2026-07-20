# Spec 015 — Tester verification

- **Verdict:** **PASS-WITH-DEFERRALS** (no defects; deferred live legs recorded; 3 nits)
- **Tester:** independent verification role (did not write the code)
- **Date:** 2026-07-13
- **Commits under test:** F1 `ff7290ee`, F2 `d7d64f33`, F3 `3ec6f028` on
  `fixes-new-workspace`, base `origin/develop` `4f98453f` (verified via
  `git log`: exactly these three commits sit on top of the base merge).
- **Defect list:** none.

## 1. Gate reproduction (all re-run independently on this machine)

| Gate | Developer claim | Tester reproduction | Match |
|---|---|---|---|
| `cargo test -p agentum-server --lib` | 687 / 0 / 5 | **687 passed, 0 failed, 5 ignored** (1.30s test run) | ✅ |
| `cargo fmt --all --check` | clean | exit 0, no output | ✅ |
| `cargo clippy -p agentum-server --lib --tests -- -D warnings` | clean | clean — first run was fully cached, so the tester **forced a recheck** (`touch routes/repos.rs`); recompiled agentum-server in 5.85s, zero warnings | ✅ |
| `npm run build --prefix crates/agentum-desktop/ui` | green, chunk-size warning only | ✓ built in 38.64s; only the pre-existing >2500 kB chunk-size warning | ✅ |
| `bunx vitest run` (9 targeted files, run together) | 157 / 0 | **9 files passed, 157 tests passed, 0 failed** (545ms) | ✅ |
| `cargo test … routes::repos` (module) | "10 passed in routes::repos::tests" | 10 passed, 0 failed | ✅ |

The 9 vitest files: `lib/find-repo-by-path.test.ts`, `store/slices/github.test.ts`,
`store/slices/github-checks.test.ts`, `store/slices/hosted-review.test.ts`,
`store/slices/hosted-review-cache.test.ts`, `store/slices/hosted-review-cache-race.test.ts`,
`components/github-project/start-work-repo-match.test.ts`,
`components/github-project/project-dialog-state.test.ts`,
`components/new-workspace/create-issue-intent-model.test.ts`.
Count cross-check: 157 = 119 (F1's 6 files) + 12 (F2's 2 files) + 26 (F3 model) —
internally consistent with the per-slice claims in tasks.md.

## 2. Sacred-surface proofs (git diff `4f98453f..HEAD`, real paths verified to exist)

| Surface | Diff lines | Result |
|---|---|---|
| `ui/src/hooks/useComposerState.ts` | 0 | ✅ untouched |
| `ui/src/components/github-item-checks-tab.tsx` | 0 | ✅ untouched |
| `ui/src/components/pull-request-checks-tab.tsx` | 0 | ✅ untouched |
| `ui/src/lib/launch-work-item-direct.ts` | 0 | ✅ untouched |
| `ui/src/components/github-projects/ProjectBindingEditor.tsx` | 0 | ✅ internals untouched. The `onBound` prop **pre-exists at base** (`:62-66`, fired at `:247`); F3 only passes it at the call site in `ProjectHubPage.tsx` — exactly the documented exception |
| `crates/agentum-server/src/routes/worktrees.rs` | 0 | ✅ `CreateBody` unchanged (D2); `load_host_for_repo`'s `unwrap_or(LOCAL_HOST_ID)` verified intact at HEAD (`repos.rs:411`) |
| `git diff ff7290ee..HEAD -- crates/agentum-server` | empty | ✅ F2+F3 touch zero Rust |
| `ui/src/components/new-workspace/CreateWorkspaceWizard.tsx` (013's create-issue panel) | 0 | ✅ untouched |

Total changed files base→HEAD = 14 (13 code/tests + `tasks.md`); per-commit
attribution matches the tasks.md sections exactly (F1: repos.rs + helper + 2
store slices; F2: classifier + ProjectViewWrapper; F3: model + project-hub trio).
Only Rust file changed in the whole spec: `routes/repos.rs`.

## 3. AC-by-AC verdicts (test bodies and code read, not names)

| AC | Verdict | Evidence |
|---|---|---|
| 1 (remote add persists distinct entry) | **PASS** | `register_repo` dedupes on `repo.path == path && repo.connection_id == connection_id` (repos.rs). Test `same_path_local_then_remote_registers_two_entries` asserts: 2 entries, distinct ids, the returned remote carries `connection_id == Some("ssh-1")` **and** `host_id == Some(host)`, and entry 0 (local) is byte-untouched (`connection_id`/`host_id` still None). `add` route passes `body.connection_id`/`body.host_id` through, so `POST /api/repos` returns the remote entry. |
| 2 (idempotent per host) | **PASS** | `same_path_same_connection_is_idempotent` asserts `added == false`, len 1, same id (remote); `local_readd_stays_idempotent` the same for `(p, None)` twice — `None == None` via `Option<String>` equality. `append_repo` writes only `if added` (no registry rewrite on a re-add). Bonus pin: `two_connections_same_path_are_two_entries` (D6: keyed by connection, not host). |
| 3 (badge + selection survive) | **PASS (deferred live leg)** | Mechanism verified: `selectedRepoBadge` derives from `repo.connectionId` (CreateWorkspaceWizard.tsx:1129-1133, untouched); keep-selection effect calls `resolveRepoIdForHost(hostScopedRepos, repoId)` and only rewrites on host mismatch (useComposerState.ts:1012-1021, untouched). In-browser leg = qa.sh/staging per handoff. |
| 4 (workspace lands on SSH host) | **PASS (deferred live leg)** | `worktrees.rs:431` `load_host_for_repo(&state, &body.repo_id)` → `resolve_repo_host_id` keyed by repo **id** (repos.rs:397-403) — dual entries have distinct ids, so the picked remote entry resolves to its own host. Zero worktree-route edits needed, as designed. Live VPS leg deferred. |
| 5 (Start-work never silently local) | **PASS (deferred browser leg)** | `startWorkForItem` classifies via `classifyStartWorkRepoMatches`; `choose` → `openComposerForChoice` → `openModal('new-workspace-composer', …)` — no `createWorktree`/launch call anywhere on that arm (grep-proof, §5 below), so the choice surfaces before any worktree exists. |
| 6 (single match: direct, unchanged; VPS-only → VPS) | **PASS** | Diff-read: the direct arm calls `launchWorkItemDirect` with the identical argument set as the removed code (`item`, `repoId`, `launchSource: 'task_page'`, `telemetrySource: 'sidebar'`, URL-opening `openModalFallback`); `url` is `row.content.url ?? null` — same value the old closure read. Classifier test `classifies a sole remote match as direct (VPS-only repo starts on the VPS)` pins the second half. |
| 7 (host step governs end-to-end) | **PASS (deferred)** | The observable end-to-end is F1's server behavior + wizard; qa.sh/staging leg per handoff. Mechanism = ACs 3/4 above. |
| 8 (no new spawn path) | **PASS** | Grep over every new/changed F2/F3 file for `startGatedWork|createWorktree|worktrees/create|harness/start-work|spawn`: only comment mentions. Both hops end in `openModal('new-workspace-composer', …)` — the wizard's existing create path. |
| 9 (Tracker tab intake panel, drafts via existing seam) | **PASS** | `ProjectTrackerConfig` mounts `TrackerIntakePanel` as a sibling whenever the tab has a workdir (architecture §4.1's sanctioned superset of "whenever a binding/tracker resolves"). Draft = `deriveIntentTitle` → `draftGithubIssueBody({workdir, title, slug?})` → `POST /api/github/issues/draft-body` (client verified at `runtime/github-issue-client.ts:189`). |
| 10 (file with resolved provider; amended text) | **PASS** | GitHub: `createGithubIssue` → `POST /api/github/issues`; Linear: `linearCreateIssue` — the landed 013 client (`runtime-linear-client.ts:153`), **no new route** per the amendment. Provider resolution reuses `resolveCreateIssueProvider({resolved, linearConnected})` unchanged; `ambiguous` renders the File-into toggle (TrackerIntakePanel, AC-10 comment block). **No code path sets `filed` without a provider-confirmed response**: `fileGithub` sets it only from the awaited `created` (number/url/slug); `fileLinear` returns early on `result.ok === false` with `filed` unchanged; both catch-arms only `setError`. |
| 11 (board visibility + gated run = pre-armed hop) | **PASS (deferred board-visibility live leg)** | `startGatedRun` guards `gate.eligible && filed.provider === 'github'`, then `openModal('new-workspace-composer', { linkedWorkItem, prefilledName, initialRepoId: repo.id, startGatedRun: true, telemetrySource: 'sidebar' })` — the spec-008 hop; `startGatedWork` is never imported/called (grep-proof). `CreateWorkspaceWizardData.startGatedRun` exists (`create-workspace-wizard-model.ts:208`), so the hop lands on the wizard's existing `maybeStartGatedRun` seam. Board refresh visibility = live leg, deferred. |
| 12 (errors inline, non-fatal; inconclusive never files) | **PASS** | Every await in `draft`/`fileGithub`/`fileLinear` sits inside try/catch/finally that only `setError` and clears the busy flag — the form stays rendered and usable; no-workdir and no-team are early-return inline messages. Error strings are `Error.message`/`result.error` only (no settings interpolation). Single POST per arm — no half-issue state possible. |
| 13 (pure model, vitest green) | **PASS** | `create-issue-intent-model.ts` additions are pure (no DOM/window/react imports — grep-proof); test diff is **111 insertions, 0 deletions** (12 pre-existing 013 cases byte-unmodified), 12 → 26 cases; 26/26 green in the reproduced run. Phase matrix (filed-beats-review, busy-beats-filed, error-beats-filed, busy-beats-stale-error) and gate matrix (github+local eligible w/ slug+number; github+remote → `remote-repo`; linear url and null-url → `not-github-url`; null → `no-linked-item`) all present as real assertions. |

## 4. Deviation audit (every numbered deviation in tasks.md)

| Deviation | Verdict | Evidence |
|---|---|---|
| F1-1: github.ts has 9 path-fallback sites, not 3 | **ACCURATE** | Diff shows exactly 9 swaps: `getRuntimeRepoTarget` (:105), `fetchPRForBranch`, `fetchIssue`, `fetchPRChecks`, `fetchPRCheckDetails`, `fetchPRComments`, `addPRConversationComment`, `addPRReviewCommentReply`, `resolveReviewThread`. Every `options?.repoId` ternary keeps its by-id arm. hosted-review.ts: exactly 5 swaps (:175/:189/:200/:213/:229), :229's ternary keeps by-id. |
| F1-2: test-6 shape via pure `apply_repo_updates` extraction | **ACCURATE** | `apply_repo_updates` skip-list = `id/path/addedAt/connectionId`; `update()` delegates. `update_refuses_connection_id_edit` refuses a new value **and** `Value::Null`, and asserts `displayName` + `hostId` still apply in the same call. |
| F1-3: vitest scope widened to the 5 pre-existing slice suites | **ACCURATE** | 6 files / 119 passed reproduced inside the 157 aggregate; those suites are unmodified base→HEAD (not in the changed-file list) — genuine regression cover for the swaps. |
| F2-1: `onUse` zero-match now shows the missing-repo dialog | **ACCURATE** | Old code called `launchWorkItemDirect` straight off `current.workItem.repoId`; new code re-classifies via `startWorkForItem`, whose `none` arm is `setRepoNotInAgentum`. `resolveRepoBackedProjectDialogState` closes the dialog when its repo leaves the registry, so it's practically unreachable, as claimed. |
| F2-2: choose-arm body fetch only when the seed is local | **ACCURATE** | `const workdir = seedRepo && seedRepo.connectionId == null ? seedRepo.path : null`; since the seed is local-first, "seed is local" ≡ "a local match exists" — faithful to §3.3's "when available". |
| F3-1: Linear probe on mount, keyed on the runtime-target field | **ACCURATE** | `linearSettings` memoized on `settings?.activeRuntimeEnvironmentId` only; the probe effect depends on `[linearSettings]`; `.catch(() => {})` keeps it best-effort GitHub-only. |
| F3-2: Linear `FiledIssue.title` from `result.title`; GitHub like the wizard | **ACCURATE** | `fileLinear` sets `title: result.title` (provider-confirmed). `fileGithub` uses the local `trimmedTitle` — verified identical to the wizard's `handleCreateIssueSubmit` (useComposerState.ts:1557 uses the local `title`, not `created.title`). The load-bearing fields (number/url/slug, identifier/url) are provider-confirmed in both arms. |
| Claim "8 new `routes::repos` tests" (handoff + F1 log) | **MINOR MISCOUNT** | Actual new test **functions** = 7 (module went 3 → 10; `#[test]` count verified at base and HEAD). The developer counted the folded hostId-editable assertion as an eighth. The parenthetical "10 passed in routes::repos::tests" is correct and reproduced. Reporting nit — every §2.2 invariant is genuinely pinned. |

Re-checked non-deviation claims that could hide drift: `resolveMissingRepoProjectDialogState`
really keys on `length > 0` (`hasRepoMatch`, project-dialog-state.ts:19-21) — the
"no edit needed" call is correct; `create`/`clone` still pass `None, None` into
`append_repo` (repos.rs:302-307, :336-341).

## 5. Adversarial spot-checks (5 run)

| # | Check | Result |
|---|---|---|
| a | Can `append_repo` still mint a duplicate via create/clone arg combos? | **No.** Both pass `(path, None, None)`; `register_repo` dedupes `(path, None)` and returns the existing entry with `added == false`, so no rewrite. A pre-existing *remote* entry at the same path gets a local sibling — the intended dual-entry shape, not a duplicate key. |
| b | Any remaining path-only repo lookups bypassing `findRepoByPathPreferLocal`? | **Two residual sites**, both benign: `GitHubItemDialog.tsx:365` and `PullRequestPage.tsx:344` use the `effectiveRepoId ? by-id : by-path` shape to read `issueSourcePreference`. The path arm only fires when `effectiveRepoId` is falsy — and their sole consumer (`detailsCacheKey`) returns `null` in exactly that case, so a dual entry cannot mislabel a cache key today. Recorded as a nit / follow-up candidate (the architecture §2.3 sweep missed them), not a defect. |
| c | Does the F2 choose-hop leak `startGatedRun`/`initialBaseBranch`? | **No.** The `openModal` payload is exactly `{ linkedWorkItem, prefilledName, initialRepoId, telemetrySource }`; grep over `components/github-project/` finds those tokens only in the explanatory comment. |
| d | Can the F3 panel wedge the tab on a rejected promise? | **No.** Every await in the hook is inside try/catch/finally that clears the busy flag; the panel's handlers are sync wrappers (`() => void draft()`, `file`, `startGatedRun`); `api.shell.openUrl` is void-prefixed, same as every pre-existing panel. No path leaves `submitting`/`generating` stuck. |
| e | Does `apply_repo_updates` really reject explicit-null `connectionId`? | **Yes.** The skip is on the *key* (`key == "connectionId"`), value-independent; the test's second half asserts `Value::Null` leaves `connection_id == Some("ssh-1")`. |

## 6. Baseline corroboration (touched areas only)

The full vitest suite (~139 fails) and bare tsc (~1650 errors) are pre-broken
develop baselines (memory + handoff) — not gated. Every pre-existing test file
covering the touched surfaces (`github.test.ts`, `github-checks.test.ts`,
`hosted-review*.test.ts` ×3, `project-dialog-state.test.ts`, and the 12
untouched 013 cases in `create-issue-intent-model.test.ts`) is **unmodified
base→HEAD and green** in the reproduced run — no NEW failures introduced in
touched areas. Vite build (the repo's TS gate) is green; the tester additionally
hand-verified every cross-module signature the new F3 code calls
(`LinearCreateIssueResult`, `CreatedGithubIssue`, `DraftedGithubIssueBody`,
`RuntimeLinearSettings`, `getLinkedWorkItemSuggestedName({title})`,
`CreateWorkspaceWizardData.startGatedRun`) since vite does not typecheck.

## 7. Deferred (recorded, per handoff — not failures)

- AC 3/4/5/7 live legs: real VPS host add → ssh badge → selection → worktree +
  session on `dyaus`; board Start-work choose-hop in a real browser (qa.sh/staging).
- AC 11 live legs: filed issue appearing on the bound board after refresh; a
  real gated run from the panel's hop.
- Real GitHub/Linear filing (credentials) — AC 10's live leg.

## 8. Nits (non-blocking)

1. "8 new tests" in the handoff/tasks is 7 new test functions (see deviation
   audit) — the substance is intact; fix the count in any downstream report.
2. Two residual path-fallback sites (`GitHubItemDialog.tsx:365`,
   `PullRequestPage.tsx:344`) share the audited shape; currently harmless (see
   spot-check b) but worth folding into the spec's follow-up ticket alongside
   the doctor check so the helper is the single lookup idiom.
3. `fileGithub` records `filed.title` from local input rather than the create
   response — deliberate wizard parity (useComposerState.ts:1557 does the same);
   the provider-confirmed fields (number/url/slug) carry the honesty contract.
