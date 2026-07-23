# Handoff — Architect → Developer (Spec 018)

**Date:** 2026-06-23 · **From:** Architect · **To:** Developer · **Gate:** Architect gate **PASSED** (5/5 + patterns honored)

---

## 1. Summary

`architecture.md` is complete and grounded in real code. The fix removes the
non-deterministic planner agent from the Chat→issue critical path and replaces
it with a **synchronous, server-side `TaskSink::Github.create_feature`** call in
`create_goal`, returning a `FeatureRef` (or a loud, typed error) as the HTTP
response. The reliable engine (`task_sink.rs`) already exists and is unit-tested
— this spec changes *where* it's called, not the engine.

## 2. Completed Work

- `ai/specs/018-reliable-chat-github-issues/architecture.md` written: Components
  (modify vs. dormant tables), the `POST /api/board/goals` contract change
  (success `FeatureRef` + AC-3 error envelope), data flow, 5 documented
  decisions, R1–R6 with mitigations, and S1→S3 slice→code mapping.
- All real paths + symbols verified by grep against `develop`.

## 3. Pending Work (your implementation, in slice order)

> Ship **S1 first** (it satisfies AC-1/3/4/5 alone and is independently
> shippable). S2 (UI), then S3 (remote) follow.

- **S1 (critical path):** edit `create_goal` (`board_goals.rs:51`) — replace
  `spawn_planner_session` (step 5) with
  `TaskSink::select(&wd).create_feature(SinkCtx{store,&wd,None}, NewFeature{title,body})`;
  return `{ goal, feature: FeatureRef }` on Ok, `422 { error:{code,message,provider} }`
  via `ApiError::Custom` on Err; reject empty/whitespace title up front. Copy the
  call shape from `plan_goal_harness` (`board_goals.rs:166`). Add handler tests
  (force `AGENTUM_TASK_SINK=board` for hermetic, gh-free determinism): returns
  FeatureRef; not-a-github-repo → typed error; empty title rejected.
- **S2:** add `createGoal()` to `board-client.ts`; re-point `ChatPage.tsx::submit`
  (`:164`) off the harness path onto it; render `feature.url` link / the specific
  error; drop the indefinite "planning…" model for Chat-create. Parse the nested
  `{error:{code,message,provider}}` envelope (don't flatten it). Build:
  `cd crates/agentum-desktop/ui && bun run build`.
- **S3 (stretch, AC-6):** add `host_id` to `CreateGoalBody`; resolve `Host` via
  `state.store.get_host(host_id ?? LOCAL_HOST_ID)`; add `gh_in_dir` to
  `host_runtime.rs` mirroring `git_in_dir` (`:1512`); dispatch GitHub create
  local-vs-ssh; feed ssh stdout to `parse_gh_issue_url`. If you defer S3, return
  `422 {code:"remote_unsupported"}` for SSH workdirs — never a silent local `gh`.
- Update `ai/specs/018-reliable-chat-github-issues/tasks.md` (create it) with
  honest per-slice checkboxes. Build + test: `cargo build`, then
  `cargo test -p agentum-server --lib` (+ `bun run build` for the UI slice).

## 4. Important Decisions (carry these — don't relitigate)

- Synchronous `TaskSink` create **over** the autonomous planner → determinism +
  loud errors beat richness.
- **1:1 issue per submit** (title = description) **over** LLM decomposition →
  working 1:1 beats a broken backlog. **Decomposition (S4) is OUT of scope.**
- Reuse `plan_goal_harness`'s proven `SinkCtx`/`create_feature` shape **over** a
  new abstraction.
- `ApiError::Custom` envelope **over** a new error type.
- Explicit `host_id` in the request **over** path→host inference.

## 5. Risks (mitigations are in `architecture.md` §Risks — honor them)

- R4: keep `tokio::process::Command` (async) — do **not** convert to blocking
  `std::process`, or you'll wedge the axum worker.
- R6: a `Board` fallback is a **surfaced success** (`provider:"board"` + UI
  note), not a silent failure — AC-4.
- R2/AC-6: isolate remote behind S3; a clear `remote_unsupported` error is an
  acceptable v1 for SSH repos.

## 6. Questions / Blockers

- ⚠️ **LOAD-BEARING — BRANCH (verified):** the current checkout
  `feat/014d-board-desktop-ui` does **not** have `crates/agentum-server/src/task_sink.rs`,
  and its `board_goals.rs` is an older version without the `TaskSink` wiring.
  **`develop` has both.** → **Base the 018 worktree off `develop`** (a fresh
  `git worktree add … develop` / branch `feat/018-reliable-chat-github-issues`),
  not off `feat/014d`. If you implement on `feat/014d` the cited files and line
  numbers won't exist. This is a setup step, **not** a spec defect — the design
  is coherent against `develop`.
- Concurrent-checkout rule applies: stage only your own files; never `git add -A`,
  `reset --hard`, `checkout`, or `stash` in the shared main checkout.
- No other open questions. Spec §10's mirror-card question is resolved → "rely on
  the board_sync fetch" (no Board change).

## 7. Recommended Next Step

**Developer** implements **S1** on a worktree based off **`develop`**: the
`create_goal` swap + handler tests, building green (`cargo test -p agentum-server
--lib`). S1 alone satisfies AC-1/3/4/5 and is the shippable core; do S2/S3 after
S1 is green. Then hand off to **Tester** with a per-AC checklist.
