//! `/api/orchestration/*` — inter-agent mail store, task DAG, and dispatch.
//! Backs the `agentum orchestration` CLI. A *handle* is a session name (also
//! injected into panes as `AGENTUM_TERMINAL_HANDLE`); group addresses
//! (`@all`/`@idle`/`@claude`/…/`@worktree:<id>`) resolve to concrete handles at
//! send time and fan out to one stored message per recipient.

use agentum_store::orchestration::{NewOrchMessage, OrchMessage};
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/orchestration/messages", post(send))
        .route("/api/orchestration/check", post(check))
        .route("/api/orchestration/reply", post(reply))
        .route("/api/orchestration/inbox", get(inbox))
        .route(
            "/api/orchestration/tasks",
            get(list_tasks).post(create_task),
        )
        .route("/api/orchestration/tasks/{id}/status", post(update_task))
        .route(
            "/api/orchestration/dispatch",
            get(dispatch_show).post(dispatch),
        )
}

/// Minimal session facts the recipient resolver needs. Decoupled from the full
/// Session so `resolve_recipients` is a pure, unit-testable function.
#[derive(Debug, Clone)]
pub struct HandleInfo {
    pub name: String,
    pub tool: String,
    pub status: String,
    pub workdir: String,
}

/// Resolve a `--to` target into concrete recipient handles. A plain handle
/// resolves to itself (delivered even if no live session matches, so a handle
/// that is briefly offline still gets mail). A group fans out:
/// `@all` (everyone but the sender), `@idle`, `@<tool>` (claude/codex/…), and
/// `@worktree:<id>` (a workdir-substring match). Unknown groups resolve to none.
pub fn resolve_recipients(target: &str, sessions: &[HandleInfo], sender: &str) -> Vec<String> {
    if let Some(group) = target.strip_prefix('@') {
        let pick = |f: &dyn Fn(&HandleInfo) -> bool| -> Vec<String> {
            sessions
                .iter()
                .filter(|s| s.name != sender && f(s))
                .map(|s| s.name.clone())
                .collect()
        };
        match group {
            "all" => pick(&|_| true),
            "idle" => pick(&|s| s.status == "idle"),
            "claude" | "codex" | "gemini" | "opencode" | "cursor" | "hermes" | "aider" => {
                let tool = group.to_string();
                pick(&|s| s.tool == tool)
            }
            other => {
                if let Some(wt) = other.strip_prefix("worktree:") {
                    let wt = wt.to_string();
                    pick(&|s| s.workdir.contains(&wt))
                } else {
                    Vec::new()
                }
            }
        }
    } else {
        vec![target.to_string()]
    }
}

pub(crate) async fn handle_infos(state: &AppState) -> Result<Vec<HandleInfo>, ApiError> {
    let sessions = state.store.list_sessions(None).await?;
    Ok(sessions
        .into_iter()
        .map(|s| HandleInfo {
            name: s.name,
            tool: s.tool,
            status: s.status.as_str().to_string(),
            workdir: s.workdir,
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct SendReq {
    to: String,
    subject: String,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    payload: Option<Value>,
}

async fn send(
    State(state): State<AppState>,
    Json(req): Json<SendReq>,
) -> Result<Json<Value>, ApiError> {
    let sender = req.from.clone().unwrap_or_else(|| "unknown".to_string());
    let infos = handle_infos(&state).await?;
    let recipients = resolve_recipients(&req.to, &infos, &sender);
    if recipients.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "`{}` resolved to no recipients",
            req.to
        )));
    }
    // One shared thread for the fan-out so a group conversation stays linked.
    let thread_id = req
        .thread_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let payload = req.payload.as_ref().map(|p| p.to_string());

    let mut delivered: Vec<OrchMessage> = Vec::with_capacity(recipients.len());
    for recipient in recipients {
        let m = state
            .store
            .orch_insert_message(&NewOrchMessage {
                thread_id: thread_id.clone(),
                sender: sender.clone(),
                recipient,
                subject: req.subject.clone(),
                body: req.body.clone().unwrap_or_default(),
                msg_type: req.msg_type.clone().unwrap_or_else(|| "status".into()),
                priority: req.priority.clone().unwrap_or_else(|| "normal".into()),
                payload: payload.clone(),
            })
            .await?;
        delivered.push(m);
    }
    Ok(Json(
        json!({ "thread_id": thread_id, "delivered": delivered }),
    ))
}

#[derive(Debug, Deserialize)]
struct CheckReq {
    recipient: String,
    #[serde(default)]
    unread: Option<bool>,
    #[serde(default)]
    types: Option<Vec<String>>,
    #[serde(default)]
    mark_read: Option<bool>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn check(
    State(state): State<AppState>,
    Json(req): Json<CheckReq>,
) -> Result<Json<Value>, ApiError> {
    let types = req.types.unwrap_or_default();
    let msgs = state
        .store
        .orch_inbox(
            &req.recipient,
            req.unread.unwrap_or(true),
            &types,
            req.limit.unwrap_or(50),
        )
        .await?;
    // `check` consumes by default: mark the returned messages read unless the
    // caller asked to peek (mark_read = false).
    if req.mark_read.unwrap_or(true) && !msgs.is_empty() {
        let ids: Vec<i64> = msgs.iter().map(|m| m.id).collect();
        state.store.orch_mark_read(&ids).await?;
    }
    Ok(Json(json!({ "messages": msgs })))
}

#[derive(Debug, Deserialize)]
struct InboxQuery {
    recipient: String,
    #[serde(default)]
    unread: Option<bool>,
    #[serde(default)]
    limit: Option<i64>,
}

/// Non-consuming list (never marks read), for `agentum orchestration inbox`.
async fn inbox(
    State(state): State<AppState>,
    Query(q): Query<InboxQuery>,
) -> Result<Json<Value>, ApiError> {
    let msgs = state
        .store
        .orch_inbox(
            &q.recipient,
            q.unread.unwrap_or(false),
            &[],
            q.limit.unwrap_or(50),
        )
        .await?;
    Ok(Json(json!({ "messages": msgs })))
}

#[derive(Debug, Deserialize)]
struct ReplyReq {
    id: i64,
    body: String,
    #[serde(default)]
    from: Option<String>,
}

async fn reply(
    State(state): State<AppState>,
    Json(req): Json<ReplyReq>,
) -> Result<Json<Value>, ApiError> {
    let orig = state
        .store
        .orch_get_message(req.id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("message {}", req.id)))?;
    // A reply goes back to the original sender, on the same thread.
    let from = req.from.clone().unwrap_or_else(|| orig.recipient.clone());
    let m = state
        .store
        .orch_insert_message(&NewOrchMessage {
            thread_id: orig.thread_id.clone(),
            sender: from,
            recipient: orig.sender.clone(),
            subject: format!("Re: {}", orig.subject),
            body: req.body.clone(),
            msg_type: "status".into(),
            priority: "normal".into(),
            payload: None,
        })
        .await?;
    Ok(Json(json!({ "message": m })))
}

#[derive(Debug, Deserialize)]
struct TaskCreateReq {
    spec: String,
    #[serde(default)]
    deps: Option<Vec<i64>>,
    #[serde(default)]
    parent: Option<i64>,
}

async fn create_task(
    State(state): State<AppState>,
    Json(req): Json<TaskCreateReq>,
) -> Result<Json<Value>, ApiError> {
    let task = state
        .store
        .orch_create_task(&req.spec, &req.deps.unwrap_or_default(), req.parent)
        .await?;
    Ok(Json(json!({ "task": task })))
}

#[derive(Debug, Deserialize)]
struct TaskListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    ready: Option<bool>,
}

async fn list_tasks(
    State(state): State<AppState>,
    Query(q): Query<TaskListQuery>,
) -> Result<Json<Value>, ApiError> {
    let tasks = state
        .store
        .orch_list_tasks(q.status.as_deref(), q.ready.unwrap_or(false))
        .await?;
    Ok(Json(json!({ "tasks": tasks })))
}

#[derive(Debug, Deserialize)]
struct TaskUpdateReq {
    status: String,
    #[serde(default)]
    result: Option<Value>,
}

async fn update_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<TaskUpdateReq>,
) -> Result<Json<Value>, ApiError> {
    let result = req.result.as_ref().map(|r| r.to_string());
    let task = state
        .store
        .orch_update_task(id, &req.status, result.as_deref())
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("task {id}")))?;
    Ok(Json(json!({ "task": task })))
}

#[derive(Debug, Deserialize)]
struct DispatchReq {
    task: i64,
    to: String,
    #[serde(default)]
    from: Option<String>,
}

async fn dispatch(
    State(state): State<AppState>,
    Json(req): Json<DispatchReq>,
) -> Result<Json<Value>, ApiError> {
    // Resolve the assignee to a concrete handle (a group would be ambiguous for
    // a single dispatch, so take the first match).
    let infos = handle_infos(&state).await?;
    let sender = req.from.clone().unwrap_or_else(|| "coordinator".into());
    let assignee = resolve_recipients(&req.to, &infos, &sender)
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::BadRequest(format!("`{}` resolved to no handle", req.to)))?;
    let d = state
        .store
        .orch_create_dispatch(req.task, &assignee)
        .await?;
    let task = state.store.orch_get_task(req.task).await?;
    Ok(Json(json!({ "dispatch": d, "task": task })))
}

#[derive(Debug, Deserialize)]
struct DispatchShowQuery {
    task: i64,
}

async fn dispatch_show(
    State(state): State<AppState>,
    Query(q): Query<DispatchShowQuery>,
) -> Result<Json<Value>, ApiError> {
    let dispatches = state.store.orch_dispatches_for_task(q.task).await?;
    let task = state.store.orch_get_task(q.task).await?;
    Ok(Json(json!({ "task": task, "dispatches": dispatches })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn infos() -> Vec<HandleInfo> {
        let mk = |name: &str, tool: &str, status: &str, workdir: &str| HandleInfo {
            name: name.into(),
            tool: tool.into(),
            status: status.into(),
            workdir: workdir.into(),
        };
        vec![
            mk("coord", "claude", "running", "/repo"),
            mk("worker-a", "claude", "idle", "/repo/wt-a"),
            mk("worker-b", "codex", "idle", "/repo/wt-b"),
        ]
    }

    #[test]
    fn plain_handle_resolves_to_itself() {
        assert_eq!(
            resolve_recipients("worker-a", &infos(), "coord"),
            vec!["worker-a"]
        );
        // Even an unknown handle is delivered (it may come online later).
        assert_eq!(
            resolve_recipients("ghost", &infos(), "coord"),
            vec!["ghost"]
        );
    }

    #[test]
    fn group_all_excludes_sender() {
        let mut got = resolve_recipients("@all", &infos(), "coord");
        got.sort();
        assert_eq!(got, vec!["worker-a", "worker-b"]);
    }

    #[test]
    fn group_by_tool_and_status_and_worktree() {
        // @claude → claude sessions except sender.
        assert_eq!(
            resolve_recipients("@claude", &infos(), "coord"),
            vec!["worker-a"]
        );
        // @idle → idle sessions.
        let mut idle = resolve_recipients("@idle", &infos(), "coord");
        idle.sort();
        assert_eq!(idle, vec!["worker-a", "worker-b"]);
        // @worktree:wt-b → workdir substring match.
        assert_eq!(
            resolve_recipients("@worktree:wt-b", &infos(), "coord"),
            vec!["worker-b"]
        );
        // Unknown group → nothing.
        assert!(resolve_recipients("@nope", &infos(), "coord").is_empty());
    }
}
