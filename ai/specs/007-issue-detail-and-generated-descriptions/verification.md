# Spec 007 — Tester Verification Report

- **Spec:** 007-issue-detail-and-generated-descriptions
- **Date:** 2026-07-02 (autonomous /sdd-loop, compressed SDD — no separate architecture.md)
- **Worktree:** `.claude/worktrees/finish-the-loop`, branch `finish-the-loop`, HEAD `d8ecf9b8`
- **Commit verified:** `96c98955` (`fix(007): issue detail hydration + side-effect toggles; feat: generated descriptions`), child of base `27f29f1c` (`chore(006): STATE — released v0.54.0`) — parents confirmed via `git log`.
- **Overall verdict:** **PASS 9/9 ACs — ADVANCE to reviewer.** All three tasks.md root causes (Bug 1 stubs, Bug 2 silent gates, harness untracked noise) independently confirmed accurate against base + head; all 4 deviations audited accurate; 4 Info findings, none blocking. Sacred harness surfaces (drive loop, verdict contract) untouched; no `is_public` additions.

## Commands run (independent re-runs, nothing trusted; `PATH=$HOME/.cargo/bin:$PATH`)

| # | Command | Result |
|---|---------|--------|
| 1 | `cargo test -p agentum-server --lib` | **539 passed / 0 failed / 5 ignored** (99.56s; ignored = pre-existing live-gh/agent/parakeet tests) |
| 2 | `cargo test -p agentum-desktop --lib` | **75 passed / 0 failed / 4 ignored** (0.02s; includes the 3 new gh mapping tests + 4 ignored speech-engine) |
| 3 | `cargo fmt --all --check` | clean |
| 4 | `cargo clippy --workspace --all-targets -- -D warnings` | green (exit 0) |
| 5 | `NODE_OPTIONS=--max-old-space-size=3072 npm run build --prefix crates/agentum-desktop/ui` | green (`✓ built in 1m 36s`; only pre-existing >2500 kB chunk-size warnings) |
| 6 | `npx vitest run src/lib/issue-side-effect-gate.test.ts src/runtime/github-issue-client.test.ts` | **10 passed** (2 files: 7 gate + 3 client, 431ms) |
| 7 | `git diff 27f29f1c..96c98955 -- crates/agentum-server/src/auth.rs` | **empty** — no `is_public` additions |
| 8 | `git diff 27f29f1c..96c98955 --stat` | **16 files** (+1072/-33): gh.rs, GitHubItemDialog, PullRequestPage, ChatPage, NewWorkspaceComposerCard, useComposerState, issue-side-effect-gate(.test), github-issue-client(.test), harness.rs, harness/types.rs, chat.rs, github.rs, + spec.md/tasks.md. Scoped; the session's "58 files" warning is worktree-wide, not this commit. |
| 9 | `git diff … -- harness/drive.rs harness/helpers.rs task_sink.rs` | **empty** — drive loop + verdict contract + tracker sink untouched |
| 10 | `git diff … -- harness.rs` | **only** the `surface_tests` `.gitignore` pin at :1403 — zero production change |
| 11 | `git show 27f29f1c:…/gh.rs` (base stubs) | Confirmed `gh_work_item()` :506 and `gh_work_item_details()` :516 both returned `None` at base — root cause independently verified |

## Per-AC verdicts

| AC | Verdict | Key evidence |
|----|---------|--------------|
| **AC1** real body + author hydration | **PASS** | Base `gh_work_item_details`/`gh_work_item` were `None` stubs (run 11). Now real: `gh_view_json` shells `gh <issue\|pr> view <n> --repo <slug> --json <fields>` (gh.rs:514-540); details fetch requests `body,comments` (+`headRefOid` for PR) at :676-677; `map_work_item_details` (:599-620) carries body + hydrated author + comments. `gh_work_item` (:633-653) is the new-issue refine fetch replacing `author: null`. `None` now means a REAL failure (`.ok()?`, non-zero exit → None), never an empty success. |
| **AC2** inline error on failure | **PASS** | GitHubItemDialog.tsx:568-578 — new `else if (result === null)` branch writes `{details:null, error:'Could not load this item from GitHub — check that the `gh` CLI is installed and signed in…'}`. `detailsLoaded` (:486-488) is false when `error` set (guarded by `!cachedEntry.error`), so the "No description provided." success path is not taken; the error renders at :1074-1075 (`text-destructive` div) which **replaces** the conversation (`{error ? … : isIssuePage ? …}`). Same fix in PullRequestPage.tsx:584-590. |
| **AC3** Rust mapping regression tests | **PASS** | `map_work_item_details_issue_keeps_body_author_and_comments` (gh.rs:1141) asserts `body` verbatim, `item.author == "MateoCerquetella"`, `state` lowercased, assignees, and `comments[0].id == 98765` parsed from `#issuecomment-98765`. `map_work_item_details_pr_carries_head_sha` (:1174) asserts `headSha`, body, `type=="pr"`. `numeric_comment_id_parses_url_fragments_and_falls_back_uniquely` (:1196): `#issuecomment-42`→42, `#discussion_r777`→777, `""`→-1, `"no-digits-here-"`@idx2→-3 (stable unique negative React keys). |
| **AC4** armed-but-skipped toast | **PASS** | `maybeScaffoldSpecFromIssue` (useComposerState.ts:2247) and `maybeStartGatedRun` (:2284) both derive `deriveIssueSideEffectGate`; on `eligible === false` they `toast.warning(describeIssueSideEffectSkip('scaffold-spec'\|'start-gated-run', reason))` then return — the old silent `return`s are gone. Routing changed from `if (submitGatedRun)` (eligibility) to `if (startGatedRun)` (ARMED) at :2442 and :2656, so an armed-but-ineligible run reaches the toast instead of falling into the scaffold branch as a no-op (tasks.md Bug 2 cause 1). |
| **AC5** pure gate + unit tests | **PASS** | `deriveIssueSideEffectGate` (lib/issue-side-effect-gate.ts:26-44) is a pure fn returning a discriminated union (`eligible` + slug/number OR `reason`). Fed by BOTH submit paths (:2382, :2652 `.eligible`) AND both callbacks (:2247, :2284). 7 vitest cases (run 6): pass (local github issue), www+trailing, no-linked-item (null+undefined), not-an-issue (PR), not-github-url (empty/api-form/gitlab/pull-url), remote-repo (connectionId), + `describeIssueSideEffectSkip` copy. |
| **AC6** Chat carries repo id | **PASS** | ChatPage.tsx:527-560 — `filedRepoId = pinnedRepo?.id ?? workspaceId ?? undefined` stamped as both `preselectedRepoId: filedRepoId` (composer preselect, TaskPage.tsx:251) AND `repoId: filedRepoId ?? ''` on the handed-off work item. TaskPage `dialogRepoPath = repoMap.get(dialogWorkItem.repoId)?.path` (:550) resolves the path for the hydration fetch (which early-returns on null repoPath at GitHubItemDialog:502). Base ChatPage omitted `repoId` entirely (tasks.md cause 3) — confirmed via diff. |
| **AC7** draft-body endpoint + tests | **PASS** | `POST /api/github/issues/draft-body` mounted (github.rs:36); `draft_issue_body` route validates non-empty `workdir` (400), delegates title-blank check to the helper. Helper `chat::draft_issue_body` (chat.rs:1538) reuses the **actual** shared chat plumbing: `resolve_auth()` (:89, None→`NO_CREDS_MSG` 400), `gather_repo_context` (:211), `build_system` (:681), `call_anthropic(DEFAULT_MODEL,…,2048)` (:752), `truncate_chars(…,12_000)` (:150). Prompt names the SDD sections + `- [ ]` shape (`DRAFT_BODY_INSTRUCTIONS`); `draft_body_user_message` names the title. Tests: `draft_body_request_deserializes_with_and_without_slug` (missing workdir → err), `draft_body_response_serializes_body_field`, `draft_body_prompt_carries_title_and_section_contract`, `sanitize_draft_body_unwraps_whole_reply_fences_and_keeps_inner_ones`. |
| **AC8** Generate-description button | **PASS** | NewWorkspaceComposerCard.tsx:632-654 — button rendered only when `!createIssueBody.trim()` (blank-body only); disabled on `createIssueSubmitting \|\| createIssueGenerating \|\| !createIssueTitle.trim()`; spinner + "Generating description…" while running. `handleGenerateIssueBody` (useComposerState.ts:1596) calls `draftGithubIssueBody` then `setCreateIssueBody(body)` — fills the textarea, never files (separate handler from `onCreateIssueSubmit`). Errors → `setCreateIssueError`, rendered at :684-686; no-credentials message reaches the user verbatim via `extractServerErrorMessage` (client test asserts `"No LLM credentials for chat: set ANTHROPIC_API_KEY"`). Submit also held while generating (:706, avoids body-after-file race). Deterministic `## Context` submit auto-fill (spec 006) untouched — no diff to that path. |
| **AC9** self-ignoring harness scaffold | **PASS** | `scaffold_files()` prepends `(".gitignore", "*\n")` (types.rs:711-722); `scaffold_harness` skips existing files (`if path.exists() { continue; }`, :684) → idempotent, never clobbers a tracked `.gitignore`. Pinned by `harness.rs:1406` (`read_to_string(".agentum-harness/.gitignore") == "*\n"`). |

## Root-cause audit (each tasks.md cause verified with my own cite)

**Bug 1 — detail view drops body + author**
- **Cause 1 (IPCs were stubs) — CONFIRMED.** `git show 27f29f1c:…/gh.rs` (run 11): `gh_work_item()` :506-508 and `gh_work_item_details()` :516-518 both `return None` unconditionally. Line numbers match tasks.md exactly (:506, :516). These are the sole details/refine sources.
- **Cause 2 (null swallowed as success) — CONFIRMED.** The base fetch effect's final `else` writes `{details: result, fetchedAt: Date.now(), error: undefined}` (context lines in diff, unchanged); with `result === null` that yields `detailsLoaded === true` (`!error && fetchedAt>0`) → "No description provided." The fix inserts the `else if (result === null)` error branch *before* it (GitHubItemDialog:568, PullRequestPage:584).
- **Cause 3 (header renders un-hydrated prop) — CONFIRMED.** Diff shows `-{workItem.author ?? 'unknown'}` → `+{(displayWorkItem ?? workItem).author ?? 'unknown'}` at both :902 and :949; `displayWorkItem` (:606-614) merges `{...workItem, ...details.item}`. ChatPage stub omitted `repoId` at base — confirmed by the `+repoId: filedRepoId ?? ''` diff.

**Bug 2 — side effects silently do nothing**
- **Cause 1 (submit-time gates return silently) — CONFIRMED.** Diff removes the twin silent guards (`if (!scaffoldSpec || !item …) return` / `if (!link || … || connectionId) return`) and replaces the branch trigger `if (submitGatedRun)` → `if (startGatedRun)` (:2442/:2656) so armed-ineligible reaches the toast, not the dead scaffold branch.
- **Cause 2a (armed state outlives toggle on repo change) — CONFIRMED.** `handleRepoChange` now sets `setScaffoldSpec(false)`+`setStartGatedRun(false)` (:1935) alongside the pre-existing `setLinkedWorkItem(null)`.
- **Cause 2b/link-removal — CONFIRMED.** `handleRemoveLinkedWorkItem` (:1435) disarms both toggles for the same reason.
- **Cause 3 (Chat entry guarantees wrong/unset repo) — CONFIRMED.** Same `repoId` fix as Bug 1 cause 3; `filedRepoId` also flows to `preselectedRepoId` so the composer preselects the repo.

**Harness untracked noise — CONFIRMED.** `scaffold_files` had no ignore rule at base (types.rs); the fix + idempotent `path.exists()` skip + git's never-untracks semantics make the claim exact.

## Deviations audit (all 4 accurate)

- **D1** `gh_work_item_by_owner_repo` stays a stub — accurate: gh.rs:655-658 still `None`; degrades safely (no resolution → keeps the picked linked item).
- **D2** PR `files`/`checks` not enriched — accurate: `map_work_item_details` PR arm adds only `headSha` (from `headRefOid`); item+body+comments returned. Strictly better than the `None` stub; checks tab has its own `gh_pr_checks`.
- **D3** comment ids from URL fragment with negative fallback — accurate: `numeric_comment_id` (:546) `rsplit(['-','r'])` then `unwrap_or(-(index)-1)`; test pins both parse and fallback.
- **D4** `.gitignore` won't untrack existing tracked files — accurate: git semantics + the `path.exists()` idempotency skip; deletable to resume committing.

## Findings (all Info, none blocking)

1. **`filedRepoId ?? ''` degenerate edge (ChatPage:551):** if a user files from Chat with neither `pinnedRepo` nor `workspaceId` set, `repoId` becomes `''` → `repoMap.get('')` → null `repoPath` → the hydration fetch early-returns (GitHubItemDialog:502) with no inline error written. This is not #237's path (that flow has a workspace), and the fix wires the realistic case correctly, but the no-workspace edge can still render the optimistic shell without an error surface. Cannot confirm `workspaceId` is always non-null purely from code — see deferred list.
2. **`numeric_comment_id` parse depends on gh URL-fragment shape:** correct comment ids require gh emitting `#issuecomment-N` / `#discussion_rN`. A mis-parse only affects the numeric id (the details view is read-only; the negative fallback keeps React keys unique), so low risk — but the exact runtime fragment for #237's comments isn't verified from code.
3. **Mapping tests use synthetic `gh view --json` fixtures:** `author.login`, `comments[].{author.login,body,createdAt,url}`, `body`, `headRefOid` match gh's documented fields and reuse the same `map_issue`/`map_pr` author/label mapping the list already ships in production — but the true runtime shape for a live #237 is deferred to staging.
4. **Armed-but-ineligible gated run runs normal automation too:** when `startGatedRun` is armed but the gate is ineligible, `submitGatedRun` (eligibility) is false, so the toast fires AND `submitShouldRunIssueAutomation` may run the normal issue command. This is strictly better than the old silent no-op and only occurs from stale armed state (which the disarm fixes largely prevent); cosmetic.

## Deferred to browser QA / staging (out of tester scope by contract — not failures)

- Real #237 render in the installed app: body + author + comments hydrate (no "unknown" / "No description provided.").
- Real hydration-failure inline error (gh missing/unauthed/item gone) shown on the detail page.
- Real armed-toggle flow: "Scaffold spec" / "Start gated run" from the detail-page composer either fire or produce the naming `toast.warning`.
- Real "Generate description" click → SDD-shaped body fills the textarea; the no-credentials message renders inline.
- Real `.agentum-harness/` scaffold no longer showing as untracked in a worktree's `git status`.
- Confirmation that a Chat-filed issue always carries a resolvable `workspaceId`/`pinnedRepo.id` (Info finding 1's degenerate edge).
