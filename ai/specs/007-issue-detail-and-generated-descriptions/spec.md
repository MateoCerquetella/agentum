# Spec 007 — Issue detail hydration + non-silent composer gates + generated descriptions

Status: In progress
Owner: sdd-developer (autonomous /loop run, compressed SDD)

## Problem

Live evidence against the installed v0.54.0 app (loopback 127.0.0.1:62267):

1. **Bug 1 — the full-page GitHub issue detail drops body + author.** Issue
   #237 is perfect on GitHub (author, 2940-char body, labels), but the app's
   detail view (`GitHubItemDialog` hosting `github-item-conversation`) renders
   "unknown opened this issue" and "No description provided." — the display /
   hydration path is broken, not creation.
2. **Bug 2 — "Scaffold spec" / "Start gated run" do nothing from this page's
   flow.** The user arms the toggles in the composer (opened via "Start
   workspace from issue" on the detail page) and nothing happens. The server
   seams are verified fine (`POST /api/harness/spec-from-issue` works against
   the live app); the client gates fail silently.
3. **Feature — generated descriptions.** Creating an issue with only a title
   should offer a real, SDD-shaped description generated from the title +
   project context, reviewable in the textarea before filing.

## Goal

- The issue detail page hydrates body, author, and comments for any GitHub
  issue/PR the local `gh` can see; hydration failures are visible inline.
- Arming "Scaffold spec" / "Start gated run" either performs the side effect
  or tells the user exactly why it was skipped (toast naming the reason).
- A "Generate description" button in the composer's create-issue form drafts
  an SDD-shaped body (## Problem / ## Goal / ## Acceptance criteria with
  `- [ ]` items) grounded in the repo context, filling the textarea for
  review — never silently posting LLM text.

## Acceptance criteria

- [ ] AC1: Opening the detail page for a fresh issue renders the real body +
      author (no "unknown" / "No description provided." for a hydratable
      issue). Backed by real `gh_work_item_details` / `gh_work_item` Tauri
      commands (were stubs returning `None`).
- [ ] AC2: When hydration fails (gh missing/unauthed/item gone) the page shows
      an inline error line instead of silently rendering an empty success.
- [ ] AC3: Regression tests exist at the Rust mapping seam (gh JSON →
      renderer details shape: body, author, numeric comment ids).
- [ ] AC4: An armed "Scaffold spec"/"Start gated run" that is skipped at
      submit produces a `toast.warning` naming the reason (no linked issue /
      not a github.com issue URL / remote repo).
- [ ] AC5: The gate derivation is a pure function with unit tests.
- [ ] AC6: The detail page opened from Chat's filed-issue card carries the
      repo id so hydration and the composer's repo preselection work.
- [ ] AC7: `POST /api/github/issues/draft-body` {workdir, title, slug?} →
      {body}: validates args, reuses chat auth/model/repo-context plumbing,
      returns an SDD-shaped body; unit tests cover arg validation, prompt
      content (title + section instruction), and response shape.
- [ ] AC8: Composer create-issue form shows "Generate description" beside the
      body textarea when the body is blank; disabled while running; fills the
      textarea; failures render inline; the no-credentials case shows the
      chat's "connect an account" style message. The deterministic `## Context`
      auto-fill at submit stays unchanged.
- [ ] AC9 (small, opportunistic): `.agentum-harness/` scaffolds no longer show
      as untracked noise in a worktree's git status (self-ignoring
      `.gitignore` written by the scaffold).

## Non-goals

- Implementing the remaining stubbed gh_* mutations (`gh_update_issue`, merge,
  reviewers, …).
- PR files/checks enrichment beyond what the shared view command returns.
- GUI verification of the installed app (left to tester/orchestrator).
