//! Which agent powers the Chat screen + the issue-creation AI legs
//! (`/api/chat`, `/api/chat/stream`, `/api/chat/issues*`,
//! `/api/github/issues/draft-body`).
//!
//! Historically those routes were hardwired to Claude (the Anthropic API).
//! The user-facing rule now: **the config decides which agent runs** — a
//! request-level `agent` field (the desktop sends its Settings pick on every
//! call), then `$XDG_CONFIG_HOME/agentum/chat.toml` (`[chat] agent = "…"`),
//! then the built-in default (Claude). Both backends reuse the SAME intake
//! prompts, repo grounding, and issue-filing plumbing — only the LLM call
//! itself differs (Anthropic `/v1/messages` vs OpenAI Responses; see
//! [`crate::routes::chat_openai`]).

use crate::error::ApiError;

/// The agents the chat pipeline knows how to drive. `Claude` is the default
/// (back-compat: a request with no `agent` and no `chat.toml` behaves exactly
/// as before the setting existed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatAgent {
    /// Claude Code / Anthropic — `ANTHROPIC_API_KEY` or the Claude Code OAuth
    /// token (`routes::chat`'s original path).
    Claude,
    /// OpenAI Codex — `OPENAI_API_KEY` or the Codex CLI's ChatGPT sign-in
    /// (`~/.codex/auth.json`), driven through the Responses API.
    Codex,
}

impl ChatAgent {
    /// The wire/config value (`agent = "claude"`).
    pub fn as_str(self) -> &'static str {
        match self {
            ChatAgent::Claude => "claude",
            ChatAgent::Codex => "codex",
        }
    }

    /// Human label for error messages and logs.
    pub fn label(self) -> &'static str {
        match self {
            ChatAgent::Claude => "Claude",
            ChatAgent::Codex => "Codex",
        }
    }

    /// The model used when neither the request nor `chat.toml` names one.
    /// Claude's is the chat route's pre-existing default (a fast, capable
    /// Sonnet for back-and-forth); Codex's mirrors the desktop's codex
    /// default (`defaultModelId` in `commit-message-agent-spec.ts`).
    pub fn default_model(self) -> &'static str {
        match self {
            ChatAgent::Claude => "claude-sonnet-4-6",
            ChatAgent::Codex => "gpt-5.5",
        }
    }

    /// The actionable "no credentials" message for this agent — names BOTH
    /// recovery paths, mirroring `NO_CREDS_MSG` in `routes::chat`.
    pub fn no_creds_message(self) -> &'static str {
        match self {
            ChatAgent::Claude => {
                "No LLM credentials for chat: set ANTHROPIC_API_KEY, or sign in to Claude (run `claude` once) so the chat can use your login."
            }
            ChatAgent::Codex => {
                "No LLM credentials for chat: set OPENAI_API_KEY, or sign in to Codex (run `codex` once) so the chat can use your login."
            }
        }
    }

    /// Parse an `agent` value from the wire or `chat.toml`. Blank → `None`
    /// (caller falls through to the next resolution level); an unknown value
    /// is a loud `Err` naming the valid set — never a silent fallback, so a
    /// typo'd config can't quietly run the wrong provider.
    pub fn parse(raw: &str) -> Result<Option<ChatAgent>, ApiError> {
        let v = raw.trim().to_ascii_lowercase();
        if v.is_empty() {
            return Ok(None);
        }
        match v.as_str() {
            "claude" => Ok(Some(ChatAgent::Claude)),
            "codex" => Ok(Some(ChatAgent::Codex)),
            other => Err(ApiError::BadRequest(format!(
                "unknown chat agent {other:?} (expected \"claude\" or \"codex\")"
            ))),
        }
    }
}

/// The resolved agent + the config-file model override (if any). `model`
/// from `chat.toml` LOSES to a request-level model but wins over the agent's
/// built-in default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChatAgent {
    pub agent: ChatAgent,
    /// `[chat] model` from `chat.toml` — a daemon-side model override.
    pub config_model: Option<String>,
}

/// Wire shape of `chat.toml` — every field optional, so a bare `[chat]`
/// header (or an empty file) is legal, exactly like `planner.toml`.
#[derive(serde::Deserialize, Default)]
struct ChatFile {
    chat: Option<ChatSection>,
}

#[derive(serde::Deserialize, Default)]
struct ChatSection {
    /// `agent = "claude" | "codex"` — the daemon-side default agent.
    agent: Option<String>,
    /// `model = "…"` — the daemon-side default model for the picked agent.
    model: Option<String>,
}

/// Read `$XDG_CONFIG_HOME/agentum/chat.toml`. A missing file is the
/// `(None, None)` default; a malformed one is a 400 (same contract as
/// `planner.toml` — operator config errors must be loud, not silently
/// ignored). Kept sync: the file is ~100 bytes and reads happen at
/// chat-turn rates (<<1 Hz), same trade-off as `planner.rs` documents.
fn load_chat_file() -> Result<(Option<String>, Option<String>), ApiError> {
    let path = agentum_store::paths::chat_config_path()
        .map_err(|e| ApiError::Internal(format!("chat config path: {e}")))?;
    if !path.exists() {
        return Ok((None, None));
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| ApiError::Internal(format!("read chat.toml: {e}")))?;
    let parsed: ChatFile = toml::from_str(&raw)
        .map_err(|e| ApiError::BadRequest(format!("invalid chat.toml: {e}")))?;
    let section = parsed.chat.unwrap_or_default();
    Ok((section.agent, section.model))
}

/// Resolve which agent runs this chat call, with precedence:
/// request `agent` field → `chat.toml [chat] agent` → Claude (back-compat).
/// Also returns the `chat.toml` model override for the caller's model chain
/// (request model → config model → agent default).
pub fn resolve_chat_agent(request_agent: Option<&str>) -> Result<ResolvedChatAgent, ApiError> {
    if let Some(agent) = request_agent.map(ChatAgent::parse).transpose()?.flatten() {
        return Ok(ResolvedChatAgent {
            agent,
            config_model: None,
        });
    }
    let (file_agent, file_model) = load_chat_file()?;
    let agent = file_agent
        .as_deref()
        .map(ChatAgent::parse)
        .transpose()?
        .flatten()
        .unwrap_or(ChatAgent::Claude);
    Ok(ResolvedChatAgent {
        agent,
        config_model: file_model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty()),
    })
}

/// Pick the model for a call given the request's override, the config's
/// override, and the agent's default — in that precedence order.
pub fn resolve_chat_model(request_model: Option<&str>, resolved: &ResolvedChatAgent) -> String {
    request_model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| resolved.config_model.clone())
        .unwrap_or_else(|| resolved.agent.default_model().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;
    use tempfile::TempDir;

    struct TestEnv {
        _dir: TempDir,
        _guard: MutexGuard<'static, ()>,
    }

    /// Same AGENTUM_HOME isolation as `planner.rs::tests::isolate_xdg` —
    /// process-global env is serialised on the crate-wide lock.
    fn isolate_xdg() -> TestEnv {
        let guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new().unwrap();
        // SAFETY: serialised by TEST_ENV_LOCK — only one thread mutates env.
        unsafe {
            std::env::set_var("AGENTUM_HOME", dir.path());
        }
        let cfg = agentum_store::paths::config_dir().expect("config_dir resolves");
        assert!(
            cfg.starts_with(dir.path()),
            "AGENTUM_HOME isolation broken: config_dir {cfg:?} escaped temp {:?}",
            dir.path()
        );
        TestEnv {
            _dir: dir,
            _guard: guard,
        }
    }

    #[test]
    fn parse_accepts_known_agents_case_insensitively() {
        assert_eq!(ChatAgent::parse("claude").unwrap(), Some(ChatAgent::Claude));
        assert_eq!(
            ChatAgent::parse("  Codex ").unwrap(),
            Some(ChatAgent::Codex)
        );
        assert_eq!(ChatAgent::parse("CLAUDE").unwrap(), Some(ChatAgent::Claude));
    }

    #[test]
    fn parse_blank_is_none_so_resolution_falls_through() {
        assert_eq!(ChatAgent::parse("").unwrap(), None);
        assert_eq!(ChatAgent::parse("   ").unwrap(), None);
    }

    #[test]
    fn parse_unknown_is_a_loud_error_naming_the_valid_set() {
        let err = ChatAgent::parse("gemini").unwrap_err();
        match err {
            ApiError::BadRequest(msg) => {
                assert!(msg.contains("gemini"), "names the offender: {msg}");
                assert!(msg.contains("claude"), "names valid values: {msg}");
                assert!(msg.contains("codex"), "names valid values: {msg}");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn defaults_match_the_ships_today_behavior() {
        // Back-compat pins: no config anywhere ⇒ Claude + the model
        // routes/chat used before this setting existed.
        assert_eq!(ChatAgent::Claude.default_model(), "claude-sonnet-4-6");
        assert_eq!(ChatAgent::Claude.as_str(), "claude");
        assert_eq!(ChatAgent::Codex.as_str(), "codex");
        assert!(!ChatAgent::Codex.default_model().is_empty());
    }

    #[test]
    fn no_creds_message_names_both_recovery_paths_per_agent() {
        assert!(
            ChatAgent::Claude
                .no_creds_message()
                .contains("ANTHROPIC_API_KEY")
        );
        assert!(
            ChatAgent::Codex
                .no_creds_message()
                .contains("OPENAI_API_KEY")
        );
        assert!(ChatAgent::Codex.no_creds_message().contains("codex"));
    }

    #[test]
    fn resolve_defaults_to_claude_with_no_request_and_no_file() {
        let _env = isolate_xdg();
        let r = resolve_chat_agent(None).unwrap();
        assert_eq!(r.agent, ChatAgent::Claude);
        assert_eq!(r.config_model, None);
    }

    #[test]
    fn request_agent_wins_over_chat_toml() {
        let _env = isolate_xdg();
        let cfg_dir = agentum_store::paths::config_dir().unwrap();
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(cfg_dir.join("chat.toml"), "[chat]\nagent = \"codex\"\n").unwrap();

        let r = resolve_chat_agent(Some("claude")).unwrap();
        assert_eq!(r.agent, ChatAgent::Claude);
        // A request-level pick is self-contained: the file's model override
        // must NOT leak into a request that chose a different agent.
        assert_eq!(r.config_model, None);
    }

    #[test]
    fn chat_toml_agent_and_model_apply_when_request_is_silent() {
        let _env = isolate_xdg();
        let cfg_dir = agentum_store::paths::config_dir().unwrap();
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("chat.toml"),
            "[chat]\nagent = \"codex\"\nmodel = \"gpt-5.4-mini\"\n",
        )
        .unwrap();

        let r = resolve_chat_agent(None).unwrap();
        assert_eq!(r.agent, ChatAgent::Codex);
        assert_eq!(r.config_model.as_deref(), Some("gpt-5.4-mini"));
        assert_eq!(resolve_chat_model(None, &r), "gpt-5.4-mini");
    }

    #[test]
    fn chat_toml_unknown_agent_is_a_loud_error() {
        let _env = isolate_xdg();
        let cfg_dir = agentum_store::paths::config_dir().unwrap();
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(cfg_dir.join("chat.toml"), "[chat]\nagent = \"gpt\"\n").unwrap();

        let err = resolve_chat_agent(None).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)), "got {err:?}");
    }

    #[test]
    fn invalid_toml_is_a_bad_request_like_planner_toml() {
        let _env = isolate_xdg();
        let cfg_dir = agentum_store::paths::config_dir().unwrap();
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(cfg_dir.join("chat.toml"), "this isn't toml [\n").unwrap();

        let err = resolve_chat_agent(None).unwrap_err();
        match err {
            ApiError::BadRequest(msg) => {
                assert!(msg.contains("invalid"), "mentions 'invalid': {msg}")
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn model_resolution_precedence_request_then_config_then_default() {
        let claude = ResolvedChatAgent {
            agent: ChatAgent::Claude,
            config_model: None,
        };
        assert_eq!(resolve_chat_model(None, &claude), "claude-sonnet-4-6");
        assert_eq!(
            resolve_chat_model(Some("claude-opus-4-8"), &claude),
            "claude-opus-4-8"
        );
        // Blank request model falls through to the next level.
        assert_eq!(resolve_chat_model(Some("  "), &claude), "claude-sonnet-4-6");

        let codex_cfg = ResolvedChatAgent {
            agent: ChatAgent::Codex,
            config_model: Some("gpt-5.4".into()),
        };
        assert_eq!(resolve_chat_model(None, &codex_cfg), "gpt-5.4");
        assert_eq!(
            resolve_chat_model(Some("gpt-5.3-codex"), &codex_cfg),
            "gpt-5.3-codex"
        );
        let codex_plain = ResolvedChatAgent {
            agent: ChatAgent::Codex,
            config_model: None,
        };
        assert_eq!(resolve_chat_model(None, &codex_plain), "gpt-5.5");
    }
}
