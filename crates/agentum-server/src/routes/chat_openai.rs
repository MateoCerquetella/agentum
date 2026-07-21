//! The Codex / OpenAI backend for the chat pipeline ([`ChatAgent::Codex`]).
//!
//! Mirrors `routes::chat`'s Anthropic surface so the rest of the module is
//! provider-agnostic: same "resolve credentials → build payload → stream /
//! accumulate → compact SSE events" shape, same typed errors, same secret
//! redaction.
//!
//! **Auth:** prefers an explicit `OPENAI_API_KEY` (clean API billing,
//! terms-safe — hits `api.openai.com/v1/responses`); otherwise falls back to
//! the Codex CLI's ChatGPT sign-in (`~/.codex/auth.json`, the same zero-setup
//! trick the Claude path pulls with `claude` — the user already opted in by
//! signing in to `codex`). The ChatGPT token only works against the Codex
//! backend (`chatgpt.com/backend-api/codex/responses`) with the CLI's own
//! headers (`originator`, `OpenAI-Beta: responses=experimental`, and the
//! account id) — the exact mirror of the Claude Code OAuth identity gotcha
//! documented in `routes::chat`.
//!
//! **Streaming:** both endpoints speak the Responses API SSE dialect; both
//! our non-streaming helper and the SSE route always send `stream: true`
//! (the Codex backend is stream-only) — the non-streaming helper simply
//! accumulates deltas into one reply. `store: false` keeps every call
//! stateless: the full transcript rides `input` each turn, exactly like the
//! Anthropic path.

use std::path::PathBuf;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;

use crate::error::ApiError;

const RESPONSES_API_URL: &str = "https://api.openai.com/v1/responses";
const RESPONSES_CHATGPT_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
/// The ChatGPT backend gates on the Codex CLI's identity — these two headers
/// are the OpenAI twin of `CLAUDE_CODE_USER_AGENT` in `routes::chat`.
const CODEX_CLI_ORIGINATOR: &str = "codex_cli_rs";
const RESPONSES_EXPERIMENTAL_BETA: &str = "responses=experimental";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// How the chat authenticates to OpenAI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenAiAuth {
    /// A real `sk-…` platform key (env `OPENAI_API_KEY`, or the key the Codex
    /// CLI stashed in `auth.json`) — clean API billing on `api.openai.com`.
    ApiKey(String),
    /// The Codex CLI's ChatGPT sign-in (`~/.codex/auth.json` →
    /// `tokens.access_token` + `tokens.account_id`) — the subscription path.
    ChatGptOauth {
        token: String,
        account_id: Option<String>,
    },
}

impl OpenAiAuth {
    /// The credential, kept so it can be scrubbed from any error/log —
    /// the same role `secret` plays in `routes::chat`.
    pub fn secret(&self) -> &str {
        match self {
            OpenAiAuth::ApiKey(k) => k,
            OpenAiAuth::ChatGptOauth { token, .. } => token,
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Parse the Codex CLI's `auth.json`. Pure (string in, auth out) so the shape
/// is unit-testable without touching the real home dir. The CLI writes either
/// an API key (top-level `OPENAI_API_KEY`) or OAuth `tokens` from a ChatGPT
/// sign-in; prefer the API key when both exist (same "explicit key wins"
/// rule as the env-first resolution).
fn parse_codex_auth_json(raw: &str) -> Option<OpenAiAuth> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    if let Some(key) = v
        .get("OPENAI_API_KEY")
        .and_then(|k| k.as_str())
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        return Some(OpenAiAuth::ApiKey(key.to_string()));
    }
    let tokens = v.get("tokens")?;
    let token = tokens
        .get("access_token")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())?;
    let account_id = tokens
        .get("account_id")
        .and_then(|a| a.as_str())
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string);
    Some(OpenAiAuth::ChatGptOauth {
        token: token.to_string(),
        account_id,
    })
}

/// Resolve chat credentials for the Codex agent: `OPENAI_API_KEY` first
/// (explicit, terms-safe), else the Codex CLI's stored login. `None` when
/// neither exists — the caller surfaces [`ChatAgent::no_creds_message`].
pub fn resolve_openai_auth() -> Option<OpenAiAuth> {
    if let Ok(k) = std::env::var("OPENAI_API_KEY") {
        let k = k.trim();
        if !k.is_empty() {
            return Some(OpenAiAuth::ApiKey(k.to_string()));
        }
    }
    let path = home_dir()?.join(".codex").join("auth.json");
    let raw = std::fs::read_to_string(path).ok()?;
    parse_codex_auth_json(&raw)
}

/// The Responses API endpoint for this auth kind — the ChatGPT subscription
/// token is ONLY valid on the Codex backend (it 401s on `api.openai.com`),
/// exactly like the Claude Code OAuth token demands its own identity rules.
fn endpoint_for(auth: &OpenAiAuth) -> &'static str {
    match auth {
        OpenAiAuth::ApiKey(_) => RESPONSES_API_URL,
        OpenAiAuth::ChatGptOauth { .. } => RESPONSES_CHATGPT_URL,
    }
}

/// Build the `/responses` body. `instructions` carries the interviewer /
/// extraction system prompt verbatim (there is NO identity block to lead —
/// that requirement is Anthropic-specific). `messages` must already be
/// sanitized (`routes::chat::sanitize_messages`): roles map to `input_text`
/// (user) / `output_text` (assistant) content parts, since the API rejects
/// `input_text` on assistant items. `store: false` keeps calls stateless —
/// the full history is resent each turn by construction.
fn build_responses_payload(
    model: &str,
    instructions: &str,
    messages: &[serde_json::Value],
    thinking: bool,
) -> serde_json::Value {
    let input: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let text = m
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or_default();
            let part_type = if role == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            json!({
                "type": "message",
                "role": role,
                "content": [ { "type": part_type, "text": text } ],
            })
        })
        .collect();

    let mut payload = json!({
        "model": model,
        "instructions": instructions,
        "input": input,
        "store": false,
        "stream": true,
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "include": [],
    });
    if thinking {
        // The chat's "extended thinking" twin: ask for reasoning + stream the
        // human-readable summary (the encrypted trace is required for
        // store:false reasoning on the API endpoint).
        payload["reasoning"] = json!({ "effort": "high", "summary": "auto" });
        payload["include"] = json!(["reasoning.encrypted_content"]);
    }
    payload
}

/// Apply the auth-specific headers and return the secret for redaction —
/// the mirror of `routes::chat::apply_auth`. The ChatGPT path adds the Codex
/// CLI identity headers its backend gates on.
fn apply_openai_auth(
    req: reqwest::RequestBuilder,
    auth: &OpenAiAuth,
) -> (reqwest::RequestBuilder, String) {
    match auth {
        OpenAiAuth::ApiKey(k) => (req.bearer_auth(k), k.clone()),
        OpenAiAuth::ChatGptOauth { token, account_id } => {
            let req = req
                .bearer_auth(token)
                .header("OpenAI-Beta", RESPONSES_EXPERIMENTAL_BETA)
                .header("originator", CODEX_CLI_ORIGINATOR);
            let req = match account_id {
                Some(id) => req.header("chatgpt-account-id", id),
                None => req,
            };
            (req, token.clone())
        }
    }
}

/// Map one Responses SSE `data:` JSON to our compact event, or `None` for
/// frames we don't forward (`response.created`, `*.output_item.added`,
/// `*.content_part.*`, `*.output_text.done`, …). The terminal events are
/// `response.completed` / `response.failed` / `error`; `response.incomplete`
/// (token cap hit) is treated as done — the partial answer already streamed.
/// Pure → unit-tested without a live stream.
pub fn parse_responses_sse(data: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    match v.get("type").and_then(|t| t.as_str())? {
        "response.output_text.delta" => {
            Some(json!({ "type": "text", "text": v.get("delta")?.as_str()? }))
        }
        "response.reasoning_summary_text.delta" => {
            Some(json!({ "type": "thinking", "text": v.get("delta")?.as_str()? }))
        }
        "response.completed" | "response.incomplete" => Some(json!({ "type": "done" })),
        "response.failed" => {
            let message = v
                .pointer("/response/error/message")
                .and_then(|m| m.as_str())
                .unwrap_or("the model stream failed");
            Some(json!({ "type": "error", "message": message }))
        }
        "error" => {
            let message = v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("the model stream errored");
            Some(json!({ "type": "error", "message": message }))
        }
        _ => None,
    }
}

/// Replace a credential anywhere in a string before it can be logged/returned.
/// Local copy of `routes::chat::redact` (same rule, own module boundary).
fn redact(msg: &str, token: &str) -> String {
    if token.is_empty() {
        msg.to_string()
    } else {
        msg.replace(token, "<redacted>")
    }
}

/// POST `/responses` and return the live stream + the secret (for redaction
/// in the SSE generator). A non-2xx is a typed 502 BEFORE the stream opens —
/// the same "clean failure, never a 200 that errors" contract the Anthropic
/// route gives the desktop, with the per-auth recovery hint on 401/403.
pub async fn open_responses_stream(
    auth: &OpenAiAuth,
    model: &str,
    instructions: &str,
    messages: &[serde_json::Value],
    thinking: bool,
) -> Result<(reqwest::Response, String), ApiError> {
    let payload = build_responses_payload(model, instructions, messages, thinking);
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| ApiError::Internal(format!("build http client: {e}")))?;
    let (req, secret) = apply_openai_auth(
        client
            .post(endpoint_for(auth))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&payload),
        auth,
    );
    let resp = req.send().await.map_err(|e| {
        ApiError::Internal(format!(
            "openai request failed: {}",
            redact(&e.to_string(), &secret)
        ))
    })?;

    let status = resp.status();
    if !status.is_success() {
        let raw = resp.text().await.unwrap_or_default();
        let hint = if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            match auth {
                OpenAiAuth::ApiKey(_) => " (check OPENAI_API_KEY)",
                OpenAiAuth::ChatGptOauth { .. } => {
                    " (your Codex login may have expired — run `codex` to refresh it, or set OPENAI_API_KEY)"
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
    Ok((resp, secret))
}

/// The non-streaming twin of [`open_responses_stream`]: the same streamed
/// POST (the Codex backend is stream-only), accumulated into one reply text.
/// Mirrors `routes::chat::call_anthropic`'s contract — typed 502 on upstream
/// failure, hard error on an empty reply.
pub async fn call_responses(
    auth: &OpenAiAuth,
    model: &str,
    instructions: &str,
    messages: &[serde_json::Value],
) -> Result<String, ApiError> {
    // `thinking` is a streaming-UX feature (the reasoning panel); the plain
    // reply paths (non-streaming chat, plan extraction, body drafting) run
    // with it off — same as the Anthropic call they mirror.
    let (resp, secret) = open_responses_stream(auth, model, instructions, messages, false).await?;

    let mut text = String::new();
    let mut err: Option<String> = None;
    collect_stream(
        resp,
        &mut |ev| match ev.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(t) = ev.get("text").and_then(|t| t.as_str()) {
                    text.push_str(t);
                }
            }
            Some("error") => {
                err = Some(
                    ev.get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("the model stream errored")
                        .to_string(),
                );
            }
            _ => {}
        },
    )
    .await;

    if let Some(message) = err {
        return Err(ApiError::Custom(
            StatusCode::BAD_GATEWAY,
            json!({ "error": { "code": "llm_failed", "message": redact(&message, &secret) } }),
        ));
    }
    if text.trim().is_empty() {
        return Err(ApiError::Internal(
            "chat model returned an empty reply".into(),
        ));
    }
    Ok(text)
}

/// Read a Responses SSE body to completion, invoking `on_event` for every
/// mapped compact event. Frames on the ASCII `\n\n` separator so a multi-byte
/// char split across chunks is never mangled (same buffering rule as the
/// Anthropic proxy generator).
async fn collect_stream<F>(resp: reqwest::Response, on_event: &mut F)
where
    F: FnMut(serde_json::Value) + Send,
{
    use futures_util::StreamExt;
    let mut bytes = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = bytes.next().await {
        let Ok(c) = chunk else { return };
        buf.extend_from_slice(&c);
        while let Some(pos) = find_double_newline(&buf) {
            let frame = String::from_utf8_lossy(&buf[..pos]).into_owned();
            buf.drain(..pos + 2);
            for line in frame.lines() {
                let Some(data) = line.trim_start().strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }
                if let Some(ev) = parse_responses_sse(data) {
                    on_event(ev);
                }
            }
        }
    }
}

/// Index of the first `\n\n` (SSE frame separator) in `buf`, or `None`.
fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msgs() -> Vec<serde_json::Value> {
        vec![
            json!({ "role": "user", "content": "add csv export" }),
            json!({ "role": "assistant", "content": "who is it for?" }),
            json!({ "role": "user", "content": "admins" }),
        ]
    }

    #[test]
    fn payload_maps_roles_to_the_right_content_part_types() {
        let p = build_responses_payload("gpt-5.5", "INSTRUCTIONS", &msgs(), false);
        assert_eq!(p["model"], "gpt-5.5");
        assert_eq!(p["instructions"], "INSTRUCTIONS");
        assert_eq!(p["store"], false);
        assert_eq!(p["stream"], true);
        let input = p["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[0]["content"][0]["text"], "add csv export");
        assert_eq!(input[1]["role"], "assistant");
        // The API rejects input_text on assistant items — pin the mapping.
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        // No reasoning block when thinking is off.
        assert!(p.get("reasoning").is_none());
        assert_eq!(p["include"], json!([]));
    }

    #[test]
    fn payload_thinking_adds_reasoning_and_encrypted_include() {
        let p = build_responses_payload("gpt-5.5", "I", &msgs(), true);
        assert_eq!(p["reasoning"]["effort"], "high");
        assert_eq!(p["reasoning"]["summary"], "auto");
        assert_eq!(p["include"], json!(["reasoning.encrypted_content"]));
    }

    #[test]
    fn sse_maps_text_and_thinking_deltas() {
        let ev =
            parse_responses_sse(r#"{"type":"response.output_text.delta","delta":"Hel"}"#).unwrap();
        assert_eq!(ev, json!({ "type": "text", "text": "Hel" }));
        let ev = parse_responses_sse(
            r#"{"type":"response.reasoning_summary_text.delta","delta":"thinking…"}"#,
        )
        .unwrap();
        assert_eq!(ev, json!({ "type": "thinking", "text": "thinking…" }));
    }

    #[test]
    fn sse_terminal_events_map_to_done_or_error() {
        assert_eq!(
            parse_responses_sse(r#"{"type":"response.completed","response":{}}"#).unwrap(),
            json!({ "type": "done" })
        );
        // Token-cap termination still finalizes the turn.
        assert_eq!(
            parse_responses_sse(r#"{"type":"response.incomplete","response":{}}"#).unwrap(),
            json!({ "type": "done" })
        );
        let failed = parse_responses_sse(
            r#"{"type":"response.failed","response":{"error":{"message":"boom"}}}"#,
        )
        .unwrap();
        assert_eq!(failed, json!({ "type": "error", "message": "boom" }));
        let err = parse_responses_sse(r#"{"type":"error","message":"rate limited"}"#).unwrap();
        assert_eq!(err, json!({ "type": "error", "message": "rate limited" }));
    }

    #[test]
    fn sse_ignores_non_forwarded_frames_and_garbage() {
        assert!(parse_responses_sse(r#"{"type":"response.created"}"#).is_none());
        assert!(parse_responses_sse(r#"{"type":"response.output_item.added"}"#).is_none());
        assert!(parse_responses_sse("not json").is_none());
    }

    #[test]
    fn auth_json_prefers_an_explicit_api_key() {
        let raw = r#"{"OPENAI_API_KEY":"sk-test-123","tokens":{"access_token":"oat","account_id":"ac1"}}"#;
        assert_eq!(
            parse_codex_auth_json(raw),
            Some(OpenAiAuth::ApiKey("sk-test-123".into()))
        );
    }

    #[test]
    fn auth_json_reads_chatgpt_oauth_tokens() {
        let raw = r#"{"OPENAI_API_KEY":null,"tokens":{"id_token":"i","access_token":"  oat-1 ","refresh_token":"r","account_id":"ac1"}}"#;
        assert_eq!(
            parse_codex_auth_json(raw),
            Some(OpenAiAuth::ChatGptOauth {
                token: "oat-1".into(),
                account_id: Some("ac1".into()),
            })
        );
        // Missing account id is tolerated (older CLI writes).
        let raw = r#"{"tokens":{"access_token":"oat-2"}}"#;
        assert_eq!(
            parse_codex_auth_json(raw),
            Some(OpenAiAuth::ChatGptOauth {
                token: "oat-2".into(),
                account_id: None,
            })
        );
    }

    #[test]
    fn auth_json_rejects_garbage_and_empty_credentials() {
        assert_eq!(parse_codex_auth_json("not json"), None);
        assert_eq!(parse_codex_auth_json(r#"{"tokens":{}}"#), None);
        assert_eq!(parse_codex_auth_json(r#"{"OPENAI_API_KEY":"  "}"#), None);
    }

    #[test]
    fn endpoint_and_secret_follow_the_auth_kind() {
        let key = OpenAiAuth::ApiKey("sk-1".into());
        assert_eq!(endpoint_for(&key), RESPONSES_API_URL);
        assert_eq!(key.secret(), "sk-1");
        let oauth = OpenAiAuth::ChatGptOauth {
            token: "oat-1".into(),
            account_id: None,
        };
        assert_eq!(endpoint_for(&oauth), RESPONSES_CHATGPT_URL);
        assert_eq!(oauth.secret(), "oat-1");
    }

    #[test]
    fn redact_scrubs_the_credential() {
        assert_eq!(
            redact("failed with sk-secret-9 in url", "sk-secret-9"),
            "failed with <redacted> in url"
        );
        // Empty token never panics / never mangles the message.
        assert_eq!(redact("plain", ""), "plain");
    }
}
