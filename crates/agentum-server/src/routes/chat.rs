//! `POST /api/chat` — the Socratic feature-intake chat behind the Mission
//! Control "Chat" screen.
//!
//! This replaces the old one-shot "describe → create an issue" form with a real
//! multi-turn conversation: the model interviews the user to flesh out a feature
//! and, once it's clear, proposes a GitHub task breakdown the user can confirm.
//!
//! **Auth (user-authorized):** the chat reuses the user's Claude Code OAuth token
//! — the same `sk-ant-oat-…` credential `usage.rs` already reads for plan stats —
//! to call the Anthropic Messages API. The user explicitly opted into reusing
//! their `claude` login to power this chat.
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

pub fn router() -> Router<AppState> {
    Router::new().route("/api/chat", post(chat))
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
        return Err(ApiError::BadRequest("chat: messages cannot be empty".into()));
    }

    // Reuse the Claude Code OAuth token (user-authorized). Absent → actionable error.
    let token = crate::usage::read_claude_oauth_token().ok_or_else(|| {
        ApiError::BadRequest(
            "Sign in to Claude (run `claude` once) so the chat can use your login.".into(),
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

    // The `system` array MUST lead with the Claude Code identity for the OAuth
    // token to be accepted (see module docs).
    let payload = json!({
        "model": model,
        "max_tokens": 1024,
        "system": [
            { "type": "text", "text": CLAUDE_CODE_IDENTITY },
            { "type": "text", "text": interviewer_instructions(body.workdir.as_deref(), body.repo_slug.as_deref()) },
        ],
        "messages": messages,
    });

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| ApiError::Internal(format!("build http client: {e}")))?;

    let resp = client
        .post(MESSAGES_URL)
        .bearer_auth(&token)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("anthropic-beta", OAUTH_BETA_HEADER)
        .header(reqwest::header::USER_AGENT, CLAUDE_CODE_USER_AGENT)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("anthropic request failed: {}", redact(&e.to_string(), &token))))?;

    let status = resp.status();
    let raw = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        // 401/403 here is almost always the OAuth-token-not-authorized case.
        let hint = if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            " (your Claude login may have expired — run `claude` to refresh it)"
        } else {
            ""
        };
        let detail = redact(raw.trim(), &token);
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
        return Err(ApiError::Internal("chat model returned an empty reply".into()));
    }

    Ok(Json(ChatResponse {
        role: "assistant",
        content: text,
    }))
}

/// Replace a bearer token anywhere in a string before it can be logged/returned.
fn redact(msg: &str, token: &str) -> String {
    if token.is_empty() {
        msg.to_string()
    } else {
        msg.replace(token, "<redacted>")
    }
}
