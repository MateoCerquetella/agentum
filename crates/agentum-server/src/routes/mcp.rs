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
                let tool = params.get("name").and_then(Value::as_str);
                if let Some(a) = params.get_mut("arguments").and_then(Value::as_object_mut) {
                    if tool == Some("agentum_browser")
                        && !a.contains_key("worktreeId")
                        && !a.contains_key("cdpPort")
                    {
                        a.insert("worktreeId".to_string(), Value::from(wt.to_string()));
                    } else if tool == Some("agentum_harness_run")
                        && !a.contains_key("worktreeId")
                    {
                        // The MCP URL provisioned into a worktree agent already
                        // carries its authoritative registry id. Harness launch
                        // consumes the same identity instead of trusting a path
                        // that could exist on both the daemon and an SSH host.
                        a.insert("worktreeId".to_string(), Value::from(wt.to_string()));
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
                // `prompts` = the server-owned SDD playbooks (crate::sdd). Agents
                // that render MCP prompts as slash commands (Claude Code, Gemini
                // CLI) get native /sdd-*; everyone else reaches the same bodies
                // through the `agentum_sdd` tool — tools/call is the lowest
                // common denominator every MCP client supports.
                "capabilities": {
                    "tools": { "listChanged": false },
                    "prompts": { "listChanged": false },
                },
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
        "prompts/list" => Ok(prompts_list()),
        "prompts/get" => prompts_get(params),
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
                (each is one tmux pane running an agent CLI). Returns \
                {total_matching, returned, truncated, sessions:[{id, name, tool, \
                status, workdir}]}. A long-lived install accumulates hundreds of \
                rows, so the page is capped (`limit`, default 50) — filter with \
                `status` (exact, e.g. Running), `name_contains`, or \
                `workdir_contains` instead of paging through everything.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "description": "Exact status filter (case-insensitive): Running|Stopped|Crashed|…" },
                    "name_contains": { "type": "string", "description": "Substring filter on the session name" },
                    "workdir_contains": { "type": "string", "description": "Substring filter on the working directory" },
                    "limit": { "type": "integer", "description": "Max sessions to return (default 50, cap 500)" }
                },
                "additionalProperties": false,
            },
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
                session's id, name, tool, status, and workdir. Mailbox timing: a message \
                sent right after spawn can land BEFORE the new agent's first \
                agentum_check_messages poll — make its bootstrap prompt poll the mailbox \
                in a retry loop (every ~20s), or push directly with \
                agentum_inject_prompt. End the session later with agentum_stop_session.",
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
            "description": "Report a work item's pipeline phase to its external tracker: GitHub = flip the status/* label, Linear = move the workflow state. Best-effort by contract — a tracker hiccup returns a 'skipped' note, never a tool error — so call it freely on every phase change (todo, in_progress, in_review, ready_to_test, done).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "provider": { "type": "string", "enum": ["github", "linear"] },
                    "id": { "type": "string", "description": "The tracker's stable handle: Linear identifier (ENG-42) or GitHub issue number. For github it may be omitted when `url` is given (derived from the URL)." },
                    "url": { "type": "string", "description": "The ticket URL. Required for github — owner/repo and the issue number are parsed from it. Ignored by linear." },
                    "phase": { "type": "string", "enum": ["todo", "in_progress", "in_review", "ready_to_test", "done"] }
                },
                "required": ["provider", "phase"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_sdd_loop_control",
            "description": "Start, stop, or inspect the server-owned SDD loop for an agentum session. This is the control-plane companion to `agentum_sdd_loop`, which is only the end-of-step agent check-in. Use `agentum_list_sessions` to obtain the session UUID, then call action `start` to inject autonomous `sdd-orchestrate` steps, `status` to inspect progress, or `stop` to cancel further injections. Starting is idempotent and uses the same generation/event/step-cap machinery as the desktop Loop toggle.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Agentum session UUID (obtain from agentum_list_sessions)" },
                    "action": { "type": "string", "enum": ["start", "stop", "status"] },
                    "max_steps": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Optional unattended step cap for action=start (default 10)" }
                },
                "required": ["session", "action"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_sdd_loop",
            "description": "Check in with the per-session SDD loop that injected the current \
                step prompt. Call it at the END of every loop step: `done: true` when the work \
                is complete (the spec's phase is done or there is no actionable next step) — \
                this stops the loop so no further step prompts are injected; `done: false` to \
                keep the loop running, with `summary` as a one-line progress note. Always safe \
                to call: with no active loop on the session (or from an earlier activation) it \
                returns success and stops nothing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "The session uuid, exactly as given in the step prompt" },
                    "done": { "type": "boolean", "description": "true = work complete, stop the loop; false = more to do, keep looping" },
                    "summary": { "type": "string", "description": "One-line progress note; surfaced on the next step event when done is false" },
                    "generation": { "type": "integer", "description": "The loop generation from the step prompt; a mismatch makes the check-in a no-op" }
                },
                "required": ["session", "done"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_sdd",
            "description": "Fetch a server-owned SDD (spec-driven development) playbook. \
                Call with no arguments to list the available playbooks (sdd-spec, \
                sdd-spec-socratic, sdd-orchestrate, sdd-status, sdd-handoff, sdd-init); \
                call with `name` to fetch one — then FOLLOW the returned playbook exactly. \
                These are the same playbooks the desktop SDD buttons and the SDD loop \
                deliver; agentum owns the bodies so every agent gets the same procedure.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Playbook to fetch, e.g. `sdd-spec`. Omit to list all." },
                    "args": { "type": "string", "description": "Optional free-form arguments for the playbook (e.g. `autonomous` or a spec id for sdd-orchestrate)." }
                },
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
        },
        {
            "name": "agentum_stop_session",
            "description": "Stop or kill an agent session by id or name — the lifecycle \
                END for agentum_spawn_session (#378), the same core as the desktop \
                stop/kill actions. `mode` 'stop' (default) ends the pane gracefully; \
                'kill' hard-kills it. An external (user-owned) tmux session is only \
                detached, never destroyed. The session row and pane log survive as \
                evidence; the pane is gone.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Session id (uuid) or unique session name" },
                    "mode": { "type": "string", "enum": ["stop", "kill"], "description": "stop = graceful (default), kill = hard" }
                },
                "required": ["session"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_inject_prompt",
            "description": "Inject a prompt DIRECTLY into a running agent session's REPL \
                and submit it — the push channel (#378); the mailbox is pull and needs \
                the recipient to poll. Same robust delivery as the SDD loop / the \
                `/submit` route: waits for the REPL to go idle (accepting Claude's \
                one-time workspace-trust dialog), types the text, then submits with a \
                SEPARATE Enter so a multi-line prompt isn't swallowed as a paste. \
                Delivery is asynchronous — the tool returns once queued; a busy agent \
                can take tens of seconds to idle. `session` is an id or name.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Session id (uuid) or unique session name" },
                    "prompt": { "type": "string", "description": "The prompt to type into the REPL and submit" }
                },
                "required": ["session", "prompt"],
                "additionalProperties": false,
            },
        },
        {
            "name": "agentum_harness_run",
            "description": "Register (or reuse) a project's harness run and kick off the \
                verify-gated drive loop in the background — the MCP equivalent of \
                POST /api/harness + POST /{id}/run, completing the Goals surface \
                (scaffold → plan → check → RUN, #378). Requires a ready \
                `.agentum-harness/` (probe with agentum_harness_check). Returns \
                {harness_id, started}: started=false means a driver is already live \
                (not restarted). Watch progress with agentum_harness_board or the \
                desktop Harness view.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                    "workdir": { "type": "string", "description": "Registered worktree path containing .agentum-harness/" },
                    "worktreeId": { "type": "string", "description": "Exact registered worktree identity; normally supplied by the worktree-scoped MCP URL" }
                },
                "required": ["workdir", "worktreeId"],
                "additionalProperties": false,
            },
        }
    ]);

    if let Some(arr) = tools.as_array_mut() {
        arr.extend(json!([
            {
                "name": "agentum_harness_task_context",
                "description": "Worker-only: retrieve the immutable bounded packet for one orchestrated harness task. An optional exact file or symbol expands only that named context.",
                "inputSchema": {"type":"object","properties":{"run_id":{"type":"string"},"task_id":{"type":"string"},"capability_token":{"type":"string"},"file":{"type":"string"},"symbol":{"type":"string"}},"required":["run_id","task_id","capability_token"],"additionalProperties":false}
            },
            {
                "name": "agentum_harness_submit_patch",
                "description": "Worker-only: submit a capability-scoped, hash-checked create/update/delete/rename transaction to the shared-worktree patch broker.",
                "inputSchema": {"type":"object","properties":{"run_id":{"type":"string"},"task_id":{"type":"string"},"capability_token":{"type":"string"},"summary":{"type":"string"},"operations":{"type":"array","items":{"type":"object"}}},"required":["run_id","task_id","capability_token","summary","operations"],"additionalProperties":false}
            },
            {
                "name": "agentum_harness_request_verify",
                "description": "Worker-only: run the task's targeted gate in the serialized verification lane. A failure returns only its relevant output tail.",
                "inputSchema": {"type":"object","properties":{"run_id":{"type":"string"},"task_id":{"type":"string"},"capability_token":{"type":"string"}},"required":["run_id","task_id","capability_token"],"additionalProperties":false}
            },
            {
                "name": "agentum_harness_report_blocked",
                "description": "Worker-only: report a concrete blocker without stopping independent tasks.",
                "inputSchema": {"type":"object","properties":{"run_id":{"type":"string"},"task_id":{"type":"string"},"capability_token":{"type":"string"},"reason":{"type":"string"}},"required":["run_id","task_id","capability_token","reason"],"additionalProperties":false}
            },
            {
                "name": "agentum_harness_run_state",
                "description": "Coordinator-only: return durable DAG/task summaries, leases, patch ledger and managed sessions without source dumps or worker transcripts.",
                "inputSchema": {"type":"object","properties":{"run_id":{"type":"string"},"capability_token":{"type":"string"}},"required":["run_id","capability_token"],"additionalProperties":false}
            },
            {
                "name": "agentum_harness_dispatch",
                "description": "Coordinator-only: dispatch one ready task to a new isolated managed worker in the same worktree; server enforces the concurrency ceiling.",
                "inputSchema": {"type":"object","properties":{"run_id":{"type":"string"},"task_id":{"type":"string"},"capability_token":{"type":"string"}},"required":["run_id","task_id","capability_token"],"additionalProperties":false}
            },
            {
                "name": "agentum_harness_transfer_ownership",
                "description": "Coordinator-only: transfer a frozen or idle exact-file lease between tasks after server validation.",
                "inputSchema": {"type":"object","properties":{"run_id":{"type":"string"},"path":{"type":"string"},"from_task":{"type":"string"},"to_task":{"type":"string"},"capability_token":{"type":"string"}},"required":["run_id","path","from_task","to_task","capability_token"],"additionalProperties":false}
            },
            {
                "name": "agentum_harness_create_repair_task",
                "description": "Coordinator-only: create a focused repair task with explicit ownership and dependencies.",
                "inputSchema": {"type":"object","properties":{"run_id":{"type":"string"},"capability_token":{"type":"string"},"task":{"type":"object"}},"required":["run_id","capability_token","task"],"additionalProperties":false}
            },
            {
                "name": "agentum_harness_retry_or_block",
                "description": "Coordinator/reviewer: retry a blocked task, block the run, or complete the run after read-only review.",
                "inputSchema": {"type":"object","properties":{"run_id":{"type":"string"},"capability_token":{"type":"string"},"action":{"type":"string","enum":["retry","block","complete"]},"task_id":{"type":"string"},"reason":{"type":"string"}},"required":["run_id","capability_token","action"],"additionalProperties":false}
            }
        ]).as_array().cloned().unwrap_or_default());
    }

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
        "agentum_list_sessions" => tool_list_sessions(state, &args).await,
        "agentum_spawn_session" => tool_spawn_session(state, &args).await,
        "agentum_stop_session" => tool_stop_session(state, &args).await,
        "agentum_inject_prompt" => tool_inject_prompt(state, &args).await,
        "agentum_harness_run" => tool_harness_run(state, &args).await,
        "agentum_harness_task_context" => tool_harness_task_context(state, &args).await,
        "agentum_harness_submit_patch" => tool_harness_submit_patch(state, &args).await,
        "agentum_harness_request_verify" => tool_harness_request_verify(state, &args).await,
        "agentum_harness_report_blocked" => tool_harness_report_blocked(state, &args).await,
        "agentum_harness_run_state" => tool_harness_run_state(state, &args).await,
        "agentum_harness_dispatch" => tool_harness_dispatch(state, &args).await,
        "agentum_harness_transfer_ownership" => tool_harness_transfer_ownership(state, &args).await,
        "agentum_harness_create_repair_task" => tool_harness_create_repair_task(state, &args).await,
        "agentum_harness_retry_or_block" => tool_harness_retry_or_block(state, &args).await,
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
        "agentum_sdd_loop_control" => tool_sdd_loop_control(state, &args).await,
        "agentum_sdd_loop" => tool_sdd_loop(state, &args).await,
        "agentum_sdd" => tool_sdd(&args),
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

/// Default page size for `agentum_list_sessions` (#378): the raw table on a
/// long-lived install runs to hundreds of rows (~93 KB observed), which blows
/// MCP result limits — so the tool pages by default and reports truncation.
const DEFAULT_SESSION_PAGE: usize = 50;

async fn tool_list_sessions(state: &AppState, args: &Value) -> anyhow::Result<String> {
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
    Ok(serde_json::to_string_pretty(&apply_session_filters(
        rows, args,
    ))?)
}

/// Filter + bound the session listing (#378). Pure over the projected JSON
/// rows → unit-testable without a store. `status` is an exact (case-insensitive)
/// match; the `*_contains` filters are substrings; `limit` caps the page (the
/// envelope's `truncated` says whether anything was cut).
fn apply_session_filters(rows: Vec<Value>, args: &Value) -> Value {
    let want = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
    };
    let (status, name_c, workdir_c) = (
        want("status"),
        want("name_contains"),
        want("workdir_contains"),
    );
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|l| l.clamp(1, 500) as usize)
        .unwrap_or(DEFAULT_SESSION_PAGE);
    let field = |r: &Value, k: &str| {
        r.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    let kept: Vec<Value> = rows
        .into_iter()
        .filter(|r| {
            status.as_deref().is_none_or(|s| field(r, "status") == s)
                && name_c
                    .as_deref()
                    .is_none_or(|n| field(r, "name").contains(n))
                && workdir_c
                    .as_deref()
                    .is_none_or(|w| field(r, "workdir").contains(w))
        })
        .collect();
    let total = kept.len();
    let page: Vec<Value> = kept.into_iter().take(limit).collect();
    json!({
        "total_matching": total,
        "returned": page.len(),
        "truncated": total > page.len(),
        "sessions": page,
    })
}

/// Resolve a session reference — uuid first, then unique name — for the
/// lifecycle/injection tools (#378). Agents usually hold the NAME (it's the
/// mailbox handle); the uuid is what spawn returned.
async fn resolve_session_ref(
    state: &AppState,
    sref: &str,
) -> anyhow::Result<agentum_core::Session> {
    if let Ok(id) = uuid::Uuid::parse_str(sref) {
        if let Some(s) = state.store.get_session_by_id(id).await? {
            return Ok(s);
        }
    }
    state
        .store
        .get_session_by_name(sref)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no session with id or name `{sref}`"))
}

/// End a session — a thin view over [`super::sessions::stop_session_core`],
/// the same core the desktop stop/kill routes use (#378: spawn finally has a
/// lifecycle end on the MCP surface).
async fn tool_stop_session(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let sref = args
        .get("session")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `session` (id or name)"))?;
    let mode = args.get("mode").and_then(Value::as_str).unwrap_or("stop");
    let force_kill = match mode {
        "stop" => false,
        "kill" => true,
        other => anyhow::bail!("unknown `mode` {other:?} (stop|kill)"),
    };
    let session = resolve_session_ref(state, sref).await?;
    let stopped = super::sessions::stop_session_core(state, session.id, force_kill)
        .await
        .map_err(|e| anyhow::anyhow!("{mode} session: {e}"))?;
    Ok(serde_json::to_string_pretty(&json!({
        "id": stopped.id.to_string(),
        "name": stopped.name,
        "status": format!("{:?}", stopped.status),
        "mode": mode,
    }))?)
}

/// Push a prompt into a running session's REPL — a thin view over
/// [`super::sessions::submit_prompt_core`] (the `/submit` route's core), which
/// itself reuses the harness's robust two-step `inject_prompt` delivery (#378).
async fn tool_inject_prompt(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let sref = args
        .get("session")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `session` (id or name)"))?;
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `prompt`"))?;
    let session = resolve_session_ref(state, sref).await?;
    let name = session.name.clone();
    super::sessions::submit_prompt_core(state, session, prompt.to_string())
        .await
        .map_err(|e| anyhow::anyhow!("inject prompt: {e}"))?;
    Ok(serde_json::to_string_pretty(&json!({
        "queued": true,
        "session": name,
        "note": "delivery is asynchronous — the prompt is typed once the REPL idles \
                 (can take tens of seconds on a busy agent) and submitted with a \
                 separate Enter",
    }))?)
}

/// Register (or reuse) + run a project's harness — the MCP equivalent of
/// `POST /api/harness` + `POST /{id}/run` (#378: Goals were preparable via
/// scaffold/plan/check but not launchable). Same background kick as the route;
/// the drive loop owns its own error handling (emits Error + Failed state).
async fn tool_harness_run(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let raw = args
        .get("workdir")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `workdir`"))?;
    let worktree_id = args
        .get("worktreeId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "missing `worktreeId`; harness launch requires an exact registered worktree"
            )
        })?;
    let (scope, host) = super::worktrees::resolve_harness_scope(state, worktree_id, raw)
        .await
        .map_err(|e| anyhow::anyhow!("resolve harness worktree: {e:?}"))?;
    let workspace = crate::harness::HarnessWorkspace::scoped(scope.clone(), host.clone());
    if !workspace.is_dir(&workspace.root()).await? {
        anyhow::bail!(
            "registered harness worktree does not exist on host {}: {}",
            host.id,
            scope.path
        );
    }
    let mut config = crate::harness::HarnessConfig::load_from(&workspace).await?;
    workspace
        .strict_remote_preflight(
            state,
            Some(&config.features.agent_tool),
            config.features.qa_agent_tool.as_deref(),
            false,
        )
        .await?;
    if workspace.is_remote()
        && (config.features.execution_mode != crate::harness::ExecutionMode::Sequential
            || config.features.max_concurrency != 1)
    {
        config.features.execution_mode = crate::harness::ExecutionMode::Sequential;
        config.features.max_concurrency = 1;
        config.save_features(&config.features).await?;
    }

    // Reuse only the exact registered worktree. A same-looking path on a
    // different host/repo is a different scope and can never cross-attach.
    let harness_id = match state.harness.find_by_scope(&scope).await {
        Some(id) => id,
        None => state
            .harness
            .start_scoped(scope.clone(), host)
            .await
            .map_err(|e| anyhow::anyhow!("register harness: {e}"))?,
    };
    let claimed = state
        .harness
        .claim_driver(harness_id)
        .await
        .map_err(|e| anyhow::anyhow!("claim driver: {e}"))?;
    if claimed {
        let st = state.clone();
        tokio::spawn(async move { crate::harness::drive(st, harness_id).await });
    }
    Ok(serde_json::to_string_pretty(&json!({
        "harness_id": harness_id.to_string(),
        "started": claimed,
        "scope": scope,
        "note": if claimed {
            "drive loop kicked off in the background — watch agentum_harness_board \
             (or the desktop Harness view)"
        } else {
            "a driver is already running this harness — not restarted"
        },
    }))?)
}

fn harness_arg<'a>(args: &'a Value, name: &str) -> anyhow::Result<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `{name}`"))
}

async fn tool_harness_task_context(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let value = crate::harness::orchestrated::task_context(
        state,
        harness_arg(args, "run_id")?,
        harness_arg(args, "task_id")?,
        harness_arg(args, "capability_token")?,
        args.get("file").and_then(Value::as_str),
        args.get("symbol").and_then(Value::as_str),
    )
    .await?;
    Ok(serde_json::to_string_pretty(&value)?)
}

async fn tool_harness_submit_patch(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let submission: crate::harness::orchestrated::PatchSubmission =
        serde_json::from_value(args.clone())?;
    let run_id = uuid::Uuid::parse_str(&submission.run_id)?;
    let task_id = submission.task_id.clone();
    let receipt = crate::harness::orchestrated::submit_patch(state, submission).await?;
    state
        .harness
        .emit(crate::harness::HarnessEvent::PatchChanged {
            harness_id: run_id,
            task_id,
            patch_id: receipt.patch_id.clone(),
            state: "accepted".into(),
        });
    Ok(serde_json::to_string_pretty(&receipt)?)
}

async fn tool_harness_request_verify(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let run_id = harness_arg(args, "run_id")?;
    let task_id = harness_arg(args, "task_id")?;
    let (success, output_tail) = crate::harness::orchestrated::verify_task(
        state,
        run_id,
        task_id,
        harness_arg(args, "capability_token")?,
    )
    .await?;
    state
        .harness
        .emit(crate::harness::HarnessEvent::TaskVerification {
            harness_id: uuid::Uuid::parse_str(run_id)?,
            task_id: task_id.to_string(),
            success,
        });
    Ok(serde_json::to_string_pretty(
        &json!({"success":success,"output_tail":output_tail}),
    )?)
}

async fn tool_harness_report_blocked(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let run_id = harness_arg(args, "run_id")?;
    let task_id = harness_arg(args, "task_id")?;
    crate::harness::orchestrated::authorize_worker(
        state,
        run_id,
        task_id,
        harness_arg(args, "capability_token")?,
    )
    .await?;
    let reason = harness_arg(args, "reason")?;
    let tail = reason
        .char_indices()
        .rev()
        .take(4000)
        .last()
        .map_or(reason, |(start, _)| &reason[start..]);
    state
        .store
        .harness_update_task(run_id, task_id, "blocked", None, None, Some(tail))
        .await?;
    state
        .store
        .harness_record_decision(
            run_id,
            "worker_blocked",
            Some(&json!({"task_id":task_id,"reason":tail}).to_string()),
        )
        .await?;
    Ok(json!({"blocked":true,"independent_tasks_continue":true}).to_string())
}

async fn tool_harness_run_state(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let value = crate::harness::orchestrated::run_state(
        state,
        harness_arg(args, "run_id")?,
        harness_arg(args, "capability_token")?,
    )
    .await?;
    Ok(serde_json::to_string_pretty(&value)?)
}

async fn tool_harness_dispatch(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let session = crate::harness::orchestrated::dispatch_worker(
        state,
        harness_arg(args, "run_id")?,
        harness_arg(args, "task_id")?,
        harness_arg(args, "capability_token")?,
    )
    .await?;
    Ok(serde_json::to_string_pretty(
        &json!({"session_id":session.id,"session_name":session.name,"task_id":harness_arg(args,"task_id")?}),
    )?)
}

async fn tool_harness_transfer_ownership(state: &AppState, args: &Value) -> anyhow::Result<String> {
    crate::harness::orchestrated::transfer_ownership(
        state,
        harness_arg(args, "run_id")?,
        harness_arg(args, "path")?,
        harness_arg(args, "from_task")?,
        harness_arg(args, "to_task")?,
        harness_arg(args, "capability_token")?,
    )
    .await?;
    Ok(json!({"transferred":true}).to_string())
}

async fn tool_harness_create_repair_task(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let task: crate::harness::orchestrated::ExecutionTask = serde_json::from_value(
        args.get("task")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing `task`"))?,
    )?;
    let id = task.id.clone();
    crate::harness::orchestrated::create_repair_task(
        state,
        harness_arg(args, "run_id")?,
        harness_arg(args, "capability_token")?,
        task,
    )
    .await?;
    Ok(json!({"created":true,"task_id":id}).to_string())
}

async fn tool_harness_retry_or_block(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let run_id = harness_arg(args, "run_id")?;
    crate::harness::orchestrated::authorize_coordinator(
        state,
        run_id,
        harness_arg(args, "capability_token")?,
    )
    .await?;
    let action = harness_arg(args, "action")?;
    let reason = args.get("reason").and_then(Value::as_str).unwrap_or("");
    match action {
        "retry" => {
            let task_id = harness_arg(args, "task_id")?;
            let task = state
                .store
                .harness_task(run_id, task_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("unknown task {task_id}"))?;
            if task.status != "blocked" {
                anyhow::bail!("task {task_id} is not blocked");
            }
            state
                .store
                .harness_update_task(run_id, task_id, "ready", None, None, None)
                .await?;
        }
        "block" => {
            state
                .store
                .harness_update_run(
                    run_id,
                    "blocked",
                    None,
                    Some(&json!({"reason":reason}).to_string()),
                )
                .await?
        }
        "complete" => {
            let tasks = state.store.harness_tasks(run_id).await?;
            if tasks.iter().any(|t| t.status != "completed") {
                anyhow::bail!("cannot complete: tasks remain unfinished");
            }
            state
                .store
                .harness_update_run(
                    run_id,
                    "completed",
                    None,
                    Some(&json!({"review":reason}).to_string()),
                )
                .await?;
            if let Some(run) = state.store.harness_get_orchestrated_run(run_id).await? {
                if let Ok(config) =
                    crate::harness::HarnessConfig::load(std::path::Path::new(&run.workdir)).await
                {
                    crate::harness::orchestrated::transition_run_tracker(
                        state,
                        &config,
                        crate::task_sink::TrackerPhase::Done,
                    )
                    .await;
                }
            }
            if let Ok(id) = uuid::Uuid::parse_str(run_id) {
                state.harness.release_driver(id).await;
                state
                    .harness
                    .set_state(id, crate::harness::HarnessState::Done)
                    .await?;
                state
                    .harness
                    .emit(crate::harness::HarnessEvent::HarnessCompleted {
                        harness_id: id,
                        success: true,
                    });
            }
        }
        other => anyhow::bail!("unknown action {other}"),
    }
    state
        .store
        .harness_record_decision(
            run_id,
            action,
            Some(&json!({"reason":reason,"task_id":args.get("task_id")}).to_string()),
        )
        .await?;
    Ok(json!({"action":action,"accepted":true}).to_string())
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
        anyhow::anyhow!(
            "unknown `phase` {phase_str:?} (todo|in_progress|in_review|ready_to_test|done)"
        )
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

/// How long `agentum_report_status` waits for the transition seam before
/// answering with a "still running" note (#377). A COLD first call on the
/// github arm chains up to 7 sequential `gh` invocations (5 label
/// ensure-creates + the edit + a Projects write), each individually bounded at
/// 30s — worst case minutes, which outlives MCP client timeouts and surfaced
/// to callers as a raw socket close (the best-effort contract violated at the
/// transport layer). The work is detached, so on deadline it keeps running and
/// the labels/state still land.
const REPORT_STATUS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// #377: answer within `deadline` no matter what the seam does. Applied/Skipped
/// map through [`report_status_text`]; a panic inside the detached transition
/// becomes a readable note (isolated from the HTTP task, so the response can't
/// die with it); a deadline overrun reports "still running". Pure over the
/// JoinHandle → unit-testable with scripted tasks.
async fn bounded_transition_text(
    handle: tokio::task::JoinHandle<anyhow::Result<crate::task_sink::TransitionResult>>,
    deadline: std::time::Duration,
    provider: &str,
    phase: crate::task_sink::TrackerPhase,
) -> String {
    match tokio::time::timeout(deadline, handle).await {
        Err(_) => format!(
            "skipped (tracker still running in the background after {}s — the {provider} \
             update may still land; re-check the ticket rather than retrying immediately)",
            deadline.as_secs()
        ),
        Ok(Err(join)) => format!("skipped (tracker crashed, non-fatal): {join}"),
        Ok(Ok(outcome)) => report_status_text(outcome, provider, phase),
    }
}

/// Report a work item's pipeline phase to its tracker — a thin arm over
/// [`crate::task_sink::apply_tracker_transition`] (spec 005 F4), the same seam
/// the harness's own transitions use. Never reimplements label/state mechanics.
/// The seam runs DETACHED and deadline-bounded (#377) so a stalled `gh` chain
/// or a panic can never take the MCP response down with it.
async fn tool_report_status(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let (provider, id, url, phase) = parse_report_status_args(args)?;
    let bus = state.bus.clone();
    let prov = provider.clone();
    let handle = tokio::spawn(async move {
        crate::task_sink::apply_tracker_transition(
            &prov,
            &id,
            url.as_deref(),
            phase,
            crate::task_sink::TrackerEmit {
                bus: &bus,
                worktree_id: None,
            },
        )
        .await
    });
    Ok(bounded_transition_text(handle, REPORT_STATUS_DEADLINE, &provider, phase).await)
}

/// Parse + validate `agentum_sdd_loop` inputs (spec 016 F1). Pure →
/// unit-testable without AppState. Errors here are CALLER bugs (missing
/// `session`/`done`, a junk uuid) and DO surface as `isError: true`; everything
/// downstream — no live loop, a stale generation — is a SUCCESS string by
/// contract, so the check-in can never fail an agent's turn.
fn parse_sdd_loop_args(
    args: &Value,
) -> anyhow::Result<(uuid::Uuid, Option<u64>, bool, Option<String>)> {
    let session = args
        .get("session")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `session`"))?;
    let session = uuid::Uuid::parse_str(session)
        .map_err(|_| anyhow::anyhow!("`session` is not a uuid: {session}"))?;
    let done = args
        .get("done")
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow::anyhow!("missing `done` (boolean)"))?;
    let generation = args.get("generation").and_then(Value::as_u64);
    let summary = args
        .get("summary")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok((session, generation, done, summary))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SddLoopControlAction {
    Start,
    Stop,
    Status,
}

/// Parse the MCP control-plane shape separately from the agent check-in shape.
/// Keeping the verbs distinct is a safety property: a controller cannot
/// accidentally stop a loop by being interpreted as `done:true`.
fn parse_sdd_loop_control_args(
    args: &Value,
) -> anyhow::Result<(uuid::Uuid, SddLoopControlAction, Option<u32>)> {
    let session = args
        .get("session")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `session`"))?;
    let session = uuid::Uuid::parse_str(session)
        .map_err(|_| anyhow::anyhow!("`session` is not a uuid: {session}"))?;
    let action = match args.get("action").and_then(Value::as_str) {
        Some("start") => SddLoopControlAction::Start,
        Some("stop") => SddLoopControlAction::Stop,
        Some("status") => SddLoopControlAction::Status,
        Some(other) => {
            return Err(anyhow::anyhow!(
                "unknown `action` {other:?} (expected start, stop, or status)"
            ));
        }
        None => return Err(anyhow::anyhow!("missing `action`")),
    };
    let max_steps = match args.get("max_steps") {
        None => None,
        Some(value) => {
            let raw = value
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("`max_steps` must be an integer from 1 to 100"))?;
            if !(1..=100).contains(&raw) {
                return Err(anyhow::anyhow!(
                    "`max_steps` must be an integer from 1 to 100"
                ));
            }
            Some(raw as u32)
        }
    };
    if max_steps.is_some() && action != SddLoopControlAction::Start {
        return Err(anyhow::anyhow!(
            "`max_steps` is only valid when `action` is `start`"
        ));
    }
    Ok((session, action, max_steps))
}

/// MCP start/stop/status over the same server-owned loop seam as the desktop
/// toggle. The response is compact JSON text so agents can reliably read the
/// authoritative active/step/max_steps tuple from a normal MCP tool result.
async fn tool_sdd_loop_control(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let (session, action, max_steps) = parse_sdd_loop_control_args(args)?;
    let loop_state = match action {
        SddLoopControlAction::Status => super::sdd::read_loop_state(state, session),
        SddLoopControlAction::Start => {
            super::sdd::set_loop_active(state, session, true, max_steps).await?
        }
        SddLoopControlAction::Stop => {
            super::sdd::set_loop_active(state, session, false, None).await?
        }
    };
    Ok(json!({
        "session": session,
        "active": loop_state.active,
        "step": loop_state.step,
        "max_steps": loop_state.max_steps,
    })
    .to_string())
}

/// SDD-loop check-in — a thin arm over [`super::sdd::agent_checkin`] (spec 016
/// F1); the loop mechanics stay in `routes/sdd.rs` beside the map they mutate.
async fn tool_sdd_loop(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let (session, generation, done, summary) = parse_sdd_loop_args(args)?;
    Ok(super::sdd::agent_checkin(state, session, generation, done, summary).await)
}

/// Fetch (or list) the server-owned SDD playbooks. This is the universal
/// delivery path — the bootstrap line the SDD buttons/loop inject tells the
/// agent to call this; agents whose client renders MCP prompts can use
/// `prompts/get` instead, but `tools/call` works everywhere.
fn tool_sdd(args: &Value) -> anyhow::Result<String> {
    let name = args.get("name").and_then(Value::as_str);
    let extra = args.get("args").and_then(Value::as_str);
    match name {
        None => {
            let list = crate::sdd::playbooks()
                .into_iter()
                .map(|p| format!("- `{}` ({}): {}", p.name, p.title, p.description))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(format!(
                "Available SDD playbooks (fetch one with {{\"name\": …}}):\n{list}"
            ))
        }
        Some(name) => {
            let playbook = crate::sdd::get(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown playbook `{name}` — call agentum_sdd with no arguments to list them"
                )
            })?;
            Ok(crate::sdd::full_prompt(&playbook, extra))
        }
    }
}

/// MCP `prompts/list`: the SDD playbooks as native prompts. Clients that
/// surface these as slash commands (Claude Code, Gemini CLI) get `/sdd-*`
/// with zero per-agent installation.
fn prompts_list() -> Value {
    let prompts: Vec<Value> = crate::sdd::playbooks()
        .into_iter()
        .map(|p| {
            json!({
                "name": p.name,
                "title": p.title,
                "description": p.description,
                "arguments": [{
                    "name": "args",
                    "description": "Optional free-form arguments (e.g. `autonomous` or a spec id for sdd-orchestrate)",
                    "required": false,
                }],
            })
        })
        .collect();
    json!({ "prompts": prompts })
}

/// MCP `prompts/get`: resolve one playbook into a user-role message.
fn prompts_get(params: Option<&Value>) -> Result<Value, (i64, String)> {
    let name = params
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .ok_or((-32602, "missing prompt name".to_string()))?;
    let playbook =
        crate::sdd::get(name).ok_or_else(|| (-32602, format!("unknown prompt: {name}")))?;
    let args = params
        .and_then(|p| p.get("arguments"))
        .and_then(|a| a.get("args"))
        .and_then(Value::as_str);
    Ok(json!({
        "description": playbook.description,
        "messages": [{
            "role": "user",
            "content": { "type": "text", "text": crate::sdd::full_prompt(&playbook, args) },
        }],
    }))
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
    fn agentum_sdd_is_advertised_regardless_of_the_orchestration_gate() {
        assert!(tool_names(true).contains(&"agentum_sdd".to_string()));
        assert!(tool_names(false).contains(&"agentum_sdd".to_string()));
    }

    #[test]
    fn tool_sdd_lists_fetches_and_rejects_unknown_playbooks() {
        // No name → a discoverable list of all six playbooks.
        let list = tool_sdd(&json!({})).unwrap();
        for name in [
            "sdd-spec",
            "sdd-spec-socratic",
            "sdd-orchestrate",
            "sdd-status",
        ] {
            assert!(list.contains(name), "list mentions {name}");
        }
        // Named fetch → the playbook body (with args appended when given).
        let body = tool_sdd(&json!({ "name": "sdd-orchestrate", "args": "autonomous" })).unwrap();
        assert!(
            body.contains("validate_handoff"),
            "carries the real procedure"
        );
        assert!(body.contains("Arguments: autonomous"));
        // Unknown → an actionable error, not a silent empty result.
        let err = tool_sdd(&json!({ "name": "sdd-nope" }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("sdd-nope") && err.contains("list"));
    }

    #[test]
    fn prompts_surface_serves_the_sdd_playbooks() {
        let list = prompts_list();
        let names: Vec<&str> = list["prompts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["name"].as_str())
            .collect();
        assert_eq!(names.len(), 6);
        assert!(names.contains(&"sdd-spec"));

        let got = prompts_get(Some(&json!({
            "name": "sdd-status",
            "arguments": { "args": "" },
        })))
        .unwrap();
        let text = got["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("STATE.md"));

        let err = prompts_get(Some(&json!({ "name": "nope" }))).unwrap_err();
        assert_eq!(err.0, -32602);
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

    // --- spec 016 F1: agentum_sdd_loop ---

    #[test]
    fn sdd_loop_tool_is_advertised_regardless_of_the_orchestration_gate() {
        // Loop control, not the mailbox/DAG surface: deliberately NOT in
        // ORCHESTRATION_TOOLS, so it is advertised (and callable) whenever the
        // MCP server itself is on.
        assert!(!is_orchestration_tool("agentum_sdd_loop"));
        assert!(tool_names(true).contains(&"agentum_sdd_loop".to_string()));
        assert!(tool_names(false).contains(&"agentum_sdd_loop".to_string()));
    }

    #[test]
    fn sdd_loop_control_is_advertised_regardless_of_the_orchestration_gate() {
        assert!(!is_orchestration_tool("agentum_sdd_loop_control"));
        assert!(tool_names(true).contains(&"agentum_sdd_loop_control".to_string()));
        assert!(tool_names(false).contains(&"agentum_sdd_loop_control".to_string()));
    }

    #[test]
    fn parse_sdd_loop_control_args_separates_control_from_checkin() {
        let u = uuid::Uuid::new_v4();
        assert!(parse_sdd_loop_control_args(&json!({})).is_err());
        assert!(
            parse_sdd_loop_control_args(&json!({ "session": "nope", "action": "start" })).is_err()
        );
        assert!(
            parse_sdd_loop_control_args(&json!({ "session": u.to_string(), "action": "continue" }))
                .is_err()
        );
        assert!(
            parse_sdd_loop_control_args(
                &json!({ "session": u.to_string(), "action": "status", "max_steps": 4 })
            )
            .is_err(),
            "step caps only make sense for start"
        );
        assert!(
            parse_sdd_loop_control_args(
                &json!({ "session": u.to_string(), "action": "start", "max_steps": 0 })
            )
            .is_err()
        );

        let (id, action, max_steps) = parse_sdd_loop_control_args(
            &json!({ "session": u.to_string(), "action": "start", "max_steps": 12 }),
        )
        .unwrap();
        assert_eq!(id, u);
        assert_eq!(action, SddLoopControlAction::Start);
        assert_eq!(max_steps, Some(12));

        let (_, action, max_steps) =
            parse_sdd_loop_control_args(&json!({ "session": u.to_string(), "action": "status" }))
                .unwrap();
        assert_eq!(action, SddLoopControlAction::Status);
        assert_eq!(max_steps, None);
    }

    #[test]
    fn parse_sdd_loop_args_requires_session_and_done() {
        let u = uuid::Uuid::new_v4();

        // Missing session / missing done / junk uuid → caller errors.
        assert!(parse_sdd_loop_args(&json!({ "done": true })).is_err());
        assert!(parse_sdd_loop_args(&json!({ "session": u.to_string() })).is_err());
        assert!(parse_sdd_loop_args(&json!({ "session": "not-a-uuid", "done": true })).is_err());
        // A stringly-typed `done` is a caller bug, never coerced.
        assert!(parse_sdd_loop_args(&json!({ "session": u.to_string(), "done": "true" })).is_err());

        // Minimal valid shape: generation + summary are optional.
        let (id, generation, done, summary) =
            parse_sdd_loop_args(&json!({ "session": u.to_string(), "done": false })).unwrap();
        assert_eq!(id, u);
        assert_eq!(generation, None);
        assert!(!done);
        assert_eq!(summary, None);

        // Full shape.
        let (_, generation, done, summary) = parse_sdd_loop_args(&json!({
            "session": u.to_string(),
            "done": true,
            "generation": 7,
            "summary": "F1 green",
        }))
        .unwrap();
        assert_eq!(generation, Some(7));
        assert!(done);
        assert_eq!(summary.as_deref(), Some("F1 green"));
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

    /// `in_review` is a first-class phase (the SDD Reviewer step reports it) —
    /// it must parse AND be advertised in the tool spec's enum, or a
    /// schema-respecting client can never send it.
    #[test]
    fn report_status_accepts_in_review() {
        use crate::task_sink::TrackerPhase;

        let (_, _, _, phase) = parse_report_status_args(&json!({
            "provider": "github",
            "url": "https://github.com/owner/repo/issues/42",
            "phase": "in_review",
        }))
        .unwrap();
        assert_eq!(phase, TrackerPhase::InReview);

        let specs = tool_specs(true);
        let phase_enum = specs
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "agentum_report_status")
            .expect("agentum_report_status is in the catalog")["inputSchema"]["properties"]["phase"]
            ["enum"]
            .clone();
        assert!(
            phase_enum.as_array().unwrap().contains(&json!("in_review")),
            "tool-spec phase enum must advertise in_review, got {phase_enum}"
        );
        let provider_enum = specs
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "agentum_report_status")
            .unwrap()["inputSchema"]["properties"]["provider"]["enum"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(provider_enum, vec![json!("github"), json!("linear")]);
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
                Ok(TransitionResult::Skipped("unknown tracker provider".into())),
                "board",
                TrackerPhase::Todo,
            ),
            "skipped: unknown tracker provider"
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
            sdd_loops: Default::default(),
            events_ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    #[tokio::test]
    async fn sdd_loop_control_delegates_to_the_shared_loop_state() {
        let state = fresh_state().await;
        let session = state
            .store
            .create_session(agentum_core::NewSession {
                name: "mcp-loop-control".into(),
                workdir: "/tmp/mcp-loop-control".into(),
                tool: "claude".into(),
                model: None,
                flags: vec![],
                card_id: None,
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await
            .unwrap();

        let status = tool_sdd_loop_control(
            &state,
            &json!({ "session": session.id.to_string(), "action": "status" }),
        )
        .await
        .unwrap();
        let status: Value = serde_json::from_str(&status).unwrap();
        assert_eq!(status["session"], session.id.to_string());
        assert_eq!(status["active"], false);
        assert_eq!(status["step"], 0);
        assert_eq!(status["max_steps"], 0);

        let stopped = tool_sdd_loop_control(
            &state,
            &json!({ "session": session.id.to_string(), "action": "stop" }),
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&stopped).unwrap()["active"],
            false
        );

        let err = tool_sdd_loop_control(
            &state,
            &json!({ "session": session.id.to_string(), "action": "start" }),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("session is not running"), "got: {err}");
    }

    /// Legacy board metadata remains readable as input but is a bounded,
    /// non-writing best-effort skip through the shared transition seam.
    #[tokio::test]
    async fn report_status_legacy_board_provider_is_non_writing() {
        let state = fresh_state().await;
        let text = tool_report_status(
            &state,
            &json!({ "provider": "board", "id": "AG-12", "phase": "in_progress" }),
        )
        .await
        .unwrap();
        assert_eq!(text, "skipped: unknown tracker provider \"board\"");

        // Unknown provider flows to the seam's Skipped — visible, non-fatal.
        let text = tool_report_status(
            &state,
            &json!({ "provider": "jira", "id": "X-1", "phase": "done" }),
        )
        .await
        .unwrap();
        assert!(text.starts_with("skipped:"), "got: {text}");
    }

    // --- #377: the seam can stall or crash; the tool must always answer ---

    /// The transport pin: a stalled tracker chain answers with a "still
    /// running" note before the client's own timeout, a panic inside the seam
    /// is isolated to a readable note (the old behavior killed the whole HTTP
    /// response → the client saw a raw socket close), and a fast outcome still
    /// reads exactly as before.
    #[tokio::test]
    async fn report_status_bounds_a_stalled_or_crashing_tracker() {
        use crate::task_sink::{TrackerPhase, TransitionResult};

        // Stall: outlives the deadline → note, never a hang or an Err.
        let h: tokio::task::JoinHandle<anyhow::Result<TransitionResult>> = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Ok(TransitionResult::Applied)
        });
        let text = bounded_transition_text(
            h,
            std::time::Duration::from_millis(40),
            "github",
            TrackerPhase::InProgress,
        )
        .await;
        assert!(
            text.starts_with("skipped (tracker still running"),
            "got: {text}"
        );

        // Panic inside the seam: isolated into the best-effort note.
        let h: tokio::task::JoinHandle<anyhow::Result<TransitionResult>> =
            tokio::spawn(async { panic!("label table hole") });
        let text = bounded_transition_text(
            h,
            std::time::Duration::from_secs(5),
            "github",
            TrackerPhase::Done,
        )
        .await;
        assert!(
            text.starts_with("skipped (tracker crashed, non-fatal):"),
            "got: {text}"
        );

        // Fast success is byte-identical to the pre-#377 text.
        let h: tokio::task::JoinHandle<anyhow::Result<TransitionResult>> =
            tokio::spawn(async { Ok(TransitionResult::Applied) });
        let text = bounded_transition_text(
            h,
            std::time::Duration::from_secs(5),
            "board",
            TrackerPhase::Todo,
        )
        .await;
        assert_eq!(text, "applied: board → Todo");
    }

    // --- #378: session lifecycle end, pane injection, harness run, bounded list ---

    #[test]
    fn lifecycle_and_run_tools_are_in_the_catalog_ungated() {
        // Control-plane verbs like spawn: advertised regardless of the
        // orchestration gate (they are not the mailbox/DAG surface).
        for tool in [
            "agentum_stop_session",
            "agentum_inject_prompt",
            "agentum_harness_run",
        ] {
            assert!(!is_orchestration_tool(tool));
            assert!(tool_names(true).contains(&tool.to_string()), "{tool} on");
            assert!(tool_names(false).contains(&tool.to_string()), "{tool} off");
        }
    }

    #[test]
    fn session_filters_bound_and_select_the_listing() {
        let rows: Vec<Value> = (0..120)
            .map(|i| {
                json!({
                    "id": format!("id-{i}"),
                    "name": if i % 2 == 0 { format!("worker-{i}") } else { format!("terminal-{i}") },
                    "tool": "claude",
                    "status": if i < 100 { "Stopped" } else { "Running" },
                    "workdir": if i < 60 { "/projects/alpha" } else { "/projects/beta" },
                })
            })
            .collect();

        // No filters: capped at the default page with the truncation flagged —
        // the ~93 KB whole-table dump (#378) can't happen again.
        let out = apply_session_filters(rows.clone(), &json!({}));
        assert_eq!(out["total_matching"], 120);
        assert_eq!(out["returned"], 50);
        assert_eq!(out["truncated"], true);

        // Status is exact + case-insensitive; substring filters compose.
        let out = apply_session_filters(rows.clone(), &json!({ "status": "running" }));
        assert_eq!(out["total_matching"], 20);
        assert_eq!(out["truncated"], false);
        let out = apply_session_filters(
            rows.clone(),
            &json!({ "name_contains": "worker", "workdir_contains": "beta" }),
        );
        assert_eq!(out["total_matching"], 30);

        // The limit clamps to something sane in both directions.
        let out = apply_session_filters(rows.clone(), &json!({ "limit": 0 }));
        assert_eq!(out["returned"], 1);
        let out = apply_session_filters(rows, &json!({ "limit": 10 }));
        assert_eq!(out["returned"], 10);
        assert_eq!(out["truncated"], true);
    }

    #[tokio::test]
    async fn stop_session_rejects_unknown_sessions_and_modes() {
        let state = fresh_state().await;
        // Unknown ref (neither uuid nor name) → an actionable caller error.
        let err = tool_stop_session(&state, &json!({ "session": "nope-123" }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no session"), "got: {err}");
        // A junk mode is a caller bug, never coerced.
        let err = tool_stop_session(&state, &json!({ "session": "x", "mode": "vaporize" }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown `mode`"), "got: {err}");
        // Missing session arg entirely.
        assert!(tool_stop_session(&state, &json!({})).await.is_err());
    }

    #[tokio::test]
    async fn inject_prompt_requires_a_live_session() {
        let state = fresh_state().await;
        assert!(
            tool_inject_prompt(&state, &json!({ "prompt": "hi" }))
                .await
                .is_err()
        );
        assert!(
            tool_inject_prompt(&state, &json!({ "session": "ghost" }))
                .await
                .is_err()
        );
        let err = tool_inject_prompt(&state, &json!({ "session": "ghost", "prompt": "hi" }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no session"), "got: {err}");
    }

    #[tokio::test]
    async fn harness_run_fails_fast_without_a_ready_surface() {
        let state = fresh_state().await;
        let dir = tempfile::tempdir().unwrap();
        // No `.agentum-harness/` → the register step fails loudly (the tool
        // points the caller at agentum_harness_check), nothing is spawned.
        let err = tool_harness_run(&state, &json!({ "workdir": dir.path().to_string_lossy() }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("register harness"), "got: {err}");
        assert!(tool_harness_run(&state, &json!({})).await.is_err());
    }
}
