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

/// Does the request carry the correct `Authorization: Bearer <mcp_token>`?
/// Constant-time compare so a wrong token can't be brute-forced by timing.
fn mcp_authorized(state: &AppState, headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|tok| ct_eq(tok, state.mcp_token.as_str()))
        .unwrap_or(false)
}

/// Length-checked constant-time byte comparison (the length itself isn't secret).
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// One JSON-RPC message in, one response out. Notifications (no `id`) are
/// acknowledged with `202 Accepted` and no body, per the streamable-HTTP spec.
async fn handle(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(msg): Json<Value>,
) -> Response {
    // Token gate FIRST — before parsing the message, and required on EVERY
    // request (including the no-auth embedded server). /mcp may be reached over a
    // reverse SSH tunnel from a host where other users/processes share localhost;
    // without the bearer token they get 401.
    if !mcp_authorized(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, "missing or invalid MCP token").into_response();
    }
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
        },
        {
            "name": "agentum_send_message",
            "description": "Send a message to another agent (or a group) via agentum's \
                inter-agent mailbox. `to` is a handle (another session's name) or a \
                group: @all, @idle, @claude/@codex/… (by tool), or @worktree:<id>. \
                The recipient reads it with agentum_check_messages. Use to coordinate \
                with sibling agents — hand off work, ask a question, report status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "Recipient handle or @group" },
                    "subject": { "type": "string", "description": "Short subject line" },
                    "body": { "type": "string", "description": "Message body" },
                    "from": { "type": "string", "description": "Sender handle (your own session name, e.g. $AGENTUM_TERMINAL_HANDLE)" }
                },
                "required": ["to", "subject"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_check_messages",
            "description": "Read (and consume) your unread inter-agent messages from \
                agentum's mailbox. `recipient` is your own handle (your session name, \
                e.g. $AGENTUM_TERMINAL_HANDLE). Returns the messages and marks them read.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "recipient": { "type": "string", "description": "Your handle (session name)" },
                    "limit": { "type": "integer", "description": "Max messages to return (default 50)" }
                },
                "required": ["recipient"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_create_task",
            "description": "Create a task in agentum's orchestration DAG (the \
                orchestration skill's task system). `spec` is the task description; \
                optional `deps` (task ids that must finish first) and `parent` (id). \
                Dependents auto-promote to ready when their deps complete.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec": { "type": "string", "description": "Task description" },
                    "deps": { "type": "array", "items": { "type": "integer" }, "description": "Blocking task ids" },
                    "parent": { "type": "integer", "description": "Parent task id" }
                },
                "required": ["spec"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_list_tasks",
            "description": "List tasks in agentum's orchestration DAG. Optional `status` \
                filter and `ready` (only tasks whose dependencies are all met).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "description": "Filter by status (pending/in_progress/completed/…)" },
                    "ready": { "type": "boolean", "description": "Only dependency-ready tasks" }
                },
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_computer",
            "description": "Control a local desktop app via agentum's computer-use \
                (macOS accessibility) — the computer-use skill. Pass `op` and its \
                params: capabilities | permissions | list-apps | get-app-state \
                (app) | click (app, ...) | set-value | type-text (text) | press-key \
                (key) | scroll. Requires the agentum desktop app (not the headless \
                daemon).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": { "type": "string", "description": "capabilities|permissions|list-apps|get-app-state|click|set-value|type-text|press-key|scroll" }
                },
                "required": ["op"],
                "additionalProperties": true,
            },
        },
        {
            "name": "agentum_browser",
            "description": "Drive agentum's built-in browser webview — the agentum-cli \
                browser skill. Pass `op` and its params: open (url) — opens a NEW tab \
                navigated to url and returns its `tab` id; tabs — lists open tabs; \
                navigate (url) | snapshot | click (selector) | fill (selector, text) | \
                screenshot — act on a tab (optional `tab` id, else the active one); \
                annotations — read the design-feedback annotations the user marked on \
                page elements (returns structured markdown the agent can act on); \
                grab (selector) — extract an element's metadata (tag, text, selector, \
                rect, computed styles) by CSS selector; \
                annotate (selector, comment, intent?) — attach a design-feedback \
                annotation to an element (intent: change|fix|question|approve), which \
                shows in the browser tray and is returned by `annotations`. \
                Start with `open` when no tab is listed by `tabs`. Requires the agentum \
                desktop app. (For headless browser automation an agent should use the \
                Playwright MCP instead.)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": { "type": "string", "description": "open|tabs|navigate|snapshot|click|fill|screenshot|annotations|grab|annotate" },
                    "url": { "type": "string", "description": "Target URL for `open`/`navigate`" },
                    "tab": { "type": "string", "description": "Tab id to act on (default: the active tab)" },
                    "selector": { "type": "string", "description": "CSS selector for `click`/`fill`/`grab`/`annotate`" },
                    "text": { "type": "string", "description": "Text to type for `fill`" },
                    "comment": { "type": "string", "description": "Annotation feedback text for `annotate`" },
                    "intent": { "type": "string", "description": "Annotation intent for `annotate`: change|fix|question|approve" }
                },
                "required": ["op"],
                "additionalProperties": true,
            },
        },
        {
            "name": "agentum_harness_scaffold",
            "description": "Scaffold the unified `.agentum-harness/` surface into a \
                project (spec 010). Writes ONLY `.agentum-harness/` (a small AGENTS.md \
                router, feature_list.json, init.sh, verify.sh) into `workdir` — no \
                generic playbooks/engine are copied in. Idempotent; the folder is \
                committable to git.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workdir": { "type": "string", "description": "Project directory to scaffold" }
                },
                "required": ["workdir"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_harness_migrate",
            "description": "Migrate a pre-010 project into `.agentum-harness/` without \
                hand-rewrite: copies SDD `ai/specs/*` and any legacy `.harness/` contract \
                files into `.agentum-harness/`. Idempotent; pass `remove_legacy: true` to \
                delete the old `.harness/` after copying.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workdir": { "type": "string", "description": "Project directory to migrate" },
                    "remove_legacy": { "type": "boolean", "description": "Delete legacy .harness/ after copying (default false)" }
                },
                "required": ["workdir"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_harness_board",
            "description": "Reconstruct a project's harness board by scanning \
                `.agentum-harness/` on disk (spec 010b) — the spec deliverables under \
                specs/* and the active feature_list.json states. Pure read; the repo is \
                the durable source of truth, so this rebuilds the board with no agentum \
                store consulted (survives a store wipe). Empty when there's no surface.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workdir": { "type": "string", "description": "Project directory to scan" }
                },
                "required": ["workdir"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_harness_plan",
            "description": "Turn an authored spec into the engine's verify-gated backlog \
                (spec 010c): reads `.agentum-harness/specs/<spec_id>/spec.md`, maps each \
                acceptance-criteria checkbox (`- [ ]`/`- [x]`) to a feature, and writes \
                `.agentum-harness/feature_list.json`. Deterministic; errors if the spec \
                has no criteria.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workdir": { "type": "string", "description": "Project directory" },
                    "spec_id": { "type": "string", "description": "Spec dir under .agentum-harness/specs/ (e.g. 010a-agentum-harness-surface)" }
                },
                "required": ["workdir", "spec_id"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_harness_check",
            "description": "Bootstrap-Contract readiness check (spec 010d): scan \
                `.agentum-harness/` and report whether it can start (init.sh), can verify \
                (verify.sh), has instructions (AGENTS.md), and has a non-empty backlog — \
                plus an overall `ready`. Names what's missing. Pure read; the mechanized \
                cold-start test.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workdir": { "type": "string", "description": "Project directory to check" }
                },
                "required": ["workdir"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_harness_log_decision",
            "description": "Append one entry to the project's append-only decision log \
                (`.agentum-harness/decisions.md`, spec 010e) — the durable 'why', incl. \
                rejected alternatives. Never overwrites prior entries. Returns the updated \
                log. Use to record a deliberate choice so a resumed session won't reverse it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workdir": { "type": "string", "description": "Project directory" },
                    "entry": { "type": "string", "description": "The decision (one line; include the why + any rejected alternative)" }
                },
                "required": ["workdir", "entry"],
                "additionalProperties": false,
            },
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

    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let outcome: anyhow::Result<String> = match name {
        "agentum_list_sessions" => tool_list_sessions(state).await,
        "agentum_list_worktrees" => tool_list_worktrees().await,
        "agentum_send_message" => tool_send_message(state, &args).await,
        "agentum_check_messages" => tool_check_messages(state, &args).await,
        "agentum_create_task" => tool_create_task(state, &args).await,
        "agentum_list_tasks" => tool_list_tasks(state, &args).await,
        "agentum_computer" => tool_bridge(state, "computer", &args).await,
        "agentum_browser" => tool_bridge(state, "browser", &args).await,
        "agentum_harness_scaffold" => tool_harness_scaffold(&args).await,
        "agentum_harness_migrate" => tool_harness_migrate(&args).await,
        "agentum_harness_board" => tool_harness_board(&args).await,
        "agentum_harness_plan" => tool_harness_plan(&args).await,
        "agentum_harness_check" => tool_harness_check(&args).await,
        "agentum_harness_log_decision" => tool_harness_log_decision(&args).await,
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

/// Send to another agent / group, reusing the orchestration route's recipient
/// resolution + the store's mailbox insert (same path as `agentum orchestration
/// send`). The `orchestration` skill becomes this tool.
async fn tool_send_message(state: &AppState, args: &Value) -> anyhow::Result<String> {
    use agentum_store::orchestration::NewOrchMessage;

    let to = args
        .get("to")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `to`"))?;
    let subject = args
        .get("subject")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `subject`"))?;
    let from = args.get("from").and_then(Value::as_str).unwrap_or("agent");
    let body = args.get("body").and_then(Value::as_str).unwrap_or("");

    let infos = super::orchestration::handle_infos(state)
        .await
        .map_err(|e| anyhow::anyhow!("resolve handles: {e}"))?;
    let recipients = super::orchestration::resolve_recipients(to, &infos, from);
    if recipients.is_empty() {
        anyhow::bail!("`{to}` resolved to no recipients");
    }

    // One thread for the fan-out so a group conversation stays linked.
    let thread_id = uuid::Uuid::new_v4().to_string();
    for recipient in &recipients {
        state
            .store
            .orch_insert_message(&NewOrchMessage {
                thread_id: thread_id.clone(),
                sender: from.to_string(),
                recipient: recipient.clone(),
                subject: subject.to_string(),
                body: body.to_string(),
                msg_type: "status".to_string(),
                priority: "normal".to_string(),
                payload: None,
            })
            .await?;
    }
    Ok(serde_json::to_string_pretty(
        &json!({ "thread_id": thread_id, "delivered_to": recipients }),
    )?)
}

/// Read + consume the caller's unread mailbox (same as `agentum orchestration
/// check`). Marks the returned messages read.
async fn tool_check_messages(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let recipient = args
        .get("recipient")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `recipient`"))?;
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50);

    let msgs = state.store.orch_inbox(recipient, true, &[], limit).await?;
    let ids: Vec<i64> = msgs.iter().map(|m| m.id).collect();
    if !ids.is_empty() {
        state.store.orch_mark_read(&ids).await?;
    }
    Ok(serde_json::to_string_pretty(&json!({ "messages": msgs }))?)
}

/// Create an orchestration-DAG task (same store call as the
/// `/api/orchestration/tasks` route).
async fn tool_create_task(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let spec = args
        .get("spec")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `spec`"))?;
    let deps: Vec<i64> = args
        .get("deps")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    let parent = args.get("parent").and_then(Value::as_i64);
    let task = state.store.orch_create_task(spec, &deps, parent).await?;
    Ok(serde_json::to_string_pretty(&json!({ "task": task }))?)
}

/// List orchestration-DAG tasks (same store call as the route).
async fn tool_list_tasks(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let status = args.get("status").and_then(Value::as_str);
    let ready = args.get("ready").and_then(Value::as_bool).unwrap_or(false);
    let tasks = state.store.orch_list_tasks(status, ready).await?;
    Ok(serde_json::to_string_pretty(&json!({ "tasks": tasks }))?)
}

/// Forward a `{op, …}` payload to the desktop bridge (`computer`/`browser`).
/// These ops only exist in the agentum desktop app — the headless daemon has no
/// bridge, so return a clear, actionable error there rather than a silent empty.
async fn tool_bridge(state: &AppState, kind: &str, args: &Value) -> anyhow::Result<String> {
    let bridge = state.desktop_bridge.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "agentum_{kind} requires the agentum desktop app (this is a headless daemon \
             with no desktop bridge)"
        )
    })?;
    let result = match kind {
        "computer" => bridge.computer(args.clone()).await,
        "browser" => bridge.browser(args.clone()).await,
        other => anyhow::bail!("unknown bridge kind: {other}"),
    }?;
    Ok(serde_json::to_string_pretty(&result)?)
}

/// Scaffold the unified `.agentum-harness/` surface — a thin view over
/// [`crate::harness::scaffold_harness`] (the only thing agentum writes into a repo).
async fn tool_harness_scaffold(args: &Value) -> anyhow::Result<String> {
    let raw = args
        .get("workdir")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `workdir`"))?;
    let workdir =
        super::util::expand_workdir(raw).map_err(|e| anyhow::anyhow!("invalid workdir: {e:?}"))?;
    let out = crate::harness::scaffold_harness(&workdir).await?;
    Ok(serde_json::to_string_pretty(&out)?)
}

/// Migrate a pre-010 project into `.agentum-harness/` — thin view over
/// [`crate::harness::migrate_harness`].
async fn tool_harness_migrate(args: &Value) -> anyhow::Result<String> {
    let raw = args
        .get("workdir")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `workdir`"))?;
    let workdir =
        super::util::expand_workdir(raw).map_err(|e| anyhow::anyhow!("invalid workdir: {e:?}"))?;
    let remove_legacy = args
        .get("remove_legacy")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let out = crate::harness::migrate_harness(&workdir, remove_legacy).await?;
    Ok(serde_json::to_string_pretty(&out)?)
}

/// Reconstruct a project's harness board by scanning `.agentum-harness/` — a thin
/// view over [`crate::harness::scan_board`] (spec 010b; the rebuildable index).
async fn tool_harness_board(args: &Value) -> anyhow::Result<String> {
    let raw = args
        .get("workdir")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `workdir`"))?;
    let workdir =
        super::util::expand_workdir(raw).map_err(|e| anyhow::anyhow!("invalid workdir: {e:?}"))?;
    let board = crate::harness::scan_board(&workdir).await;
    Ok(serde_json::to_string_pretty(&board)?)
}

/// Turn a spec into the engine backlog — a thin view over
/// [`crate::harness::plan_from_spec`] (spec 010c).
async fn tool_harness_plan(args: &Value) -> anyhow::Result<String> {
    let raw = args
        .get("workdir")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `workdir`"))?;
    let spec_id = args
        .get("spec_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `spec_id`"))?;
    let workdir =
        super::util::expand_workdir(raw).map_err(|e| anyhow::anyhow!("invalid workdir: {e:?}"))?;
    let list = crate::harness::plan_from_spec(&workdir, spec_id).await?;
    Ok(serde_json::to_string_pretty(&list)?)
}

/// Bootstrap-Contract readiness — a thin view over [`crate::harness::check_bootstrap`]
/// (spec 010d).
async fn tool_harness_check(args: &Value) -> anyhow::Result<String> {
    let raw = args
        .get("workdir")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `workdir`"))?;
    let workdir =
        super::util::expand_workdir(raw).map_err(|e| anyhow::anyhow!("invalid workdir: {e:?}"))?;
    let report = crate::harness::check_bootstrap(&workdir).await;
    Ok(serde_json::to_string_pretty(&report)?)
}

/// Append to the project decision log + return it — thin view over
/// [`crate::harness::append_decision`] / [`crate::harness::read_decisions`] (010e).
async fn tool_harness_log_decision(args: &Value) -> anyhow::Result<String> {
    let raw = args
        .get("workdir")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `workdir`"))?;
    let entry = args
        .get("entry")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `entry`"))?;
    let workdir =
        super::util::expand_workdir(raw).map_err(|e| anyhow::anyhow!("invalid workdir: {e:?}"))?;
    crate::harness::append_decision(&workdir, entry).await?;
    Ok(crate::harness::read_decisions(&workdir).await)
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
