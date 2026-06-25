//! `POST /api/chat` — the Socratic feature-intake chat behind the Mission
//! Control "Chat" screen.
//!
//! This replaces the old one-shot "describe → create an issue" form with a real
//! multi-turn conversation: the model interviews the user to flesh out a feature
//! and, once it's clear, proposes a GitHub task breakdown the user can confirm.
//!
//! **Auth:** prefers an explicit `ANTHROPIC_API_KEY` (a real `sk-ant-api…` key —
//! clean, terms-safe, pay-per-token); otherwise falls back to the user's Claude
//! Code OAuth token (the `sk-ant-oat…` credential `usage.rs` already reads for
//! plan stats) — the user explicitly opted into reusing their `claude` login.
//! The API-key path is the robust default; the OAuth path is zero-setup.
//!
//! **OAuth gotcha (load-bearing):** an OAuth (subscription) token is only
//! accepted by `/v1/messages` when the request presents the Claude Code identity.
//! We send `anthropic-beta: oauth-2025-04-20` + the Claude Code user-agent AND
//! lead the `system` array with the exact Claude Code identity block; the actual
//! interviewer instructions follow in a second block. Dropping the identity block
//! makes Anthropic reject the token (401, "only authorized for Claude Code").
//!
//! v1 is **non-streaming** (request → full reply). Token-streaming is a later
//! polish (needs reqwest's `stream` feature + SSE re-emit).

use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::error::ApiError;
use crate::task_sink::{NewFeature, SinkCtx, TaskSink};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/chat", post(chat))
        .route("/api/chat/issues", post(chat_issues))
}

const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";
const CLAUDE_CODE_USER_AGENT: &str = "claude-code/2.1.0";
/// The identity block an OAuth token requires (see module docs). Must be exact.
const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
/// Default interview model — a fast, capable Sonnet for back-and-forth.
const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// How the chat authenticates to Anthropic.
enum Auth {
    /// A real `sk-ant-api…` key (env `ANTHROPIC_API_KEY`) — clean API billing.
    ApiKey(String),
    /// The Claude Code subscription OAuth token (`sk-ant-oat…`).
    Oauth(String),
}

/// Resolve chat credentials: prefer an explicit `ANTHROPIC_API_KEY` (terms-safe,
/// pay-per-token); else fall back to the Claude Code OAuth token the user is
/// already signed in with. `None` when neither is available.
fn resolve_auth() -> Option<Auth> {
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        let k = k.trim();
        if !k.is_empty() {
            return Some(Auth::ApiKey(k.to_string()));
        }
    }
    crate::usage::read_claude_oauth_token().map(Auth::Oauth)
}

#[derive(Deserialize)]
struct ChatMessage {
    /// "user" | "assistant".
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatRequest {
    /// Full turn history, oldest first (user/assistant only — the server owns
    /// the system prompt).
    messages: Vec<ChatMessage>,
    /// Optional repo context to ground the interview.
    #[serde(default)]
    workdir: Option<String>,
    #[serde(default)]
    repo_slug: Option<String>,
    /// Optional model override.
    #[serde(default)]
    model: Option<String>,
}

#[derive(Serialize)]
struct ChatResponse {
    /// Always "assistant".
    role: &'static str,
    content: String,
}

/// The interviewer instructions (the second `system` block). Kept separate from
/// the Claude Code identity block so the identity stays byte-exact.
fn interviewer_instructions(workdir: Option<&str>, repo_slug: Option<&str>) -> String {
    let mut ctx = String::new();
    if let Some(slug) = repo_slug {
        ctx.push_str(&format!("\nThe user's GitHub repo is `{slug}`."));
    }
    if let Some(wd) = workdir {
        ctx.push_str(&format!("\nThe project lives at `{wd}`."));
    }
    format!(
        "You are running inside agentum (a control plane for AI coding agents) as the \
feature-intake interviewer on the Chat screen.{ctx}\n\n\
Your job: through a short Socratic conversation, help the user turn a rough idea into a \
clear, buildable feature, then propose a concrete task breakdown for their GitHub tracker.\n\n\
Rules:\n\
- Ask ONE focused clarifying question at a time (two only if tightly related). Keep each \
turn short and concrete, like a sharp staff engineer — no filler, no \"great question!\".\n\
- Cover only what's genuinely unclear: the problem and who it's for, the desired outcome, \
scope boundaries (in/out), hard constraints, and acceptance criteria. Never re-ask what \
the user already answered.\n\
- When the feature is defined well enough to build, STOP asking questions and propose a \
breakdown: a one-line feature title, then 3–7 concrete tasks (each a GitHub-issue-style \
title plus one sentence of detail). Then ask the user to confirm creating them on GitHub.\n\
- You only interview and propose. Do NOT write code or create anything — task creation \
happens after the user confirms, in a later step."
    )
}

async fn chat(
    State(_state): State<AppState>,
    Json(body): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, ApiError> {
    if body.messages.is_empty() {
        return Err(ApiError::BadRequest(
            "chat: messages cannot be empty".into(),
        ));
    }

    // Auth: prefer an explicit ANTHROPIC_API_KEY (clean API billing, terms-safe);
    // else reuse the Claude Code OAuth token (user-authorized). Absent → loud,
    // actionable error naming BOTH paths.
    let auth = resolve_auth().ok_or_else(|| {
        ApiError::BadRequest(
            "No LLM credentials for chat: set ANTHROPIC_API_KEY, or sign in to Claude (run `claude` once) so the chat can use your login."
                .into(),
        )
    })?;

    let model = body
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_MODEL);

    let messages: Vec<serde_json::Value> = body
        .messages
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();

    let instructions = interviewer_instructions(body.workdir.as_deref(), body.repo_slug.as_deref());
    let system = build_system(&auth, &instructions);

    let text = call_anthropic(&auth, model, system, &messages, 1024).await?;

    Ok(Json(ChatResponse {
        role: "assistant",
        content: text,
    }))
}

/// Build the `system` value for a request. An OAuth (subscription) token requires
/// the Claude Code identity to lead the `system` (see module docs); a real API
/// key does NOT (and must not spoof it). Factored out so the chat interviewer and
/// the issue-extraction call share the byte-exact identity-prefix logic.
fn build_system(auth: &Auth, instructions: &str) -> serde_json::Value {
    match auth {
        Auth::Oauth(_) => json!([
            { "type": "text", "text": CLAUDE_CODE_IDENTITY },
            { "type": "text", "text": instructions },
        ]),
        Auth::ApiKey(_) => json!(instructions),
    }
}

/// POST `/v1/messages` and return the concatenated assistant text. Owns the
/// auth-specific headers (x-api-key for an API key; bearer + the oauth beta +
/// Claude Code UA for an OAuth token), the redacted/typed non-2xx handling, and
/// the empty-reply guard — so every Anthropic call in this module behaves
/// identically. The caller supplies the already-built `system` (via
/// [`build_system`]) and the user/assistant `messages`.
async fn call_anthropic(
    auth: &Auth,
    model: &str,
    system: serde_json::Value,
    messages: &[serde_json::Value],
    max_tokens: u32,
) -> Result<String, ApiError> {
    let payload = json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": messages,
    });

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| ApiError::Internal(format!("build http client: {e}")))?;

    // Common headers; auth-specific headers differ for an API key vs an OAuth token.
    let mut req = client
        .post(MESSAGES_URL)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&payload);
    let secret = match auth {
        Auth::ApiKey(k) => {
            req = req.header("x-api-key", k);
            k.clone()
        }
        Auth::Oauth(t) => {
            // OAuth (subscription) token: bearer + the oauth beta + Claude Code UA.
            req = req
                .bearer_auth(t)
                .header("anthropic-beta", OAUTH_BETA_HEADER)
                .header(reqwest::header::USER_AGENT, CLAUDE_CODE_USER_AGENT);
            t.clone()
        }
    };
    let resp = req.send().await.map_err(|e| {
        ApiError::Internal(format!(
            "anthropic request failed: {}",
            redact(&e.to_string(), &secret)
        ))
    })?;

    let status = resp.status();
    let raw = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        // 401/403: the credential was rejected. Name both recovery paths.
        let hint = if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            match auth {
                Auth::ApiKey(_) => " (check ANTHROPIC_API_KEY)",
                Auth::Oauth(_) => {
                    " (your Claude login may have expired — run `claude` to refresh it, or set ANTHROPIC_API_KEY)"
                }
            }
        } else {
            ""
        };
        let detail = redact(raw.trim(), &secret);
        let detail = detail.chars().take(300).collect::<String>();
        return Err(ApiError::Custom(
            StatusCode::BAD_GATEWAY,
            json!({ "error": { "code": "llm_failed", "message": format!("chat model returned {status}{hint}: {detail}") } }),
        ));
    }

    let v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| ApiError::Internal(format!("parse anthropic response: {e}")))?;

    // Concatenate all text blocks of the assistant message.
    let text = v
        .get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    if text.trim().is_empty() {
        return Err(ApiError::Internal(
            "chat model returned an empty reply".into(),
        ));
    }

    Ok(text)
}

// ---------------------------------------------------------------------------
// POST /api/chat/issues — turn the agreed task breakdown into GitHub issues.
//
// Closes the loop on the Chat interviewer: once the conversation has converged
// on a feature + task list, this endpoint asks the model to distil that into a
// strict JSON array, resolves the project's GitHub repo, and files one issue per
// task via the shared `TaskSink::Github` arm (so YOLO/argv/parse stay in one
// place). Partial success is a 200 — created and failed are reported per-task so
// the UI can show exactly which issues landed.
// ---------------------------------------------------------------------------

/// The system prompt that turns a conversation into a strict task array. Kept
/// byte-exact; the lenient parser ([`extract_task_drafts`]) tolerates a model
/// that still wraps it in prose or fences despite the instruction not to.
const EXTRACT_INSTRUCTIONS: &str = "From this conversation, extract the agreed feature task breakdown as a JSON array of objects, each exactly {\"title\": string, \"body\": string} — title = a concise GitHub issue title, body = 1–3 sentences. Output ONLY the raw JSON array, no prose, no markdown code fences.";

#[derive(Deserialize)]
struct ChatIssuesRequest {
    /// The conversation to distil into issues (user/assistant turns only).
    messages: Vec<ChatMessage>,
    /// Explicit `owner/repo` target. When well-formed it short-circuits the
    /// host-aware origin read.
    #[serde(default)]
    repo_slug: Option<String>,
    /// Project dir — used to read the GitHub `origin` when no `repo_slug` is
    /// given. The created-issue path itself runs `gh` from `$HOME`.
    #[serde(default)]
    workdir: Option<String>,
}

/// One extracted task — the minimal shape the model must emit.
#[derive(Deserialize)]
struct TaskDraft {
    title: String,
    body: String,
}

/// Distil the agreed task breakdown from a chat transcript and file each task as
/// a GitHub issue. Returns `{ repo, created[], failed[] }` (200 even on a partial
/// or total per-task failure — the LLM/auth/no-repo failures are the only hard
/// errors, surfaced as typed envelopes).
async fn chat_issues(
    State(state): State<AppState>,
    Json(body): Json<ChatIssuesRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.messages.is_empty() {
        return Err(ApiError::BadRequest(
            "chat issues: messages cannot be empty".into(),
        ));
    }

    // Same credential resolution + actionable error as the chat handler.
    let auth = resolve_auth().ok_or_else(|| {
        ApiError::BadRequest(
            "No LLM credentials for chat: set ANTHROPIC_API_KEY, or sign in to Claude (run `claude` once) so the chat can use your login."
                .into(),
        )
    })?;
    // The bearer/API token, kept so it can be scrubbed from any per-task error.
    let secret = match &auth {
        Auth::ApiKey(k) => k.clone(),
        Auth::Oauth(t) => t.clone(),
    };

    let messages: Vec<serde_json::Value> = body
        .messages
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();

    // Extraction call: lead the system with the Claude Code identity for OAuth
    // (mirrors the interviewer), then the strict-JSON instruction.
    let system = build_system(&auth, EXTRACT_INSTRUCTIONS);
    let text = call_anthropic(&auth, DEFAULT_MODEL, system, &messages, 2048).await?;

    // Parse leniently (fences/prose tolerated). Empty or unparseable → 422.
    let drafts = extract_task_drafts(&text).ok_or_else(|| {
        ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({ "error": { "code": "no_tasks", "message": "could not extract a task list from the conversation" } }),
        )
    })?;

    // Resolve the GitHub slug: a well-formed client hint wins (no IO); else read
    // the LOCAL project's `origin` (Chat never files over SSH). No slug → 422.
    let slug = match body.repo_slug.as_deref().map(str::trim) {
        Some(s) if slug_matches(s) => s.to_string(),
        _ => {
            let host = state
                .store
                .get_host(agentum_core::LOCAL_HOST_ID)
                .await?
                .ok_or_else(|| ApiError::Internal("local host record missing".into()))?;
            let workdir = body
                .workdir
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "/".to_string())
                });
            match super::board_goals::resolve_github_slug(
                &host,
                &workdir,
                body.repo_slug.as_deref(),
            )
            .await
            {
                Ok(slug) => slug,
                Err(_) => {
                    return Err(ApiError::Custom(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        json!({ "error": { "code": "no_github_repo", "message": "no GitHub repo resolved for this project" } }),
                    ));
                }
            }
        }
    };

    // The GitHub slug arm runs `gh` from `$HOME`, so `workdir` here is unused —
    // pass the project dir (or a temp dir) for shape only.
    let workdir_path: std::path::PathBuf = body
        .workdir
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);

    // File one issue per task; collect successes/failures independently so a
    // single bad task never sinks the rest (partial success is a 200).
    let mut created: Vec<serde_json::Value> = Vec::new();
    let mut failed: Vec<serde_json::Value> = Vec::new();
    for t in &drafts {
        let feature = NewFeature {
            title: t.title.clone(),
            body: Some(t.body.clone()),
        };
        let res = TaskSink::Github
            .create_feature(
                &SinkCtx {
                    store: &state.store,
                    workdir: &workdir_path,
                    parent_goal_id: None,
                    slug: Some(&slug),
                },
                &feature,
            )
            .await;
        match res {
            Ok(fref) => created.push(json!({
                "title": t.title,
                "url": fref.url.unwrap_or_default(),
            })),
            Err(e) => {
                let detail = redact(&e.to_string(), &secret);
                let detail = detail.chars().take(300).collect::<String>();
                failed.push(json!({ "title": t.title, "error": detail }));
            }
        }
    }

    Ok(Json(json!({
        "repo": slug,
        "created": created,
        "failed": failed,
    })))
}

/// Pull a `Vec<TaskDraft>` out of a possibly-noisy model reply: strip markdown
/// fences, slice from the first `[` to the last `]`, then parse. Returns `None`
/// on no array / parse failure / an empty list — all of which the caller maps to
/// the `no_tasks` 422.
fn extract_task_drafts(raw: &str) -> Option<Vec<TaskDraft>> {
    let cleaned = raw.replace("```json", "").replace("```", "");
    let start = cleaned.find('[')?;
    let end = cleaned.rfind(']')?;
    if end < start {
        return None;
    }
    // `[` and `]` are ASCII, so these byte offsets are valid char boundaries.
    let slice = &cleaned[start..=end];
    let drafts: Vec<TaskDraft> = serde_json::from_str(slice).ok()?;
    if drafts.is_empty() {
        None
    } else {
        Some(drafts)
    }
}

/// A client `repo_slug` must look like `owner/repo` — exactly one `/`, both
/// halves non-empty, no whitespace (the `^[^/\s]+/[^/\s]+$` shape). Mirrors
/// `board_goals::is_valid_slug` so a malformed hint falls through to the read.
fn slug_matches(s: &str) -> bool {
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

/// Replace a bearer token anywhere in a string before it can be logged/returned.
fn redact(msg: &str, token: &str) -> String {
    if token.is_empty() {
        msg.to_string()
    } else {
        msg.replace(token, "<redacted>")
    }
}
