# Tasks — Spec 018: Reliable Chat → GitHub issues

> Implemented in worktree `/private/tmp/agentum-work` (branch
> `spec/reliable-chat-github-issues`, the develop+011/012 integration branch).
> Build: `cargo build -p agentum-server` green. Tests:
> `cargo test -p agentum-server --lib` → **347 passed / 0 failed / 4 ignored**
> (independently re-verified by the orchestrator, 52s). UI: `bun run build` green.

## S1 — deterministic create (local, 1:1) — AC-1/3/4/5  [DONE]
- [x] `FeatureRef` derives `Serialize` (task_sink.rs)
- [x] `parse_gh_issue_url` → `pub(crate)` (task_sink.rs)
- [x] `CreateGoalBody`: add `host_id`; `CreateGoalResponse`: `planner_session_id` → `feature: FeatureRef`
- [x] `create_goal`: empty-title 422 up front; sync `TaskSink::select().create_feature()`; typed-error envelope
- [x] `spawn_planner_session` retained dormant (`#[allow(dead_code)]`)
- [x] Handler tests (force `AGENTUM_TASK_SINK=board`): returns-FeatureRef; not-a-github-repo → typed error; empty-title rejected; + 2 unit tests
- [x] `cargo build -p agentum-server` green
- [x] `cargo test -p agentum-server --lib` green (347/0/4)

## S2 — Chat UI feedback — AC-2  [DONE]
- [x] `board-client.ts`: `FeatureRef`/`CreateGoalError` types; `createGoal` parses `{error:{code,message,provider}}`, accepts `host_id`
- [x] `ChatPage.tsx::submit` → deterministic createGoal; passes `repo.hostId`
- [x] Render created issue link (`CreatedFeatureCard`); surface typed error (`describeGoalError`)
- [x] Remove indefinite "planning…"/`pendingGoalId` model
- [x] `bun run build` green; changed files tsc-clean

## S3 — remote repos — AC-6  [DONE]
- [x] `host_id` on `CreateGoalBody`; resolve `Host` via `get_host(host_id ?? LOCAL_HOST_ID)`
- [x] `host_runtime::gh_in_dir` (mirrors `git_in_dir`)
- [x] Dispatch GitHub create local-vs-ssh; ssh stdout → `parse_gh_issue_url`; transport fail → `remote_unsupported`

## S4 — LLM decomposition — OUT OF SCOPE (separate spec)
- [ ] (not built — intentionally)

## Notes for Tester / Reviewer
- Remote SSH `gh` path (AC-6) has no automated test (repo's `#[ignore]`-for-live-infra convention); `gh_in_dir` local branch is built/compiled but the SSH branch needs a live host.
- The account picker in `ChatPage.tsx` is now vestigial (no planner runs on submit); left intact to keep S2 minimal — candidate removal in a follow-up. The agent picker stays meaningful.
