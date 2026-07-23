# Spec 018 — Reliable Chat → GitHub issues

| | |
|---|---|
| **Status** | Draft (PM gate) |
| **Owner** | Mateo Cerquetella |
| **Created** | 2026-06-23 |
| **Supersedes** | the agent-planner Chat flow shipped in v0.20.7 |

---

## 1. Problem (one sentence)

Describing a feature in the desktop **Chat** does **not** reliably create GitHub
issues on the Board — the goal hangs at "planning…" forever — because issue
creation depends on an autonomous agent that is non-deterministic, has no
completion guarantee, gives the UI no error feedback, and runs locally (so it
also can't reach remote SSH repos).

### Evidence / root cause
- v0.20.7 routes Chat submit → `create_goal` → `spawn_planner_session` (a real
  `claude` agent) whose prompt tells it to run `gh issue create` per decomposed
  feature. Observed: a goal in the **local, gh-authed** `agentum` repo still
  hangs at "planning…" with zero issues created → the agent is the weak link,
  not auth or the repo.
- Secondary: when a goal's `workdir` is a **remote SSH repo** (e.g.
  `/home/malloc/...` on the Freebee host; status bar "SSH Partial"), the planner
  runs **locally**, so local `gh` cannot see the repo at all.
- The reliable engine **already exists and is unit-tested**:
  `crate::task_sink::TaskSink::Github.create_feature()` runs
  `gh issue create --title --body` and parses the issue URL/number.

## 2. Goal

On Chat submit, **deterministically** create the GitHub issue(s) **server-side**
via `TaskSink`, surface the result (or a clear error) in the Chat, and show the
issue on the Board — no autonomous agent in the critical path.

## 3. Users / personas

- **Mateo (solo maintainer)** — describes a feature in Chat and expects a real
  GitHub issue on the Board within seconds, or an actionable error. Works across
  local repos and remote SSH repos.

## 4. Acceptance criteria

1. **AC-1 (happy path, local):** Submitting a non-empty description in Chat,
   with a local GitHub repo selected, creates **≥1 real GitHub issue** within a
   few seconds — no agent spawned, no "planning…" hang.
2. **AC-2 (feedback):** The created issue's number + URL is returned by the API
   and rendered in the Chat thread, and the issue appears on the **Board**
   (Tasks/GitHub view) on the next refresh.
3. **AC-3 (errors are loud):** If creation fails (no `gh`, not a GitHub repo,
   `gh` non-zero), the Chat shows a **specific error** (e.g. "Connect GitHub /
   not a GitHub repo") — never a silent indefinite "planning…".
4. **AC-4 (provider selection):** GitHub is preferred when available
   (`TaskSink::select` → Github); Linear/board fallbacks behave per `TaskSink`
   and are also surfaced, not silent.
5. **AC-5 (tested):** The create-on-submit path and its error envelope are
   covered by unit/handler tests (no live `gh` needed); the existing
   `#[ignore]` live `gh` test still documents the real path.
6. **AC-6 (remote, stretch):** When the repo lives on an SSH host, the issue is
   created **on that host** (run `gh` via the existing host_runtime exec), or a
   clear "remote repos not yet supported" error — never a silent local failure.

## 5. Non-goals (this slice)

- LLM-powered decomposition of one description into 3–7 issues — v1 is **one
  issue per submit** (title = description). Decomposition is a follow-up spec.
- Removing the agent-planner code wholesale — it may stay dormant; this spec only
  removes it from the issue-creation **critical path**.
- GitHub issue **state** sync back to the Board columns (already a documented
  TaskSink follow-up).

## 6. Approach (architecture notes for the architect)

- **Server is the source of truth.** Replace the agent in the Chat path with a
  direct call: `TaskSink::select(workdir).create_feature(SinkCtx{…}, NewFeature{
  title, body })`. Reuse the seam exactly — no new provider logic.
- **Contract:** `POST /api/board/goals` (or a sibling endpoint) returns the
  created `FeatureRef { provider, id, url }` (or a structured error), instead of
  a `planner_session_id`. Keep the goal row as the Chat-side tracking record.
- **Remote (AC-6):** route `gh` through the same host_runtime exec used for
  remote git/worktree ops when `workdir` belongs to an SSH host; otherwise local.
- **UI:** Chat renders the returned issue (link) or the error; drop the
  children-poll/"planning…" model for this path. The Board already lists GitHub
  issues — no Board change needed.

## 7. Implementation slices (incremental, each independently shippable)

1. **S1 — deterministic create (local, 1:1).** Chat submit → server
   `TaskSink::Github.create_feature` → return `FeatureRef`/error. Unit + handler
   tests. *(satisfies AC-1, AC-3, AC-4, AC-5)*
2. **S2 — Chat UI feedback.** Render the created issue link + surface errors;
   remove the indefinite "planning…". *(AC-2)*
3. **S3 — remote repos.** Run `gh` on the workdir's host via host_runtime.
   *(AC-6)*
4. **S4 (separate a follow-up spec) — LLM decomposition** into 3–7 issues.

## 8. Test plan

- Unit: `TaskSink::Github` argv + URL parse (exists); provider selection (exists).
- Handler: create-goal-creates-issue (mock/stub sink) returns `FeatureRef`;
  not-a-github-repo returns the structured error; empty title rejected.
- Live (`#[ignore]`): real `gh issue create` in a gh-authed repo (exists).

## 9. Risks

- **R1:** 1:1 (no decomposition) is less rich than the "backlog" vision — but a
  *working* 1:1 beats a broken planner. Mitigated by a follow-up spec.
- **R2:** Remote `gh` exec adds host-runtime coupling — isolate behind S3.
- **R3:** `gh` rate limits / auth drift — surfaced as AC-3 errors, not hangs.

## 10. Open questions

- Should S1 also write the issue to the board as a mirror card (so it shows even
  before a GitHub re-fetch), or rely on the Tasks/GitHub fetch? (Lean: rely on
  the fetch; `board_sync` already mirrors issues.)
