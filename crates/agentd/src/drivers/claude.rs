//! Claude Code: `claude -p` speaking stream-json on stdio, permission prompts over the control
//! protocol (`--permission-prompt-tool stdio`).
use super::{Agent, DriverOut, Proc, Session, StartOpts, Tx, permission_options, tool_kind, tool_title, unified_from_hunks};
use agentum_proto::machine::ModelInfo;
use agentum_proto::{AgentKind, AskKind, AskOption, EventKind, FileDiff, Input, Permission, PlanEntry, RateLimit, StopReason, Usage};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::oneshot;

/// What the translator wants done with one line of output.
#[derive(Debug, PartialEq)]
pub enum Act {
    Ev(EventKind),
    /// Write this to the CLI's stdin.
    Write(Value),
    /// A control_response to one of our requests (request id, ok, response).
    Control(String, bool, Value),
}

#[derive(Debug, Clone)]
struct ToolUse {
    name: String,
    input: Value,
}

#[derive(Debug, Clone)]
struct PendingAsk {
    tool: String,
    input: Value,
    suggestions: Value,
    question: bool,
}

/// Claude's stream-json → thread events.
#[derive(Default)]
pub struct Translator {
    cwd: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    msg_id: String,
    /// The content block being streamed: (message id, index, item id).
    open: Option<(String, u64, String)>,
    /// Blocks seen per message, to name items when there are no partial messages.
    blocks: HashMap<String, u64>,
    tools: HashMap<String, ToolUse>,
    asks: HashMap<String, PendingAsk>,
    /// We asked for an interrupt; the turn's error result means "interrupted".
    pub interrupting: bool,
    /// Prompt size of the last API call (input + cache), and its output.
    ctx_in: u64,
    ctx_out: u64,
    context_window: Option<u64>,
    total_cost: f64,
    warned: HashSet<String>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}
fn n(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(|x| x.as_u64()).unwrap_or(0)
}

impl Translator {
    pub fn new(cwd: &str) -> Translator {
        Translator { cwd: cwd.to_string(), ..Default::default() }
    }

    fn warn_once(&mut self, what: String) {
        if self.warned.insert(what.clone()) {
            tracing::info!("claude: ignoring {what}");
        }
    }

    fn item_for(&mut self, msg: &str, kind_hint: &str) -> String {
        if let Some((m, _, item)) = &self.open
            && m == msg
            && item.ends_with(kind_hint)
        {
            return item.clone();
        }
        let i = self.blocks.entry(msg.to_string()).or_insert(0);
        *i += 1;
        format!("{msg}:{}{kind_hint}", *i - 1)
    }

    pub fn line(&mut self, v: &Value) -> Vec<Act> {
        let mut out = Vec::new();
        let t = s(v, "type");
        match t.as_str() {
            "system" => match s(v, "subtype").as_str() {
                "init" => {
                    let sid = s(v, "session_id");
                    let model = v.get("model").and_then(|m| m.as_str()).map(str::to_string);
                    if !sid.is_empty() && (self.session_id.as_deref() != Some(&sid) || self.model != model) {
                        self.session_id = Some(sid.clone());
                        self.model = model.clone();
                        out.push(Act::Ev(EventKind::Session { session_id: sid, model }));
                    }
                }
                "compact_boundary" => out.push(Act::Ev(EventKind::Notice { text: "Context compacted".into() })),
                _ => {}
            },
            "stream_event" => self.stream_event(v.get("event").unwrap_or(&Value::Null), &mut out),
            "assistant" => self.assistant(v, &mut out),
            "user" => self.user(v, &mut out),
            "result" => self.result(v, &mut out),
            "rate_limit_event" => {
                if let Some(l) = rate_limits(v.get("rate_limit_info").unwrap_or(&Value::Null))
                    && !l.is_empty()
                {
                    out.push(Act::Ev(EventKind::RateLimits { limits: l, plan: subscription() }));
                }
            }
            "control_request" => self.control_request(v, &mut out),
            "control_cancel_request" => {
                let id = s(v, "request_id");
                if self.asks.remove(&id).is_some() {
                    out.push(Act::Ev(EventKind::Answered { id, option: "cancelled".into(), text: None }));
                }
            }
            "control_response" => {
                let r = v.get("response").unwrap_or(&Value::Null);
                let ok = s(r, "subtype") == "success";
                let body = if ok { r.get("response").cloned().unwrap_or(Value::Null) } else { r.get("error").cloned().unwrap_or(Value::Null) };
                out.push(Act::Control(s(r, "request_id"), ok, body));
            }
            "keep_alive" => {}
            other => self.warn_once(format!("message type {other:?}")),
        }
        out
    }

    fn stream_event(&mut self, e: &Value, out: &mut Vec<Act>) {
        match s(e, "type").as_str() {
            "message_start" => {
                let m = e.get("message").unwrap_or(&Value::Null);
                self.msg_id = s(m, "id");
                if let Some(u) = m.get("usage") {
                    self.ctx_in = n(u, "input_tokens") + n(u, "cache_read_input_tokens") + n(u, "cache_creation_input_tokens");
                    self.ctx_out = n(u, "output_tokens");
                }
            }
            "content_block_start" => {
                let idx = n(e, "index");
                let b = e.get("content_block").unwrap_or(&Value::Null);
                let item = match s(b, "type").as_str() {
                    "tool_use" | "server_tool_use" => s(b, "id"),
                    "thinking" | "redacted_thinking" => format!("{}:{idx}t", self.msg_id),
                    _ => format!("{}:{idx}m", self.msg_id),
                };
                *self.blocks.entry(self.msg_id.clone()).or_insert(0) = idx + 1;
                self.open = Some((self.msg_id.clone(), idx, item));
            }
            "content_block_delta" => {
                let d = e.get("delta").unwrap_or(&Value::Null);
                let item = self.open.as_ref().map(|o| o.2.clone()).unwrap_or_default();
                match s(d, "type").as_str() {
                    "text_delta" => {
                        let text = s(d, "text");
                        if !text.is_empty() {
                            out.push(Act::Ev(EventKind::Delta { item, text }));
                        }
                    }
                    "thinking_delta" => {
                        let text = s(d, "thinking");
                        if !text.is_empty() {
                            out.push(Act::Ev(EventKind::ThinkingDelta { item, text }));
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(u) = e.get("usage") {
                    self.ctx_out = n(u, "output_tokens");
                }
            }
            _ => {}
        }
    }

    fn assistant(&mut self, v: &Value, out: &mut Vec<Act>) {
        let m = v.get("message").unwrap_or(&Value::Null);
        let msg = s(m, "id");
        if !msg.is_empty() {
            self.msg_id = msg.clone();
        }
        if let Some(u) = m.get("usage")
            && self.ctx_in == 0
        {
            self.ctx_in = n(u, "input_tokens") + n(u, "cache_read_input_tokens") + n(u, "cache_creation_input_tokens");
        }
        for b in m.get("content").and_then(|c| c.as_array()).into_iter().flatten() {
            match s(b, "type").as_str() {
                "text" => {
                    let text = s(b, "text");
                    let item = self.item_for(&msg, "m");
                    if !text.is_empty() {
                        out.push(Act::Ev(EventKind::Message { item, text }));
                    }
                }
                "thinking" => {
                    let text = s(b, "thinking");
                    let item = self.item_for(&msg, "t");
                    if !text.is_empty() {
                        out.push(Act::Ev(EventKind::Thinking { item, text }));
                    }
                }
                "tool_use" | "server_tool_use" => {
                    let id = s(b, "id");
                    let name = s(b, "name");
                    let input = b.get("input").cloned().unwrap_or(Value::Null);
                    self.tools.insert(id.clone(), ToolUse { name: name.clone(), input: input.clone() });
                    if name == "TodoWrite" {
                        out.push(Act::Ev(EventKind::Plan { entries: todo_entries(&input) }));
                    }
                    let title = tool_title(&name, &input, &self.cwd);
                    out.push(Act::Ev(EventKind::Tool { item: id, kind: tool_kind(&name), name, title, input }));
                }
                _ => {}
            }
        }
    }

    fn user(&mut self, v: &Value, out: &mut Vec<Act>) {
        let m = v.get("message").unwrap_or(&Value::Null);
        let Some(content) = m.get("content").and_then(|c| c.as_array()) else { return };
        let meta = v.get("tool_use_result");
        for b in content {
            if s(b, "type") != "tool_result" {
                continue;
            }
            let id = s(b, "tool_use_id");
            let ok = !b.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false);
            let output = match b.get("content") {
                Some(Value::String(t)) => t.clone(),
                Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"),
                _ => String::new(),
            };
            let diffs = match self.tools.remove(&id) {
                Some(tu) if ok => diffs_for(&tu, meta, &self.cwd),
                _ => vec![],
            };
            let output = (!output.is_empty()).then(|| crate::util::truncate(&output, 32 * 1024));
            out.push(Act::Ev(EventKind::ToolDone { item: id, ok, output, diffs }));
        }
    }

    fn result(&mut self, v: &Value, out: &mut Vec<Act>) {
        let sub = s(v, "subtype");
        let is_error = v.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false);
        let stop = match sub.as_str() {
            "success" if !is_error => StopReason::End,
            "error_max_turns" => StopReason::MaxTurns,
            _ if self.interrupting => StopReason::Interrupted,
            _ => StopReason::Error,
        };
        self.interrupting = false;
        // The context window from the model's usage.
        if let Some(mu) = v.get("modelUsage").and_then(|m| m.as_object()) {
            let pick = self.model.as_ref().and_then(|m| mu.get(m)).or_else(|| mu.values().next());
            if let Some(w) = pick.and_then(|p| p.get("contextWindow")).and_then(|w| w.as_u64()) {
                self.context_window = Some(w);
            }
        }
        let u = v.get("usage").unwrap_or(&Value::Null);
        let usage = Usage {
            input: n(u, "input_tokens"),
            output: n(u, "output_tokens"),
            cache_read: n(u, "cache_read_input_tokens"),
            cache_write: n(u, "cache_creation_input_tokens"),
            context_used: (self.ctx_in > 0).then_some(self.ctx_in + self.ctx_out),
            context_window: self.context_window,
        };
        // total_cost_usd counts the whole process; the turn's share is the per-result usage.
        let total = v.get("total_cost_usd").and_then(|c| c.as_f64());
        let since: Option<f64> = v
            .get("model_usage_since_last_result")
            .and_then(|m| m.as_object())
            .map(|m| m.values().filter_map(|x| x.get("costUSD").and_then(|c| c.as_f64())).sum());
        let cost = since.or_else(|| total.map(|t| (t - self.total_cost).max(0.0)));
        if let Some(t) = total {
            self.total_cost = t;
        }
        if stop == StopReason::Error {
            let msg = v
                .get("result")
                .and_then(|r| r.as_str())
                .map(str::to_string)
                .or_else(|| v.get("errors").and_then(|e| e.as_array()).map(|e| e.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n")))
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| format!("Claude stopped ({sub})"));
            out.push(Act::Ev(EventKind::Error { message: msg, fatal: false }));
        }
        out.push(Act::Ev(EventKind::Usage { usage: usage.clone() }));
        out.push(Act::Ev(EventKind::TurnDone { turn: 0, stop, usage: Some(usage), cost_usd: cost, duration_ms: n(v, "duration_ms") }));
        self.ctx_in = 0;
        self.ctx_out = 0;
    }

    fn control_request(&mut self, v: &Value, out: &mut Vec<Act>) {
        let id = s(v, "request_id");
        let r = v.get("request").unwrap_or(&Value::Null);
        match s(r, "subtype").as_str() {
            "can_use_tool" => {
                let tool = s(r, "tool_name");
                let input = r.get("input").cloned().unwrap_or(Value::Null);
                if tool == "AskUserQuestion" {
                    let q = input.get("questions").and_then(|q| q.get(0)).cloned().unwrap_or(Value::Null);
                    let options = q
                        .get("options")
                        .and_then(|o| o.as_array())
                        .into_iter()
                        .flatten()
                        .map(|o| AskOption { id: s(o, "label"), label: s(o, "label"), kind: "answer".into() })
                        .collect();
                    let detail = q.get("header").and_then(|h| h.as_str()).map(str::to_string);
                    self.asks.insert(id.clone(), PendingAsk { tool, input: input.clone(), suggestions: Value::Null, question: true });
                    out.push(Act::Ev(EventKind::Ask { id, kind: AskKind::Question, title: s(&q, "question"), detail, input, options }));
                } else {
                    let what = tool_title(&tool, &input, &self.cwd);
                    let title = if what == tool { tool.clone() } else { format!("{tool}: {what}") };
                    let detail = r.get("description").and_then(|d| d.as_str()).filter(|d| *d != what).map(str::to_string);
                    let suggestions = r.get("permission_suggestions").cloned().unwrap_or(Value::Null);
                    self.asks.insert(id.clone(), PendingAsk { tool, input: input.clone(), suggestions, question: false });
                    out.push(Act::Ev(EventKind::Ask { id, kind: AskKind::Permission, title, detail, input, options: permission_options() }));
                }
            }
            other => {
                // Hooks, MCP over the SDK…: not ours. Say so, so the CLI doesn't wait.
                self.warn_once(format!("control request {other:?}"));
                out.push(Act::Write(json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": format!("agentd does not handle {other}")}})));
            }
        }
    }

    /// The reply to an `Ask`, and its `Answered` event.
    pub fn answer(&mut self, ask: &str, option: &str, text: Option<&str>) -> anyhow::Result<(Value, EventKind)> {
        let p = self.asks.remove(ask).ok_or_else(|| anyhow::anyhow!("no open question {ask}"))?;
        let resp = if p.question {
            if option == "deny" || option == "cancel" {
                json!({"behavior": "deny", "message": text.unwrap_or("The user declined to answer.")})
            } else {
                let chosen = if option.is_empty() || option == "other" { text.unwrap_or("").to_string() } else { option.to_string() };
                let many: Option<serde_json::Map<String, Value>> = text.and_then(|t| serde_json::from_str(t).ok());
                let mut answers = serde_json::Map::new();
                for q in p.input.get("questions").and_then(|q| q.as_array()).into_iter().flatten() {
                    let qt = s(q, "question");
                    let a = many.as_ref().and_then(|m| m.get(&qt)).cloned().unwrap_or_else(|| Value::String(chosen.clone()));
                    answers.insert(qt, a);
                }
                let mut input = p.input.clone();
                if let Some(o) = input.as_object_mut() {
                    o.insert("answers".into(), Value::Object(answers));
                }
                json!({"behavior": "allow", "updatedInput": input})
            }
        } else {
            match option {
                "allow" => json!({"behavior": "allow", "updatedInput": p.input}),
                "allow-always" => {
                    let mut r = json!({"behavior": "allow", "updatedInput": p.input});
                    if p.suggestions.as_array().is_some_and(|a| !a.is_empty()) {
                        r["updatedPermissions"] = p.suggestions.clone();
                    }
                    r
                }
                _ => json!({"behavior": "deny", "message": text.unwrap_or("The user denied this.")}),
            }
        };
        let _ = &p.tool;
        let msg = json!({"type": "control_response", "response": {"subtype": "success", "request_id": ask, "response": resp}});
        Ok((msg, EventKind::Answered { id: ask.to_string(), option: option.to_string(), text: text.map(str::to_string) }))
    }

    /// Questions still waiting (for a restart or a stop).
    pub fn open_asks(&self) -> Vec<String> {
        self.asks.keys().cloned().collect()
    }
}

fn todo_entries(input: &Value) -> Vec<PlanEntry> {
    input
        .get("todos")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
        .map(|t| PlanEntry {
            text: t.get("content").or_else(|| t.get("activeForm")).and_then(|c| c.as_str()).unwrap_or("").to_string(),
            status: match s(t, "status").as_str() {
                "in_progress" => "in-progress".into(),
                "completed" => "done".into(),
                _ => "pending".into(),
            },
        })
        .collect()
}

/// File diffs of an edit tool, from its input and the CLI's `tool_use_result`.
fn diffs_for(tu: &ToolUse, meta: Option<&Value>, cwd: &str) -> Vec<FileDiff> {
    let i = &tu.input;
    let path = i.get("file_path").or_else(|| i.get("notebook_path")).and_then(|p| p.as_str()).unwrap_or("").to_string();
    let rel = path.strip_prefix(cwd).map(|p| p.trim_start_matches('/').to_string()).unwrap_or_else(|| path.clone());
    let hunks = meta.and_then(|m| m.get("structuredPatch")).and_then(|h| h.as_array()).cloned().unwrap_or_default();
    let unified = unified_from_hunks(&rel, &hunks);
    let st = |k: &str| i.get(k).and_then(|v| v.as_str()).map(str::to_string);
    match tu.name.as_str() {
        "Edit" => vec![FileDiff { path, unified, old: st("old_string"), new: st("new_string") }],
        "MultiEdit" => {
            let edits = i.get("edits").and_then(|e| e.as_array()).cloned().unwrap_or_default();
            if unified.is_some() || edits.is_empty() {
                vec![FileDiff { path, unified, old: None, new: None }]
            } else {
                edits.iter().map(|e| FileDiff { path: path.clone(), unified: None, old: Some(s(e, "old_string")), new: Some(s(e, "new_string")) }).collect()
            }
        }
        "Write" => {
            let old = meta.and_then(|m| m.get("originalFile")).and_then(|o| o.as_str()).map(str::to_string);
            vec![FileDiff { path, unified, old, new: st("content") }]
        }
        "NotebookEdit" => vec![FileDiff { path, unified, old: None, new: st("new_source") }],
        _ => vec![],
    }
}

/// Claude Code's sign-in on this machine (its credentials file; macOS keeps it in the Keychain,
/// not read here). The token stays in this process: never logged, never sent to the hub.
struct Creds {
    token: String,
    expires_ms: Option<i64>,
    plan: Option<String>,
}
fn creds() -> Option<Creds> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| crate::util::home().join(".claude"));
    let v: Value = serde_json::from_slice(&std::fs::read(dir.join(".credentials.json")).ok()?).ok()?;
    let o = v.get("claudeAiOauth")?;
    let st = |k: &str| o.get(k).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string);
    Some(Creds { token: st("accessToken").unwrap_or_default(), expires_ms: o.get("expiresAt").and_then(|e| e.as_i64()), plan: plan_label(st("subscriptionType"), st("rateLimitTier")) })
}
/// "Max (20x)" from the rate-limit tier, else the subscription ("Pro").
pub fn plan_label(sub: Option<String>, tier: Option<String>) -> Option<String> {
    if let Some(n) = tier.as_deref().and_then(|t| t.split("max_").nth(1)).and_then(|t| t.split('x').next()).filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
        return Some(format!("Max ({n}x)"));
    }
    sub.map(|s| s[..1].to_uppercase() + &s[1..])
}
/// The Claude subscription this machine is signed in with.
pub fn subscription() -> Option<String> {
    creds()?.plan
}

/// The account's limits now, from Anthropic's OAuth usage endpoint (as Claude Code's /usage and
/// BB's usage tracker read them), with the token Claude Code keeps. An expired token isn't
/// refreshed here: refreshing rotates it under Claude Code.
pub async fn usage_now() -> anyhow::Result<(Vec<RateLimit>, Option<String>)> {
    let c = creds().filter(|c| !c.token.is_empty()).ok_or_else(|| anyhow::anyhow!("Claude Code isn't signed in on this machine"))?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    if c.expires_ms.is_some_and(|e| e <= now) {
        anyhow::bail!("Claude Code's sign-in on this machine has expired: open Claude Code there to renew it");
    }
    let headers = vec![
        ("Authorization", format!("Bearer {}", c.token)),
        ("anthropic-beta", "oauth-2025-04-20".to_string()),
        ("Accept", "application/json".to_string()),
        ("Content-Type", "application/json".to_string()),
        ("User-Agent", format!("agentd/{}", env!("CARGO_PKG_VERSION"))),
    ];
    let (status, body) = tokio::task::spawn_blocking(move || crate::https::get("api.anthropic.com", "/api/oauth/usage", &headers)).await??;
    match status {
        200..=299 => {}
        401 => anyhow::bail!("Anthropic refused Claude Code's sign-in on this machine: open Claude Code there to renew it"),
        429 => anyhow::bail!("Anthropic throttled the usage check (not your limit): try again later"),
        s => anyhow::bail!("Claude usage check: HTTP {s}"),
    }
    let v: Value = serde_json::from_slice(&body).map_err(|_| anyhow::anyhow!("Claude usage check: the answer is not JSON"))?;
    Ok((oauth_usage(&v), c.plan))
}

/// The OAuth usage answer → windows: `five_hour`, `seven_day` (utilization in percent) and the
/// model-scoped weekly ones under `limits` ("Fable", "Opus"…).
pub fn oauth_usage(v: &Value) -> Vec<RateLimit> {
    let mut out = Vec::new();
    for (k, window) in [("five_hour", "5h"), ("seven_day", "weekly")] {
        if let Some(u) = v.get(k).and_then(|w| w.get("utilization")).and_then(|u| u.as_f64()) {
            out.push(RateLimit { window: window.into(), used_percent: u, resets_at: v[k].get("resets_at").and_then(|r| r.as_str()).and_then(iso_ms) });
        }
    }
    for l in v.get("limits").and_then(|l| l.as_array()).into_iter().flatten() {
        let name = l.pointer("/scope/model/display_name").and_then(|n| n.as_str()).map(str::trim).filter(|n| !n.is_empty());
        let (Some(name), Some(p)) = (name, l.get("percent").and_then(|p| p.as_f64())) else { continue };
        if l.get("kind").and_then(|k| k.as_str()) != Some("weekly_scoped") || out.iter().any(|w: &RateLimit| w.window.eq_ignore_ascii_case(name)) {
            continue;
        }
        out.push(RateLimit { window: name.to_string(), used_percent: p, resets_at: l.get("resets_at").and_then(|r| r.as_str()).and_then(iso_ms) });
    }
    out
}

/// "2026-11-25T19:20:00.5+00:00" (or …Z) → unix milliseconds.
pub fn iso_ms(s: &str) -> Option<i64> {
    let (date, rest) = s.trim().split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let (time, off_min) = if let Some(t) = rest.strip_suffix('Z').or_else(|| rest.strip_suffix('z')) {
        (t, 0)
    } else if let Some(i) = rest.rfind(['+', '-']) {
        let (t, o) = rest.split_at(i);
        let sign = if o.starts_with('-') { -1 } else { 1 };
        let o = o[1..].replace(':', "");
        (t, sign * (o.get(..2)?.parse::<i64>().ok()? * 60 + o.get(2..4).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0)))
    } else {
        (rest, 0)
    };
    let mut t = time.split(':');
    let (h, mi) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let sec = t.next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
    // Days from the civil date (Howard Hinnant's algorithm).
    let yy = if m <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some(((days * 86_400 + h * 3600 + mi * 60 - off_min * 60) as f64 * 1000.0 + sec * 1000.0) as i64)
}

/// Claude's `rate_limit_info` → windows (utilization is a fraction).
pub fn rate_limits(info: &Value) -> Option<Vec<RateLimit>> {
    let mut out = Vec::new();
    let window = |k: &str| match k {
        "five_hour" => "5h".to_string(),
        "seven_day" => "weekly".to_string(),
        "seven_day_opus" => "opus-weekly".to_string(),
        "seven_day_sonnet" => "sonnet-weekly".to_string(),
        other => other.replace('_', "-"),
    };
    if let Some(w) = info.get("unifiedWindows").and_then(|w| w.as_object()) {
        for (k, v) in w {
            let Some(u) = v.get("utilization").and_then(|u| u.as_f64()) else { continue };
            out.push(RateLimit {
                window: window(k),
                used_percent: (u * 100.0 * 10.0).round() / 10.0,
                resets_at: v.get("resetsAt").and_then(|r| r.as_i64()).map(|r| r * 1000),
            });
        }
    } else if let (Some(k), Some(u)) = (info.get("rateLimitType").and_then(|k| k.as_str()), info.get("utilization").and_then(|u| u.as_f64())) {
        out.push(RateLimit { window: window(k), used_percent: u * 100.0, resets_at: info.get("resetsAt").and_then(|r| r.as_i64()).map(|r| r * 1000) });
    }
    out.sort_by(|a, b| a.window.cmp(&b.window));
    Some(out)
}

/// A user message in stream-json (images as base64 blocks).
pub fn user_message(input: &Input) -> Value {
    let mut content = Vec::new();
    for img in &input.images {
        if let Some((media, data)) = super::image_base64(img) {
            content.push(json!({"type": "image", "source": {"type": "base64", "media_type": media, "data": data}}));
        }
    }
    content.push(json!({"type": "text", "text": input.text}));
    json!({"type": "user", "message": {"role": "user", "content": content}, "parent_tool_use_id": null})
}

pub fn permission_mode(p: Permission) -> &'static str {
    match p {
        Permission::Ask => "default",
        Permission::Edits => "acceptEdits",
        Permission::Plan => "plan",
        Permission::Full => "bypassPermissions",
    }
}

pub struct ClaudeAgent;

type Waiters = Arc<Mutex<HashMap<String, oneshot::Sender<(bool, Value)>>>>;

struct Live {
    proc: Proc,
    tr: Arc<Mutex<Translator>>,
    waiters: Waiters,
    out: Tx,
    next: u64,
    effort: Option<String>,
}

impl Live {
    fn control(&mut self, req: Value) -> anyhow::Result<oneshot::Receiver<(bool, Value)>> {
        self.next += 1;
        let id = format!("agentd-{}", self.next);
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().unwrap().insert(id.clone(), tx);
        self.proc.write(&json!({"type": "control_request", "request_id": id, "request": req}))?;
        Ok(rx)
    }
}

fn command(opts: &StartOpts) -> Command {
    let mut c = Command::new(&opts.bin);
    c.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
    c.args(["--permission-prompt-tool", "stdio", "--permission-mode", permission_mode(opts.permission)]);
    if let Some(m) = &opts.model {
        c.args(["--model", m]);
    }
    if let Some(e) = &opts.effort {
        c.args(["--effort", e]);
    }
    if let Some(r) = &opts.resume {
        c.args(["--resume", r]);
    }
    if let Some(i) = opts.instructions.as_deref().filter(|i| !i.trim().is_empty()) {
        c.args(["--append-system-prompt", i]);
    }
    c.envs(opts.env.iter().map(|(k, v)| (k, v)));
    c.current_dir(&opts.cwd);
    if crate::util::is_root() {
        // Claude refuses bypassPermissions as root unless it is told it is in a sandbox.
        c.env("IS_SANDBOX", "1");
    }
    c
}

async fn spawn(opts: &StartOpts, out: Tx) -> anyhow::Result<(Live, Value)> {
    let (proc, child, stdout) = Proc::spawn(command(opts), "claude")?;
    let tr = Arc::new(Mutex::new(Translator::new(&opts.cwd.to_string_lossy())));
    let waiters: Waiters = Arc::new(Mutex::new(HashMap::new()));
    let (t2, w2, o2) = (tr.clone(), waiters.clone(), out.clone());
    let writer = proc.writer.clone();
    super::read_lines(stdout, child, proc.stderr.clone(), out.clone(), move |v| {
        let acts = t2.lock().unwrap().line(&v);
        for a in acts {
            match a {
                Act::Ev(e) => {
                    let _ = o2.send(DriverOut::Event(e));
                }
                Act::Write(v) => {
                    let _ = writer.send(v.to_string());
                }
                Act::Control(id, ok, body) => {
                    if let Some(w) = w2.lock().unwrap().remove(&id) {
                        let _ = w.send((ok, body));
                    }
                }
            }
        }
    });
    let mut live = Live { proc, tr, waiters, out, next: 0, effort: opts.effort.clone() };
    let rx = live.control(json!({"subtype": "initialize"}))?;
    let tail = live.proc.stderr.clone();
    match tokio::time::timeout(Duration::from_secs(90), rx).await {
        Ok(Ok((true, init))) => Ok((live, init)),
        Ok(Ok((false, err))) => {
            live.proc.kill();
            anyhow::bail!("claude refused to start: {err}")
        }
        Ok(Err(_)) => {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let t = super::tail_text(&tail);
            anyhow::bail!("claude exited while starting{}", if t.is_empty() { String::new() } else { format!(": {t}") })
        }
        Err(_) => {
            live.proc.kill();
            anyhow::bail!("claude did not answer initialize in 90s")
        }
    }
}

#[async_trait]
impl Agent for ClaudeAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }

    async fn start(&self, opts: StartOpts, out: Tx) -> anyhow::Result<Box<dyn Session>> {
        let (live, _init) = spawn(&opts, out).await?;
        Ok(Box::new(live))
    }

    async fn models(&self, bin: PathBuf, cwd: PathBuf) -> anyhow::Result<Vec<ModelInfo>> {
        // The initialize response lists them; no API call is made.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let opts = StartOpts { thread: String::new(), bin, cwd, model: None, effort: None, permission: Permission::Ask, resume: None, instructions: None, env: vec![] };
        let (mut live, init) = spawn(&opts, tx).await?;
        live.close().await;
        Ok(models_from_init(&init))
    }
}

pub fn models_from_init(init: &Value) -> Vec<ModelInfo> {
    init.get("models")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
        .map(|m| {
            let efforts: Vec<String> = m.get("supportedEffortLevels").and_then(|e| e.as_array()).into_iter().flatten().filter_map(|e| e.as_str().map(str::to_string)).collect();
            ModelInfo {
                id: s(m, "value"),
                label: m.get("displayName").and_then(|d| d.as_str()).unwrap_or_else(|| m.get("value").and_then(|v| v.as_str()).unwrap_or("")).to_string(),
                default_effort: None,
                efforts,
                default: s(m, "value") == "default",
                context_window: None,
            }
        })
        .collect()
}

#[async_trait]
impl Session for Live {
    async fn send(&mut self, input: &Input) -> anyhow::Result<()> {
        self.proc.write(&user_message(input))
    }

    async fn interrupt(&mut self) -> anyhow::Result<()> {
        self.tr.lock().unwrap().interrupting = true;
        let _ = self.control(json!({"subtype": "interrupt"}))?;
        Ok(())
    }

    async fn answer(&mut self, ask: &str, option: &str, text: Option<&str>) -> anyhow::Result<()> {
        let (msg, ev) = self.tr.lock().unwrap().answer(ask, option, text)?;
        self.proc.write(&msg)?;
        let _ = self.out.send(DriverOut::Event(ev));
        Ok(())
    }

    async fn set_model(&mut self, model: &str, effort: Option<&str>) -> anyhow::Result<bool> {
        let rx = self.control(json!({"subtype": "set_model", "model": model}))?;
        match tokio::time::timeout(Duration::from_secs(20), rx).await {
            Ok(Ok((true, _))) => {}
            Ok(Ok((false, err))) => anyhow::bail!("claude refused the model: {err}"),
            _ => anyhow::bail!("claude did not answer set_model"),
        }
        // The effort is a launch flag: a change needs a restart.
        Ok(effort.is_none() || effort == self.effort.as_deref())
    }

    async fn close(&mut self) {
        self.proc.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(fixture: &str) -> Vec<EventKind> {
        let mut t = Translator::new("/tmp/agentd-rec/proj");
        let mut evs = Vec::new();
        for l in fixture.lines().filter(|l| !l.trim().is_empty()) {
            let v: Value = serde_json::from_str(l).unwrap();
            for a in t.line(&v) {
                if let Act::Ev(e) = a {
                    evs.push(e);
                }
            }
        }
        evs
    }

    #[test]
    fn permission_prompt_and_write_tool() {
        let evs = run(include_str!("../../tests/fixtures/claude_perm.jsonl"));
        let EventKind::Session { session_id, model } = &evs[0] else { panic!("{:?}", evs[0]) };
        assert_eq!(session_id, "fa9e8168-b121-477e-b92c-6f8b6ba2ff4f");
        assert_eq!(model.as_deref(), Some("claude-haiku-5-5"));
        assert!(matches!(&evs[1], EventKind::RateLimits { limits, .. } if limits.iter().any(|l| l.window == "5h" && (l.used_percent - 40.0).abs() < 0.01)));
        let tool = evs.iter().find_map(|e| match e {
            EventKind::Tool { item, name, kind, title, .. } => Some((item.clone(), name.clone(), *kind, title.clone())),
            _ => None,
        });
        assert_eq!(tool, Some(("toolu_012N5zGHk7yJHxpXtMtpY2bq".into(), "Write".into(), agentum_proto::ToolKind::Edit, "hi.txt".into())));
        let ask = evs.iter().find_map(|e| match e {
            EventKind::Ask { id, kind, title, options, .. } => Some((id.clone(), *kind, title.clone(), options.len())),
            _ => None,
        });
        assert_eq!(ask, Some(("fe74c3f6-024f-427d-9dc7-0f842f44e248".into(), AskKind::Permission, "Write: hi.txt".into(), 3)));
        let done = evs.iter().find_map(|e| match e {
            EventKind::ToolDone { item, ok, diffs, .. } => Some((item.clone(), *ok, diffs.clone())),
            _ => None,
        });
        let (item, ok, diffs) = done.unwrap();
        assert_eq!(item, "toolu_012N5zGHk7yJHxpXtMtpY2bq");
        assert!(ok);
        assert_eq!(diffs[0].new.as_deref(), Some("hi"));
        // Thinking had no text; the reply streams as deltas then a whole message under one item.
        let delta_item = evs.iter().find_map(|e| match e {
            EventKind::Delta { item, text } if text == "done" => Some(item.clone()),
            _ => None,
        });
        let msg_item = evs.iter().find_map(|e| match e {
            EventKind::Message { item, text } if text == "done" => Some(item.clone()),
            _ => None,
        });
        assert!(delta_item.is_some());
        assert_eq!(delta_item, msg_item);
        let EventKind::TurnDone { stop, usage, cost_usd, duration_ms, .. } = evs.last().unwrap() else { panic!() };
        assert_eq!(*stop, StopReason::End);
        assert!(cost_usd.unwrap() > 0.0);
        assert!(*duration_ms > 0);
        let u = usage.as_ref().unwrap();
        assert_eq!(u.context_window, Some(1_000_000));
        assert!(u.context_used.unwrap() > 20_000);
    }

    #[test]
    fn answering_a_permission() {
        let mut t = Translator::new("/r");
        let req: Value = serde_json::from_str(r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"ls"}],"behavior":"allow","destination":"session"}]}}"#).unwrap();
        let acts = t.line(&req);
        assert!(matches!(&acts[0], Act::Ev(EventKind::Ask { title, .. }) if title == "Bash: ls"));
        let (msg, ev) = t.answer("r1", "allow-always", None).unwrap();
        assert_eq!(msg["response"]["response"]["behavior"], "allow");
        assert_eq!(msg["response"]["response"]["updatedInput"]["command"], "ls");
        assert_eq!(msg["response"]["response"]["updatedPermissions"][0]["type"], "addRules");
        assert_eq!(ev, EventKind::Answered { id: "r1".into(), option: "allow-always".into(), text: None });
        assert!(t.answer("r1", "allow", None).is_err());
        t.line(&req);
        let (msg, _) = t.answer("r1", "deny", Some("no")).unwrap();
        assert_eq!(msg["response"]["response"], json!({"behavior": "deny", "message": "no"}));
    }

    #[test]
    fn tools_edit_and_bash() {
        let evs = run(include_str!("../../tests/fixtures/claude_tools.jsonl"));
        let tools: Vec<(String, agentum_proto::ToolKind, String)> = evs
            .iter()
            .filter_map(|e| match e {
                EventKind::Tool { name, kind, title, .. } => Some((name.clone(), *kind, title.clone())),
                _ => None,
            })
            .collect();
        assert!(tools.contains(&("Edit".into(), agentum_proto::ToolKind::Edit, "notes.txt".into())));
        assert!(tools.contains(&("Bash".into(), agentum_proto::ToolKind::Exec, "cat notes.txt".into())));
        let edit = evs
            .iter()
            .find_map(|e| match e {
                EventKind::ToolDone { diffs, .. } if !diffs.is_empty() => Some(diffs[0].clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(edit.old.as_deref(), Some("world"));
        assert_eq!(edit.new.as_deref(), Some("there"));
        assert!(edit.unified.unwrap().contains("-hello world\n+hello there"));
        let bash_out = evs.iter().find_map(|e| match e {
            EventKind::ToolDone { output: Some(o), .. } if o == "hello there" => Some(()),
            _ => None,
        });
        assert!(bash_out.is_some());
    }

    #[test]
    fn question_and_denied_tool() {
        let lines = include_str!("../../tests/fixtures/claude_ask.jsonl");
        let mut t = Translator::new("/tmp/agentd-rec/proj");
        let mut asks = Vec::new();
        let mut denied = None;
        for l in lines.lines().filter(|l| !l.is_empty()) {
            for a in t.line(&serde_json::from_str(l).unwrap()) {
                match a {
                    Act::Ev(EventKind::Ask { id, kind, title, options, .. }) => asks.push((id, kind, title, options)),
                    Act::Ev(EventKind::ToolDone { ok: false, output, .. }) => denied = output,
                    _ => {}
                }
            }
        }
        let (qid, kind, title, options) = &asks[0];
        assert_eq!(*kind, AskKind::Question);
        assert_eq!(title, "Do you prefer tea or coffee?");
        assert_eq!(options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), vec!["Tea", "Coffee"]);
        let (msg, _) = t.answer(qid, "Tea", None).unwrap();
        assert_eq!(msg["response"]["response"]["updatedInput"]["answers"]["Do you prefer tea or coffee?"], "Tea");
        assert_eq!(asks[1].1, AskKind::Permission);
        assert_eq!(denied.as_deref(), Some("The user said no."));
    }

    #[test]
    fn interrupt_ends_the_turn_interrupted() {
        let mut t = Translator::new("/r");
        t.interrupting = true;
        let evs: Vec<EventKind> = include_str!("../../tests/fixtures/claude_interrupt.jsonl")
            .lines()
            .filter(|l| !l.is_empty())
            .flat_map(|l| t.line(&serde_json::from_str(l).unwrap()))
            .filter_map(|a| if let Act::Ev(e) = a { Some(e) } else { None })
            .collect();
        assert!(matches!(evs.last(), Some(EventKind::TurnDone { stop: StopReason::Interrupted, .. })));
        assert!(!evs.iter().any(|e| matches!(e, EventKind::Error { .. })));
    }

    #[test]
    fn per_turn_cost_in_one_process() {
        let evs = run(include_str!("../../tests/fixtures/claude_two.jsonl"));
        let costs: Vec<f64> = evs.iter().filter_map(|e| if let EventKind::TurnDone { cost_usd, .. } = e { *cost_usd } else { None }).collect();
        assert_eq!(costs.len(), 2);
        assert!((costs[0] - 0.00200089).abs() < 1e-9);
        assert!((costs[1] - 0.00218144).abs() < 1e-9);
    }

    #[test]
    fn todo_write_is_a_plan_and_cancel_is_answered() {
        let mut t = Translator::new("/r");
        let a = t.line(&json!({"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"tu1","name":"TodoWrite","input":{"todos":[{"content":"a","status":"completed"},{"content":"b","status":"in_progress"}]}}]}}));
        assert_eq!(
            a[0],
            Act::Ev(EventKind::Plan { entries: vec![PlanEntry { text: "a".into(), status: "done".into() }, PlanEntry { text: "b".into(), status: "in-progress".into() }] })
        );
        t.line(&json!({"type":"control_request","request_id":"r9","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"rm x"}}}));
        let c = t.line(&json!({"type":"control_cancel_request","request_id":"r9"}));
        assert_eq!(c, vec![Act::Ev(EventKind::Answered { id: "r9".into(), option: "cancelled".into(), text: None })]);
        // Unknown types are ignored; unknown control requests get an error reply.
        assert!(t.line(&json!({"type":"brand_new"})).is_empty());
        assert!(matches!(&t.line(&json!({"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback"}}))[0], Act::Write(_)));
    }

    #[test]
    fn models_from_the_initialize_response() {
        let init = json!({"models":[{"value":"default","displayName":"Default (recommended)","supportedEffortLevels":["low","high"]},{"value":"haiku","displayName":"Haiku 5.5"}]});
        let m = models_from_init(&init);
        assert_eq!(m.len(), 2);
        assert!(m[0].default);
        assert_eq!(m[0].efforts, vec!["low", "high"]);
        assert_eq!(m[1].label, "Haiku 5.5");
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn oauth_usage_windows() {
        // The shape BB's usage tracker tests against.
        let v = serde_json::json!({
            "five_hour": {"utilization": 12.5, "resets_at": "2026-09-22T18:00:00Z"},
            "seven_day": {"utilization": 40, "resets_at": null},
            "limits": [{"kind": "weekly_scoped", "scope": {"model": {"display_name": "Opus"}}, "percent": 7, "resets_at": "2026-09-25T00:00:00+00:00"}, null, {"kind": "other", "percent": 3}]
        });
        let w = oauth_usage(&v);
        assert_eq!(w.iter().map(|w| w.window.as_str()).collect::<Vec<_>>(), ["5h", "weekly", "Opus"]);
        assert_eq!(w[0].used_percent, 12.5);
        assert_eq!(w[0].resets_at, Some(1_790_100_000_000));
        assert_eq!(w[1].resets_at, None);
        assert_eq!(w[2].used_percent, 7.0);
    }

    #[test]
    fn iso_times() {
        assert_eq!(iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_ms("2026-11-25T19:20:00.5+00:00"), Some(1_795_634_400_500));
        assert_eq!(iso_ms("2026-11-25T16:20:00-03:00"), Some(1_795_634_400_000));
        assert_eq!(iso_ms("2000-03-01T00:00:00Z"), Some(951_868_800_000));
    }

    #[test]
    fn plans() {
        assert_eq!(plan_label(Some("max".into()), Some("default_claude_max_20x".into())).as_deref(), Some("Max (20x)"));
        assert_eq!(plan_label(Some("pro".into()), None).as_deref(), Some("Pro"));
        assert_eq!(plan_label(None, None), None);
    }
}

/// `cargo test -p agentd --lib live_usage -- --ignored --nocapture`: this machine's limits, asked now.
#[cfg(test)]
#[tokio::test]
#[ignore]
async fn live_usage() {
    match usage_now().await {
        Ok((w, plan)) => println!("claude: plan {plan:?}, {w:?}"),
        Err(e) => println!("claude: {e}"),
    }
    match crate::machine::agent_bin(&crate::config::Config::default(), AgentKind::Codex) {
        Some(bin) => match super::codex::usage_now(bin).await {
            Ok((w, plan)) => println!("codex: plan {plan:?}, {w:?}"),
            Err(e) => println!("codex: {e}"),
        },
        None => println!("codex: not installed"),
    }
}
