//! `/api/agents` — runtime probe of which first-class agent binaries are
//! actually installed on the daemon's `PATH`.
//!
//! The TUI and dashboard hit this once on startup / dialog-open so that
//! the agent picker can mark unavailable entries with a clear hint
//! instead of silently letting the user spawn a session that will crash
//! with `command not found` on `tmux send-keys`.
//!
//! The probe is intentionally read-only and fast: a single `which` call
//! per first-class tool. Unknown / passthrough tools are not included —
//! callers that allow free-form tool input should accept anything; this
//! endpoint only describes the curated palette.

use agentum_core::{HostKind, LOCAL_HOST_ID};
use agentum_executor::{adapter_for, binary_for, probed_tools};
use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::routing::get;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/agents", get(list_agents))
}

#[derive(Serialize)]
pub struct AgentInfo {
    /// Stable tool id used by `Session::tool` and the `--tool` CLI arg.
    pub name: String,
    /// Binary the adapter actually launches. Usually equals `name` but
    /// disagrees when the headless CLI ships under a different command
    /// (e.g. `cursor` ↦ `cursor-agent`).
    pub binary: String,
    /// True when `binary` resolves on the daemon's `PATH`.
    pub available: bool,
    /// Tool-specific YOLO flag, mirrored from the adapter trait so the
    /// dashboard can preview it on the toggle without duplicating the
    /// per-tool table client-side. `None` ⇒ the toggle is a no-op for
    /// this tool.
    pub yolo_flag: Option<String>,
    /// Optional resolved absolute path of the binary. Only populated
    /// when `available`. Useful for the doctor view; safe to surface
    /// since we already trust the user's PATH.
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentsQuery {
    host_id: Option<Uuid>,
}

async fn list_agents(
    State(state): State<AppState>,
    Query(q): Query<AgentsQuery>,
) -> Result<Json<Vec<AgentInfo>>, ApiError> {
    let host_id = q.host_id.unwrap_or(LOCAL_HOST_ID);
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("unknown host: {host_id}")))?;
    // `which` blocks on filesystem I/O, but the per-tool cost is a few
    // microseconds for cached PATH entries — keeping this synchronous
    // is simpler than spawning a blocking task and the endpoint is
    // already off the hot path.
    let mut out = Vec::new();
    for name in probed_tools() {
        let bin = binary_for(name);
        let resolved = match &host.kind {
            HostKind::Local => which::which(bin).ok().map(|p| p.display().to_string()),
            HostKind::Ssh { .. } => remote_command_path(&host, bin).await,
        };
        let adapter = adapter_for(name);
        out.push(AgentInfo {
            name: name.to_string(),
            binary: bin.to_string(),
            available: resolved.is_some(),
            yolo_flag: adapter.yolo_flag().map(|s| s.to_string()),
            path: resolved,
        });
    }
    Ok(Json(out))
}

async fn remote_command_path(host: &agentum_core::Host, bin: &str) -> Option<String> {
    let quoted = shlex::try_quote(bin).ok()?;
    crate::host_runtime::ssh_stdout(host, &format!("command -v {quoted}"))
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
