//! agentum's own MCP server (`POST /mcp`).
//!
//! This is the "skills → MCP" foundation: instead of shipping per-agent skill
//! files, agentum exposes its capabilities as **MCP tools** that *any* agent
//! (Claude, Codex, Gemini, …) calls over the same streamable-HTTP transport it
//! already uses for Playwright. One server, agent-agnostic, app-owned logic.
//!
//! Transport: streamable-HTTP (MCP spec 2025-03-26 / 2025-06-18). We implement
//! the minimal stateless surface — `initialize`, `notifications/*` (ack only),
//! `ping`, `tools/list`, `tools/call` — and answer each JSON-RPC *request* with
//! a single `application/json` body (the spec allows this instead of an SSE
//! stream for request/response; we never push server-initiated messages, so a
//! GET SSE channel isn't needed → `GET /mcp` is 405). No session id is issued,
//! so the server is fully stateless and every agent can share it.
//!
//! Tools are backed by the same `AppState` (store, bridge, …) the REST routes
//! use — a tool is just another view over existing logic, never a reimpl.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::AppState;

/// MCP protocol version we default to when a client doesn't pin one. We echo
/// the client's requested version when present (below) for max compatibility.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

pub fn router() -> Router<AppState> {
    Router::new().route("/mcp", post(handle).get(handle_get))
}

/// We don't push server-initiated messages, so there's no SSE channel to open.
async fn handle_get() -> Response {
    StatusCode::METHOD_NOT_ALLOWED.into_response()
}

/// One JSON-RPC message in, one response out. Notifications (no `id`) are
/// acknowledged with `202 Accepted` and no body, per the streamable-HTTP spec.
async fn handle(State(state): State<AppState>, Json(msg): Json<Value>) -> Response {
    let Some(id) = msg.get("id").cloned() else {
        // notifications/initialized, notifications/cancelled, … — just ack.
        return StatusCode::ACCEPTED.into_response();
    };
    let method = msg
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = msg.get("params");

    let body = match dispatch(&state, method, params).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    };
    Json(body).into_response()
}

/// Route a JSON-RPC method to its result. `Err((code, msg))` becomes a JSON-RPC
/// *protocol* error; tool *execution* errors instead come back as a normal
/// result with `isError: true` (so the model sees them) — see [`call_tool`].
async fn dispatch(
    state: &AppState,
    method: &str,
    params: Option<&Value>,
) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => {
            // Echo the client's protocol version when it pins one, else default.
            let pv = params
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PROTOCOL_VERSION);
            Ok(json!({
                "protocolVersion": pv,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "agentum", "version": state.version },
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_specs() })),
        "tools/call" => call_tool(state, params).await,
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

/// The advertised tool catalog. Grows as skills get ported; each entry needs a
/// matching arm in [`call_tool`].
fn tool_specs() -> Value {
    json!([
        {
            "name": "agentum_list_sessions",
            "description": "List the agent sessions agentum manages on this machine \
                (each is one tmux pane running an agent CLI). Returns name, tool, \
                status, and working directory. Use to see sibling agents/worktrees.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        },
        {
            "name": "agentum_list_worktrees",
            "description": "List the git worktrees agentum tracks (isolated branch \
                checkouts under <repo>/.claude/worktrees). Returns each worktree's \
                id, repo, branch, path, and comment. Use to see what other agents \
                are working on in parallel.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        }
    ])
}

/// Execute a `tools/call`. A bad request (missing name/params) is a JSON-RPC
/// error; a tool that runs but fails returns `isError: true` in the result.
async fn call_tool(state: &AppState, params: Option<&Value>) -> Result<Value, (i64, String)> {
    let params = params.ok_or((-32602, "missing params".to_string()))?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "missing tool name".to_string()))?;

    let outcome: anyhow::Result<String> = match name {
        "agentum_list_sessions" => tool_list_sessions(state).await,
        "agentum_list_worktrees" => tool_list_worktrees().await,
        other => return Err((-32602, format!("unknown tool: {other}"))),
    };

    Ok(match outcome {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
        Err(e) => json!({
            "content": [{ "type": "text", "text": format!("tool error: {e:#}") }],
            "isError": true,
        }),
    })
}

async fn tool_list_sessions(state: &AppState) -> anyhow::Result<String> {
    let sessions = state.store.list_sessions(None).await?;
    let rows: Vec<Value> = sessions
        .iter()
        .map(|s| {
            json!({
                "id": s.id.to_string(),
                "name": s.name,
                "tool": s.tool,
                "status": format!("{:?}", s.status),
                "workdir": s.workdir,
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&Value::Array(rows))?)
}

/// Reuses the same registry reader the `/api/worktrees` route uses — a tool is
/// a second view over existing logic, never a reimplementation.
async fn tool_list_worktrees() -> anyhow::Result<String> {
    let worktrees =
        super::worktrees::read_worktrees().map_err(|e| anyhow::anyhow!("read worktrees: {e}"))?;
    Ok(serde_json::to_string_pretty(&worktrees)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_catalog_is_well_formed() {
        // Every advertised tool needs name + description + object inputSchema,
        // or agents reject the listing.
        let tools = tool_specs();
        let arr = tools.as_array().expect("tools is an array");
        assert!(!arr.is_empty());
        for t in arr {
            assert!(t.get("name").and_then(Value::as_str).is_some());
            assert!(t.get("description").and_then(Value::as_str).is_some());
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn list_sessions_is_in_the_catalog() {
        let tools = tool_specs();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .collect();
        assert!(names.contains(&"agentum_list_sessions"));
    }
}
