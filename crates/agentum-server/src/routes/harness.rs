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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/harness", get(list).post(start))
        .route("/api/harness/events", get(events))
        .route("/api/harness/spec-from-issue", post(spec_from_issue))
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

/// Wire shape of `/api/harness/settings` — the engine-wide run-behavior knobs
/// the desktop Settings pane reflects (today just the browser-QA capability).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarnessSettings {
    browser_qa_agent_enabled: bool,
}

/// `GET /api/harness/settings` — is the browser-QA agent capable without the
/// env flag? (default off, D3). Mirrors `routes/mcp.rs`'s settings route.
async fn get_settings(
    State(state): State<AppState>,
) -> Result<axum::Json<HarnessSettings>, ApiError> {
    let enabled = state
        .store
        .setting_get_bool(BROWSER_QA_ENABLED_SETTING, false)
        .await?;
    Ok(axum::Json(HarnessSettings {
        browser_qa_agent_enabled: enabled,
    }))
}

/// `PUT /api/harness/settings` — flip the browser-QA capability switch. Read
/// per gate decision in `drive_inner`, so it applies to the next QA gate with
/// no restart.
async fn put_settings(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<HarnessSettings>,
) -> Result<axum::Json<HarnessSettings>, ApiError> {
    state
        .store
        .setting_set_bool(BROWSER_QA_ENABLED_SETTING, req.browser_qa_agent_enabled)
        .await?;
    Ok(axum::Json(req))
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
/// scaffold's keep-existing ethos.
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
    let issue =
        super::github::fetch_github_issue(&state, &workdir_str, &req.number, req.slug.as_deref())
            .await?;

    // Idempotent: existing contract files are kept; only missing ones written.
    let scaffold = crate::harness::scaffold_harness(&workdir)
        .await
        .map_err(|e| ApiError::Internal(format!("could not scaffold the harness: {e}")))?;

    let spec_id = crate::harness::issue_spec_id(req.number.trim(), &issue.title);
    let rel_spec_path = format!("{}/specs/{spec_id}/spec.md", crate::harness::HARNESS_DIR);
    let spec_md_path = workdir.join(&rel_spec_path);
    if spec_md_path.exists() {
        return Err(ApiError::BadRequest(format!(
            "spec {spec_id} already exists — not overwriting {rel_spec_path}"
        )));
    }
    if let Some(parent) = spec_md_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::Internal(format!("could not create the spec dir: {e}")))?;
    }
    let spec_md = crate::harness::spec_md_from_issue(
        req.number.trim(),
        &issue.title,
        &issue.body,
        &issue.url,
    );
    tokio::fs::write(&spec_md_path, spec_md)
        .await
        .map_err(|e| ApiError::Internal(format!("could not write spec.md: {e}")))?;

    let mut written = scaffold.written;
    written.push(rel_spec_path.clone());

    // The transform guarantees ≥1 checkbox, so a plan failure here is IO-class.
    let features = if req.plan {
        let list =
            crate::harness::plan_from_spec_with_tracker(&workdir, &spec_id, "github", &issue.url)
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

    Ok(axum::Json(SpecFromIssueResponse {
        spec_id,
        spec_path: rel_spec_path,
        written,
        features,
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The wire shape is `{"browserQaAgentEnabled": bool}` — camelCase, matching
    /// the desktop client (`getHarnessSettings`/`setHarnessSettings`).
    #[test]
    fn harness_settings_wire_shape_is_camel_case() {
        let json = serde_json::to_string(&HarnessSettings {
            browser_qa_agent_enabled: true,
        })
        .unwrap();
        assert_eq!(json, r#"{"browserQaAgentEnabled":true}"#);
        let parsed: HarnessSettings =
            serde_json::from_str(r#"{"browserQaAgentEnabled":false}"#).unwrap();
        assert!(!parsed.browser_qa_agent_enabled);
    }
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
