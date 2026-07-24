# Handoff — Developer → Tester (Spec 018)

**Date:** 2026-06-23 · **From:** Developer · **To:** Tester · **Gate:** Developer gate **PASSED** (6/6, build+tests independently re-verified)

---

## 1. Summary

The Chat→issue critical path is now deterministic and server-side. `create_goal`
calls `TaskSink::select(&wd).create_feature(...)` synchronously and returns
`FeatureRef{provider,id,url}` or a typed `422 {error:{code,message,provider}}`
envelope — no planner agent, no "planning…" hang. S1+S2+S3 all implemented.

## 2. Completed Work (committed)

- **Worktree:** `/private/tmp/agentum-work` · **branch:** `spec/reliable-chat-github-issues`
  · **commit:** `58a406e` (clean tree). The develop+011/012 integration branch
  (has `task_sink.rs`).
- **Files (5):** `routes/board_goals.rs` (+637/−, the core rewrite),
  `task_sink.rs` (`FeatureRef: Serialize`, `parse_gh_issue_url` pub(crate)),
  `host_runtime.rs` (`gh_in_dir`), `ui/.../board-client.ts` (`createGoal` + typed
  error), `ui/.../harness/ChatPage.tsx` (deterministic submit + issue-link card).
- **Verified by orchestrator:** `cargo test -p agentum-server --lib` →
  **347 passed / 0 failed / 4 ignored** (52s); `cargo build -p agentum-server`
  green; `bun run build` green; diff clean (no TODO/unimplemented!).

## 3. Pending Work

- None for implementation. S4 (LLM decomposition) is intentionally out of scope.

## 4. Important Decisions / Deviations from architecture.md

1. `ChatPage.tsx::submit` was **already** on the `createGoal` (planner) path in
   this worktree, **not** the harness path the architecture cited — so S2 was a
   contract+rendering update, not a re-point. Same end state, less surgery.
2. `git_in_dir` is at `host_runtime.rs:1612` (not `:1512`) — minor line drift;
   pattern matched.
3. **Removed two obsolete tests** (`invalid_planner_config_does_not_orphan_goal`,
   `create_goal_with_missing_planner_binary_*`) that asserted planner-spawn
   behavior no longer in the critical path; replaced with the 3 S1 AC tests + 2
   units. Planner *code* stays dormant; only its create-path *tests* were dropped.
4. **S3 implemented fully** (real remote `gh` exec) rather than the sanctioned
   `remote_unsupported` minimum — `gh_in_dir` mirrored `git_in_dir` cheaply.
5. Remote `gh_in_dir` transport failure maps to `remote_unsupported`.

## 5. Risks / Known gaps for you to probe

- **AC-6 remote SSH path has NO automated test** (repo's `#[ignore]`-for-live-infra
  convention). `gh_in_dir`'s local branch compiles; the SSH branch needs a live
  host. Decide if a `#[ignore]` live test is wanted.
- The **account picker** in `ChatPage.tsx` is now vestigial (no planner runs on
  submit); left intact to keep S2 minimal. Cosmetic, not a correctness issue.

## 6. Questions

- None blocking.

## 7. How to verify (Tester)

Work in the worktree: `cd /private/tmp/agentum-work`.
- **Rust (AC-1/3/4/5):** `~/.cargo/bin/cargo test -p agentum-server --lib`
  (expect 347/0/4). Key tests: `board_goals::tests::create_goal_returns_feature_ref_for_created_card`,
  `…create_goal_not_a_github_repo_returns_typed_error`, `…create_goal_empty_title_is_rejected`,
  `…create_error_builds_the_typed_envelope`, `…classify_gh_stderr_distinguishes_repo_from_other_failures`,
  `…create_goal_inserts_board_item_with_lbl_goal` (provider=="board").
- **UI (AC-2):** `cd crates/agentum-desktop/ui && bun run build` (green).
- **Per-AC verdict requested:** AC-1 (sync local create, no agent), AC-2
  (FeatureRef rendered + Board fetch), AC-3 (typed loud errors, never silent
  hang), AC-4 (provider selection + surfaced Board fallback), AC-5 (tests, no
  live gh), AC-6 (remote path returns issue-or-typed-error, never silent local).
- Recommended: give a pass/fail per AC + repro steps; AC-6's live SSH leg can be
  marked "not live-tested (infra), code-reviewed only" per the repo convention.

**Recommended next step:** Tester runs the suite above, issues a per-AC verdict,
then hands to Reviewer for the final maintainability sign-off.
