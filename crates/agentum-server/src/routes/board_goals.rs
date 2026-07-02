//! `/api/board/goals` — atomic create-goal + deterministic issue create.
//! The goal IS a `BoardItem` with `lbl="goal"` (CONTEXT D-01); no
//! parallel table.
//!
//! Spec 018: Chat submit no longer spawns an autonomous planner agent (which
//! had no completion guarantee and gave the UI nothing to render on failure —
//! the "planning…" hang). Instead `create_goal` makes a **synchronous,
//! server-side** `TaskSink::create_feature` call and returns the created
//! [`crate::task_sink::FeatureRef`] (the real GitHub issue / Linear ticket /
//! board card) — or a typed, loud error — as the HTTP response. The planner
//! (`spawn_planner_session`, `planner.rs`) stays in the tree but is OUT of the
//! issue-creation critical path (dormant), so the decomposition vision can
//! return later (spec 018 §5 Non-goals).

use agentum_core::{BoardItem, Event, Host, NewBoardItem, NewSession, Status, TransitionCtx};
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::planner;
use crate::task_sink::{FeatureRef, NewFeature, SinkCtx, TaskSink};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/board/goals", post(create_goal))
        .route(
            "/api/board/goals/{id}/harness-plan",
            post(plan_goal_harness),
        )
}

#[derive(Deserialize)]
struct CreateGoalBody {
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    workdir: Option<String>,
    /// Which agent the goal's child cards inherit when later started from the
    /// board (`spawn_card_session` resolves tool/model via the parent goal) —
    /// e.g. "claude" | "codex" | "gemini". No longer drives a planner (spec
    /// 018), but still rides on the goal row for the board-start path.
    #[serde(default)]
    tool: Option<String>,
    /// Optional model hint inherited the same way.
    #[serde(default)]
    model: Option<String>,
    /// SSH host the `workdir` lives on (spec 018 S3 / AC-6). Absent or the
    /// nil-UUID (`LOCAL_HOST_ID`) means the repo is local. Mirrors
    /// `sessions::create`'s explicit `host_id` — path→host has no reliable
    /// mapping, so the client states it.
    #[serde(default)]
    host_id: Option<Uuid>,
    /// Optional `owner/repo` hint for the GitHub issue target (spec 019). When
    /// present and well-formed it short-circuits the host-aware `origin` read
    /// (the UI fills it from its slug index when known). Absent/malformed → the
    /// server resolves the slug authoritatively from the project's `origin`.
    #[serde(default)]
    repo_slug: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateGoalResponse {
    goal: BoardItem,
    /// The created tracker item (GitHub issue / Linear ticket / board card).
    /// `FeatureRef` serialized verbatim: `{ provider, id, url }`. For
    /// `provider:"board"`/`"linear"` the `url` may be null — render
    /// conditionally. Replaces the old `planner_session_id` (no live consumer).
    feature: FeatureRef,
}

/// `POST /api/board/goals` — create a goal card and **deterministically** create
/// one tracker item (GitHub issue / Linear ticket / board card) for it, returning
/// the [`FeatureRef`] (spec 018 S1). No agent is spawned.
///
/// **Error contract (AC-3, loud + specific):** failures return
/// `422 { "error": { code, message, provider } }` via [`ApiError::Custom`]
/// (chosen over the default `{"error": string}` so the UI can branch on `code`).
/// `code` ∈ `empty_title` | `no_gh` | `not_github_repo` | `gh_failed`
/// | `linear_failed` | `no_tracker`. **Spec 019:** the Chat path targets GitHub
/// or Linear ONLY — there is no internal-Board fallback. When neither a GitHub
/// repo nor Linear resolves, the response is `no_tracker` (a loud error, never a
/// `provider:"board"` success) — this reverses the spec-018 Board-as-Ok contract.
async fn create_goal(
    State(state): State<AppState>,
    Json(body): Json<CreateGoalBody>,
) -> Result<(StatusCode, Json<CreateGoalResponse>), ApiError> {
    // AC-3: reject an empty/whitespace title up front with the typed envelope,
    // BEFORE the column gate or any sink call — a blank description can never
    // become a meaningful issue title, and the error must be specific.
    let title = body.title.trim();
    if title.is_empty() {
        return Err(create_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "empty_title",
            "Describe the feature — the title can't be empty.",
            None,
        ));
    }

    // Step 1: enforce column rules for `todo` exactly like routes/board::create.
    // Goals land in todo by definition (CONTEXT D-02) so the target is hardcoded.
    let target_status = "todo";
    let mut ctx = TransitionCtx {
        title: Some(title),
        lbl: Some("goal"),
        workdir: body.workdir.as_deref(),
        tool: None,
        claimed_by: None,
        session_id: None,
        has_comment: false,
    };
    super::board::enforce_transition(&state.store, &state.bus, None, target_status, &mut ctx)
        .await?;

    // The chosen agent still rides on the goal row so child cards inherit it
    // when started from the board (spawn_card_session resolves tool/model via
    // parent_goal). It no longer drives a planner — spec 018.
    let tool = body
        .tool
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // Step 2: resolve the host (S3 / AC-6) and the workdir BEFORE writing the
    // goal row, so a bad host_id or a missing remote-workdir is a loud error
    // that never orphans a goal. Body workdir wins; else the daemon cwd.
    let host_id = body.host_id.unwrap_or(agentum_core::LOCAL_HOST_ID);
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("unknown host: {host_id}")))?;
    let workdir = body.workdir.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "/".to_string())
    });

    // Step 3: create the goal BoardItem (lbl=goal, status=todo) — the Chat-side
    // tracking record, returned regardless of which sink backs the feature.
    let new_item = NewBoardItem {
        title: title.to_string(),
        body: body.body.clone(),
        lbl: Some("goal".into()),
        status: Some(target_status.into()),
        workdir: body.workdir.clone(),
        parent_goal_id: None,
        tool: tool.map(str::to_string),
        model: body.model.clone(),
        session_id: None,
        priority: None,
    };
    let goal = state.store.create_board_item(new_item).await?;

    // `board.created` was emitted inside create_board_item → emit the
    // goal-specific event so plan 01-04's watchdog can filter cleanly.
    let _ = state.bus.send(
        Event::new("goal.created")
            .with_payload(json!({"id": goal.id, "key": goal.key, "title": goal.title})),
    );

    // Step 4: create the feature SYNCHRONOUSLY in the configured task sink and
    // return the FeatureRef (or a typed error). This replaces the autonomous
    // planner: a direct call returns a terminal result in one round trip. The
    // call shape mirrors `plan_goal_harness` (the proven, unit-tested seam).
    let feature = create_feature_for_goal(
        &state,
        &host,
        &workdir,
        body.repo_slug.as_deref(),
        &NewFeature {
            title: title.to_string(),
            body: body.body.clone(),
            labels: vec![],
        },
    )
    .await;

    match feature {
        Ok(fref) => {
            let _ = state
                .bus
                .send(Event::new("goal.feature.created").with_payload(json!({
                    "goal_id": goal.id,
                    "provider": fref.provider,
                    "id": fref.id,
                    "url": fref.url,
                })));
            Ok((
                StatusCode::CREATED,
                Json(CreateGoalResponse {
                    goal,
                    feature: fref,
                }),
            ))
        }
        Err(e) => {
            // The goal row stays (the Chat-side record); the error is loud and
            // specific so the UI never sits at an indefinite "planning…".
            tracing::warn!(error = %e, goal_id = goal.id, "feature create failed; goal retained");
            let _ = state
                .bus
                .send(Event::new("goal.feature.failed").with_payload(json!({
                    "goal_id": goal.id,
                    "error": e.to_string(),
                })));
            Err(e)
        }
    }
}

/// Why a GitHub slug could NOT be resolved — threaded so the `no_tracker`
/// message can distinguish "couldn't reach the project's host to read its
/// remote" (an SSH read errored) from "no GitHub remote" (the read succeeded but
/// the origin isn't GitHub / there's no origin). See architecture.md Risk #2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlugReason {
    /// The git read (`remote get-url origin`) succeeded but the origin is not a
    /// GitHub remote (GitLab/unknown host), or there is no origin at all.
    NoGithubRemote,
    /// The host-aware git read could not run — the SSH host was unreachable or
    /// the remote command transport failed. The slug is unknown, not absent.
    HostUnreachable,
}

/// Resolve the `owner/repo` slug for a project's GitHub remote, host-aware
/// (spec 019). Precedence:
///   1. a well-formed client `hint` (`owner/repo`) short-circuits with no IO;
///   2. else read `git remote get-url origin` via `host_runtime::git_in_dir`
///      (local for a local host, one bounded SSH hop for a remote host),
///      parse it, and keep it only when it classifies as GitHub.
///
/// Returns `Ok(slug)` on success, or `Err(SlugReason)` explaining the miss so
/// the caller can craft an actionable `no_tracker` message. A malformed hint is
/// ignored (falls through to the authoritative read), never an error — the read
/// is the source of truth; the hint is only a no-IO fast path.
///
/// `pub(crate)` so the Chat-issues route (`routes::chat`) can reuse the exact
/// same host-aware origin read when no client `repo_slug` is supplied.
pub(crate) async fn resolve_github_slug(
    host: &Host,
    workdir: &str,
    hint: Option<&str>,
) -> Result<String, SlugReason> {
    // 1. Client fast-path: a well-formed `owner/repo` hint is trusted to skip the
    //    read (gh itself is the final arbiter — a bogus slug yields gh_failed).
    if let Some(h) = hint {
        let h = h.trim();
        if is_valid_slug(h) {
            return Ok(h.to_string());
        }
        // A malformed hint is NOT an error — fall through to the read below.
    }

    // 2. Authoritative read: ask git (on the project's host) for origin.
    let out = crate::host_runtime::git_in_dir(host, workdir, &["remote", "get-url", "origin"])
        .await
        // A transport/timeout error (e.g. SSH host down) — slug unknown.
        .map_err(|_| SlugReason::HostUnreachable)?;
    if !out.success {
        // git ran but exited non-zero: not a repo / no `origin` remote.
        return Err(SlugReason::NoGithubRemote);
    }
    let url = out.stdout_string();
    let url = url.trim();
    let (h, project) = super::forge::parse_remote_url(url).ok_or(SlugReason::NoGithubRemote)?;
    match super::forge::classify_remote(&h, project) {
        Some(remote) if remote.is_github() => Ok(remote.project),
        // Parsed, but GitLab/unknown host — not a GitHub target.
        _ => Err(SlugReason::NoGithubRemote),
    }
}

/// A client `repo_slug` hint must look like `owner/repo`: exactly one `/`, no
/// whitespace, both halves non-empty. Kept strict so a malformed hint can never
/// reach `gh` as an argv token; a failing hint falls through to the origin read.
fn is_valid_slug(s: &str) -> bool {
    let mut parts = s.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(repo), None) => {
            !owner.is_empty()
                && !repo.is_empty()
                && !owner.chars().any(char::is_whitespace)
                && !repo.chars().any(char::is_whitespace)
        }
        _ => false,
    }
}

/// Build the typed AC-3 error envelope: `{ "error": { code, message, provider } }`.
/// Nested under `error` (an object) — distinct from the default
/// `{"error": string}` envelope — so the UI can branch on `code`.
fn create_error(status: StatusCode, code: &str, message: &str, provider: Option<&str>) -> ApiError {
    ApiError::Custom(
        status,
        json!({ "error": { "code": code, "message": message, "provider": provider } }),
    )
}

/// Create one feature for a goal, decoupled from the local filesystem (spec 019).
///
/// **No `wd.exists()` gate** — filing an issue is a GitHub/Linear API action, not
/// a filesystem action; a remote/SSH project's path never exists locally and must
/// not block creation (the bug this spec fixes). Precedence:
///
///   1. resolve a GitHub `owner/repo` slug (client hint → host-aware `origin`
///      read); if found, file via `gh issue create --repo <slug>` from `$HOME`;
///   2. else, if Linear is configured, create the ticket there (path-free);
///   3. else, fail loudly with a typed `422 no_tracker` — **never** the internal
///      Board (AC-4: Chat targets GitHub + Linear only).
///
/// The `host` is used only to *read the slug* (local for a local host, one SSH
/// hop for a remote one). Filing always runs the **local** `gh` — Chat pins
/// `host_id=null` so `host.kind` here is `Local`, and even a non-local host never
/// routes the `gh` create over SSH (AC-6: "never SSH from Chat" holds
/// structurally — only the read-only slug lookup may touch SSH).
async fn create_feature_for_goal(
    state: &AppState,
    host: &Host,
    workdir: &str,
    slug_hint: Option<&str>,
    feature: &NewFeature,
) -> Result<FeatureRef, ApiError> {
    // Step 1: resolve a GitHub slug (client hint, else host-aware origin read).
    match resolve_github_slug(host, workdir, slug_hint).await {
        Ok(slug) => {
            // A GitHub target resolved → file via the explicit-`--repo` path, run
            // from $HOME (never the project workdir). Reuses the shared GitHub
            // arm in `TaskSink` so YOLO/argv/parse stay in one place.
            return TaskSink::Github
                .create_feature(
                    &SinkCtx {
                        store: &state.store,
                        // workdir is unused by the explicit-slug GitHub arm (it
                        // runs from $HOME); pass it for shape only.
                        workdir: std::path::Path::new(workdir),
                        parent_goal_id: None,
                        slug: Some(&slug),
                    },
                    feature,
                )
                .await
                .map_err(|e| map_sink_error(TaskSink::Github, &e));
        }
        // Step 2/3: no GitHub target — fall through to Linear, else `no_tracker`.
        Err(reason) => {
            if crate::linear::available() {
                return TaskSink::Linear
                    .create_feature(
                        &SinkCtx {
                            store: &state.store,
                            workdir: std::path::Path::new(workdir),
                            parent_goal_id: None,
                            slug: None,
                        },
                        feature,
                    )
                    .await
                    .map_err(|e| map_sink_error(TaskSink::Linear, &e));
            }
            // No GitHub repo AND no Linear → loud, typed error. NEVER the Board.
            let message = match reason {
                SlugReason::HostUnreachable => {
                    "Couldn't reach the project's host to read its GitHub remote, and no Linear workspace is connected. Check the host, or connect GitHub/Linear."
                }
                SlugReason::NoGithubRemote => {
                    "This project has no GitHub remote and no Linear workspace is connected. Add a GitHub `origin` remote, or connect Linear in Settings."
                }
            };
            Err(create_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "no_tracker",
                message,
                None,
            ))
        }
    }
}

/// Run `gh issue create` on an SSH host's `workdir` and parse the issue URL —
/// the remote analogue of `TaskSink::Github` (S3 / AC-6). Goes through
/// `host_runtime::gh_in_dir` (mirrors `git_in_dir`), so quoting + the bounded
/// SSH timeout are shared with every other host-aware exec.
///
/// Spec 019: **no longer reached from the Chat path** — Chat now files locally
/// via `gh issue create --repo <slug>` (it reads the slug host-aware but never
/// SSHes to *file*). Kept in-tree (with tests) as the documented remote-file
/// path for any non-Chat caller; `#[allow(dead_code)]` because nothing invokes
/// it today.
#[allow(dead_code)]
async fn create_github_issue_remote(
    host: &Host,
    workdir: &str,
    feature: &NewFeature,
) -> Result<FeatureRef, ApiError> {
    let body = feature.body.clone().unwrap_or_default();
    let args = [
        "issue",
        "create",
        "--title",
        feature.title.as_str(),
        "--body",
        body.as_str(),
    ];
    let out = crate::host_runtime::gh_in_dir(host, workdir, &args)
        .await
        .map_err(|e| {
            create_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "remote_unsupported",
                &format!("could not run `gh` on the remote host: {e}"),
                Some("github"),
            )
        })?;
    if !out.success {
        let stderr = out.stderr.trim();
        let code = classify_gh_stderr(stderr);
        return Err(create_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            code,
            if stderr.is_empty() {
                "remote `gh issue create` failed"
            } else {
                stderr
            },
            Some("github"),
        ));
    }
    crate::task_sink::parse_gh_issue_url(&out.stdout_string()).map_err(|e| {
        create_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "gh_failed",
            &format!("could not parse the created issue URL: {e}"),
            Some("github"),
        )
    })
}

/// Map a sink `create_feature` error onto the typed AC-3 envelope by inspecting
/// the message the sink emits (`task_sink.rs` raises distinct strings). The
/// provider is known from `sink`, so the UI can show "Connect GitHub" vs.
/// "Linear failed". `pub(crate)` so the composer's create-issue route
/// (`routes::github`) reuses the exact same classification (spec 004 F3).
pub(crate) fn map_sink_error(sink: TaskSink, err: &anyhow::Error) -> ApiError {
    let msg = err.to_string();
    let (code, message): (&str, String) = match sink {
        TaskSink::Github => {
            // `task_sink.rs`: "failed to run `gh`: …" (binary missing) vs.
            // "gh issue create failed: <stderr>" (non-zero). Classify the
            // stderr so "no default remote repository" reads as not_github_repo.
            if msg.contains("failed to run `gh`") {
                (
                    "no_gh",
                    "GitHub CLI (`gh`) is not installed or not on PATH.".to_string(),
                )
            } else {
                (classify_gh_stderr(&msg), msg.clone())
            }
        }
        TaskSink::Linear => ("linear_failed", msg.clone()),
        // Board create only fails on a real store error → surface as a generic
        // gh_failed-shaped envelope is wrong; report it plainly.
        TaskSink::Board => ("board_failed", msg.clone()),
    };
    create_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        code,
        &message,
        Some(sink.provider()),
    )
}

/// Pick a finer error code from `gh`'s stderr: a repo that `gh` can't resolve
/// (no remote, not a git repo) is `not_github_repo`; everything else (auth,
/// rate-limit, validation) is `gh_failed`. Heuristic on `gh`'s own wording —
/// kept lenient so a wording change degrades to `gh_failed`, not a panic.
fn classify_gh_stderr(stderr: &str) -> &'static str {
    let s = stderr.to_lowercase();
    if s.contains("no default remote repository")
        || s.contains("not a git repository")
        || s.contains("none of the git remotes")
        || s.contains("could not determine")
    {
        "not_github_repo"
    } else {
        "gh_failed"
    }
}

#[derive(Debug, Serialize)]
struct PlanGoalHarnessResponse {
    /// Which task manager backed the features ("board" | "github" | …).
    provider: &'static str,
    workdir: String,
    feature_count: usize,
    features: crate::harness::FeatureList,
}

/// `POST /api/board/goals/{id}/harness-plan` — take the planner-produced child
/// cards of a goal and write them into the harness backlog (spec 011a).
///
/// This is the "auto-generate the backlog, human-gated Run" step: it writes
/// `.agentum-harness/feature_list.json` (every feature `Pending`) but never
/// registers or runs the harness — the user reviews the board and clicks Run.
/// The board is the source of truth here (011a fallback); external sinks
/// (GitHub/Linear) layer in via [`crate::task_sink::TaskSink`] in 011b/011c.
async fn plan_goal_harness(
    State(state): State<AppState>,
    Path(goal_id): Path<i64>,
) -> Result<Json<PlanGoalHarnessResponse>, ApiError> {
    // The goal must exist and actually be a goal — guard against planning the
    // harness off a random feature card.
    let goal = state
        .store
        .get_board_item(goal_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no board item {goal_id}")))?;
    if goal.lbl.as_deref() != Some("goal") {
        return Err(ApiError::BadRequest(format!(
            "board item {goal_id} is not a goal"
        )));
    }

    // The goal's workdir is where `.agentum-harness/` lives.
    let workdir = goal.workdir.clone().ok_or_else(|| {
        ApiError::BadRequest("goal has no workdir; cannot locate .agentum-harness".into())
    })?;
    let wd = super::util::expand_workdir(&workdir)?;
    if !wd.exists() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            wd.display()
        )));
    }

    // Collect the planner's child cards in board order (priority, then
    // created_at — the order the user sees top-to-bottom). The board key is the
    // stable tracker id; it becomes the harness feature id (and
    // `$HARNESS_FEATURE_ID` in verify.sh).
    let children: Vec<BoardItem> = state
        .store
        .list_board_items()
        .await?
        .into_iter()
        .filter(|c| c.parent_goal_id == Some(goal_id))
        .collect();
    if children.is_empty() {
        return Err(ApiError::BadRequest(
            "goal has no feature cards yet; let the planner decompose it first".into(),
        ));
    }

    // Pick the destination from what's configured (external manager = source of
    // truth; the board is the agnostic fallback). For the board the planner's
    // cards ARE the source, so we reuse their keys; for an external sink we
    // mirror each card out and use the tracker's id as the harness feature id.
    // (Re-running against an external sink re-creates issues — idempotent sync
    // is deferred to 011d.)
    let sink = crate::task_sink::TaskSink::select(&wd).await;
    let mut feats: Vec<crate::harness::BacklogFeature> = Vec::with_capacity(children.len());
    for c in &children {
        let body = c.body.clone().unwrap_or_default();
        // For the board the planner's cards ARE the tracker, so reuse their key
        // and provider; an external sink mirrors the card out and we carry its
        // provider + url so the harness can drive ticket-state transitions later.
        let (id, provider, url) = match sink {
            crate::task_sink::TaskSink::Board => (c.key.clone(), Some("board".to_string()), None),
            other => {
                let fref = other
                    .create_feature(
                        &crate::task_sink::SinkCtx {
                            store: &state.store,
                            // deref Arc<Store> → &Store
                            workdir: &wd,
                            parent_goal_id: Some(goal_id),
                            // Harness path: keep cwd-relative GitHub resolution
                            // (spec 019 scopes the explicit-`--repo` slug to Chat).
                            slug: None,
                        },
                        &crate::task_sink::NewFeature {
                            title: c.title.clone(),
                            body: c.body.clone(),
                            labels: vec![],
                        },
                    )
                    .await
                    .map_err(|e| {
                        ApiError::Internal(format!("create feature in {}: {e}", other.provider()))
                    })?;
                (fref.id, Some(fref.provider.to_string()), fref.url)
            }
        };
        // A freshly created ticket starts in the tracker's "Todo" state so the
        // board mirrors the harness's Pending backlog from the very first moment
        // (best-effort: a tracker hiccup must not fail planning).
        if let Some(p) = provider.as_deref() {
            match crate::task_sink::apply_tracker_transition(
                &state.store,
                p,
                &id,
                url.as_deref(),
                crate::task_sink::TrackerPhase::Todo,
            )
            .await
            {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(provider = p, id = %id, error = %e, "initial Todo transition failed (non-fatal)")
                }
            }
        }
        feats.push(crate::harness::BacklogFeature {
            id,
            name: c.title.clone(),
            description: body,
            provider,
            url,
        });
    }

    let list = crate::harness::write_backlog_from_features(&wd, &feats)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let _ = state
        .bus
        .send(Event::new("goal.harness.planned").with_payload(json!({
            "goal_id": goal_id,
            "workdir": wd.to_string_lossy(),
            "provider": sink.provider(),
            "feature_count": list.features.len(),
        })));

    Ok(Json(PlanGoalHarnessResponse {
        provider: sink.provider(),
        workdir: wd.to_string_lossy().into_owned(),
        feature_count: list.features.len(),
        features: list,
    }))
}

/// Spawn a tool session bound to the given card and atomically dual-write
/// the binding via `Store::claim_card`. Mirrors `spawn_planner_session`
/// but takes a card directly instead of a goal + planner config.
///
/// CONTEXT D-01: PATCH→doing auto-spawn fires this from board.rs::patch
/// after `enforce_transition` passes.
/// Provision a per-card git worktree so a card-start agent runs in an
/// isolated checkout instead of mutating the project's main working tree
/// (design 2026-06-18: "the agent creates the worktree of that project").
///
/// Reuses the same `crate::git::create_worktree` primitive the session
/// `worktree` spec consumes — we do NOT fork worktree creation. Returns:
///   * `Ok(Some(resolved))` — a fresh worktree on branch `agentum/card-<key>`.
///   * `Ok(None)` — the project dir is not a git repo, so isolation is
///     impossible; the caller falls back to spawning directly in `repo`
///     (preserves card-start for non-git workdirs rather than 400-ing them).
///   * `Err(_)` — git was a repo but the worktree could not be created
///     (e.g. the per-card branch already exists from a prior un-pruned run).
async fn provision_card_worktree(
    repo: &std::path::Path,
    session_name: &str,
) -> Result<Option<crate::git::ResolvedWorktree>, ApiError> {
    if !crate::git::is_git_repo(repo).await {
        return Ok(None);
    }
    match crate::git::create_worktree(repo, session_name, None, None).await {
        Ok(resolved) => Ok(Some(resolved)),
        Err(e) => Err(ApiError::BadRequest(format!("card worktree: {e}"))),
    }
}

/// Best-effort cleanup of a card's isolated worktree once the card reaches
/// `done` (design 2026-06-18 "Open questions": per-card worktree, prune on
/// done). Safe by construction — it only removes the worktree when:
///
/// - the card has a bound session with a `worktree_path` (it was isolated),
/// - that session is no longer `Running` (don't yank a worktree out from under
///   a live agent), and
/// - the worktree is clean (never silently discard uncommitted work — the same
///   guard the manual `/worktree/prune` route enforces).
///
/// Any unmet condition (or a git error) is a logged skip, never a caller error:
/// marking a card done must not fail because cleanup couldn't run. Returns
/// `true` only when a worktree was actually pruned.
pub(crate) async fn prune_card_worktree_on_done(state: &AppState, card: &BoardItem) -> bool {
    let session_id = match card.session_id.as_deref().and_then(|s| s.parse().ok()) {
        Some(id) => id,
        None => return false,
    };
    let session = match state.store.get_session_by_id(session_id).await {
        Ok(Some(s)) => s,
        _ => return false,
    };
    let Some(wt_path) = session.worktree_path.as_deref() else {
        return false; // non-isolated card (e.g. non-git project) — nothing to prune
    };
    if matches!(session.status, Status::Running) {
        return false; // a live agent still owns the checkout
    }
    let wt = std::path::Path::new(wt_path);
    if let Ok(status) = crate::git::worktree_status(wt).await {
        if !status.is_clean() {
            tracing::info!(card = %card.key, "skip prune-on-done: worktree has uncommitted changes");
            return false;
        }
    }
    // `git worktree remove` runs from the project root, not the worktree.
    let repo = std::path::Path::new(&session.workdir);
    match crate::git::prune_worktree(repo, wt, session.worktree_branch.as_deref(), false).await {
        Ok(()) => {
            let _ = state.store.clear_session_worktree(session.id).await;
            true
        }
        Err(e) => {
            tracing::warn!(card = %card.key, "prune-on-done failed (non-fatal): {e}");
            false
        }
    }
}

/// CONTEXT D-02: tool defaults to "claude"; workdir falls through to
/// parent_goal.workdir, then HTTP 400.
/// CONTEXT D-03 (superseded): the ticket title+body is sent as the first
/// prompt so the agent starts with context instead of a blank pane. The
/// send is fire-and-forget on a tokio task after a short delay so the
/// agent's splash/trust dialog is past before keystrokes arrive.
/// CLAUDE.md YOLO rule: push the canonical YOLO marker into flags; let
/// translate_yolo_marker in the adapter substitute per-tool.
/// Plan-checker iter-1 W-3: does NOT emit `session.started` — the
/// watchdog's per-session loop already emits that event when status
/// flips to Running (watchdog/src/lib.rs:147).
pub(crate) async fn spawn_card_session(
    state: &AppState,
    card: &BoardItem,
) -> Result<String, ApiError> {
    // 1. Resolve tool: card.tool → parent_goal.tool → "claude" (CONTEXT D-02).
    let tool = match card.tool.as_deref() {
        Some(t) => t.to_string(),
        None => {
            if let Some(pg_id) = card.parent_goal_id {
                state
                    .store
                    .get_board_item(pg_id)
                    .await?
                    .and_then(|pg| pg.tool)
                    .unwrap_or_else(|| "claude".to_string())
            } else {
                "claude".to_string()
            }
        }
    };

    // 2. Resolve workdir: card.workdir → parent_goal.workdir → 400 (CONTEXT D-02).
    let workdir = match card.workdir.as_deref() {
        Some(w) => w.to_string(),
        None => {
            let from_parent = if let Some(pg_id) = card.parent_goal_id {
                state
                    .store
                    .get_board_item(pg_id)
                    .await?
                    .and_then(|pg| pg.workdir)
            } else {
                None
            };
            from_parent.ok_or_else(|| {
                ApiError::Custom(
                    axum::http::StatusCode::BAD_REQUEST,
                    serde_json::json!({"missing": ["workdir"], "status": "doing"}),
                )
            })?
        }
    };

    // 3. Verify workdir exists on disk (mirrors spawn_planner_session :155-160).
    let wd = super::util::expand_workdir(&workdir)?;
    if !wd.exists() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            wd.display()
        )));
    }
    let workdir = wd.to_string_lossy().into_owned();
    let session_name = format!("card-{}", card.key.to_lowercase());

    // 4. Provision a per-card worktree so the agent runs in an isolated
    //    checkout (design 2026-06-18 "Open questions": per-card, prune on
    //    done). Falls through to a direct workdir spawn for non-git projects.
    let worktree = provision_card_worktree(&wd, &session_name).await?;

    // 5. Build NewSession with the canonical YOLO marker pushed verbatim —
    //    adapters call translate_yolo_marker on launch (CLAUDE.md YOLO rule).
    //    agentum_executor::YOLO_MARKER = "--dangerously-skip-permissions".
    //    The worktree_* fields (when present) make session.effective_cwd()
    //    resolve to the isolated checkout instead of the project root.
    let new_session = NewSession {
        name: session_name,
        workdir: workdir.clone(),
        tool: tool.clone(),
        model: None,
        flags: vec![agentum_executor::YOLO_MARKER.to_string()],
        // card_id is overwritten unconditionally by claim_card — set it
        // here for clarity but claim_card will enforce it.
        card_id: Some(card.id),
        worktree_path: worktree
            .as_ref()
            .map(|w| w.path.to_string_lossy().into_owned()),
        worktree_branch: worktree.as_ref().map(|w| w.branch.clone()),
        worktree_base_ref: worktree.as_ref().map(|w| w.base_ref.clone()),
    };

    // 6. Atomic dual-write: INSERT session row + UPDATE card.session_id in one tx.
    //    claim_card returns AlreadyExists (→ HTTP 409) if the card is already bound.
    let (_card_after, session) = state
        .store
        .claim_card(card.id, new_session)
        .await
        .map_err(ApiError::from)?;

    // 7. Launch through the ONE shared spawn path (YOLO translation, loopback
    //    env, Claude --settings hook, MCP wiring, pipe-pane, status→Running) —
    //    never a parallel reimplementation. `effective_cwd()` returns the
    //    worktree when isolation was provisioned, else the project workdir.
    let host = state
        .store
        .get_host(agentum_core::LOCAL_HOST_ID)
        .await?
        .ok_or_else(|| ApiError::Internal("local host missing".into()))?;
    let target = agentum_tmux::target_for(&session.name);
    let spawn_wd = super::util::expand_workdir(session.effective_cwd())?;
    super::sessions::spawn_agent_into_pane(state, &session, &host, &target, &spawn_wd).await?;

    // 8. Send the ticket title+body as the first prompt so the agent starts with
    //    context. Fire-and-forget via the harness's `inject_prompt`: it waits for
    //    the REPL (accepting Claude's trust dialog, outlasting an MCP-slowed boot)
    //    then submits in two steps. A fixed-delay one-shot `send_keys(.., true)`
    //    raced the splash and was swallowed by bracketed-paste — the prompt never
    //    ran. The HTTP response doesn't block on this.
    if let Some(prompt) = build_card_prompt(card) {
        let state = state.clone();
        let session = session.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::harness::inject_prompt(&state, &session, &prompt).await {
                tracing::warn!(error = %e, "send card prompt failed; session still running");
            }
        });
    }

    Ok(session.id.to_string())
}

/// Compose the opening prompt sent to a freshly-spawned card session.
/// Returns `None` when the card has neither body nor a non-empty title
/// (defensive — title is required by the schema but we don't want to
/// send an empty Enter to the agent on edge cases).
fn build_card_prompt(card: &BoardItem) -> Option<String> {
    let title = card.title.trim();
    let body = card.body.as_deref().map(str::trim).unwrap_or("");
    match (title.is_empty(), body.is_empty()) {
        (true, true) => None,
        (false, true) => Some(format!("Working on {key}: {title}", key = card.key)),
        (true, false) => Some(body.to_string()),
        (false, false) => Some(format!(
            "Working on {key}: {title}\n\n{body}",
            key = card.key
        )),
    }
}

/// Spawn a planner agent session bound to the given goal.
///
/// Goes through the centralized `sessions::spawn_agent_into_pane` + the
/// harness's `inject_prompt` — the same one launch+prompt path as the card
/// spawn and the harness, so `pane_env`/MCP/trust-dialog handling stay in one
/// place. Do not reintroduce a hand-rolled tmux spawn here.
///
/// DORMANT (spec 018): removed from the Chat→issue critical path (it was the
/// non-deterministic "planning…" hang). Retained in-tree so the decomposition
/// vision (one description → many issues) can return as a follow-up spec without
/// re-deriving the launch wiring. `#[allow(dead_code)]` because nothing calls it
/// today — deleting it would lose that path; see spec 018 §5 Non-goals.
#[allow(dead_code)]
async fn spawn_planner_session(
    state: &AppState,
    goal: &BoardItem,
    cfg: &planner::PlannerConfig,
    workdir: &str,
) -> Result<String, ApiError> {
    // Name convention: `planner-<lowercase-goal-key>` e.g. `planner-ag-42`.
    let session_name = format!("planner-{}", goal.key.to_lowercase());

    // Expand `~`/`~/x` once so the stored session row and the tmux spawn
    // both see the same canonical absolute path.
    let wd = super::util::expand_workdir(workdir)?;
    if !wd.exists() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            wd.display()
        )));
    }
    let workdir_resolved = wd.to_string_lossy().into_owned();

    let new_session = NewSession {
        name: session_name.clone(),
        workdir: workdir_resolved,
        tool: cfg.tool.clone(),
        model: goal.model.clone(),
        // YOLO is mandatory for an autonomous planner: it must run `gh issue
        // create` (a bash tool call) without stopping at a permission prompt —
        // otherwise it hangs forever and never creates issues (the chat sits at
        // "Drafting…"). Mirrors spawn_card_session and the harness, which
        // CLAUDE.md calls non-negotiable. The adapter translates the marker
        // per-tool via translate_yolo_marker.
        flags: vec![agentum_executor::YOLO_MARKER.to_string()],
        // card_id binds this session to the goal; the watchdog (plan 01-04)
        // uses this FK to decide which goal to recompute on session events.
        card_id: Some(goal.id),
        worktree_path: None,
        worktree_branch: None,
        worktree_base_ref: None,
    };
    // The planner runs locally inside the repo: it reads the project on this
    // machine and shells out to `gh issue create`, which uses the repo's own
    // `gh` auth (no agentum server round-trip). Resolve the local host so the
    // launch still goes through the one centralized spawn path, like the harness.
    let host = state
        .store
        .get_host(agentum_core::LOCAL_HOST_ID)
        .await?
        .ok_or_else(|| ApiError::Internal("local host missing".into()))?;
    let session = state
        .store
        .create_session_on_host(new_session, Some(agentum_core::LOCAL_HOST_ID))
        .await?;

    // Launch through the ONE shared spawn path (YOLO translation, loopback
    // `pane_env`, Claude `--settings` hook, MCP wiring, pipe-pane, status→Running)
    // — the same helper the harness and spawn_card_session use.
    let target = agentum_tmux::target_for(&session.name);
    super::sessions::spawn_agent_into_pane(state, &session, &host, &target, &wd).await?;

    // Deliver the planner prompt robustly, in the background. `inject_prompt`
    // waits for the REPL (accepting Claude's "trust this folder?" dialog and
    // outlasting an MCP-slowed boot), then submits in two steps (type, pause,
    // bare Enter). A one-shot `send_keys(prompt, true)` is swallowed by the
    // REPL's bracketed-paste for a multi-line prompt — the text lands in the box
    // but never executes, which left the chat stuck at "Drafting cards…".
    // Fire-and-forget so the HTTP response returns the session id immediately;
    // the UI polls the board for the cards the planner then drafts.
    // Inject the goal's description so the planner knows what to decompose into
    // GitHub issues — it previously received only the AG-key and had nothing to
    // plan from.
    let goal_text = match goal.body.as_deref().map(str::trim) {
        Some(b) if !b.is_empty() => format!("{}\n\n{}", goal.title, b),
        _ => goal.title.clone(),
    };
    let prompt = cfg
        .prompt
        .replace("<GOAL>", &goal_text)
        .replace("<AG-KEY>", &goal.key);
    {
        let state = state.clone();
        let session = session.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::harness::inject_prompt(&state, &session, &prompt).await {
                tracing::warn!(error = %e, "planner prompt injection failed; session still running");
            }
        });
    }

    let _ = state
        .bus
        .send(Event::new("goal.planner.spawned").with_payload(json!({
            "goal_id": goal.id,
            "session_id": session.id.to_string(),
            "tool": cfg.tool,
        })));

    Ok(session.id.to_string())
}

#[cfg(test)]
mod tests {
    //! Handler-level tests for the board-goals endpoint.
    //! Uses the same in-process AppState harness as board.rs and
    //! board_rules.rs tests — no real tmux or HTTP server.
    //!
    //! Auth middleware is verified at the lib.rs::router() merge site
    //! (top-level `require_token` layer). The in-process harness calls
    //! handlers directly and bypasses middleware; a "unauthenticated request"
    //! test is documented as deferred to the end-to-end plan 01-08.
    //!
    //! Tmux spawn tests are marked `#[ignore]` because they require a live
    //! tmux server — uncomment and run with `--ignored` in a tmux session.

    use super::*;
    use agentum_store::Store;
    use std::sync::Arc;
    use std::sync::MutexGuard;
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    async fn fresh_state() -> AppState {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        std::mem::forget(dir);
        let store = Store::open(&p).await.unwrap();
        let (bus, _rx) = broadcast::channel(16);
        AppState {
            store: Arc::new(store),
            bus,
            started_at: std::time::Instant::now(),
            version: "test",
            auth_limiter: Arc::new(crate::ratelimit::RateLimiter::new(
                8,
                std::time::Duration::from_secs(60),
            )),
            cert_fingerprint: Arc::new(String::new()),
            transcripts: crate::TranscriptStore::new(broadcast::channel(16).0),
            stream_positions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            hostname: "test".to_string(),
            no_auth: true,
            clipboard_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            clipboard_request_bus: broadcast::channel(64).0,
            hook_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            mcp_token: Arc::new(String::from("test-mcp-token")),
            api_base_url: None,
            desktop_bridge: None,
            harness: std::sync::Arc::new(crate::harness::HarnessEngine::new()),
            events_ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    struct TestEnv {
        _dir: TempDir,
        _guard: MutexGuard<'static, ()>,
    }

    fn isolate_xdg() -> TestEnv {
        // Shared crate-wide lock: AGENTUM_HOME is process-global, so serialise
        // against profiles/planner too (a per-module lock would not).
        let guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new().unwrap();
        // SAFETY: `set_var` is unsound under concurrent access.
        // `ENV_LOCK` serialises all tests in this module so only one thread
        // mutates the env at a time. AGENTUM_HOME isolates on every platform
        // (XDG_CONFIG_HOME is a no-op on macOS).
        unsafe {
            std::env::set_var("AGENTUM_HOME", dir.path());
            // Spec 019: the Chat create path now consults `linear::available()`
            // directly (no `AGENTUM_TASK_SINK` short-circuit). `linear` resolves
            // its creds via `dirs::data_local_dir()` (NOT AGENTUM_HOME), so on a
            // dev machine with a real `linear.json` every test would see Linear
            // as available. Point its override at a guaranteed-missing file so
            // create-goal tests are hermetic. A test that wants Linear available
            // overrides this var itself.
            std::env::set_var("AGENTUM_LINEAR_CREDS", dir.path().join("no-linear.json"));
        }
        // Guard the isolation seam: if config_dir() ever stops honoring
        // AGENTUM_HOME, fail loudly here instead of writing test fixtures into
        // the user's real config dir (the planner.toml/profiles.toml leak).
        // Read-only assert — cheap, runs once per test.
        let cfg = agentum_store::paths::config_dir().expect("config_dir resolves");
        assert!(
            cfg.starts_with(dir.path()),
            "AGENTUM_HOME isolation broken: config_dir {cfg:?} escaped temp {:?}",
            dir.path()
        );
        TestEnv {
            _dir: dir,
            _guard: guard,
        }
    }

    /// Build a `CreateGoalBody` with the spec-018 defaults (local host, no
    /// agent inheritance). Keeps the per-test sites focused on what they vary.
    fn goal_body(title: &str, workdir: Option<&str>) -> CreateGoalBody {
        CreateGoalBody {
            title: title.into(),
            body: None,
            workdir: workdir.map(str::to_string),
            tool: None,
            model: None,
            host_id: None,
            repo_slug: None,
        }
    }

    /// Spec 019: a hermetic, non-tracking workdir for create-goal tests — a temp
    /// dir that is NOT a git repo (so no GitHub origin resolves) under
    /// `isolate_xdg` (so no Linear). The Chat path therefore deterministically
    /// hits `no_tracker` without touching the network or the dev machine's real
    /// trackers. Replaces the old `force_board_sink()` (the Board is no longer a
    /// Chat outcome — AC-4). Returns the dir (keep it alive) + its path string.
    fn untracked_workdir() -> (TempDir, String) {
        let dir = TempDir::new().unwrap();
        let wd = dir.path().to_string_lossy().into_owned();
        (dir, wd)
    }

    /// Spec 019 AC-4: Chat no longer falls back to the internal Board. POST
    /// /api/board/goals still creates the goal BoardItem (lbl=goal, status=todo),
    /// but when no GitHub/Linear target resolves the feature-create fails loudly
    /// with `no_tracker` — and NO `feat` board card is created.
    #[tokio::test]
    async fn create_goal_inserts_board_item_but_never_a_board_feature() {
        let _env = isolate_xdg();
        let (_dir, wd) = untracked_workdir();
        let state = fresh_state().await;

        let err = create_goal(
            State(state.clone()),
            Json(goal_body("build OAuth", Some(&wd))),
        )
        .await
        .expect_err("no tracker resolves → loud error, never a board card");

        // AC-4: the failure is typed `no_tracker`, never a board provider.
        match err {
            ApiError::Custom(status, ref v) => {
                assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
                assert_eq!(v["error"]["code"], "no_tracker", "got {v}");
                assert_ne!(v["error"]["provider"], "board");
            }
            other => panic!("expected Custom 422 no_tracker, got {other:?}"),
        }

        // The goal row is still created and retained (the Chat-side record) — only
        // the feature-create errored.
        let items = state.store.list_board_items().await.unwrap();
        let goal = items
            .iter()
            .find(|i| i.lbl.as_deref() == Some("goal"))
            .expect("the goal row must be created and retained");
        assert_eq!(goal.status, "todo", "goal lands in todo");
        assert_eq!(goal.title, "build OAuth");
        assert!(goal.parent_goal_id.is_none(), "goals have no parent goal");
        // The crux: NO board feature card was silently created.
        assert!(
            items.iter().all(|i| i.lbl.as_deref() != Some("feat")),
            "Chat must NEVER create an internal board feature card (AC-4)"
        );
    }

    /// Spec 019 AC-4 (was 018 AC-3): a project that resolves to NO GitHub repo
    /// (a temp dir with no git origin) and no Linear returns a LOUD, TYPED
    /// `422 { error: { code, message, provider } }` envelope — never a silent
    /// fallback and never a generic 500. With the local-workdir gate removed, the
    /// loud failure is `no_tracker` (the new replacement for the silent Board).
    #[tokio::test]
    async fn create_goal_not_a_github_repo_returns_typed_error() {
        let _env = isolate_xdg();
        let state = fresh_state().await;
        let (_dir, wd) = untracked_workdir();

        let err = create_goal(State(state.clone()), Json(goal_body("ship it", Some(&wd))))
            .await
            .expect_err("a non-GitHub repo with no Linear must error loudly");

        // Must be the typed envelope: Custom(422, {error:{code,message,provider}}).
        match err {
            ApiError::Custom(status, ref v) => {
                assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "uses 422");
                let e = &v["error"];
                assert_eq!(e["code"], "no_tracker", "got {v}");
                // AC-4: never the internal board.
                assert_ne!(e["provider"], "board");
                assert!(
                    e["message"].as_str().is_some_and(|m| !m.is_empty()),
                    "message must be a non-empty, human-readable reason"
                );
            }
            other => panic!("expected Custom 422 typed envelope, got {other:?}"),
        }

        // The goal row is retained (the Chat-side record) even though the
        // feature create failed — only the feature creation errored.
        let items = state.store.list_board_items().await.unwrap();
        assert!(
            items.iter().any(|i| i.lbl.as_deref() == Some("goal")),
            "the goal row must be retained on a feature-create failure"
        );
    }

    /// Spec 018 AC-3: an empty/whitespace title is rejected up front with a
    /// typed `422 {code:"empty_title"}` envelope — before the column gate or any
    /// sink call — so a blank description can never silently create a junk issue.
    #[tokio::test]
    async fn create_goal_empty_title_is_rejected() {
        let _env = isolate_xdg();
        let state = fresh_state().await;

        let err = create_goal(State(state.clone()), Json(goal_body("   ", None)))
            .await
            .expect_err("a whitespace-only title must be rejected");

        match err {
            ApiError::Custom(status, ref v) => {
                assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
                assert_eq!(v["error"]["code"], "empty_title");
            }
            other => panic!("expected Custom 422 empty_title, got {other:?}"),
        }

        // Nothing was written — the reject fired before the goal row.
        let items = state.store.list_board_items().await.unwrap();
        assert!(items.is_empty(), "empty-title reject must not write a goal");
    }

    /// POST /api/board/goals emits a goal.created event on the bus. The goal row
    /// + event fire BEFORE the feature-create, so this holds even when the
    /// feature-create later fails `no_tracker` (untracked workdir, spec 019).
    #[tokio::test]
    async fn create_goal_emits_goal_created_event() {
        let _env = isolate_xdg();
        let (_dir, wd) = untracked_workdir();
        let state = fresh_state().await;
        let mut rx = state.bus.subscribe();

        // The feature-create fails (no tracker) but the goal row + goal.created
        // event are already on the bus — that's what this test asserts.
        let _ = create_goal(
            State(state.clone()),
            Json(goal_body("event test", Some(&wd))),
        )
        .await;

        // Two events should be on the bus: board.created (from create_board_item path
        // handled inside the handler) then goal.created.  Drain until we see goal.created.
        let mut saw_goal_created = false;
        loop {
            match rx.try_recv() {
                Ok(ev) if ev.kind == "goal.created" => {
                    assert_eq!(ev.payload["title"], "event test");
                    saw_goal_created = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(saw_goal_created, "goal.created event must fire on the bus");
    }

    /// POST /api/board/goals respects the column-rule gate.
    /// If PUT /api/board/rules/todo requires "body", the goal POST without body returns 400.
    #[tokio::test]
    async fn create_goal_respects_column_rule_gate() {
        let _env = isolate_xdg();
        let state = fresh_state().await;

        // Raise the bar: require body for the todo column.
        // The `body` required field maps to RequiredField::Body if that variant
        // exists; since the schema only defines Title/Lbl/Workdir/Tool/ClaimedBy/
        // SessionOrComment, we use an unknown string that the store serialises and
        // the validate_against function treats as "not present" => gate fires.
        // In practice we just require all the default `todo` fields plus `workdir`
        // so the POST without workdir is rejected.
        state
            .store
            .upsert_board_column_rule(
                "todo",
                &[
                    agentum_core::RequiredField::Title,
                    agentum_core::RequiredField::Lbl,
                    agentum_core::RequiredField::Workdir,
                ],
            )
            .await
            .unwrap();

        // POST goal without workdir — must be rejected by the gate (the gate
        // fires after the empty-title check but before any sink call).
        let err = create_goal(State(state), Json(goal_body("missing workdir", None)))
            .await
            .expect_err("gate must reject when workdir is required");

        // The error must be the Custom(400, {missing, status}) envelope shape.
        assert!(
            matches!(err, ApiError::Custom(s, ref v)
                if s == StatusCode::BAD_REQUEST
                && v["missing"].as_array().is_some_and(|a| !a.is_empty())
                && v["status"] == "todo"),
            "expected Custom 400 gate envelope, got {err:?}"
        );
    }

    /// Spec 018 unit: the AC-3 error envelope nests under `error` (an object)
    /// with `code`/`message`/`provider`, distinct from the default
    /// `{"error": string}` shape — so the UI can branch on `code`.
    #[test]
    fn create_error_builds_the_typed_envelope() {
        let err = create_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_gh",
            "GitHub CLI not installed",
            Some("github"),
        );
        match err {
            ApiError::Custom(status, v) => {
                assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
                assert_eq!(v["error"]["code"], "no_gh");
                assert_eq!(v["error"]["message"], "GitHub CLI not installed");
                assert_eq!(v["error"]["provider"], "github");
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    /// Spec 018 unit: `gh` stderr is classified into a finer error code — a repo
    /// `gh` can't resolve is `not_github_repo`; everything else is `gh_failed`.
    #[test]
    fn classify_gh_stderr_distinguishes_repo_from_other_failures() {
        assert_eq!(
            classify_gh_stderr(
                "none of the git remotes configured for this repository point to a known GitHub host"
            ),
            "not_github_repo"
        );
        assert_eq!(
            classify_gh_stderr("fatal: not a git repository (or any of the parent directories)"),
            "not_github_repo"
        );
        // Auth / rate-limit / validation → gh_failed (the generic GitHub error).
        assert_eq!(classify_gh_stderr("HTTP 401: Bad credentials"), "gh_failed");
    }

    /// Auth middleware is verified at the lib.rs::router() merge site via the
    /// top-level `require_token` layer — the in-process test harness calls
    /// handlers directly and bypasses middleware. Testing 401 here would
    /// require spinning up a full axum server, which is deferred to the
    /// end-to-end integration tests in plan 01-08.
    #[test]
    fn goals_route_requires_auth_verified_at_router_merge() {
        // Documented skip — see comment above.
    }

    // --- spawn_card_session tests (plan 02-03) ---

    /// spawn_card_session returns HTTP 400 with the canonical missing-workdir
    /// envelope when the card has no workdir and no parent_goal.
    ///
    /// This test exercises the CONTEXT D-02 fallthrough path without needing
    /// a live tmux server. The tmux-requiring happy-path is deferred to plan
    /// 02-06 e2e.
    #[tokio::test]
    async fn spawn_card_session_missing_workdir_returns_400() {
        let state = fresh_state().await;

        // Create a card with no workdir and no parent_goal_id.
        let card = state
            .store
            .create_board_item(agentum_core::NewBoardItem {
                title: "no workdir card".into(),
                body: None,
                status: Some("todo".into()),
                lbl: Some("feat".into()),
                tool: Some("claude".into()),
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();

        let err = spawn_card_session(&state, &card)
            .await
            .expect_err("spawn_card_session must fail when workdir is absent");

        // Must be ApiError::Custom(400, {missing: ["workdir"], status: "doing"}).
        assert!(
            matches!(&err, ApiError::Custom(s, v)
                if *s == axum::http::StatusCode::BAD_REQUEST
                && v["missing"].as_array().is_some_and(|a| a.iter().any(|x| x == "workdir"))
                && v["status"] == "doing"),
            "expected Custom 400 missing-workdir envelope, got {err:?}"
        );
    }

    /// spawn_card_session with a live tmux requires a running tmux server.
    /// The full happy-path (workdir resolved, session spawned, dual-write committed,
    /// board.updated carries session_id) is covered by plan 02-06 e2e.
    ///
    /// Marked `#[ignore]` — run with `--ignored` inside a tmux session to exercise
    /// the live tmux path.
    #[tokio::test]
    #[ignore = "requires a live tmux server; covered by plan 02-06 e2e"]
    async fn spawn_card_session_happy_path_requires_live_tmux() {
        // Deferred to plan 02-06 end-to-end integration tests.
    }

    /// `git init` + one commit so `git worktree add … HEAD` has a born HEAD.
    fn init_git_repo(dir: &std::path::Path) {
        use std::process::Command;
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .expect("git available in test env")
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@agentum.dev"]);
        run(&["config", "user.name", "agentum-test"]);
        std::fs::write(dir.join("README.md"), "seed").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "seed"]);
    }

    /// Set the `origin` remote of an already-initialised repo to `url`.
    fn set_origin(dir: &std::path::Path, url: &str) {
        use std::process::Command;
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["remote", "add", "origin", url])
            .output()
            .expect("git available in test env")
            .status
            .success();
        assert!(ok, "git remote add origin {url} failed");
    }

    /// A local [`Host`] for the slug-read tests (the Chat path always files local).
    fn local_host() -> Host {
        Host {
            id: agentum_core::LOCAL_HOST_ID,
            name: "local".into(),
            kind: agentum_core::HostKind::Local,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        }
    }

    // --- spec 019: slug resolution + Chat-from-anywhere ---

    /// `is_valid_slug` accepts exactly `owner/repo` and rejects anything with the
    /// wrong shape — the guard that keeps a malformed hint off the `gh` argv.
    #[test]
    fn is_valid_slug_accepts_owner_repo_only() {
        assert!(is_valid_slug("owner/repo"));
        assert!(is_valid_slug("octo-cat/My_Repo.git-ish"));
        assert!(!is_valid_slug("owner"), "no slash");
        assert!(!is_valid_slug("owner/repo/extra"), "two slashes");
        assert!(!is_valid_slug("owner /repo"), "whitespace");
        assert!(!is_valid_slug("/repo"), "empty owner");
        assert!(!is_valid_slug("owner/"), "empty repo");
        assert!(!is_valid_slug(""), "empty");
    }

    /// A valid client hint short-circuits resolution with NO git IO — even when
    /// the workdir doesn't exist and the host is local (proves the fast path).
    #[tokio::test]
    async fn resolve_github_slug_trusts_valid_hint_without_io() {
        let host = local_host();
        let slug = resolve_github_slug(&host, "/path/does/not/exist", Some("acme/widgets"))
            .await
            .expect("a valid hint must resolve");
        assert_eq!(slug, "acme/widgets");
    }

    /// A real local repo with a GitHub `origin` → `Some("owner/repo")` from the
    /// host-aware read (no hint).
    #[tokio::test]
    async fn resolve_github_slug_reads_github_origin() {
        let dir = TempDir::new().unwrap();
        init_git_repo(dir.path());
        set_origin(dir.path(), "git@github.com:owner/repo.git");
        let host = local_host();

        let slug = resolve_github_slug(&host, &dir.path().to_string_lossy(), None)
            .await
            .expect("a github origin must resolve to its slug");
        assert_eq!(slug, "owner/repo");
    }

    /// A GitLab origin is NOT a GitHub target → `Err(NoGithubRemote)`.
    #[tokio::test]
    async fn resolve_github_slug_rejects_gitlab_origin() {
        let dir = TempDir::new().unwrap();
        init_git_repo(dir.path());
        set_origin(dir.path(), "git@gitlab.com:group/proj.git");
        let host = local_host();

        let reason = resolve_github_slug(&host, &dir.path().to_string_lossy(), None)
            .await
            .expect_err("a gitlab origin is not a github target");
        assert_eq!(reason, SlugReason::NoGithubRemote);
    }

    /// A non-git directory (no `origin`) → `Err(NoGithubRemote)` (git ran but
    /// exited non-zero — that's "no remote", not "host unreachable").
    #[tokio::test]
    async fn resolve_github_slug_non_git_dir_is_no_remote() {
        let dir = TempDir::new().unwrap();
        let host = local_host();

        let reason = resolve_github_slug(&host, &dir.path().to_string_lossy(), None)
            .await
            .expect_err("a non-git dir has no github remote");
        assert_eq!(reason, SlugReason::NoGithubRemote);
    }

    /// AC-1: a workdir that does NOT exist locally + a valid GitHub slug hint
    /// must NOT return "workdir does not exist". The Chat path resolves the slug
    /// from the hint (no IO, no existence check) and drives `gh issue create
    /// --repo <slug>` from `$HOME`. The create itself may fail at `gh`
    /// (auth/repo-not-found — a typed `gh_failed`, never the old filesystem
    /// gate, never the Board). The point this asserts: the existence precondition
    /// is gone. Hermetic: the slug points at a repo that doesn't exist, so a live
    /// `gh` 404s without creating anything; a missing `gh` yields `no_gh`.
    #[tokio::test]
    async fn create_goal_missing_workdir_with_slug_skips_existence_gate() {
        let _env = isolate_xdg();
        let state = fresh_state().await;

        // A path that cannot exist on this machine (the remote-project bug), plus
        // a well-formed but non-existent slug so a live `gh` 404s (no side effect).
        let body = CreateGoalBody {
            title: "file from anywhere".into(),
            body: None,
            workdir: Some("/definitely/not/here/spec019".into()),
            tool: None,
            model: None,
            host_id: None,
            repo_slug: Some("agentum-nonexistent-org-xyz/no-such-repo".into()),
        };

        let result = create_goal(State(state.clone()), Json(body)).await;

        // Whatever the outcome, it must NEVER be the old filesystem error.
        if let Err(ref e) = result {
            let msg = match e {
                ApiError::Custom(_, v) => v["error"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                other => other.to_string(),
            };
            assert!(
                !msg.contains("workdir does not exist"),
                "AC-1 regression: the local-workdir gate is back — got {msg:?}"
            );
            // A failure here must be a typed GitHub error (gh missing/auth/repo),
            // never the Board.
            if let ApiError::Custom(status, v) = e {
                assert_eq!(*status, StatusCode::UNPROCESSABLE_ENTITY);
                assert_eq!(
                    v["error"]["provider"], "github",
                    "a slug-resolved failure is a github error"
                );
                assert_ne!(v["error"]["provider"], "board", "Chat never lands on board");
            }
        }
    }

    /// AC-4: no resolvable GitHub slug AND no Linear → `422 no_tracker`, and the
    /// response is NEVER `provider:"board"`. Uses a non-git temp workdir (no
    /// origin) with the default sink selection (no AGENTUM_TASK_SINK), and no
    /// Linear token on disk (AGENTUM_HOME is an isolated temp dir).
    #[tokio::test]
    async fn create_goal_no_github_no_linear_is_no_tracker_never_board() {
        let _env = isolate_xdg();
        // No AGENTUM_TASK_SINK: the real precedence runs. With no github origin
        // and no Linear token (isolated AGENTUM_HOME), the Chat path must error
        // with no_tracker — NOT silently create a board card.
        // SAFETY: serialised by isolate_xdg's TEST_ENV_LOCK.
        unsafe { std::env::remove_var("AGENTUM_TASK_SINK") };
        let state = fresh_state().await;
        let dir = TempDir::new().unwrap(); // not a git repo → no origin
        let wd = dir.path().to_string_lossy().into_owned();

        let err = create_goal(
            State(state.clone()),
            Json(goal_body("untracked work", Some(&wd))),
        )
        .await
        .expect_err("no github + no linear must be a loud error, not a board card");

        match err {
            ApiError::Custom(status, ref v) => {
                assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "AC-4 uses 422");
                assert_eq!(v["error"]["code"], "no_tracker", "got {v}");
                // The crux of AC-4: Chat NEVER lands on the internal board.
                assert_ne!(v["error"]["provider"], "board");
            }
            other => panic!("expected Custom 422 no_tracker, got {other:?}"),
        }

        // No board feature card was silently created (only the goal row remains).
        let items = state.store.list_board_items().await.unwrap();
        assert!(
            items.iter().all(|i| i.lbl.as_deref() != Some("feat")),
            "no_tracker must NOT create a feat card on the board"
        );
    }

    /// card-start MUST provision an isolated worktree when the project is a
    /// git repo — this is the backend gap closed in Phase 2 (design 2026-06-18).
    #[tokio::test]
    async fn provision_card_worktree_creates_worktree_in_git_repo() {
        let dir = TempDir::new().unwrap();
        init_git_repo(dir.path());

        let resolved = provision_card_worktree(dir.path(), "card-ag-1")
            .await
            .expect("worktree provisioning must succeed in a git repo")
            .expect("a git repo must yield Some(worktree)");

        assert!(
            resolved.path.exists(),
            "the worktree directory must exist on disk: {}",
            resolved.path.display()
        );
        assert_eq!(
            resolved.branch, "agentum/card-ag-1",
            "branch must be derived per-card from the session name"
        );
        assert!(
            resolved.path != dir.path(),
            "the worktree must be a separate checkout, not the project root"
        );
    }

    /// Non-git project dirs can't be isolated — card-start falls back to a
    /// direct spawn (None) rather than 400-ing, so the card still runs.
    #[tokio::test]
    async fn provision_card_worktree_returns_none_for_non_git_dir() {
        let dir = TempDir::new().unwrap();
        let resolved = provision_card_worktree(dir.path(), "card-ag-2")
            .await
            .expect("non-git dir must not error");
        assert!(
            resolved.is_none(),
            "a non-git project dir must yield None (direct-spawn fallback)"
        );
    }

    /// Create a card bound to a session whose worktree is `resolved`, in the
    /// given repo, with the given session status. Returns the persisted card.
    async fn card_bound_to_worktree(
        state: &AppState,
        repo: &std::path::Path,
        resolved: &crate::git::ResolvedWorktree,
        name: &str,
        status: Status,
    ) -> BoardItem {
        let session = state
            .store
            .create_session(NewSession {
                name: name.into(),
                workdir: repo.to_string_lossy().into_owned(),
                tool: "claude".into(),
                model: None,
                flags: vec![],
                card_id: None,
                worktree_path: Some(resolved.path.to_string_lossy().into_owned()),
                worktree_branch: Some(resolved.branch.clone()),
                worktree_base_ref: Some(resolved.base_ref.clone()),
            })
            .await
            .unwrap();
        if !matches!(status, Status::Idle) {
            state
                .store
                .update_status_and_target(session.id, status, None)
                .await
                .unwrap();
        }
        state
            .store
            .create_board_item(NewBoardItem {
                title: "done card".into(),
                body: None,
                status: Some("done".into()),
                lbl: Some("feat".into()),
                tool: Some("claude".into()),
                workdir: Some(repo.to_string_lossy().into_owned()),
                model: None,
                session_id: Some(session.id.to_string()),
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap()
    }

    /// A done card with a clean, non-running worktree IS pruned (design
    /// 2026-06-18 "prune on card done").
    #[tokio::test]
    async fn prune_on_done_removes_a_clean_idle_worktree() {
        let dir = TempDir::new().unwrap();
        init_git_repo(dir.path());
        let resolved = crate::git::create_worktree(dir.path(), "card-ag-3", None, None)
            .await
            .unwrap();
        assert!(resolved.path.exists());

        let state = fresh_state().await;
        let card =
            card_bound_to_worktree(&state, dir.path(), &resolved, "card-ag-3", Status::Idle).await;

        let pruned = prune_card_worktree_on_done(&state, &card).await;
        assert!(
            pruned,
            "a clean, non-running worktree must be pruned on done"
        );
        assert!(
            !resolved.path.exists(),
            "the worktree directory must be gone after prune"
        );
    }

    /// A done card whose agent is still RUNNING is NOT pruned — never yank a
    /// worktree out from under a live agent.
    #[tokio::test]
    async fn prune_on_done_skips_a_running_session() {
        let dir = TempDir::new().unwrap();
        init_git_repo(dir.path());
        let resolved = crate::git::create_worktree(dir.path(), "card-ag-4", None, None)
            .await
            .unwrap();

        let state = fresh_state().await;
        let card =
            card_bound_to_worktree(&state, dir.path(), &resolved, "card-ag-4", Status::Running)
                .await;

        let pruned = prune_card_worktree_on_done(&state, &card).await;
        assert!(!pruned, "a running session's worktree must be left intact");
        assert!(
            resolved.path.exists(),
            "the worktree directory must still exist when the agent is running"
        );
    }

    fn card_with(title: &str, body: Option<&str>) -> BoardItem {
        BoardItem {
            id: 1,
            key: "AG-1".into(),
            title: title.into(),
            body: body.map(str::to_string),
            status: "doing".into(),
            claimed_by: None,
            created_at: time::OffsetDateTime::now_utc(),
            updated_at: time::OffsetDateTime::now_utc(),
            lbl: None,
            tool: None,
            workdir: None,
            model: None,
            session_id: None,
            priority: 0,
            parent_goal_id: None,
            external_url: None,
            external_provider: None,
            external_id: None,
            external_synced_at: None,
        }
    }

    #[test]
    fn build_card_prompt_combines_title_and_body() {
        let card = card_with("Wire dashboard", Some("Use Svelte 5 runes"));
        let p = build_card_prompt(&card).expect("title+body must produce a prompt");
        assert!(p.contains("AG-1"), "prompt must include the card key");
        assert!(p.contains("Wire dashboard"), "prompt must include title");
        assert!(p.contains("Use Svelte 5 runes"), "prompt must include body");
    }

    #[test]
    fn build_card_prompt_title_only_includes_key() {
        let card = card_with("Wire dashboard", None);
        let p = build_card_prompt(&card).expect("title-only must produce a prompt");
        assert!(p.contains("AG-1"));
        assert!(p.contains("Wire dashboard"));
    }

    #[test]
    fn build_card_prompt_body_only_is_verbatim() {
        let card = card_with("", Some("Investigate the panic"));
        let p = build_card_prompt(&card).expect("body-only must produce a prompt");
        assert_eq!(p, "Investigate the panic");
    }

    #[test]
    fn build_card_prompt_empty_returns_none() {
        let card = card_with("   ", Some("   "));
        assert!(build_card_prompt(&card).is_none());
    }

    // --- plan_goal_harness tests (spec 011a) ---

    async fn make_goal_with_children(
        state: &AppState,
        workdir: &str,
        children: &[(&str, Option<&str>)],
    ) -> BoardItem {
        let goal = state
            .store
            .create_board_item(agentum_core::NewBoardItem {
                title: "Ship auth".into(),
                body: None,
                lbl: Some("goal".into()),
                status: Some("todo".into()),
                workdir: Some(workdir.to_string()),
                parent_goal_id: None,
                tool: None,
                model: None,
                session_id: None,
                priority: None,
            })
            .await
            .unwrap();
        for (title, body) in children {
            state
                .store
                .create_board_item(agentum_core::NewBoardItem {
                    title: (*title).into(),
                    body: body.map(str::to_string),
                    lbl: Some("feat".into()),
                    status: Some("todo".into()),
                    workdir: None,
                    parent_goal_id: Some(goal.id),
                    tool: None,
                    model: None,
                    session_id: None,
                    priority: None,
                })
                .await
                .unwrap();
        }
        goal
    }

    /// The happy path: a goal's child cards become a Pending harness backlog on
    /// disk, loadable by the engine, and the harness is left Idle (not run).
    #[tokio::test]
    async fn plan_goal_harness_writes_idle_backlog_from_children() {
        // Force the agnostic board path under the env lock so the test never
        // probes the dev machine's connected GitHub/Linear (no network).
        let _env = isolate_xdg();
        unsafe { std::env::set_var("AGENTUM_TASK_SINK", "board") };
        let state = fresh_state().await;
        let dir = TempDir::new().unwrap();
        let wd = dir.path().to_string_lossy().into_owned();
        let goal = make_goal_with_children(
            &state,
            &wd,
            &[
                ("Login screen", Some("user sees a login form")),
                ("Logout", None),
            ],
        )
        .await;

        let resp = plan_goal_harness(State(state.clone()), Path(goal.id))
            .await
            .expect("plan must succeed");
        assert_eq!(resp.0.feature_count, 2);
        // A temp workdir is not a GitHub repo, so the agnostic fallback (board)
        // is the source of truth.
        assert_eq!(resp.0.provider, "board");

        // feature_list.json is on disk, loadable, and every feature is Pending.
        let cfg = crate::harness::HarnessConfig::load(dir.path())
            .await
            .unwrap();
        assert_eq!(cfg.features.features.len(), 2);
        assert!(
            cfg.features
                .features
                .iter()
                .all(|f| f.state == crate::harness::FeatureState::Pending),
            "harness must be Idle — every feature Pending until the user runs it"
        );
    }

    /// `goal.harness.planned` fires so the UI can refresh the Harness view.
    #[tokio::test]
    async fn plan_goal_harness_emits_event() {
        let _env = isolate_xdg();
        unsafe { std::env::set_var("AGENTUM_TASK_SINK", "board") };
        let state = fresh_state().await;
        let dir = TempDir::new().unwrap();
        let wd = dir.path().to_string_lossy().into_owned();
        let goal = make_goal_with_children(&state, &wd, &[("F1", None)]).await;
        let mut rx = state.bus.subscribe();

        let _resp = plan_goal_harness(State(state.clone()), Path(goal.id))
            .await
            .unwrap();

        let mut saw = false;
        while let Ok(ev) = rx.try_recv() {
            if ev.kind == "goal.harness.planned" {
                assert_eq!(ev.payload["goal_id"], goal.id);
                assert_eq!(ev.payload["feature_count"], 1);
                saw = true;
                break;
            }
        }
        assert!(saw, "goal.harness.planned must fire on the bus");
    }

    /// A goal whose planner hasn't produced any cards yet is rejected loudly —
    /// we never write a silent empty backlog.
    #[tokio::test]
    async fn plan_goal_harness_rejects_goal_without_children() {
        let state = fresh_state().await;
        let dir = TempDir::new().unwrap();
        let wd = dir.path().to_string_lossy().into_owned();
        let goal = make_goal_with_children(&state, &wd, &[]).await;

        let err = plan_goal_harness(State(state), Path(goal.id))
            .await
            .expect_err("a childless goal must be rejected");
        assert!(matches!(err, ApiError::BadRequest(_)), "got {err:?}");
    }

    /// Planning the harness off a non-goal card is rejected.
    #[tokio::test]
    async fn plan_goal_harness_rejects_non_goal() {
        let state = fresh_state().await;
        let card = state
            .store
            .create_board_item(agentum_core::NewBoardItem {
                title: "just a feature".into(),
                body: None,
                lbl: Some("feat".into()),
                status: Some("todo".into()),
                workdir: None,
                parent_goal_id: None,
                tool: None,
                model: None,
                session_id: None,
                priority: None,
            })
            .await
            .unwrap();

        let err = plan_goal_harness(State(state), Path(card.id))
            .await
            .expect_err("non-goal must be rejected");
        assert!(matches!(err, ApiError::BadRequest(_)), "got {err:?}");
    }
}
