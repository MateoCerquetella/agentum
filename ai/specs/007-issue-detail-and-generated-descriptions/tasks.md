# Spec 007 — tasks + root causes

## Root causes (with evidence)

### Bug 1 — detail view drops body + author

Three stacked causes:

1. **The hydration IPCs are stubs.**
   - `crates/agentum-desktop/src/commands/gh.rs:516` —
     `gh_work_item_details()` returned `None` unconditionally. This is the
     ONLY details source for the page (`GitHubItemDialog.tsx:529` →
     `getWorkItemDetailsForRepo` → `api.gh.workItemDetails` →
     `gh_work_item_details`).
   - `crates/agentum-desktop/src/commands/gh.rs:506` — `gh_work_item()`
     returned `None`; it is the refine fetch the new-issue flow uses to
     replace its optimistic stub (`TaskPage.tsx:2546-2567`), so the stub
     (`author: null`) never got replaced.
2. **The `null` result is swallowed as success.** `GitHubItemDialog.tsx`
   (fetch effect, `.then` at :551-574): a `null` IPC result with no cached
   details was written as `{details: null, fetchedAt: Date.now(), error:
   undefined}` — which makes `detailsLoaded` true (:486-488) → the
   conversation renders "No description provided."
   (`github-item-conversation.tsx:1293`) with zero error surface. Same
   pattern duplicated in `PullRequestPage.tsx` (~:585).
3. **The header renders the un-hydrated prop.** `GitHubItemDialog.tsx:892`
   and `:938` render `workItem.author ?? 'unknown'` from the list-row PROP;
   the hydrated `displayWorkItem` (:595, merges `details.item`) was only used
   for the conversation body. Any entry path that builds a stub item
   (`ChatPage.tsx:532` — `author: null`, NO `repoId`;
   `WorktreeCard.tsx:518-529` — `author: null`; `TaskPage.tsx:2531-2543` new
   issue stub — `author: null`) therefore shows "unknown" forever.
   `ChatPage.tsx:532` additionally omitted `repoId`, so
   `TaskPage.tsx`'s `dialogRepoPath` is null and the details fetch never even
   fires (`GitHubItemDialog.tsx:502` early-return) — body + author both
   permanently empty for detail pages opened from Chat's filed-issue cards.

### Bug 2 — "Scaffold spec" / "Start gated run" silently do nothing

The side effects are gated twice and every failure path was silent:

1. **Submit-time gates return silently.**
   `useComposerState.ts:2187` (`maybeScaffoldSpecFromIssue`) and `:2222`
   (`maybeStartGatedRun`): when `parseGitHubIssueOrPRLink(item.url)` isn't an
   issue link or `selectedRepo?.connectionId` is set, the callback `return`s
   with no user-visible signal. The submit paths' own `submitGatedRun`
   re-derivations (`useComposerState.ts:2310-2314`, `:2577-2581`) route an
   armed-but-ineligible gated run into the scaffold branch (where
   `scaffoldSpec` is false because the toggle is hidden while gated-run is
   armed — `NewWorkspaceComposerCard.tsx:688`) → complete no-op.
2. **Armed state can outlive its toggle.** Two ways the state stays armed
   while the eligibility (and the checkbox) disappears:
   - `useComposerState.ts:1851-1890` (`handleRepoChange`) wipes
     `linkedWorkItem` (:1875) but did NOT reset `scaffoldSpec`/`startGatedRun`
     → submit hits the `!item` guard and returns silently.
   - `TaskPage.tsx:4531` arms `startGatedRun: true` via `initialStartGatedRun`
     (`useComposerState.ts:578`) with no eligibility check at all.
3. **The Chat entry path guarantees a wrong/unset repo.** `ChatPage.tsx:532`
   builds the detail-page item without `repoId` → `openComposerForItem`
   (`TaskPage.tsx:2386`) finds no workdir and passes `initialRepoId:
   undefined` → the composer falls back to `activeRepoId`/first repo
   (`useComposerState.ts:382-389`); if the user corrects the repo by hand,
   cause 2a wipes the linked issue and the armed toggles no-op silently.

Fixes make the gate a tested pure function (`lib/issue-side-effect-gate.ts`),
warn on every armed-but-skipped path, reset armed toggles when the linked
item is wiped, and stamp `repoId` on Chat's detail-page item.

### Note — `.agentum-harness/` untracked noise

`scaffold_harness` (`crates/agentum-server/src/harness/types.rs:678`) wrote no
ignore rule, so a scaffold into a fresh worktree left `.agentum-harness/**`
untracked in `git status` (the "it's bad inside the worktree" complaint).
Fixed by adding a self-ignoring `.gitignore` (`*`) to `scaffold_files()` —
idempotent, and pre-existing tracked files are unaffected (gitignore never
untracks).

## Tasks

- [x] T1: implement `gh_work_item_details` + `gh_work_item` via the `gh` CLI
      (issue + PR view), with mapping unit tests (Rust).
- [x] T2: GitHubItemDialog/PullRequestPage: null-with-no-cache → visible error
      entry; header author uses hydrated item.
- [x] T3: ChatPage stamps `repoId` on the detail-page hand-off.
- [x] T4: pure `deriveIssueSideEffectGate` + `describeIssueSideEffectSkip`
      (+ vitest), used by both submit paths and both maybe* callbacks;
      `toast.warning` on armed-but-skipped; repo switch resets armed toggles.
- [x] T5: server `POST /api/github/issues/draft-body` reusing chat plumbing
      (auth resolution, `gather_repo_context`, `call_anthropic`), with prompt
      + validation + response-shape tests.
- [x] T6: composer "Generate description" button (blank-body only, disabled
      while running, inline errors, fills the textarea).
- [x] T7: `.agentum-harness/.gitignore` (`*`) in `scaffold_files()`.

## Deviations

- D1: `gh_work_item_by_owner_repo` (smart name-field lookup) stays a stub —
  its null result degrades safely (no resolution → keeps the picked linked
  item); out of scope for this spec.
- D2: PR `files`/`checks` in `gh_work_item_details` are not enriched (the
  checks tab already has `gh_pr_checks`); the command returns item + body +
  comments (+ headSha for PRs). Strictly better than the previous stub.
- D3: comment ids are parsed from the comment URL fragment
  (`#issuecomment-N` / `#discussion_rN`); when unparsable they fall back to a
  stable negative index so React keys stay unique (gh's JSON exposes only
  GraphQL node-id strings).
- D4: `.agentum-harness/.gitignore` means NEW projects won't accidentally
  commit harness runtime state. Repos that already track `.agentum-harness/`
  keep their tracked files (gitignore does not untrack); they can delete the
  generated `.gitignore` to keep committing it.

## Not GUI-verified

Everything in this spec is verified by unit tests + builds only. The
installed-app flows (detail page hydration, toast warnings, generate button,
end-to-end scaffold/gated-run from the detail page) still need live
verification.
