//! Harness Engine API — `/api/harness/*`.
//!
//! Thin HTTP+WS surface over [`crate::harness::HarnessEngine`] (held in
//! [`AppState::harness`]). The heavy lifting — spawning real agents, the
//! verification gate, advance/block — lives in [`crate::harness::drive`], which
//! this layer kicks off as a background task on `POST /{id}/run`.
//!
//! Mounted into the main `AppState` router so it inherits the bearer-token
//! middleware (free on the embedded loopback server, which is `no_auth`).

use std::path::PathBuf;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::harness::{HarnessConfig, HarnessFiles, HarnessStatus};

/// Opt-in capability switch for the Auto QA arm (spec 005 F3, D3): when true,
/// `resolve_qa_mode`'s Auto arm treats agent-QA as capable WITHOUT
/// AGENTUM_BROWSER_VERIFY. Default OFF — Auto + no qa.sh + no env stays the
/// Script skip-pass, so non-web projects and headless/CI are byte-identical.
pub const BROWSER_QA_ENABLED_SETTING: &str = "harness.qa.agent_browser.enabled";

/// Spec 006 F3 (D1): when true, start-work-planned backlogs run the SDD role
/// loop (PM gate → Architect gate → Decompose → Execute → Review gate).
/// Default ON — the loop is the product working as designed; this is the
/// global opt-out. Read EXACTLY ONCE, in start_work's post-plan knob write:
/// `roles` is a backlog knob stamped into feature_list.json, never a
/// per-drive-tick read — manually registered runs are untouched.
pub const SDD_ROLES_ENABLED_SETTING: &str = "harness.sdd.roles.enabled";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/harness", get(list).post(start))
        .route("/api/harness/events", get(events))
        .route("/api/harness/spec-from-issue", post(spec_from_issue))
        .route("/api/harness/start-work", post(start_work))
        // Static-over-capture (matchit): coexists with `/api/harness/{id}` the
        // same way `/api/harness/events` already does.
        .route("/api/harness/settings", get(get_settings).put(put_settings))
        .route("/api/harness/{id}", get(status).delete(stop))
        .route("/api/harness/{id}/run", post(run))
        .route("/api/harness/{id}/init", post(init))
        .route("/api/harness/{id}/verify", post(verify))
        .route("/api/harness/{id}/confirm", post(confirm))
        .route("/api/harness/{id}/files", get(files))
}

/// Wire shape of `GET /api/harness/settings` (and the PUT *response*) — the
/// engine-wide run-behavior knobs the desktop Settings pane reflects. Always
/// full; declaration order = wire order (pinned).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarnessSettings {
    browser_qa_agent_enabled: bool,
    /// Spec 006 F3: run the SDD role loop on start-work-planned backlogs.
    sdd_roles_enabled: bool,
}

/// PUT body: partial by design (spec 006 C2) so a caller flipping one knob
/// can't clobber the other — and the pre-006 one-field PUT stays valid
/// (pinned by `harness_settings_patch_accepts_partial_puts`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarnessSettingsPatch {
    #[serde(default)]
    browser_qa_agent_enabled: Option<bool>,
    #[serde(default)]
    sdd_roles_enabled: Option<bool>,
}

/// Read the full effective settings. Note the DIFFERENT defaults: the QA knob
/// is opt-in (false, D3 of spec 005), the roles knob is opt-out (true, D1 of
/// spec 006).
async fn read_settings(store: &agentum_store::Store) -> Result<HarnessSettings, ApiError> {
    Ok(HarnessSettings {
        browser_qa_agent_enabled: store
            .setting_get_bool(BROWSER_QA_ENABLED_SETTING, false)
            .await?,
        sdd_roles_enabled: store
            .setting_get_bool(SDD_ROLES_ENABLED_SETTING, true)
            .await?,
    })
}

/// `GET /api/harness/settings` — the full engine-wide knob set. Mirrors
/// `routes/mcp.rs`'s settings route.
async fn get_settings(
    State(state): State<AppState>,
) -> Result<axum::Json<HarnessSettings>, ApiError> {
    Ok(axum::Json(read_settings(&state.store).await?))
}

/// `PUT /api/harness/settings` — patch semantics: write only the keys present
/// in the body, return the full effective settings (C2). The QA knob is read
/// per gate decision in `drive_inner` (applies to the next gate, no restart);
/// the roles knob is read once per start-work plan.
async fn put_settings(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<HarnessSettingsPatch>,
) -> Result<axum::Json<HarnessSettings>, ApiError> {
    if let Some(v) = req.browser_qa_agent_enabled {
        state
            .store
            .setting_set_bool(BROWSER_QA_ENABLED_SETTING, v)
            .await?;
    }
    if let Some(v) = req.sdd_roles_enabled {
        state
            .store
            .setting_set_bool(SDD_ROLES_ENABLED_SETTING, v)
            .await?;
    }
    Ok(axum::Json(read_settings(&state.store).await?))
}

#[derive(Debug, Deserialize)]
struct StartRequest {
    /// Project directory containing `.harness/`.
    workdir: String,
}

#[derive(Debug, Serialize)]
struct StartResponse {
    harness_id: Uuid,
}

/// `POST /api/harness` — register a run from a project dir (validates `.harness/`).
async fn start(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<StartRequest>,
) -> Result<axum::Json<StartResponse>, ApiError> {
    // Reuse the same `~`/relative expansion every workdir-taking route uses.
    let workdir = super::util::expand_workdir(&req.workdir)?;
    let harness_id = state
        .harness
        .start(workdir)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(axum::Json(StartResponse { harness_id }))
}

/// `GET /api/harness` — full status for every registered run (handy for a list
/// view without an extra round-trip per id).
async fn list(State(state): State<AppState>) -> axum::Json<Vec<HarnessStatus>> {
    let mut out = Vec::new();
    for id in state.harness.list().await {
        if let Ok(s) = state.harness.status(id).await {
            out.push(s);
        }
    }
    axum::Json(out)
}

/// `GET /api/harness/{id}` — one run's status snapshot.
async fn status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<axum::Json<HarnessStatus>, ApiError> {
    state
        .harness
        .status(id)
        .await
        .map(axum::Json)
        .map_err(|e| ApiError::NotFound(e.to_string()))
}

/// `POST /api/harness/{id}/run` — kick off the end-to-end drive loop in the
/// background. Idempotent-ish: a second call while a loop is live is rejected.
async fn run(State(state): State<AppState>, Path(id): Path<Uuid>) -> Result<StatusCode, ApiError> {
    let claimed = state
        .harness
        .claim_driver(id)
        .await
        .map_err(|e| ApiError::NotFound(e.to_string()))?;
    if !claimed {
        return Err(ApiError::BadRequest("harness is already running".into()));
    }
    // The drive loop owns its own error handling (emits Error + Failed state).
    let st = state.clone();
    tokio::spawn(async move { crate::harness::drive(st, id).await });
    Ok(StatusCode::ACCEPTED)
}

/// `POST /api/harness/{id}/init` — run `init.sh` only (manual environment check).
async fn init(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<axum::Json<bool>, ApiError> {
    state
        .harness
        .run_init(id)
        .await
        .map(axum::Json)
        .map_err(|e| ApiError::Internal(e.to_string()))
}

#[derive(Debug, Deserialize)]
struct VerifyRequest {
    feature_id: String,
}

/// `POST /api/harness/{id}/verify` — run the gate for one feature and finalize
/// it (manual single-shot; the drive loop has its own retry-aware path).
async fn verify(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    axum::Json(req): axum::Json<VerifyRequest>,
) -> Result<axum::Json<bool>, ApiError> {
    state
        .harness
        .run_verify(id, &req.feature_id)
        .await
        .map(axum::Json)
        .map_err(|e| ApiError::Internal(e.to_string()))
}

#[derive(Debug, Deserialize)]
struct ConfirmRequest {
    feature_id: String,
}

/// `POST /api/harness/{id}/confirm` — human confirms a feature parked at the
/// HITL-at-QA gate: finalize it `Done` and resume the drive loop from the next
/// pending feature (the paused run already freed its driver slot).
async fn confirm(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    axum::Json(req): axum::Json<ConfirmRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .harness
        .confirm_feature(id, &req.feature_id)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    // Resume autonomously: re-claim the driver and continue with the next
    // pending feature. If something else already drives it, leave it be.
    if state.harness.claim_driver(id).await.unwrap_or(false) {
        let st = state.clone();
        tokio::spawn(async move { crate::harness::drive(st, id).await });
    }
    Ok(StatusCode::ACCEPTED)
}

/// `GET /api/harness/{id}/files` — current `.harness/` file contents for the viewer.
async fn files(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<axum::Json<HarnessFiles>, ApiError> {
    let workdir: PathBuf = state
        .harness
        .workdir(id)
        .await
        .map_err(|e| ApiError::NotFound(e.to_string()))?;
    Ok(axum::Json(HarnessConfig::read_files(&workdir).await))
}

fn default_true() -> bool {
    true
}

/// Spec 021 (#379): map a request's optional `tracker` pin to the provider
/// stamped on this issue-driven path. D4: `Auto`/absent stays `"github"` (the
/// source IS a GitHub issue — probing to Linear would stamp a provider whose
/// id space doesn't match the URL); an explicit `linear` overrides; an
/// unknown value 400s, never a silent fallback.
fn resolve_tracker_pin(raw: Option<&str>) -> Result<&'static str, ApiError> {
    match crate::task_sink::parse_tracker_choice(raw) {
        Ok(crate::task_sink::TrackerChoice::Linear) => Ok("linear"),
        Ok(_) => Ok("github"),
        Err(other) => Err(ApiError::BadRequest(format!(
            "unknown tracker '{other}' — expected auto, github, or linear"
        ))),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpecFromIssueRequest {
    /// The NEW worktree's path — the spec is written INTO it.
    workdir: String,
    /// Issue number, digits-only (validated by the shared fetch).
    number: String,
    /// `owner/repo` fast path for the slug resolution.
    #[serde(default)]
    slug: Option<String>,
    /// Also derive + write `feature_list.json` (default true).
    #[serde(default = "default_true")]
    plan: bool,
    /// Spec 021 (#379): optional explicit tracker pin (`auto`/`github`/
    /// `linear`). Absent/`auto` keeps this issue-driven path's GitHub
    /// stamping (D4); `linear` overrides.
    #[serde(default)]
    tracker: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpecFromIssueResponse {
    spec_id: String,
    /// Relative to `workdir`, e.g. `.agentum-harness/specs/42-add-widget/spec.md`.
    spec_path: String,
    /// Scaffold + spec files written (relative paths).
    written: Vec<String>,
    /// The derived backlog when `plan` — every feature stamped with the issue's
    /// tracker provenance (spec 004 AC 7).
    features: Option<crate::harness::FeatureList>,
}

/// `POST /api/harness/spec-from-issue` — scaffold a spec (and optionally a
/// backlog) from a GitHub issue into a worktree (spec 004 F4, AC 6–7). The
/// issue is fetched server-side (`gh issue view`) so the transform's input is
/// authoritative, then turned into `specs/<n>-<slug>/spec.md` by the pure
/// [`crate::harness::spec_md_from_issue`] transform. An existing spec.md is
/// **never overwritten** (it may be human-edited) — that's a 400, mirroring the
/// scaffold's keep-existing ethos (start-work is the converge caller). When it
/// plans it also fires the initial Todo transition — the spec 005 F1 core is
/// shared, and AC 4 names that inheritance as intentional.
async fn spec_from_issue(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<SpecFromIssueRequest>,
) -> Result<axum::Json<SpecFromIssueResponse>, ApiError> {
    let workdir = super::util::expand_workdir(&req.workdir)?;
    if !workdir.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            workdir.display()
        )));
    }
    let workdir_str = workdir.to_string_lossy().to_string();

    // Server-authoritative fetch (validates the digits-only number). The
    // worktree shares the parent repo's `origin`, so it resolves the slug.
    // No repoId: the workdir is an is_dir-gated LOCAL worktree (spec 020
    // byte-identical pin).
    let issue = super::github::fetch_github_issue(
        &state,
        None,
        &workdir_str,
        &req.number,
        req.slug.as_deref(),
    )
    .await?;

    let provider = resolve_tracker_pin(req.tracker.as_deref())?;
    let ensured = ensure_spec_and_plan(
        &state.store,
        &state.bus,
        &workdir,
        req.number.trim(),
        &issue,
        req.plan,
        /* converge_existing */ false,
        provider,
    )
    .await?;

    Ok(axum::Json(SpecFromIssueResponse {
        spec_id: ensured.spec_id,
        spec_path: ensured.spec_path,
        written: ensured.written,
        features: ensured.features,
    }))
}

/// What [`ensure_spec_and_plan`] wrote/derived — the shared payload behind both
/// the 004 opt-in route and the 005 start-work orchestration.
struct EnsuredSpec {
    spec_id: String,
    /// Relative to `workdir`, e.g. `.agentum-harness/specs/42-add-widget/spec.md`.
    spec_path: String,
    /// Scaffold + spec files written (relative paths).
    written: Vec<String>,
    /// Converged on an existing spec (retry / D5-toggle overlap).
    spec_existed: bool,
    features: Option<crate::harness::FeatureList>,
}

/// Scaffold + write spec.md + optionally plan, from an ALREADY-FETCHED issue
/// (taking `issue` instead of fetching keeps the core unit-testable with a
/// synthetic issue + tempdir and no `gh`). `converge_existing: false` keeps the
/// spec-from-issue route's never-overwrite 400 contract; `true` (start-work)
/// plans from the existing spec instead (spec 005 AC 1 convergence). On a
/// successful plan, fires the initial `TrackerPhase::Todo` transition
/// (best-effort, logged — mirrors `board_goals.rs`'s plan-time Todo) so BOTH
/// callers inherit the label-trail start (AC 4). The transition lives here (not
/// `types.rs`) because it needs `&Store` and the fs-only plan helpers don't.
async fn ensure_spec_and_plan(
    store: &agentum_store::Store,
    // Spec 014 F1: the seam's TrackerEmit needs a bus; threaded from the
    // route handlers' `state.bus` (tests pass a throwaway channel).
    bus: &tokio::sync::broadcast::Sender<agentum_core::Event>,
    // Fully qualified: `Path` in this module is the axum extractor.
    workdir: &std::path::Path,
    number: &str,
    issue: &super::github::FetchedIssue,
    plan: bool,
    converge_existing: bool,
    // Spec 021: the resolved tracker provider stamped into every planned
    // feature + the initial Todo transition. Callers map the request's
    // `tracker` pin per D4 (Auto/absent → "github" on this issue-driven path).
    provider: &str,
) -> Result<EnsuredSpec, ApiError> {
    // Idempotent: existing contract files are kept; only missing ones written.
    let scaffold = crate::harness::scaffold_harness(workdir)
        .await
        .map_err(|e| ApiError::Internal(format!("could not scaffold the harness: {e}")))?;

    let spec_id = crate::harness::issue_spec_id(number, &issue.title);
    let rel_spec_path = format!("{}/specs/{spec_id}/spec.md", crate::harness::HARNESS_DIR);
    let spec_md_path = workdir.join(&rel_spec_path);
    let spec_existed = spec_md_path.exists();
    if spec_existed && !converge_existing {
        return Err(ApiError::BadRequest(format!(
            "spec {spec_id} already exists — not overwriting {rel_spec_path}"
        )));
    }

    let mut written = scaffold.written;
    if !spec_existed {
        if let Some(parent) = spec_md_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::Internal(format!("could not create the spec dir: {e}")))?;
        }
        let spec_md =
            crate::harness::spec_md_from_issue(number, &issue.title, &issue.body, &issue.url);
        tokio::fs::write(&spec_md_path, spec_md)
            .await
            .map_err(|e| ApiError::Internal(format!("could not write spec.md: {e}")))?;
        written.push(rel_spec_path.clone());
    }

    // The transform guarantees ≥1 checkbox, so a plan failure here is IO-class
    // (a converged human-edited spec with no checkboxes also surfaces here).
    let features = if plan {
        let list =
            crate::harness::plan_from_spec_with_tracker(workdir, &spec_id, provider, &issue.url)
                .await
                .map_err(|e| ApiError::Internal(format!("could not plan from the spec: {e}")))?;
        let backlog = format!("{}/feature_list.json", crate::harness::HARNESS_DIR);
        // The scaffold may have just seeded the same file — list it once.
        if !written.contains(&backlog) {
            written.push(backlog);
        }
        Some(list)
    } else {
        None
    };

    if let Some(list) = &features {
        let _ = list; // planned OK → start the label trail at Todo (idempotent flip)
        match crate::task_sink::apply_tracker_transition(
            store,
            provider,
            number,
            Some(&issue.url),
            crate::task_sink::TrackerPhase::Todo,
            crate::task_sink::TrackerEmit {
                bus,
                worktree_id: None,
            },
        )
        .await
        {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(number, error = %e, "initial Todo transition failed (non-fatal)")
            }
        }
    }

    Ok(EnsuredSpec {
        spec_id,
        spec_path: rel_spec_path,
        written,
        spec_existed,
        features,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartWorkRequest {
    /// The freshly created worktree (local).
    workdir: String,
    /// Issue number, digits-only.
    number: String,
    /// `owner/repo` fast path.
    #[serde(default)]
    slug: Option<String>,
    /// The composer's selected agent (spec 005 D2).
    #[serde(default)]
    agent_tool: Option<String>,
    #[serde(default)]
    agent_model: Option<String>,
    /// Spec 021 (#379): optional explicit tracker pin (`auto`/`github`/
    /// `linear`). Absent/`auto` keeps this issue-driven path's GitHub
    /// stamping (D4); `linear` overrides.
    #[serde(default)]
    tracker: Option<String>,
}

/// start_work's post-plan knobs in one pure, pinned place (spec 006 F3).
/// `sdd_roles` only ever SETS roles (the plan resets the list to defaults, so
/// false is already the resting state — never write `false` explicitly).
fn apply_start_work_knobs(
    list: &mut crate::harness::FeatureList,
    agent_tool: Option<&str>,
    agent_model: Option<&str>,
    sdd_roles: bool,
) {
    if let Some(t) = agent_tool {
        list.agent_tool = t.to_string();
    }
    if let Some(m) = agent_model {
        list.agent_model = Some(m.to_string());
    }
    if sdd_roles {
        list.roles = true;
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartWorkResponse {
    harness_id: Uuid,
    spec_id: String,
    /// Converged on an existing spec (retry / D5 overlap).
    spec_existed: bool,
    /// Feature count.
    planned: usize,
    run_started: bool,
    /// Friendly state, NOT an error (200).
    already_running: bool,
}

/// `POST /api/harness/start-work` — the one-click issue → gated run
/// orchestration (spec 005 F1, D1): converge-scaffold + plan from the linked
/// issue → Todo transition → post-plan knob write → register → claim → spawn
/// [`crate::harness::drive`]. Server-side so the composer, the Tasks page, and
/// any future caller share ONE failure surface; spawns nothing itself — the
/// engine's `drive` loop owns the (only) agent launch path (C2: `drive_inner`
/// already fires InProgress at first spawn, so no transition here).
async fn start_work(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<StartWorkRequest>,
) -> Result<axum::Json<StartWorkResponse>, ApiError> {
    // C5: serialize the whole orchestration — two concurrent retries must not
    // register two drivers on one worktree, and the already-running check must
    // precede ALL filesystem mutation (re-planning rewrites feature_list.json).
    let _g = state.harness.start_work_lock.lock().await;

    let workdir = super::util::expand_workdir(&req.workdir)?;
    if !workdir.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            workdir.display()
        )));
    }
    let workdir_str = workdir.to_string_lossy().to_string();

    // Friendly already-running check FIRST (before any fs write). A live drive
    // loop owns the worktree: report it, never clobber its mid-run state.
    if let Some(existing) = state.harness.find_by_workdir(&workdir).await {
        match state.harness.claim_driver(existing).await {
            Ok(false) => {
                let status = state
                    .harness
                    .status(existing)
                    .await
                    .map_err(|e| ApiError::Internal(e.to_string()))?;
                return Ok(axum::Json(StartWorkResponse {
                    harness_id: existing,
                    spec_id: status.features.spec_id.clone().unwrap_or_default(),
                    spec_existed: true,
                    planned: status.features.features.len(),
                    run_started: false,
                    already_running: true,
                }));
            }
            // We now own an *idle* stale run: remove it and fall through to a
            // fresh registration (`stop` emits HarnessCompleted{success:false}
            // — cosmetic, acceptable). This is what makes retries converge.
            Ok(true) => {
                state
                    .harness
                    .stop(existing)
                    .await
                    .map_err(|e| ApiError::Internal(e.to_string()))?;
            }
            // The run vanished between find and claim (concurrent DELETE) —
            // nothing is driving the worktree; register fresh.
            Err(_) => {}
        }
    }

    // Fetch — needed even when the spec exists, because
    // `spec_id = issue_spec_id(number, title)` needs the title.
    // No repoId: the workdir is an is_dir-gated LOCAL worktree (spec 020
    // byte-identical pin).
    let issue = super::github::fetch_github_issue(
        &state,
        None,
        &workdir_str,
        &req.number,
        req.slug.as_deref(),
    )
    .await?;

    // Converge-scaffold + plan (forced ON, AC 1) + Todo-at-plan (AC 4).
    let provider = resolve_tracker_pin(req.tracker.as_deref())?;
    let ensured = ensure_spec_and_plan(
        &state.store,
        &state.bus,
        &workdir,
        req.number.trim(),
        &issue,
        /* plan */ true,
        /* converge_existing */ true,
        provider,
    )
    .await?;

    // Post-plan knob write (AC 2 — "the plan itself writes defaults"): the
    // composer's agent/model become the run's knobs. `spec_id` is already
    // stamped by the plan (F2) — do not re-stamp here.
    //
    // Spec 006 F3 (D1): the one and only read of the roles knob. A store
    // hiccup falls back to the default (ON).
    let sdd_roles = state
        .store
        .setting_get_bool(SDD_ROLES_ENABLED_SETTING, true)
        .await
        .unwrap_or(true);
    let list = crate::harness::update_backlog_knobs(&workdir, |list| {
        apply_start_work_knobs(
            list,
            req.agent_tool.as_deref(),
            req.agent_model.as_deref(),
            sdd_roles,
        );
    })
    .await
    .map_err(|e| ApiError::Internal(format!("could not write the run knobs: {e}")))?;

    // Register AFTER the plan so the run's in-memory snapshot is the real
    // backlog, not the scaffold stub.
    let harness_id = state
        .harness
        .start(workdir.clone())
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // Claim + spawn — byte-identical to the `run` route's spawn. A fresh
    // registration can't be driving, so a failed claim is an internal bug;
    // nothing fallible sits between the claim and the spawn, so there is no
    // post-claim error path to release on.
    let claimed = state
        .harness
        .claim_driver(harness_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !claimed {
        // We do NOT own the slot here (the claim failed) — no release.
        return Err(ApiError::Internal(
            "freshly registered harness is already driving".into(),
        ));
    }
    let st = state.clone();
    tokio::spawn(async move { crate::harness::drive(st, harness_id).await });

    Ok(axum::Json(StartWorkResponse {
        harness_id,
        spec_id: ensured.spec_id,
        spec_existed: ensured.spec_existed,
        planned: list.features.len(),
        run_started: true,
        already_running: false,
    }))
}

/// `DELETE /api/harness/{id}` — drop the run from the engine.
async fn stop(State(state): State<AppState>, Path(id): Path<Uuid>) -> Result<StatusCode, ApiError> {
    state
        .harness
        .stop(id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `WS /api/harness/events` — live `HarnessEvent` stream (all runs). Mirrors the
/// `events.rs` WS pattern: subscribe to the engine's broadcast bus and forward
/// each event as JSON text. A slow client that lags past the bus capacity gets
/// a single `harness.lagged` marker and resumes.
async fn events(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    let rx = state.harness.subscribe();
    ws.on_upgrade(move |socket| run_events(socket, rx))
}

async fn run_events(
    mut socket: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<crate::harness::HarnessEvent>,
) {
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let payload = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string());
                    if socket.send(Message::Text(payload.into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(skipped)) => {
                    let s = serde_json::json!({ "type": "lagged", "skipped": skipped }).to_string();
                    if socket.send(Message::Text(s.into())).await.is_err() { break; }
                }
                Err(RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {} // ignore client pings/text
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh_store(dir: &tempfile::TempDir) -> agentum_store::Store {
        agentum_store::Store::open(&dir.path().join("t.db"))
            .await
            .unwrap()
    }

    /// A throwaway bus for the seam's required `TrackerEmit` (spec 014 F1) —
    /// these tests assert planning behavior, not emission.
    fn test_bus() -> tokio::sync::broadcast::Sender<agentum_core::Event> {
        tokio::sync::broadcast::channel(8).0
    }

    /// A synthetic already-fetched issue — exactly what makes
    /// `ensure_spec_and_plan` unit-testable without `gh` (it takes the issue
    /// instead of fetching). Two checkboxes → two planned features.
    fn synthetic_issue(url: &str) -> super::super::github::FetchedIssue {
        super::super::github::FetchedIssue {
            title: "Add widget".into(),
            body: "Intro.\n\n- [ ] First criterion\n- [ ] Second criterion\n".into(),
            url: url.into(),
            slug: "acme/widgets".into(),
        }
    }

    /// Spec 005 F1: the shared core writes the spec, plans a tracker-stamped
    /// backlog, and stamps `spec_id` (F2) on a fresh worktree. The non-github
    /// host in the URL makes the Todo transition a hermetic parse-skip (no
    /// `gh` spawn) while still stamping through to the features.
    #[tokio::test]
    async fn ensure_spec_and_plan_writes_and_plans_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let store = fresh_store(&dir).await;
        let url = "https://example.com/acme/widgets/issues/42";
        let issue = synthetic_issue(url);

        let ensured = ensure_spec_and_plan(
            &store,
            &test_bus(),
            dir.path(),
            "42",
            &issue,
            true,
            false,
            "github",
        )
        .await
        .unwrap();
        assert!(!ensured.spec_existed);
        assert_eq!(ensured.spec_id, "42-add-widget");
        assert_eq!(
            ensured.spec_path,
            ".agentum-harness/specs/42-add-widget/spec.md"
        );
        assert!(dir.path().join(&ensured.spec_path).is_file());
        assert!(
            ensured.written.iter().any(|w| w.ends_with("spec.md")),
            "fresh spec is reported as written"
        );

        let list = ensured.features.expect("plan=true derives a backlog");
        assert_eq!(list.features.len(), 2);
        // F2: every spec-planned backlog records its spec.
        assert_eq!(list.spec_id.as_deref(), Some("42-add-widget"));
        // 004 AC 7: every feature carries the issue's tracker provenance.
        assert!(list.features.iter().all(|f| {
            f.tracker_provider.as_deref() == Some("github") && f.tracker_url.as_deref() == Some(url)
        }));
    }

    /// Spec 021 (#379): the request-pin mapping — D4: absent/`auto` stays
    /// `"github"` on this issue-driven path, explicit `linear` overrides,
    /// unknown values 400 (never a silent fallback).
    #[test]
    fn resolve_tracker_pin_maps_d4() {
        assert_eq!(resolve_tracker_pin(None).unwrap(), "github");
        assert_eq!(resolve_tracker_pin(Some("")).unwrap(), "github");
        assert_eq!(resolve_tracker_pin(Some("auto")).unwrap(), "github");
        assert_eq!(resolve_tracker_pin(Some("github")).unwrap(), "github");
        assert_eq!(resolve_tracker_pin(Some("linear")).unwrap(), "linear");
        assert!(matches!(
            resolve_tracker_pin(Some("jira")),
            Err(ApiError::BadRequest(_))
        ));
    }

    /// Spec 005 AC 1 convergence: an existing spec is not a failure for
    /// start-work (`converge_existing: true` re-plans from the existing file,
    /// never overwriting it) while the 004 route contract stays pinned
    /// (`converge_existing: false` → the 400).
    #[tokio::test]
    async fn ensure_spec_and_plan_converges_on_existing_spec() {
        let dir = tempfile::tempdir().unwrap();
        let store = fresh_store(&dir).await;
        let url = "https://example.com/acme/widgets/issues/42";
        let issue = synthetic_issue(url);

        // Pre-write a (human-edited) spec with ONE checkbox — proves the plan
        // reads the EXISTING file, not a rewrite of the issue body (two boxes).
        let spec_dir = dir.path().join(".agentum-harness/specs/42-add-widget");
        std::fs::create_dir_all(&spec_dir).unwrap();
        std::fs::write(spec_dir.join("spec.md"), "# Edited\n\n- [ ] Only one\n").unwrap();

        let err = ensure_spec_and_plan(
            &store,
            &test_bus(),
            dir.path(),
            "42",
            &issue,
            true,
            false,
            "github",
        )
        .await
        .err()
        .expect("never-overwrite 400 without converge");
        assert!(matches!(err, ApiError::BadRequest(_)), "got {err:?}");

        let ensured = ensure_spec_and_plan(
            &store,
            &test_bus(),
            dir.path(),
            "42",
            &issue,
            true,
            true,
            "github",
        )
        .await
        .unwrap();
        assert!(ensured.spec_existed);
        let list = ensured.features.unwrap();
        assert_eq!(list.features.len(), 1, "planned from the existing spec");
        assert_eq!(list.features[0].name, "Only one");
        assert_eq!(list.spec_id.as_deref(), Some("42-add-widget"));
        // The human-edited spec body was never overwritten…
        let body = std::fs::read_to_string(spec_dir.join("spec.md")).unwrap();
        assert!(body.starts_with("# Edited"));
        // …and is accordingly NOT reported as written.
        assert!(!ensured.written.iter().any(|w| w.ends_with("spec.md")));
    }

    /// Spec 005 AC 4: a successful plan fires the initial Todo transition.
    /// `AGENTUM_GH_BIN` points at a fake `gh` (the task_sink argv-logger
    /// pattern) under the crate-wide env lock; the parseable github.com URL
    /// routes the transition through it.
    #[cfg(unix)]
    #[tokio::test]
    // The awaited call must observe the env vars, so the guard has to span the
    // await — same accepted pattern as board_sync.rs's env-locked test.
    #[allow(clippy::await_holding_lock)]
    async fn ensure_spec_and_plan_fires_todo_at_plan() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let store = fresh_store(&dir).await;
        let log = dir.path().join("calls.log");
        let script = dir.path().join("gh-fake");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$@\" >> \"{}\"\nexit 0\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let url = "https://github.com/acme/widgets/issues/42";
        let issue = synthetic_issue(url);

        let guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // SAFETY: `set_var` is unsound under concurrent access; the crate-wide
        // TEST_ENV_LOCK serialises every env-mutating test. Only this test's
        // github-arm call resolves `gh_bin()` while the var is set (all other
        // tracker tests either pass the program explicitly or skip at the URL
        // parse), so the fake never leaks into a parallel test.
        //
        // AGENTUM_GITHUB_CONFIG points at an ABSENT tempdir file (spec 005
        // F5): the github arm now resolves `GithubStateMap::from_env()`, and
        // without this override a real `<data_dir>/Agentum/github.json` on the
        // dev machine would rename the asserted default `status/todo` label.
        unsafe { std::env::set_var("AGENTUM_GH_BIN", &script) };
        unsafe { std::env::set_var("AGENTUM_GITHUB_CONFIG", dir.path().join("github.json")) };
        let result = ensure_spec_and_plan(
            &store,
            &test_bus(),
            dir.path(),
            "42",
            &issue,
            true,
            false,
            "github",
        )
        .await;
        unsafe { std::env::remove_var("AGENTUM_GH_BIN") };
        unsafe { std::env::remove_var("AGENTUM_GITHUB_CONFIG") };
        drop(guard);
        result.unwrap();

        let calls = std::fs::read_to_string(&log).expect("the Todo transition ran the fake gh");
        let todo_edits: Vec<&str> = calls
            .lines()
            .filter(|l| l.starts_with("issue edit"))
            .collect();
        assert_eq!(
            todo_edits.len(),
            1,
            "exactly one transition edit, got: {calls}"
        );
        assert!(
            todo_edits[0].starts_with("issue edit 42 --repo acme/widgets --add-label status/todo"),
            "the plan-time transition targets Todo, got: {}",
            todo_edits[0]
        );
    }

    /// Spec 005 F3 (D3): the browser-QA knob defaults OFF — absence of the
    /// setting must keep `Auto`'s skip-pass byte-identical — and round-trips
    /// through the store (mirrors `mcp_master_switch_defaults_on_and_round_trips`).
    #[tokio::test]
    async fn harness_qa_setting_defaults_off_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = agentum_store::Store::open(&dir.path().join("t.db"))
            .await
            .unwrap();
        assert!(
            !store
                .setting_get_bool(BROWSER_QA_ENABLED_SETTING, false)
                .await
                .unwrap(),
            "default must be OFF (opt-in capability, D3)"
        );
        store
            .setting_set_bool(BROWSER_QA_ENABLED_SETTING, true)
            .await
            .unwrap();
        assert!(
            store
                .setting_get_bool(BROWSER_QA_ENABLED_SETTING, false)
                .await
                .unwrap()
        );
    }

    /// The GET/PUT-response wire shape is the full camelCase two-field object
    /// (spec 006 C2) — exact string so a silent field/rename regression fails
    /// loudly, matching the desktop client (`getHarnessSettings`).
    #[test]
    fn harness_settings_wire_shape_is_camel_case() {
        let json = serde_json::to_string(&HarnessSettings {
            browser_qa_agent_enabled: true,
            sdd_roles_enabled: true,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"browserQaAgentEnabled":true,"sddRolesEnabled":true}"#
        );
        let parsed: HarnessSettings =
            serde_json::from_str(r#"{"browserQaAgentEnabled":false,"sddRolesEnabled":true}"#)
                .unwrap();
        assert!(!parsed.browser_qa_agent_enabled);
        assert!(parsed.sdd_roles_enabled);
    }

    /// Spec 006 C2: the PUT body is a PATCH — a pre-006 one-field client, an
    /// empty body, and a roles-only toggle all parse (one knob can never
    /// clobber the other).
    #[test]
    fn harness_settings_patch_accepts_partial_puts() {
        let old: HarnessSettingsPatch =
            serde_json::from_str(r#"{"browserQaAgentEnabled":false}"#).unwrap();
        assert_eq!(old.browser_qa_agent_enabled, Some(false));
        assert_eq!(old.sdd_roles_enabled, None);

        let empty: HarnessSettingsPatch = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.browser_qa_agent_enabled, None);
        assert_eq!(empty.sdd_roles_enabled, None);

        let roles_only: HarnessSettingsPatch =
            serde_json::from_str(r#"{"sddRolesEnabled":false}"#).unwrap();
        assert_eq!(roles_only.browser_qa_agent_enabled, None);
        assert_eq!(roles_only.sdd_roles_enabled, Some(false));
    }

    /// Spec 006 F3 (D1): the roles knob defaults ON — absence of the setting
    /// means the SDD loop runs (NOTE the default arg differs from the QA
    /// knob's) — and round-trips through the store.
    #[tokio::test]
    async fn sdd_roles_setting_defaults_on_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = fresh_store(&dir).await;
        assert!(
            store
                .setting_get_bool(SDD_ROLES_ENABLED_SETTING, true)
                .await
                .unwrap(),
            "default must be ON (global opt-out, D1)"
        );
        store
            .setting_set_bool(SDD_ROLES_ENABLED_SETTING, false)
            .await
            .unwrap();
        assert!(
            !store
                .setting_get_bool(SDD_ROLES_ENABLED_SETTING, true)
                .await
                .unwrap()
        );
    }

    /// Spec 006 F3: `apply_start_work_knobs` stamps `roles` only when enabled,
    /// sets agent/model only when `Some`, and never touches spec_id/features.
    #[test]
    fn start_work_knobs_stamp_roles_only_when_enabled() {
        let mut list = crate::harness::FeatureList::default();
        apply_start_work_knobs(&mut list, Some("codex"), Some("gpt-9"), true);
        assert!(list.roles, "enabled stamps roles=true");
        assert_eq!(list.agent_tool, "codex");
        assert_eq!(list.agent_model.as_deref(), Some("gpt-9"));
        assert!(list.spec_id.is_none(), "spec_id untouched");
        assert!(list.features.is_empty(), "features untouched");

        let mut list = crate::harness::FeatureList::default();
        apply_start_work_knobs(&mut list, None, None, false);
        assert!(!list.roles, "disabled leaves the plan's resting false");
        assert_eq!(
            list.agent_tool,
            crate::harness::FeatureList::default().agent_tool,
            "no tool given → default kept"
        );
        assert!(list.agent_model.is_none());
    }
}
