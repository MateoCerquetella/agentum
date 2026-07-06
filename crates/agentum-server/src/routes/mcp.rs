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
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AppState;
use crate::error::ApiError;

/// MCP protocol version we default to when a client doesn't pin one. We echo
/// the client's requested version when present (below) for max compatibility.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// Server-level guidance surfaced to every connecting agent via the MCP
/// `initialize` `instructions` field. Tailored by whether this is the desktop app
/// (a browser the user can SEE exists) or a headless daemon. The browser steer is
/// the important part: agents otherwise default to claude-in-chrome / the `agentum`
/// shell CLI / Playwright and drive a browser the user can't see — or hit "no
/// browser tab open" because the shell CLI has no tab-create. `agentum_browser`'s
/// `open` is the only thing that creates a VISIBLE tab.
fn mcp_instructions(has_desktop_bridge: bool) -> &'static str {
    if has_desktop_bridge {
        "agentum exposes this desktop app's control plane as MCP tools.\n\n\
         BROWSER: for ANY web/browser task, use the `agentum_browser` tool — it drives the \
         browser the user is watching live in this app. Open a page with op `open` and a `url` \
         (this creates a VISIBLE tab and returns its id); add `split:\"right\"` (or left/up/down) \
         to place the browser BESIDE the agent; then drive it with navigate / click / fill / \
         snapshot / screenshot. Do NOT use claude-in-chrome, Playwright, chrome-devtools, or the \
         `agentum` shell CLI for browser work in this app — they drive a browser the user cannot \
         see, or cannot open a tab. `agentum_browser` is the first (and only) browser tool to \
         reach for here; start with `open`.\n\n\
         Other tools: agentum_list_sessions / agentum_list_worktrees inspect this app's agents; \
         agentum_send_message / agentum_check_messages are the agent mailbox."
    } else {
        "agentum exposes this server's control plane as MCP tools. For browser automation use the \
         `agentum_browser` tool (it drives a server-side headless Chromium here — there is no GUI \
         on this daemon). agentum_list_sessions / agentum_list_worktrees inspect agents; \
         agentum_send_message / agentum_check_messages are the agent mailbox."
    }
}

/// Master switch (default ON) for whether agentum's own MCP server is wired into
/// the agents agentum launches. Read at provision time in `mcp_provision::provision`;
/// written by the desktop Settings → Agent MCP toggle via `/api/mcp/settings`.
/// When off, NO agentum tools (sessions, worktrees, browser, computer,
/// orchestration, harness) reach any agent. Absent = on, so existing setups are
/// unchanged. (The orchestration gate still nests under this.)
pub const MCP_ENABLED_SETTING: &str = "mcp.enabled";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/mcp", post(handle).get(handle_get))
        .route(
            "/api/mcp/settings",
            get(get_mcp_settings).put(put_mcp_settings),
        )
}

/// The agentum-MCP master-switch state — what the desktop toggle reflects and
/// what `mcp_provision::provision` reads to decide whether to wire the server.
#[derive(Serialize)]
struct McpSettings {
    enabled: bool,
}

#[derive(Deserialize)]
struct McpSettingsReq {
    enabled: bool,
}

/// `GET /api/mcp/settings` — is agentum's MCP wired into agents? (default on)
async fn get_mcp_settings(State(state): State<AppState>) -> Result<Json<McpSettings>, ApiError> {
    let enabled = state
        .store
        .setting_get_bool(MCP_ENABLED_SETTING, true)
        .await?;
    Ok(Json(McpSettings { enabled }))
}

/// `PUT /api/mcp/settings` — flip the master switch. Takes effect on the next
/// agent launch (provisioning is launch-time, unlike the per-call orchestration
/// gate), so already-running agents keep the tools they were launched with.
async fn put_mcp_settings(
    State(state): State<AppState>,
    Json(req): Json<McpSettingsReq>,
) -> Result<Json<McpSettings>, ApiError> {
    state
        .store
        .setting_set_bool(MCP_ENABLED_SETTING, req.enabled)
        .await?;
    Ok(Json(McpSettings {
        enabled: req.enabled,
    }))
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
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
    headers: axum::http::HeaderMap,
    Json(mut msg): Json<Value>,
) -> Response {
    // Token gate FIRST — before parsing the message, and required on EVERY
    // request (including the no-auth embedded server). /mcp may be reached over a
    // reverse SSH tunnel from a host where other users/processes share localhost;
    // without the bearer token they get 401.
    if !mcp_authorized(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, "missing or invalid MCP token").into_response();
    }
    // Per-worktree browser: an agent's MCP URL carries `?worktree=<id>` (appended at
    // spawn). Thread it into `agentum_browser` calls as `worktreeId` so the agent's
    // CDP ops hit ITS worktree browser — the same instance the user's pane watches.
    // Resolution is gated server-side (off → shared browser), so this is a harmless
    // tag when isolation is disabled or no worktree is present.
    if let Some(wt) = query
        .get("worktree")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        if msg.get("method").and_then(Value::as_str) == Some("tools/call") {
            if let Some(params) = msg.get_mut("params") {
                if params.get("name").and_then(Value::as_str) == Some("agentum_browser") {
                    if let Some(a) = params.get_mut("arguments").and_then(Value::as_object_mut) {
                        if !a.contains_key("worktreeId") && !a.contains_key("cdpPort") {
                            a.insert("worktreeId".to_string(), Value::from(wt.to_string()));
                        }
                    }
                }
            }
        }
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
                // Top-level guidance surfaced to every connecting agent. The browser
                // steer is the point: without it agents reach for claude-in-chrome /
                // the `agentum` shell CLI / Playwright and either drive a browser the
                // user can't see or hit "no browser tab open" (the CLI has no tab-create).
                "instructions": mcp_instructions(state.desktop_bridge.is_some()),
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_specs(orchestration_enabled(state).await) })),
        "tools/call" => call_tool(state, params).await,
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

/// The inter-agent orchestration tools (mailbox + task DAG). Gated behind the
/// `orchestration.enabled` setting: when off they are neither advertised
/// (`tools/list`) nor callable (`tools/call`). The MCP server itself stays wired
/// — only this surface toggles — so agents keep the rest of agentum's tools. The
/// switch lives in the desktop Settings → Agent Orchestration pane.
const ORCHESTRATION_TOOLS: &[&str] = &[
    "agentum_send_message",
    "agentum_check_messages",
    "agentum_create_task",
    "agentum_list_tasks",
];

fn is_orchestration_tool(name: &str) -> bool {
    ORCHESTRATION_TOOLS.contains(&name)
}

/// Read the orchestration gate (opt-in: absent/unset = off). Best-effort — a
/// store error falls back to off rather than failing the whole MCP request.
async fn orchestration_enabled(state: &AppState) -> bool {
    state
        .store
        .setting_get_bool(super::orchestration::ORCHESTRATION_ENABLED_SETTING, false)
        .await
        .unwrap_or(false)
}

/// The advertised tool catalog. Grows as skills get ported; each entry needs a
/// matching arm in [`call_tool`]. The orchestration tools are filtered out when
/// the gate is off (`orchestration_enabled`).
fn tool_specs(orchestration_enabled: bool) -> Value {
    let mut tools = json!([
        {
            "name": "agentum_list_sessions",
            "description": "List the agent sessions agentum manages on this machine \
                (each is one tmux pane running an agent CLI). Returns name, tool, \
                status, and working directory. Use to see sibling agents/worktrees.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        },
        {
            "name": "agentum_spawn_session",
            "description": "Spawn a NEW agent session INSIDE agentum — create a session \
                and launch its agent CLI into a fresh tmux pane on this machine, the same \
                way the desktop/TUI 'new session' does. Use to delegate work to a sibling \
                agent you can then coordinate with via agentum_send_message / \
                agentum_check_messages. `name` is a unique session name (also the agent's \
                mailbox handle); `workdir` is an existing directory to run in; `tool` is \
                the agent CLI (claude|codex|cursor|gemini|… — default claude); optional \
                `model`; `flags` are extra CLI args; `yolo` skips permission prompts \
                (pushed as the canonical marker and translated per tool). Returns the new \
                session's id, name, tool, status, and workdir.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Unique session name (also the agent's orchestration handle)" },
                    "workdir": { "type": "string", "description": "Existing directory the agent runs in" },
                    "tool": { "type": "string", "description": "Agent CLI: claude|codex|cursor|gemini|… (default claude)" },
                    "model": { "type": "string", "description": "Model override (optional)" },
                    "flags": { "type": "array", "items": { "type": "string" }, "description": "Extra CLI args passed to the agent" },
                    "yolo": { "type": "boolean", "description": "Skip permission prompts (default false)" }
                },
                "required": ["name", "workdir"],
                "additionalProperties": false,
            },
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
            "description": "FIRST-CHOICE browser tool in the agentum desktop app — drives the \
                browser the user is watching live in the pane. Use this directly; do NOT shell \
                out to the `agentum` CLI for browser work (it has no tab-create) and do NOT use \
                claude-in-chrome / Playwright / chrome-devtools here. Pass `op` and its params: \
                open (url) — opens a NEW tab \
                navigated to url and returns its `tab` id; tabs — lists open tabs; \
                navigate (url, wait_until?) → {http_status, final_url, title} | \
                snapshot (returns interactive element refs + a generation) | click \
                (ref or selector) | fill (ref or selector, text, submit?) | \
                scroll (direction up|down|left|right, amount?) — wheel-scroll the page \
                (or whatever scroller is under the viewport center) | \
                screenshot | node_at_point (x, y, capture?) — resolve the DOM element \
                at a viewport pixel, returning its clip + label (+ a sharp element PNG \
                when capture:true); used by the in-pane annotate picker | \
                wait (condition selector|text|url|network_idle, arg, \
                timeout_ms?) — act on a tab (optional `tab` id, \
                else the active one); prefer acting by `ref` from a fresh snapshot \
                (trusted input; a stale ref returns error `stale_ref`); \
                snapshot/screenshot take optional width+height (+ mobile, \
                deviceScaleFactor) to render at a breakpoint for responsive \
                testing, and screenshot takes full_page; get_console \
                (min_level?, since_generation?) returns buffered console \
                messages + network failures (JS errors, 404s) for debugging; \
                new_context → an isolated (cookies/storage) context + a page \
                `target` you pass to ops for per-task isolation, close_context \
                (browser_context_id) disposes it; connect_host (host) launches a \
                headless Chromium on an SSH host over an ssh -L tunnel and returns \
                a `cdpPort` to pass to subsequent ops (same contract as local); \
                annotations — read the design-feedback annotations the user marked on \
                page elements (returns structured markdown the agent can act on); \
                grab (selector) — extract an element's metadata (tag, text, selector, \
                rect, computed styles) by CSS selector; \
                annotate (selector, comment, intent?) — attach a design-feedback \
                annotation to an element (intent: change|fix|question|approve), which \
                shows in the browser tray and is returned by `annotations`. \
                Start with `open` when no tab is listed by `tabs` — `open` also creates the \
                visible pane, so it works even when nothing is open yet (and `open` with \
                `split` places the browser beside the agent). By DEFAULT these ops drive the \
                VISIBLE in-app browser the user sees — set `headless:true` (or \
                AGENTUM_BROWSER_HEADLESS=1) ONLY for QA / no-GUI, which drives a hidden \
                server-side Chromium the user CANNOT see.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": { "type": "string", "description": "open|tabs|navigate|snapshot|click|fill|scroll|screenshot|node_at_point|get_console|wait|eval|new_context|close_context|reap_contexts|connect_host|annotations|grab|annotate|set_split_ratio" },
                    "url": { "type": "string", "description": "Target URL for `open`/`navigate`" },
                    "tab": { "type": "string", "description": "Tab id to act on (default: the active tab)" },
                    "selector": { "type": "string", "description": "CSS selector for `click`/`fill`/`grab`/`annotate`" },
                    "ref": { "type": "string", "description": "Snapshot element ref (from `snapshot`.refs) for `click`/`fill` (trusted input) or `screenshot` (clip to that element); a stale ref returns error `stale_ref`" },
                    "interactive_only": { "type": "boolean", "description": "`snapshot`: return only actionable elements as refs (default true)" },
                    "submit": { "type": "boolean", "description": "`fill` by `ref`: press Enter after typing (default false)" },
                    "text": { "type": "string", "description": "Text to type for `fill`" },
                    "comment": { "type": "string", "description": "Annotation feedback text for `annotate`" },
                    "intent": { "type": "string", "description": "Annotation intent for `annotate`: change|fix|question|approve" },
                    "width": { "type": "integer", "description": "Viewport width (CSS px) for `snapshot`/`screenshot` — responsive testing; pair with `height`" },
                    "height": { "type": "integer", "description": "Viewport height (CSS px) for `snapshot`/`screenshot` — pair with `width`" },
                    "mobile": { "type": "boolean", "description": "Emulate a mobile device for the viewport override (default false)" },
                    "deviceScaleFactor": { "type": "number", "description": "Device pixel ratio for the viewport override (default 1)" },
                    "full_page": { "type": "boolean", "description": "`screenshot`: capture the full scrollable page, not just the viewport" },
                    "x": { "type": "number", "description": "`node_at_point`: viewport X (CSS px) to hit-test" },
                    "y": { "type": "number", "description": "`node_at_point`: viewport Y (CSS px) to hit-test" },
                    "capture": { "type": "boolean", "description": "`node_at_point`: also capture a sharp PNG of the resolved element (default false)" },
                    "direction": { "type": "string", "description": "`scroll`: up|down|left|right (default down)" },
                    "amount": { "type": "number", "description": "`scroll`: distance in CSS px (default 600 ≈ one screenful)" },
                    "min_level": { "type": "string", "description": "`get_console`: minimum level to return — info|warning|error (default warning)" },
                    "since_generation": { "type": "integer", "description": "`get_console`: only entries since this snapshot generation (default 0 = all)" },
                    "wait_until": { "type": "string", "description": "`navigate`: load|domcontentloaded|network_idle (default load)" },
                    "condition": { "type": "string", "description": "`wait`: selector|text|url|network_idle" },
                    "arg": { "type": "string", "description": "`wait`: the css selector / text / url substring for the condition" },
                    "timeout_ms": { "type": "integer", "description": "`wait`: max wait before returning timed_out=true (default 5000)" },
                    "expression": { "type": "string", "description": "`eval`: JS to run in the page (returns its value). Off by default — set AGENTUM_BROWSER_ALLOW_EVAL=1; every expression is logged" },
                    "target": { "type": "string", "description": "Route the op to a specific per-task context page (the `target` from `new_context`); omit for the shared active page" },
                    "browser_context_id": { "type": "string", "description": "`close_context`: the context id from `new_context` to dispose" },
                    "host": { "type": "string", "description": "`connect_host`: SSH host name/id to launch a headless Chromium on (over `ssh -L`); returns a `cdpPort`" },
                    "cdpPort": { "type": "integer", "description": "Drive a browser already reachable at 127.0.0.1:<port> (e.g. the `cdpPort` from `connect_host`) instead of the local one" },
                    "headless": { "type": "boolean", "description": "Drive a hidden server-side Chromium instead of the visible in-app browser (default false). Use for QA / no-GUI; the user won't see it" },
                    "split": { "type": "string", "description": "`open`: place the new browser pane BESIDE the worktree's agent (side by side) instead of as a stacked tab — left|right|up|down (e.g. right = browser to the right of the agent)" },
                    "ratio": { "type": "number", "description": "Split size 0–1 for `open`+`split` and for `set_split_ratio`: the fraction of space given to the LEFT/TOP pane (default 0.5 = even). With split=right the agent is the left pane, so 0.6 = agent 60% / browser 40%" }
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
            "name": "agentum_report_status",
            "description": "Report a work item's pipeline phase to its tracker: GitHub = flip the status/* label, Linear = move the workflow state, board = move the card column. Best-effort by contract — a tracker hiccup returns a 'skipped' note, never a tool error — so call it freely on every phase change (todo, in_progress, ready_to_test, done).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "provider": { "type": "string", "enum": ["github", "linear", "board"] },
                    "id": { "type": "string", "description": "The tracker's stable handle: board card key (AG-12), Linear identifier (ENG-42), or GitHub issue number. For github it may be omitted when `url` is given (derived from the URL)." },
                    "url": { "type": "string", "description": "The ticket URL. Required for github — owner/repo and the issue number are parsed from it. Ignored by linear/board." },
                    "phase": { "type": "string", "enum": ["todo", "in_progress", "ready_to_test", "done"] }
                },
                "required": ["provider", "phase"],
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
    ]);

    if !orchestration_enabled {
        if let Some(arr) = tools.as_array_mut() {
            arr.retain(|t| {
                t.get("name")
                    .and_then(Value::as_str)
                    .map(|n| !is_orchestration_tool(n))
                    .unwrap_or(true)
            });
        }
    }
    tools
}

/// Execute a `tools/call`. A bad request (missing name/params) is a JSON-RPC
/// error; a tool that runs but fails returns `isError: true` in the result.
async fn call_tool(state: &AppState, params: Option<&Value>) -> Result<Value, (i64, String)> {
    let params = params.ok_or((-32602, "missing params".to_string()))?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "missing tool name".to_string()))?;

    // Gate the orchestration surface: when off, these tools aren't advertised,
    // but a client could still call one by name — reject with an actionable hint.
    if is_orchestration_tool(name) && !orchestration_enabled(state).await {
        return Ok(json!({
            "content": [{
                "type": "text",
                "text": "agentum orchestration is turned off. Enable it in the agentum \
                         desktop app under Settings → Agent Orchestration.",
            }],
            "isError": true,
        }));
    }

    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // `agentum_browser` returns full MCP content (screenshot carries an image
    // block), so it bypasses the text-only `outcome` path below.
    if name == "agentum_browser" {
        return Ok(match tool_browser(state, &args).await {
            Ok(result) => result,
            Err(e) => json!({
                "content": [{ "type": "text", "text": format!("tool error: {e:#}") }],
                "isError": true,
            }),
        });
    }

    let outcome: anyhow::Result<String> = match name {
        "agentum_list_sessions" => tool_list_sessions(state).await,
        "agentum_spawn_session" => tool_spawn_session(state, &args).await,
        "agentum_list_worktrees" => tool_list_worktrees().await,
        "agentum_send_message" => tool_send_message(state, &args).await,
        "agentum_check_messages" => tool_check_messages(state, &args).await,
        "agentum_create_task" => tool_create_task(state, &args).await,
        "agentum_list_tasks" => tool_list_tasks(state, &args).await,
        "agentum_computer" => tool_bridge(state, "computer", &args).await,
        "agentum_harness_scaffold" => tool_harness_scaffold(&args).await,
        "agentum_harness_migrate" => tool_harness_migrate(&args).await,
        "agentum_harness_board" => tool_harness_board(&args).await,
        "agentum_harness_plan" => tool_harness_plan(&args).await,
        "agentum_harness_check" => tool_harness_check(&args).await,
        "agentum_harness_log_decision" => tool_harness_log_decision(&args).await,
        "agentum_report_status" => tool_report_status(state, &args).await,
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

/// Spawn a new agent session inside agentum — a thin view over
/// [`super::sessions::create_and_spawn_session`] (the same create+launch path the
/// `/create`+`/start` routes use). Lets one agent delegate to a sibling agent.
async fn tool_spawn_session(state: &AppState, args: &Value) -> anyhow::Result<String> {
    use agentum_core::NewSession;

    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `name`"))?;
    let workdir = args
        .get("workdir")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `workdir`"))?;
    let tool = args.get("tool").and_then(Value::as_str).unwrap_or("claude");
    let model = args
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut flags: Vec<String> = args
        .get("flags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    // YOLO is the canonical marker regardless of tool — the adapter translates
    // it to the per-tool spelling at launch (see executor::translate_yolo_marker).
    if args.get("yolo").and_then(Value::as_bool).unwrap_or(false)
        && !flags.iter().any(|f| f == agentum_executor::YOLO_MARKER)
    {
        flags.push(agentum_executor::YOLO_MARKER.to_string());
    }

    let new = NewSession {
        name: name.to_string(),
        workdir: workdir.to_string(),
        tool: tool.to_string(),
        model,
        flags,
        card_id: None,
        worktree_path: None,
        worktree_branch: None,
        worktree_base_ref: None,
    };

    let session = super::sessions::create_and_spawn_session(state, new, None)
        .await
        .map_err(|e| anyhow::anyhow!("spawn session: {e}"))?;
    Ok(serde_json::to_string_pretty(&json!({
        "id": session.id.to_string(),
        "name": session.name,
        "tool": session.tool,
        "status": format!("{:?}", session.status),
        "workdir": session.workdir,
    }))?)
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

/// `agentum_browser`: drive the **headless CDP Chromium that the screencast pane
/// streams** — the SAME browser the user sees. The page-driving / perception ops
/// (`open`/`navigate`/`tabs`/`click`/`fill`/`snapshot`/`screenshot`/`eval`/…) go to
/// the CDP driver, resolved to the worktree's browser via `worktreeId` (or, for a
/// contextless caller, the foreground pane's `cdpPort`), so they act live on the
/// visible browser. Only the renderer-owned annotation ops (`grab`/`annotate`/
/// `annotations`) round-trip to the desktop bridge. `headless:true` (or
/// `AGENTUM_BROWSER_HEADLESS=1`) forces a hidden server-side Chromium for QA / a
/// no-desktop daemon; the bridge is likewise skipped when no desktop is attached.
///
/// (Until v0.41.x the driving ops wrongly routed to the bridge, which drove Tauri
/// `browser-page-*` child webviews that no longer exist — so `tabs` was always empty
/// and `navigate`/`snapshot`/`screenshot` returned "no browser tab open".)
async fn tool_browser(state: &AppState, args: &Value) -> anyhow::Result<Value> {
    let op = args.get("op").and_then(Value::as_str).unwrap_or_default();
    // F11: launch + tunnel a headless Chromium on an SSH host, returning the local
    // `cdpPort` the agent then passes to the normal ops (contract identical to local).
    if op == "connect_host" {
        return tool_browser_connect_host(state, args).await;
    }
    // `headless:true` (or AGENTUM_BROWSER_HEADLESS=1) forces a hidden server-side
    // Chromium (QA / no-desktop daemon). Otherwise the page-driving ops use the CDP
    // driver against the SAME Chromium the screencast pane streams (the visible
    // browser); only the renderer-owned annotation ops fall through to the bridge.
    let want_headless = args
        .get("headless")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || std::env::var("AGENTUM_BROWSER_HEADLESS")
            .map(|v| matches!(v.trim(), "1" | "true"))
            .unwrap_or(false);
    if !want_headless && state.desktop_bridge.is_some() && bridge_browser_op(op) {
        return Ok(text_result(tool_bridge(state, "browser", args).await?));
    }
    if crate::cdp_driver::handles_op(op) {
        // Per-worktree isolation: when the agent's MCP carried a `worktreeId` (set
        // at spawn, injected in `handle`), drive THIS worktree's browser — the same
        // instance the user's pane watches — by resolving it to a `cdpPort`. Gated
        // inside ensure_local_cdp_browser_for: off → the shared default port, so
        // this is a no-op unless isolation is enabled. Resolution failure also
        // falls back to the default (never breaks the op).
        let mut call_args = args.clone();
        if call_args.get("cdpPort").is_none() {
            if let Some(wt) = call_args
                .get("worktreeId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
            {
                // Resolve the worktree's own browser to a `cdpPort` so the agent
                // drives the SAME instance the user's pane watches.
                let port = crate::cdp_browser::ensure_local_cdp_browser_for(&wt)
                    .await
                    .ok()
                    .map(|(_, p)| p);
                if let Some(port) = port {
                    if let Some(obj) = call_args.as_object_mut() {
                        obj.insert("cdpPort".to_string(), Value::from(port));
                    }
                }
            }
        }
        // No worktree context (e.g. a top-level agent not spawned into a worktree):
        // drive the browser the user is currently watching — the most recent
        // screencast pane attach — so the op still acts on the visible browser
        // instead of a separate shared Chromium. A worktree-scoped agent keeps its
        // own browser (the block above), so this only catches the contextless case.
        if call_args.get("cdpPort").is_none() && call_args.get("worktreeId").is_none() {
            if let Some(fg) = crate::cdp_browser::foreground_cdp_port() {
                if let Some(obj) = call_args.as_object_mut() {
                    obj.insert("cdpPort".to_string(), Value::from(fg));
                }
            }
        }
        // Visible-only guarantee in the desktop app: if we STILL have no target —
        // contextless AND no screencast pane has ever attached (foreground is None) —
        // do NOT fall through to the hidden shared Chromium. That silent fallback is
        // the "MCP drove a browser I can't see" complaint. `headless:true`, a worktree
        // context, or an SSH `cdpPort` all skip this (they resolved a target above).
        if !want_headless
            && state.desktop_bridge.is_some()
            && call_args.get("cdpPort").is_none()
            && call_args.get("worktreeId").is_none()
        {
            // `navigate` with nothing open is just "open a visible tab at this url":
            // route it through the renderer so a VISIBLE pane appears (and is driven).
            if op == "navigate" {
                let mut open_args = call_args.clone();
                if let Some(obj) = open_args.as_object_mut() {
                    obj.insert("op".to_string(), Value::from("open"));
                }
                return Ok(text_result(
                    tool_bridge(state, "browser", &open_args).await?,
                ));
            }
            // Ops that act on an existing page can't meaningfully drive a browser the
            // user can't see — tell the agent to open a visible one first instead of
            // silently driving the hidden default. (`tabs` falls through: listing is
            // a harmless read.)
            if op != "tabs" {
                anyhow::bail!(
                    "No visible browser is open in the agentum desktop app. Open one first \
                     with `agentum_browser` op `open` (add split:\"right\" to place it beside \
                     the agent), then retry `{op}`."
                );
            }
        }
        let result = crate::cdp_driver::run_browser_op(op, &call_args).await?;
        // screenshot → an MCP image content block (PNG) the agent can SEE, plus a
        // compact text meta line (NOT the giant base64) for width/height/path.
        if op == "screenshot" {
            if let Some(b64) = result.get("image_b64").and_then(Value::as_str) {
                let meta = json!({
                    "ok": result.get("ok"),
                    "format": result.get("format"),
                    "bytes": result.get("bytes"),
                    "width": result.get("width"),
                    "height": result.get("height"),
                    "path": result.get("path"),
                });
                return Ok(json!({
                    "content": [
                        { "type": "image", "data": b64, "mimeType": "image/png" },
                        { "type": "text", "text": meta.to_string() }
                    ],
                    "isError": false,
                }));
            }
            // a stale_ref / error result has no image — fall through to text.
        }
        return Ok(text_result(serde_json::to_string_pretty(&result)?));
    }
    Ok(text_result(tool_bridge(state, "browser", args).await?))
}

/// Browser ops that go to the desktop bridge (the renderer) rather than the CDP driver:
/// - `open` — the renderer owns the browser-pane lifecycle, so opening a tab there makes
///   a VISIBLE pane appear (and, with a `split`, places it beside the agent) and
///   navigates it. The page PERCEPTION/DRIVE ops (`navigate`/`tabs`/`click`/`fill`/
///   `snapshot`/`screenshot`) go to the CDP driver, which drives the SAME Chromium the
///   screencast streams — they used to route here too, but the bridge drives Tauri
///   `browser-page-*` webviews that no longer exist (the browser became a CDP
///   screencast), so they returned "no browser tab open" / an empty tab list.
/// - `grab`/`annotate`/`annotations` — live in the renderer's visual annotation store.
/// - `set_split_ratio` — resizes a renderer layout split (no browser equivalent).
fn bridge_browser_op(op: &str) -> bool {
    matches!(
        op,
        "open" | "grab" | "annotate" | "annotations" | "set_split_ratio"
    )
}

/// Wrap a string as a plain MCP text result.
fn text_result(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": false })
}

/// `connect_host`: resolve an SSH host (by name or id), ensure a headless Chromium
/// is running on it + reachable via an `ssh -L` tunnel, and return the local
/// `cdpPort` to pass to subsequent browser ops (F11 / criterion #5).
async fn tool_browser_connect_host(state: &AppState, args: &Value) -> anyhow::Result<Value> {
    let host_ref = args
        .get("host")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("connect_host requires `host` (an SSH host name or id)"))?;
    let hosts = state
        .store
        .list_hosts()
        .await
        .map_err(|e| anyhow::anyhow!("list hosts: {e}"))?;
    let host = hosts
        .into_iter()
        .find(|h| h.name == host_ref || h.id.to_string() == host_ref)
        .ok_or_else(|| anyhow::anyhow!("no SSH host named `{host_ref}` (see `agentum hosts`)"))?;
    let port = crate::cdp_browser::ensure_remote_cdp_browser(&host).await?;
    Ok(text_result(
        json!({
            "ok": true,
            "host": host.name,
            "cdpPort": port,
            "hint": "pass this cdpPort to subsequent navigate/snapshot/click/etc. to drive the remote browser",
        })
        .to_string(),
    ))
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

/// Parse + validate `agentum_report_status` inputs (spec 005 F4). Pure →
/// unit-testable without AppState. Errors here are CALLER bugs (missing/unknown
/// args) and DO surface as `isError: true` — the best-effort contract covers
/// tracker failures, not typos. `id` is required, EXCEPT `provider == "github"`
/// with a parseable issue `url` (then id := the URL's number).
fn parse_report_status_args(
    args: &Value,
) -> anyhow::Result<(
    String,
    String,
    Option<String>,
    crate::task_sink::TrackerPhase,
)> {
    let provider = args
        .get("provider")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `provider`"))?
        .to_string();
    let phase_str = args
        .get("phase")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `phase`"))?;
    let phase = crate::task_sink::parse_tracker_phase(phase_str).ok_or_else(|| {
        anyhow::anyhow!("unknown `phase` {phase_str:?} (todo|in_progress|ready_to_test|done)")
    })?;
    let url = args.get("url").and_then(Value::as_str).map(str::to_string);
    let id = match args.get("id").and_then(Value::as_str) {
        Some(id) => id.to_string(),
        None if provider == "github" => {
            let url = url
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("missing `id` (or a github issue `url`)"))?;
            crate::task_sink::github_slug_and_number_from_issue_url(url)
                .map(|(_slug, number)| number)
                .ok_or_else(|| {
                    anyhow::anyhow!("missing `id` and `url` is not a GitHub issue URL: {url}")
                })?
        }
        None => anyhow::bail!("missing `id`"),
    };
    Ok((provider, id, url, phase))
}

/// Map the transition seam's outcome to the tool's text. Pure. NEVER an `Err`
/// for a tracker failure (AC 9 / the best-effort invariant): transport errors
/// from the linear/board arms come back as a "skipped" note the agent can read.
fn report_status_text(
    outcome: anyhow::Result<crate::task_sink::TransitionResult>,
    provider: &str,
    phase: crate::task_sink::TrackerPhase,
) -> String {
    match outcome {
        Ok(crate::task_sink::TransitionResult::Applied) => {
            format!("applied: {provider} → {phase:?}")
        }
        Ok(crate::task_sink::TransitionResult::Skipped(w)) => format!("skipped: {w}"),
        Err(e) => format!("skipped (tracker error, non-fatal): {e:#}"),
    }
}

/// Report a work item's pipeline phase to its tracker — a thin arm over
/// [`crate::task_sink::apply_tracker_transition`] (spec 005 F4), the same seam
/// the harness's own transitions use. Never reimplements label/state mechanics.
async fn tool_report_status(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let (provider, id, url, phase) = parse_report_status_args(args)?;
    let outcome = crate::task_sink::apply_tracker_transition(
        &state.store,
        &provider,
        &id,
        url.as_deref(),
        phase,
    )
    .await;
    Ok(report_status_text(outcome, &provider, phase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_instructions_steer_agents_to_agentum_browser_in_the_desktop_app() {
        // Desktop-app guidance must push agents to `agentum_browser` (with `open`/`split`)
        // and AWAY from the shell CLI / other browser tools — the fix for agents fumbling
        // to claude-in-chrome / the `agentum` shell CLI (which can't open a tab). Lock the
        // key phrases so the steer can't silently regress.
        let d = mcp_instructions(true);
        assert!(d.contains("agentum_browser"), "names the tool");
        assert!(d.contains("open"), "tells them to open a visible tab");
        assert!(d.contains("split"), "mentions side-by-side");
        assert!(
            d.contains("claude-in-chrome"),
            "explicitly deprioritizes other browser tools"
        );
        assert!(d.contains("shell CLI"), "warns off the shell CLI");
        // A headless daemon (no desktop bridge) still points at agentum_browser, but
        // drops the "visible / user is watching" promise.
        assert!(mcp_instructions(false).contains("agentum_browser"));
        assert!(!mcp_instructions(false).contains("watching live"));
    }

    #[test]
    fn browser_op_routing_splits_bridge_vs_cdp() {
        // The renderer owns these: `open` (visible pane + split placement), the
        // annotation store, and layout resize. They round-trip to the desktop bridge.
        for op in ["open", "grab", "annotate", "annotations", "set_split_ratio"] {
            assert!(bridge_browser_op(op), "{op} should route to the bridge");
        }
        // Every page PERCEPTION / DRIVE op is handled by the CDP driver instead — it
        // drives the SAME Chromium the screencast streams, so the op acts on the
        // browser the user sees. They MUST NOT route to the bridge (whose webview path
        // is dead and returned "no browser tab open"). This is the fix.
        for op in [
            "tabs",
            "navigate",
            "click",
            "fill",
            "snapshot",
            "screenshot",
        ] {
            assert!(
                !bridge_browser_op(op),
                "{op} must be CDP-driven, not bridged"
            );
            assert!(
                crate::cdp_driver::handles_op(op),
                "{op} must be claimed by the CDP driver"
            );
        }
        // `open` is special: bridged for the VISIBLE pane, but ALSO CDP-claimed so a
        // `headless:true` open still gets a real new target server-side.
        assert!(crate::cdp_driver::handles_op("open"));
        // CDP-only ops have no webview equivalent → always headless CDP, never bridged.
        for op in [
            "eval",
            "get_console",
            "node_at_point",
            "wait",
            "new_context",
            "close_context",
            "reap_contexts",
            "connect_host",
            "bogus",
        ] {
            assert!(!bridge_browser_op(op), "{op} must NOT route to the bridge");
        }
    }

    #[tokio::test]
    async fn mcp_master_switch_defaults_on_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = agentum_store::Store::open(&dir.path().join("t.db"))
            .await
            .unwrap();
        // Default ON: a fresh install must wire agentum's MCP. Turning it off has
        // to be an explicit opt-out, never the mere absence of the setting —
        // otherwise every agent would silently lose agentum's tools.
        assert!(
            store
                .setting_get_bool(MCP_ENABLED_SETTING, true)
                .await
                .unwrap()
        );
        store
            .setting_set_bool(MCP_ENABLED_SETTING, false)
            .await
            .unwrap();
        assert!(
            !store
                .setting_get_bool(MCP_ENABLED_SETTING, true)
                .await
                .unwrap()
        );
    }

    fn tool_names(orchestration_enabled: bool) -> Vec<String> {
        tool_specs(orchestration_enabled)
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn tool_catalog_is_well_formed() {
        // Every advertised tool needs name + description + object inputSchema,
        // or agents reject the listing.
        let tools = tool_specs(true);
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
        // A non-orchestration tool is present regardless of the gate.
        assert!(tool_names(true).contains(&"agentum_list_sessions".to_string()));
        assert!(tool_names(false).contains(&"agentum_list_sessions".to_string()));
    }

    #[test]
    fn orchestration_tools_are_gated_off_the_catalog() {
        // Enabled: the mailbox + task DAG tools are advertised.
        let on = tool_names(true);
        for t in ORCHESTRATION_TOOLS {
            assert!(
                on.contains(&t.to_string()),
                "{t} should be listed when enabled"
            );
        }
        // Disabled: none of them are, but the rest of the catalog survives.
        let off = tool_names(false);
        for t in ORCHESTRATION_TOOLS {
            assert!(
                !off.contains(&t.to_string()),
                "{t} must be hidden when disabled"
            );
        }
        assert!(off.contains(&"agentum_list_worktrees".to_string()));
        assert!(off.len() + ORCHESTRATION_TOOLS.len() == on.len());
    }

    // --- spec 005 F4: agentum_report_status ---

    #[test]
    fn report_status_is_in_the_catalog() {
        // A status verb, not the mailbox/DAG surface — present with the gate on.
        assert!(tool_names(true).contains(&"agentum_report_status".to_string()));
    }

    #[test]
    fn report_status_survives_orchestration_gate_off() {
        // Deliberately NOT in ORCHESTRATION_TOOLS: advertised (and callable)
        // regardless of that gate, like agentum_list_sessions.
        assert!(!is_orchestration_tool("agentum_report_status"));
        assert!(tool_names(false).contains(&"agentum_report_status".to_string()));
    }

    #[test]
    fn report_status_args_require_id_except_github_url() {
        use crate::task_sink::TrackerPhase;

        // id-less linear → Err (caller bug).
        let e = parse_report_status_args(&json!({ "provider": "linear", "phase": "done" }));
        assert!(e.is_err(), "linear without id must be a caller error");

        // id-less github + issue URL → Ok with the derived number.
        let (provider, id, url, phase) = parse_report_status_args(&json!({
            "provider": "github",
            "url": "https://github.com/owner/repo/issues/42",
            "phase": "in_progress",
        }))
        .unwrap();
        assert_eq!(provider, "github");
        assert_eq!(id, "42");
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/owner/repo/issues/42")
        );
        assert_eq!(phase, TrackerPhase::InProgress);

        // id-less github + garbage URL → Err.
        assert!(
            parse_report_status_args(&json!({
                "provider": "github",
                "url": "https://github.com/o/r/pull/42",
                "phase": "done",
            }))
            .is_err(),
            "a PR link is not an issue URL"
        );
        // id-less github with NO url → Err.
        assert!(
            parse_report_status_args(&json!({ "provider": "github", "phase": "done" })).is_err()
        );

        // An explicit id always wins (no URL needed for linear/board).
        let (_, id, _, _) = parse_report_status_args(&json!({
            "provider": "board", "id": "AG-12", "phase": "todo",
        }))
        .unwrap();
        assert_eq!(id, "AG-12");

        // A junk phase is a caller error, never silently coerced.
        assert!(
            parse_report_status_args(&json!({
                "provider": "board", "id": "AG-12", "phase": "shipped",
            }))
            .is_err()
        );
    }

    /// The AC 9 pin: every outcome shape — including a seam `Err` — maps to a
    /// normal text result. A tracker hiccup is a readable "skipped" note, never
    /// a tool error.
    #[test]
    fn report_status_text_never_errs_on_tracker_failure() {
        use crate::task_sink::{TrackerPhase, TransitionResult};

        assert_eq!(
            report_status_text(Ok(TransitionResult::Applied), "github", TrackerPhase::Done),
            "applied: github → Done"
        );
        assert_eq!(
            report_status_text(
                Ok(TransitionResult::Skipped("no board card with key X".into())),
                "board",
                TrackerPhase::Todo,
            ),
            "skipped: no board card with key X"
        );
        let text = report_status_text(
            Err(anyhow::anyhow!("network down")),
            "linear",
            TrackerPhase::ReadyToTest,
        );
        assert!(
            text.starts_with("skipped (tracker error, non-fatal):"),
            "got: {text}"
        );
        assert!(text.contains("network down"));
    }

    /// Minimal AppState over a tempdir store (the board_sync `fresh_state`
    /// pattern) so the wire-level delegation test drives the REAL tool fn.
    async fn fresh_state() -> AppState {
        use std::sync::Arc;
        use tokio::sync::broadcast;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        std::mem::forget(dir); // keep the tempdir alive for the test
        let store = agentum_store::Store::open(&p).await.unwrap();
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
            wiki_keys: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
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

    /// Wire-level delegation (board arm, no subprocess): the tool moves a real
    /// board card's column through `apply_tracker_transition` — proof the arm
    /// delegates to the seam rather than reimplementing tracker mechanics.
    #[tokio::test]
    async fn report_status_moves_a_board_card() {
        let state = fresh_state().await;
        let card = state
            .store
            .create_board_item(agentum_core::NewBoardItem {
                title: "Add OAuth login".into(),
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

        let text = tool_report_status(
            &state,
            &json!({ "provider": "board", "id": card.key, "phase": "in_progress" }),
        )
        .await
        .unwrap();
        assert_eq!(text, "applied: board → InProgress");

        let moved = state
            .store
            .list_board_items()
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.key == card.key)
            .unwrap();
        assert_eq!(moved.status, "doing");

        // Unknown provider flows to the seam's Skipped — visible, non-fatal.
        let text = tool_report_status(
            &state,
            &json!({ "provider": "jira", "id": "X-1", "phase": "done" }),
        )
        .await
        .unwrap();
        assert!(text.starts_with("skipped:"), "got: {text}");
    }
}
