//! Per-server planner config loader. Reads
//! `$XDG_CONFIG_HOME/agentum/planner.toml` on every goal-submit
//! (no in-memory cache in v1 per CONTEXT D-12). Resolution order:
//! `prompt_file` -> `prompt` -> bundled default.

use std::borrow::Cow;
use std::path::Path;

use crate::error::ApiError;

/// Bundled default planner prompt, baked into the binary at compile time.
/// Users override it via `planner.toml` `prompt_file` or `prompt` fields.
/// Keeping it bundled makes the planner feature work out of the box with
/// zero setup per D-13.
const BUNDLED_PROMPT: &str = include_str!("planner_prompt.md");

/// Default planner tool — the `adapter_for()` registry resolves this to
/// `ClaudeAdapter`. Users override via `planner.tool` per D-14.
const DEFAULT_TOOL: &str = "claude";

/// Resolved planner configuration for a single goal-submit invocation.
///
/// The `prompt` field uses `Cow<'static, str>` so that the bundled-default
/// path (the common case) pays zero allocation cost — only user overrides
/// allocate a `String`.
#[derive(Debug)]
pub struct PlannerConfig {
    pub tool: String,
    pub prompt: Cow<'static, str>,
}

/// Wire shape for the `[planner]` section of `planner.toml`.
///
/// All fields are optional so that a minimal `[planner]` header (or even
/// an empty file) is legal — every absent field falls back to its default.
#[derive(serde::Deserialize, Default)]
struct PlannerFile {
    planner: Option<PlannerSection>,
}

#[derive(serde::Deserialize, Default)]
struct PlannerSection {
    tool: Option<String>,
    /// Path to an external markdown file to use as the planner prompt.
    /// When set, takes precedence over `prompt` and the bundled default.
    prompt_file: Option<String>,
    /// Inline prompt string embedded directly in planner.toml.
    /// Wins over the bundled default; loses to `prompt_file`.
    prompt: Option<String>,
}

/// Load and resolve the planner configuration.
///
/// Reading on every call is intentional — no in-memory cache in v1 per D-12
/// (file is ~1 KB, reads at <<1 Hz goal-submit rate, tokio::fs keeps the
/// handler async-clean). Phase 3 may add caching if profiling shows a need.
///
/// Resolution order: `prompt_file` → `prompt` → bundled default.
pub async fn load_planner_config() -> Result<PlannerConfig, ApiError> {
    let path = agentum_store::paths::planner_config_path()
        .map_err(|e| ApiError::Internal(format!("planner config path: {e}")))?;

    if !path.exists() {
        return Ok(PlannerConfig {
            tool: DEFAULT_TOOL.into(),
            prompt: Cow::Borrowed(BUNDLED_PROMPT),
        });
    }

    let raw = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| ApiError::Internal(format!("read planner.toml: {e}")))?;

    let parsed: PlannerFile = toml::from_str(&raw)
        .map_err(|e| ApiError::BadRequest(format!("invalid planner.toml: {e}")))?;

    let section = parsed.planner.unwrap_or_default();
    let tool = section.tool.unwrap_or_else(|| DEFAULT_TOOL.to_string());

    // Resolution order: prompt_file -> prompt -> bundled
    let prompt: Cow<'static, str> = if let Some(pf) = section.prompt_file.as_deref() {
        let abs = Path::new(pf);
        // Path-traversal guard (security_threat_model T-02-01): reject relative
        // paths so an attacker who can write planner.toml cannot pivot through the
        // daemon's CWD to read arbitrary files (e.g. `../etc/passwd`).
        if !abs.is_absolute() {
            return Err(ApiError::BadRequest(format!(
                "planner.prompt_file must be an absolute path: {pf}"
            )));
        }
        // Reject paths that contain `..` after the leading `/` — a path like
        // `/tmp/foo/../bar` is syntactically absolute but still traverses up.
        if abs
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(ApiError::BadRequest(format!(
                "planner.prompt_file must not contain `..`: {pf}"
            )));
        }
        if !abs.exists() {
            return Err(ApiError::BadRequest(format!(
                "planner.prompt_file does not exist: {pf}"
            )));
        }
        let body = tokio::fs::read_to_string(abs)
            .await
            .map_err(|e| ApiError::Internal(format!("read prompt_file: {e}")))?;
        Cow::Owned(body)
    } else if let Some(inline) = section.prompt {
        Cow::Owned(inline)
    } else {
        Cow::Borrowed(BUNDLED_PROMPT)
    };

    Ok(PlannerConfig { tool, prompt })
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

    fn isolate_xdg() -> TestEnv {
        // Shared crate-wide lock: AGENTUM_HOME is process-global, so serialise
        // against profiles/board_goals too (a per-module lock would not).
        let guard = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new().unwrap();
        // SAFETY: `set_var` is unsound under concurrent access.
        // `ENV_LOCK` serialises this whole module so only one thread
        // mutates the env at a time. AGENTUM_HOME isolates on every
        // platform (XDG_CONFIG_HOME is a no-op on macOS).
        unsafe {
            std::env::set_var("AGENTUM_HOME", dir.path());
        }
        TestEnv {
            _dir: dir,
            _guard: guard,
        }
    }

    #[tokio::test]
    async fn missing_file_returns_bundled_default() {
        let _env = isolate_xdg();
        // No planner.toml in the tempdir.
        let cfg = load_planner_config().await.unwrap();
        assert_eq!(cfg.tool, "claude");
        // The bundled prompt mentions the CLI surface so agents know how to
        // emit cards — if this fails the include_str! grabbed the wrong file.
        assert!(
            cfg.prompt.contains("agentum board add-card"),
            "bundled prompt must reference the CLI surface"
        );
    }

    #[tokio::test]
    async fn inline_prompt_overrides_default() {
        let _env = isolate_xdg();
        let cfg_dir = agentum_store::paths::config_dir().unwrap();
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("planner.toml"),
            "[planner]\ntool = \"codex\"\nprompt = \"hello world\"\n",
        )
        .unwrap();

        let cfg = load_planner_config().await.unwrap();
        assert_eq!(cfg.tool, "codex");
        assert_eq!(cfg.prompt.as_ref(), "hello world");
    }

    #[tokio::test]
    async fn prompt_file_beats_inline_when_both_set() {
        let env = isolate_xdg();
        let cfg_dir = agentum_store::paths::config_dir().unwrap();
        std::fs::create_dir_all(&cfg_dir).unwrap();

        // Write the fixture file that prompt_file will point at.
        // Must be inside the TempDir so the TestEnv cleanup removes it.
        let fixture_path = env._dir.path().join("custom_prompt.md");
        std::fs::write(&fixture_path, "file-prompt-content").unwrap();

        let toml_content = format!(
            "[planner]\nprompt_file = \"{}\"\nprompt = \"inline-content\"\n",
            fixture_path.display()
        );
        std::fs::write(cfg_dir.join("planner.toml"), toml_content).unwrap();

        let cfg = load_planner_config().await.unwrap();
        // prompt_file wins over inline prompt.
        assert_eq!(cfg.prompt.as_ref(), "file-prompt-content");
    }

    #[tokio::test]
    async fn prompt_file_relative_is_rejected() {
        let _env = isolate_xdg();
        let cfg_dir = agentum_store::paths::config_dir().unwrap();
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("planner.toml"),
            "[planner]\nprompt_file = \"../etc/passwd\"\n",
        )
        .unwrap();

        let err = load_planner_config().await.unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(
                msg.contains("absolute"),
                "error must mention 'absolute', got: {msg}"
            ),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn prompt_file_parent_dir_is_rejected() {
        let _env = isolate_xdg();
        let cfg_dir = agentum_store::paths::config_dir().unwrap();
        std::fs::create_dir_all(&cfg_dir).unwrap();
        // Syntactically absolute but contains `..`.
        std::fs::write(
            cfg_dir.join("planner.toml"),
            "[planner]\nprompt_file = \"/tmp/foo/../bar\"\n",
        )
        .unwrap();

        let err = load_planner_config().await.unwrap_err();
        assert!(
            matches!(err, ApiError::BadRequest(_)),
            "expected BadRequest for `..` path, got {err:?}"
        );
    }

    #[tokio::test]
    async fn prompt_file_missing_is_rejected() {
        let _env = isolate_xdg();
        let cfg_dir = agentum_store::paths::config_dir().unwrap();
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("planner.toml"),
            "[planner]\nprompt_file = \"/tmp/definitely-not-a-real-path-XYZZY\"\n",
        )
        .unwrap();

        let err = load_planner_config().await.unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(
                msg.contains("does not exist"),
                "error must mention 'does not exist', got: {msg}"
            ),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invalid_toml_returns_bad_request() {
        let _env = isolate_xdg();
        let cfg_dir = agentum_store::paths::config_dir().unwrap();
        std::fs::create_dir_all(&cfg_dir).unwrap();
        // Malformed TOML — unterminated array header.
        std::fs::write(cfg_dir.join("planner.toml"), "this isn't toml [\n").unwrap();

        let err = load_planner_config().await.unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(
                msg.contains("invalid"),
                "error must mention 'invalid', got: {msg}"
            ),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }
}
