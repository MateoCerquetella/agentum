//! Built-in [`ToolAdapter`](super::ToolAdapter) implementations.

use agentum_core::Session;

use crate::{LaunchCommand, ToolAdapter};

/// Append `--model=<v>` to argv if the session has a model set.
fn push_model(argv: &mut Vec<String>, session: &Session) {
    if let Some(m) = &session.model {
        argv.push(format!("--model={m}"));
    }
}

/// Append all user-provided flags verbatim.
fn push_user_flags(argv: &mut Vec<String>, session: &Session) {
    argv.extend(session.flags.iter().cloned());
}

// ---------- claude ----------

pub struct ClaudeAdapter;

impl ToolAdapter for ClaudeAdapter {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn launch(&self, session: &Session) -> LaunchCommand {
        let mut argv = vec!["claude".to_string()];
        push_model(&mut argv, session);
        push_user_flags(&mut argv, session);
        LaunchCommand::argv_only(argv)
    }

    fn compact_trigger(&self) -> Option<&'static str> {
        Some("/compact")
    }

    fn crash_signatures(&self) -> &'static [&'static str] {
        &["redacted_thinking", "panic: cannot continue"]
    }

    // Claude Code's spinner footer always carries "esc to interrupt"
    // while a turn is active and disappears the moment the model returns
    // control. Cheap, stable Working→Idle signal — no transcript parsing
    // required.
    fn busy_signature(&self) -> Option<&'static str> {
        Some("esc to interrupt")
    }

    // Permission prompts are rendered as a numbered options box. The
    // wording varies slightly across versions but every variant we've
    // seen carries one of these substrings on screen at the same time
    // as the input field is suppressed.
    fn awaiting_input_signatures(&self) -> &'static [&'static str] {
        &[
            "Do you want to proceed?",
            "Do you want to make this edit",
            "Do you want to create",
            "❯ 1. Yes",
        ]
    }
}

// ---------- codex ----------

pub struct CodexAdapter;

impl ToolAdapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn launch(&self, session: &Session) -> LaunchCommand {
        let mut argv = vec!["codex".to_string()];
        push_model(&mut argv, session);
        push_user_flags(&mut argv, session);
        LaunchCommand::argv_only(argv)
    }

    // Codex CLI uses `/compact` too as of late 2025.
    fn compact_trigger(&self) -> Option<&'static str> {
        Some("/compact")
    }
}

// ---------- gemini ----------

pub struct GeminiAdapter;

impl ToolAdapter for GeminiAdapter {
    fn name(&self) -> &'static str {
        "gemini"
    }

    fn launch(&self, session: &Session) -> LaunchCommand {
        let mut argv = vec!["gemini".to_string()];
        push_model(&mut argv, session);
        push_user_flags(&mut argv, session);
        LaunchCommand::argv_only(argv)
    }
}

// ---------- hermes ----------

pub struct HermesAdapter;

impl ToolAdapter for HermesAdapter {
    fn name(&self) -> &'static str {
        "hermes"
    }

    fn launch(&self, session: &Session) -> LaunchCommand {
        // bash-bridge convention: `hermes run --workdir <dir> [--model <m>] [user flags]`
        let mut argv = vec!["hermes".to_string(), "run".to_string()];
        argv.push("--workdir".to_string());
        argv.push(session.workdir.clone());
        if let Some(m) = &session.model {
            argv.push(format!("--model={m}"));
        }
        push_user_flags(&mut argv, session);
        LaunchCommand::argv_only(argv)
    }
}

// ---------- terminal ----------

/// Plain interactive shell. Honors `$SHELL` (the user's login shell), falling
/// back to `bash`. The session's user flags are appended verbatim and any
/// configured `model` is ignored — shells don't take one.
pub struct TerminalAdapter;

impl ToolAdapter for TerminalAdapter {
    fn name(&self) -> &'static str {
        "terminal"
    }

    fn launch(&self, session: &Session) -> LaunchCommand {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".into());
        let mut argv = vec![shell];
        push_user_flags(&mut argv, session);
        LaunchCommand::argv_only(argv)
    }
}

// ---------- passthrough ----------

/// Catch-all adapter for any binary we don't have first-class knowledge of.
/// Trusts the user to know what flags their tool wants.
pub struct PassthroughAdapter {
    tool: String,
}

impl PassthroughAdapter {
    pub fn new(tool: String) -> Self {
        Self { tool }
    }
}

impl ToolAdapter for PassthroughAdapter {
    fn name(&self) -> &'static str {
        "passthrough"
    }

    fn launch(&self, session: &Session) -> LaunchCommand {
        let mut argv = vec![self.tool.clone()];
        push_model(&mut argv, session);
        push_user_flags(&mut argv, session);
        LaunchCommand::argv_only(argv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter_for;
    use agentum_core::{Session, Status};
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn fixture(tool: &str, model: Option<&str>, flags: &[&str]) -> Session {
        let now = OffsetDateTime::now_utc();
        Session {
            id: Uuid::new_v4(),
            name: "alpha".into(),
            workdir: "/tmp/work".into(),
            tool: tool.into(),
            model: model.map(String::from),
            flags: flags.iter().map(|s| s.to_string()).collect(),
            status: Status::Idle,
            tmux_target: None,
            created_at: now,
            updated_at: now,
            last_activity_at: None,
        }
    }

    #[test]
    fn claude_argv() {
        let s = fixture(
            "claude",
            Some("opus-4-7"),
            &["--dangerously-skip-permissions"],
        );
        let cmd = ClaudeAdapter.launch(&s);
        assert_eq!(
            cmd.argv,
            vec![
                "claude",
                "--model=opus-4-7",
                "--dangerously-skip-permissions"
            ]
        );
        assert_eq!(ClaudeAdapter.compact_trigger(), Some("/compact"));
    }

    #[test]
    fn hermes_argv_includes_workdir() {
        let s = fixture("hermes", Some("hermes-3"), &[]);
        let cmd = HermesAdapter.launch(&s);
        assert_eq!(
            cmd.argv,
            vec![
                "hermes",
                "run",
                "--workdir",
                "/tmp/work",
                "--model=hermes-3"
            ]
        );
    }

    #[test]
    fn passthrough_uses_tool_as_argv0() {
        let s = fixture("weird", None, &["--foo", "--bar=baz"]);
        let cmd = adapter_for(&s.tool).launch(&s);
        assert_eq!(cmd.argv, vec!["weird", "--foo", "--bar=baz"]);
    }

    #[test]
    fn registry_routes_first_class() {
        for &t in &["claude", "codex", "gemini", "hermes"] {
            let a = adapter_for(t);
            assert_eq!(a.name(), t);
        }
        let a = adapter_for("totally-custom");
        assert_eq!(a.name(), "passthrough");
    }
}
