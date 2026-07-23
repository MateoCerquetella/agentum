# Architecture Notes — Spec 018: Reliable Chat → GitHub issues

> Source of truth: `ai/specs/018-reliable-chat-github-issues/spec.md` (no PM
> handoff; STATE.md was clobbered — written from the spec directly).
> Grounded against the integration branch where spec 011/012 are merged
> (`task_sink.rs` exists). **Branch caveat — read "Boundaries" first.**
>
> ⚠️ **BRANCH (verified by orchestrator 2026-06-23):** the current checkout
> `feat/014d-board-desktop-ui` does **not** track `crates/agentum-server/src/task_sink.rs`
> (the core dependency) — `develop` does. `board_goals.rs` exists here but is an
> older version without the `TaskSink` wiring. **The Developer must base the 018
> worktree off `develop`** (per the integration-branch `CLAUDE.md`), not off
> `feat/014d`, or the cited files/line numbers won't exist. The design itself is
> coherent against `develop`/`main` and needs no rework.

## Components

The fix replaces the autonomous planner agent in the Chat→issue critical path
with a **synchronous, server-side `TaskSink` create**. The seam already exists
and is unit-tested; this spec moves the call from the *post-decomposition*
endpoint into the *create* endpoint and makes the result (or a loud error) the
HTTP response.

### Modify

| Component | Real path | Symbol(s) | Change |
|---|---|---|---|
| Goals route handler | `crates/agentum-server/src/routes/board_goals.rs` | `create_goal` (`:51`), `CreateGoalBody` (`:28`), `CreateGoalResponse` (`:45`) | Replace step 5 (`spawn_planner_session`) with a direct `TaskSink::select(&wd).create_feature(SinkCtx, NewFeature)`. Return `FeatureRef` (S1) or a structured error (AC-3). Add optional `host_id` to the body (S3). |
| The reliable engine (reused verbatim) | `crates/agentum-server/src/task_sink.rs` | `TaskSink` enum, `TaskSink::select(&Path) -> TaskSink` (`:95`), `TaskSink::create_feature(&SinkCtx, &NewFeature) -> anyhow::Result<FeatureRef>` (`:109`), `SinkCtx{ store, workdir, parent_goal_id }` (`:42`), `NewFeature{ title, body }` (`:24`), `FeatureRef{ provider: &'static str, id: String, url: Option<String> }` (`:33`) | **No change.** Call it; do not reimplement provider logic. |
| Router merge | `crates/agentum-server/src/lib.rs` | `routes::board_goals::router()` (`:261`) | No change — route already mounted. |
| Error envelope | `crates/agentum-server/src/error.rs` | `ApiError::Custom(StatusCode, serde_json::Value)` (`:31`), default `{"error": msg}` (`:53`) | No change — `Custom` is the existing escape hatch for a structured body; reuse it for AC-3. |
| Remote exec seam (S3 only) | `crates/agentum-server/src/host_runtime.rs` | `git_in_dir(&Host, cwd, args)` (`:1512`) as the pattern; `ssh_output` (resilient runner) | **Add one small helper** `gh_in_dir`/`command_in_dir` mirroring `git_in_dir`'s `Local`→`Command::new(bin)`, `Ssh`→`sh -c 'cd <cwd> && <argv>'` dispatch. No new abstraction beyond this. |
| Host resolution (S3) | `crates/agentum-server/src/routes/repos.rs` | `load_host_for_repo(state, repo_id) -> Host` (`:370`); `Host`/`HostKind::{Local,Ssh}` in `crates/agentum-core/src/lib.rs` (`:70`,`:85`); `LOCAL_HOST_ID` (`:127`) | No change — call `state.store.get_host(host_id)` (pattern: `sessions.rs::create` `:142`). |
| UI Chat surface (S2) | `crates/agentum-desktop/ui/src/components/harness/ChatPage.tsx` | `submit()` (`:164`) | Currently submits to the **harness** (`scaffoldHarness`+`startHarness`), gated behind a "Soon" easter egg. Re-point `submit()` at the goals client, render the returned `FeatureRef` (issue link) in the thread, and surface the structured error. Drop the "planning…"/children-poll model for this path. |
| UI goals client (S2) | `crates/agentum-desktop/ui/src/runtime/board-client.ts` | `request<T>` (`:59`); siblings `getBoard`/`pushCard`/… | Add `createGoal(body): Promise<CreateGoalResponse>` (POST `/api/board/goals`). **Improve error handling** (`:70-71`) to parse the structured `{ error: { code, message, provider } }` envelope instead of flattening it into a string — AC-3 needs the specific message. |
| Handler tests (S1, AC-5) | `crates/agentum-server/src/routes/board_goals.rs` `#[cfg(test)] mod tests` | existing `fresh_state()`, `isolate_xdg()` harness | Add: create-goal-returns-FeatureRef (force `AGENTUM_TASK_SINK=board` for hermeticity → deterministic `FeatureRef{provider:"board"}`); not-a-github-repo → structured error; empty title rejected. |
| Sink unit tests (exist) | `crates/agentum-server/src/task_sink.rs` `#[cfg(test)] mod tests` | `pick_provider_precedence_*`, `gh_create_argv_is_noninteractive`, `parse_gh_issue_url_*`, `#[ignore] github_sink_creates_a_real_issue` | No change — already cover AC-4 selection + the `#[ignore]` live `gh` path (AC-5's live documentation clause). |

### Leave untouched (dormant)

- **The planner agent** — `spawn_planner_session` (`board_goals.rs:286`),
  `crates/agentum-server/src/planner.rs`, `PlannerConfig`, `goal.planner.*`
  events. Per Non-goals §5, it stays in the tree but is removed from the
  issue-creation critical path. `spawn_card_session` (`board_goals.rs:296`,
  the PATCH→doing auto-spawn) is a **different** flow — do not touch it.
- **`plan_goal_harness`** (`board_goals.rs:166`) — the post-decomposition
  endpoint that already calls `TaskSink` over a goal's *children*. It is the
  pattern donor (copy its `SinkCtx`/`create_feature`/`apply_tracker_transition`
  call shape), not the edit site.
- **The Board** — `routes/board.rs`, `routes/board_sync.rs`, the board UI. Per
  spec §6, the Board already lists GitHub issues (`board_sync` mirrors them); no
  Board change is needed. Open question §10 (mirror card vs. fetch) resolves to
  "rely on the fetch" — do nothing here.
- **`apply_tracker_transition` / `TrackerPhase`** (`task_sink.rs:215`) — the
  harness lifecycle layer (spec 012). Out of scope for 018.

---

## APIs

### Contract change — `POST /api/board/goals`

**Request** (`CreateGoalBody`, `board_goals.rs:28`) — add `host_id`:

```jsonc
{
  "title": "string (required, non-empty)",
  "body": "string | null",
  "workdir": "string | null",   // repo dir; falls back to daemon cwd
  "host_id": "uuid | null"       // NEW (S3): SSH host the workdir lives on;
                                 //   absent / nil-UUID = local (LOCAL_HOST_ID)
}
```

**Response — success (200/201)** — return the `FeatureRef`, drop
`planner_session_id`:

```jsonc
{
  "goal": { /* BoardItem — kept as the Chat-side tracking record */ },
  "feature": { "provider": "github", "id": "42",
               "url": "https://github.com/owner/repo/issues/42" }
}
```

> `feature` is `FeatureRef` serialized verbatim (`task_sink.rs:33`):
> `provider: &'static str`, `id: String`, `url: Option<String>`. For
> `provider:"board"`/`"linear"`, `url` may be null — render conditionally.
> Backward-compat: the `goal` `BoardItem` row is still created first and still
> returned, so any consumer keyed on the goal row keeps working; only the
> `planner_session_id` field is removed (no live UI consumer reads it — the
> current branch's Chat never called this route).

**Response — error (AC-3, loud + specific)** — use `ApiError::Custom` with a
typed envelope so the UI can show the exact reason, not a generic 500:

```jsonc
// HTTP 422 (creation failed) — body shape:
{ "error": {
    "code": "no_gh" | "not_github_repo" | "gh_failed" | "empty_title"
          | "remote_unsupported" | "linear_failed",
    "message": "Connect GitHub / not a GitHub repo",  // human, actionable
    "provider": "github" | "linear" | "board" | null
} }
```

- Map `create_feature`'s `anyhow::Error` → this envelope by inspecting the
  cause (the sink already emits distinct messages: `"failed to run \`gh\`"` →
  `no_gh`; `"gh issue create failed: ..."` → `gh_failed`; `select` returning
  `Board` when the user expected GitHub is **not** an error — it is the
  documented fallback, surfaced as a success with `provider:"board"` + a UI
  note, satisfying AC-4 "also surfaced, not silent").
- Empty/whitespace title → `422 {code:"empty_title"}` **before** any sink call.
- This nests under `error` (an object) rather than the default `{"error": msg}`
  (a string). Choose **one** shape and keep the default-`{"error": string}`
  variant for the empty-title 400 if you prefer minimal surface; the nested
  object is recommended because AC-3 needs `code` for UI branching. Document the
  chosen shape in the handler doc-comment.

### Remote exec (S3 / AC-6)

- Resolve `host = state.store.get_host(host_id.unwrap_or(LOCAL_HOST_ID))`.
- `HostKind::Local` → `TaskSink::Github.create_feature` as-is (it already runs
  `gh` in `ctx.workdir` via `tokio::process::Command`, `task_sink.rs:145`).
- `HostKind::Ssh{..}` → run `gh issue create …` through the new `gh_in_dir`
  helper (mirrors `git_in_dir`), then feed stdout to the existing
  `parse_gh_issue_url` (`task_sink.rs:272`). If S3 is deferred, return
  `422 {code:"remote_unsupported", message:"remote repos not yet supported"}`
  — **never** a silent local `gh` against a path that doesn't exist locally.

---

## Data Flow

```
Chat composer (ChatPage.tsx submit)
  → board-client.ts createGoal({title, body, workdir, host_id})
  → POST /api/board/goals
  → create_goal (board_goals.rs):
       1. enforce_transition (todo column rule)                    [unchanged]
       2. create_board_item (lbl=goal, todo) → goal row + goal.created event
       3. host = get_host(host_id ?? LOCAL_HOST_ID)                [S3]
       4. wd = expand_workdir(workdir ?? cwd); verify exists
       5. sink = TaskSink::select(&wd)                             [AC-4]
       6. fref = sink.create_feature(SinkCtx{store,&wd,parent:None},
                                     NewFeature{title, body})       [S1, synchronous]
            ├─ Github → gh issue create (local) or gh_in_dir (ssh)  [S3]
            ├─ Linear → linear::create_issue
            └─ Board  → create feat card (fallback)
       7. Ok  → 201 { goal, feature: fref }
          Err → 422 { error:{ code, message, provider } }          [AC-3]
  → UI renders issue link (fref.url) in thread, or the specific error
  → Board (Tasks/GitHub view) shows the issue on next board_sync fetch  [AC-2]
```

No agent is spawned; no tmux pane; the HTTP request blocks only for the
single `gh`/Linear/store call and returns a terminal result.

---

## Important Decisions

- **Synchronous server-side `TaskSink` create over keeping the autonomous
  planner agent — because determinism + loud errors beat richness.** The agent
  has no completion guarantee and gives the UI nothing to render on failure
  (root cause of the "planning…" hang, spec §1). A direct `create_feature` call
  returns a terminal `FeatureRef`-or-error in one round trip. The planner stays
  in-tree (dormant) so the decomposition vision (S4) can return later.
- **1:1 (one issue per submit, title = description) over LLM decomposition —
  because a working 1:1 beats a broken backlog (R1).** `create_feature` already
  takes exactly one `NewFeature`; no new code is needed to ship 1:1.
  Decomposition is a separate spec (S4, explicitly out).
- **Reuse `plan_goal_harness`'s proven call shape over a new abstraction —
  because the seam is already unit-tested and the only gap is *where* it's
  called.** `plan_goal_harness` (`board_goals.rs:166`) already wires
  `TaskSink::select`+`create_feature`+`SinkCtx`; copy that into `create_goal`.
- **`ApiError::Custom` envelope over a new error type — because the codebase
  already standardizes structured error bodies through `Custom`** (`error.rs:23`
  doc-comment; precedent: the `{"missing":[...],"status":"doing"}` gate). No new
  enum variant per error.
- **Add `host_id` to the request over inferring host from the path string —
  because path→host has no reliable mapping** (the documented `connectionId →
  host_id` crux). Mirror `sessions.rs::create`'s explicit `host_id` field.

---

## Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | 1:1 is less rich than the backlog vision. | **Accepted** — a working 1:1 ships value now; decomposition is the S4 follow-up spec. Planner code retained dormant so the path back is cheap. |
| R2 | Remote `gh` exec adds host-runtime coupling. | **Isolate behind S3.** S1/S2 are local-only and shippable without it; S3 is one additive `gh_in_dir` helper + an `host_id` resolution, both mirroring existing `git_in_dir`/`sessions::create` patterns. If S3 slips, return `remote_unsupported` (still satisfies AC-6's "clear error" clause). |
| R3 | `gh` rate-limits / auth drift. | Surfaced as AC-3 `422 {code:"gh_failed", message:<gh stderr>}`, never a hang — `create_feature` already bails non-zero with stderr (`task_sink.rs:151`). |
| R4 | **Blocking the axum request thread on the `gh` subprocess.** | **Accepted/low** — `create_feature` uses `tokio::process::Command` (async, `.output().await`), so it yields the worker, not blocks it. SSH path runs under `ssh_output`'s bounded `GIT_TIMEOUT` (120s). One slow create can't wedge the runtime. Do **not** convert to blocking `std::process`. |
| R5 | **Removing `planner_session_id` breaks a consumer.** | **Low** — on this branch no UI reads it (Chat goes through the harness); grep confirmed zero `planner_session_id`/`plannerSessionId` references in `ui/`. The `goal` row stays in the response for any other consumer. |
| R6 | `TaskSink::select` silently falls back to `Board` when the user expected GitHub (e.g. `gh` logged out → `github_ready` false). | Surface it: when `select` returns `Board` but a GitHub binding/intent was expected, the response still carries `provider:"board"`; the UI labels it ("created on the internal board — connect GitHub for issues"). Satisfies AC-4 "fallbacks are surfaced, not silent." Hermetic tests pin `AGENTUM_TASK_SINK` so this is deterministic. |

---

## Slice mapping (spec §7 → code)

- **S1 — deterministic create (local, 1:1)** *(AC-1/3/4/5, critical path)*:
  edit `create_goal` (`board_goals.rs:51`) — replace `spawn_planner_session`
  with `TaskSink::select(&wd).create_feature(...)`; return `FeatureRef` or the
  `Custom` error envelope. Add handler tests (force `AGENTUM_TASK_SINK=board`
  for hermeticity). **No new files.**
- **S2 — Chat UI feedback** *(AC-2)*: add `createGoal` to `board-client.ts`;
  re-point `ChatPage.tsx::submit` at it; render `feature.url` link / structured
  error; remove the indefinite "planning…"/harness path for Chat-create.
- **S3 — remote repos** *(AC-6)*: add `host_id` to `CreateGoalBody`; resolve
  `Host` via `state.store.get_host`; add `gh_in_dir` to `host_runtime.rs`
  (mirror `git_in_dir`); dispatch GitHub create local-vs-ssh. Isolated — does
  not touch S1's local path.
- **S4 — LLM decomposition**: **out of scope** (separate spec). Do not build it.

## Honored patterns

- Route handler shape, `SinkCtx`/`create_feature` call, and `#[cfg(test)]`
  harness mirror **`plan_goal_harness`** (`board_goals.rs:166`) — cite it as the
  template.
- Host resolution mirrors **`sessions::create`** (`sessions.rs:142`) and
  **`repos::load_host_for_repo`** (`repos.rs:370`).
- Remote command exec mirrors **`host_runtime::git_in_dir`** (`:1512`).
- UI client mirrors the sibling functions in **`board-client.ts`**.
- Structured error body mirrors the **`ApiError::Custom`** precedent
  (`error.rs:23`).
