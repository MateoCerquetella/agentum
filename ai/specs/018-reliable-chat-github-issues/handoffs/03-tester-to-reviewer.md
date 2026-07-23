# Handoff — Tester → Reviewer (Spec 018)

**Date:** 2026-06-23 · **From:** Tester · **To:** Reviewer · **Gate:** Tester gate **PASSED** (5/5) · **Overall verdict: PASS — 6/6 acceptance criteria**

---

## 1. Summary

Every acceptance criterion is met against commit `58a406e` (worktree
`/private/tmp/agentum-work`, branch `spec/reliable-chat-github-issues`, clean
tree). The autonomous planner is genuinely off the issue-creation critical path;
errors are typed and loud end-to-end; the Board fallback is a surfaced success;
remote routing is code-correct (live SSH leg infra-gated per repo convention).

## 2. Test run evidence (reproduced)

- **Rust:** `~/.cargo/bin/cargo test -p agentum-server --lib` →
  **`347 passed; 0 failed; 4 ignored`** (reproduced twice: 60.03s / 60.68s). The
  4 ignored = live-infra tests (tmux ×3, live `gh` ×1), per convention; none are
  spec-018 unit/handler tests. The 8 spec-018 tests all pass.
- **UI:** `cd crates/agentum-desktop/ui && bun run build` → **`✓ built in 1m 47s`**
  (green; only the pre-existing chunk-size advisory).

## 3. Per-AC verdict

| AC | Verdict | Evidence (file:func) |
|---|---|---|
| AC-1 sync local create, no agent, no "planning…" | **PASS** | `board_goals.rs::create_goal`→`create_feature_for_goal`→`TaskSink::select(&wd).create_feature()` synchronous; `spawn_planner_session` `#[allow(dead_code)]`, never called; response has `feature: FeatureRef`, no `planner_session_id`. Test `create_goal_returns_feature_ref_for_created_card`. |
| AC-2 feedback (number+URL) + on Board | **PASS** | `FeatureRef` serialized verbatim; `ChatPage::CreatedFeatureCard` renders "Open GitHub issue" link; Board unchanged (fetch-based per §6/§10). UI build green. |
| AC-3 loud, specific errors, never silent | **PASS** | `422 {error:{code,message,provider}}` via `create_error`→`ApiError::Custom`; empty title rejected up front; `board-client.createGoal` parses the structured envelope; `describeGoalError` maps codes. Tests: `…not_a_github_repo…`, `…empty_title…`, `…create_error_builds…`, `…classify_gh_stderr…`. |
| AC-4 provider selection + surfaced fallback | **PASS** | `pick_provider` GitHub→Linear→Board; `Board` outcome = `Ok(provider:"board")` surfaced (UI: "Connect GitHub to create real issues"). Tests: `pick_provider_precedence_*`, `…inserts_board_item_with_lbl_goal` (provider=="board"). |
| AC-5 tested, no live gh | **PASS** | Handler tests force `AGENTUM_TASK_SINK=board` under `TEST_ENV_LOCK`, hermetic; the `#[ignore] github_sink_creates_a_real_issue` live test is intact/untouched. |
| AC-6 remote: host create or typed error, never silent local | **PASS (code-verified, not live — infra)** | `create_feature_for_goal` branches `Ssh`→`create_github_issue_remote`→`gh_in_dir` runs `gh` on the host (shell-quoted, bounded `GIT_TIMEOUT`); transport fail→`remote_unsupported`; non-zero→`classify_gh_stderr`. SSH branch passes the raw remote workdir (no local FS check) → no silent local `gh`. UI passes `host_id`. Live leg needs a real host (repo `#[ignore]`-for-live convention). |

## 4. Scope / flakiness

- Right-sized (6 handler + 2 unit tests for the new path; existing `task_sink`
  suite reused untouched). The two obsolete planner-spawn tests were correctly
  removed (behavior no longer on the path). No flaky tests (deterministic;
  reproduced twice). Spec edge cases covered (empty title, non-GH repo,
  precedence, surfaced board fallback).
- One acceptable gap: AC-6's live SSH leg has no automated test (per convention;
  spec §9 R2 sanctions the `remote_unsupported` fallback).

## 5. Observations for Reviewer (NON-blocking — correctness-neutral)

1. **Vestigial account picker** in `ChatPage.tsx` (`:128–165`, `:483–498`): the
   Claude/Codex account selector no longer affects issue creation (no planner on
   submit). Dead UX, not a bug — a cleanup candidate (Developer disclosed it).
2. **Theoretical stale-repo `host_id` window:** if a remote repo's `hostId`
   isn't backfilled, `submit()` sends `host_id:null` → server resolves
   `LOCAL_HOST_ID` → local `gh`. In practice `backfillRemoteRepoHostIds` runs on
   store load before submit; worst case fails loudly as `not_github_repo`
   (AC-3-compliant, never silent). Flag only.
3. `goal.created` emitted after `board.created` is intentional (test drains past
   `board.created`).

## 6. Questions

- None.

## 7. Recommended Next Step

**Reviewer** does the final maintainability sign-off (the spec is functionally
complete and correct). Optionally weigh in on observation #1 (vestigial account
picker) — a correctness-neutral cleanup, not a blocker. One optional test worth
considering: an `#[ignore]` live-SSH smoke test for `create_github_issue_remote`
mirroring `github_sink_creates_a_real_issue`.
