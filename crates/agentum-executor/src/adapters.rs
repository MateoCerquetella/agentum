//! Built-in [`ToolAdapter`](super::ToolAdapter) implementations.

use agentum_core::Session;

use crate::{LaunchCommand, ToolAdapter, translate_yolo_marker};

/// Append `--model=<v>` to argv if the session has a model set.
fn push_model(argv: &mut Vec<String>, session: &Session) {
    if let Some(m) = &session.model {
        argv.push(format!("--model={m}"));
    }
}

/// Append all user-provided flags, translating the YOLO marker into the
/// adapter's own per-tool YOLO flag (or dropping it for tools without
/// one). Adapters call this instead of pushing `session.flags` raw so
/// that a single Claude-flavoured marker on the wire becomes the
/// correct flag for whichever binary the adapter actually launches.
fn push_user_flags(argv: &mut Vec<String>, session: &Session, yolo_flag: Option<&str>) {
    argv.extend(translate_yolo_marker(&session.flags, yolo_flag));
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
        // Pin Claude's session UUID to the agentum session UUID so its
        // transcript lands at `~/.claude/projects/<enc-cwd>/<id>.jsonl`
        // — the agent-tasks watcher reads that exact path. Without
        // this, two agents in the same workdir share one project dir
        // and the watcher (which used to pick the most-recently-mtimed
        // .jsonl) cross-pollinated todos. See
        // crates/agentum-server/src/transcript_store.rs.
        argv.push("--session-id".to_string());
        argv.push(session.id.to_string());
        push_user_flags(&mut argv, session, self.yolo_flag());
        LaunchCommand::argv_only(argv)
    }

    fn compact_trigger(&self) -> Option<&'static str> {
        Some("/compact")
    }

    fn crash_signatures(&self) -> &'static [&'static str] {
        &["redacted_thinking", "panic: cannot continue"]
    }

    // Claude Code's native spelling — the YOLO_MARKER constant matches
    // this exactly, so translation here is the identity. Kept explicit
    // so a future Claude rename only has to update this single spot.
    fn yolo_flag(&self) -> Option<&'static str> {
        Some("--dangerously-skip-permissions")
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
        push_user_flags(&mut argv, session, self.yolo_flag());
        LaunchCommand::argv_only(argv)
    }

    // Codex CLI uses `/compact` too as of late 2025.
    fn compact_trigger(&self) -> Option<&'static str> {
        Some("/compact")
    }

    // Codex's no-confirmations switch. Codex itself emits this exact
    // string as the "did you mean?" tip when handed Claude's flag, so
    // it's the documented spelling — not a guess.
    fn yolo_flag(&self) -> Option<&'static str> {
        Some("--dangerously-bypass-approvals-and-sandbox")
    }
}

// ---------- cursor ----------

/// Cursor's headless agent CLI — distributed as `cursor-agent` (separate
/// from the Cursor IDE binary). Honors `--model`, accepts free-form user
/// flags, and uses `--force` as its skip-confirmations toggle.
pub struct CursorAdapter;

impl ToolAdapter for CursorAdapter {
    fn name(&self) -> &'static str {
        "cursor"
    }

    fn launch(&self, session: &Session) -> LaunchCommand {
        let mut argv = vec!["cursor-agent".to_string()];
        push_model(&mut argv, session);
        push_user_flags(&mut argv, session, self.yolo_flag());
        LaunchCommand::argv_only(argv)
    }

    fn yolo_flag(&self) -> Option<&'static str> {
        Some("--force")
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
        push_user_flags(&mut argv, session, self.yolo_flag());
        LaunchCommand::argv_only(argv)
    }

    // Gemini CLI accepts `--yolo` for non-interactive permission skipping.
    fn yolo_flag(&self) -> Option<&'static str> {
        Some("--yolo")
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
        push_user_flags(&mut argv, session, self.yolo_flag());
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
        push_user_flags(&mut argv, session, self.yolo_flag());
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
        push_user_flags(&mut argv, session, self.yolo_flag());
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
            tokens: None,
            cost_usd: None,
            ctx: None,
            last_log: None,
            uptime_seconds: None,
            state: None,
            pinned: false,
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
                "claude".to_string(),
                "--model=opus-4-7".to_string(),
                "--session-id".to_string(),
                s.id.to_string(),
                "--dangerously-skip-permissions".to_string(),
            ]
        );
        assert_eq!(ClaudeAdapter.compact_trigger(), Some("/compact"));
    }

    #[test]
    fn claude_session_id_is_agentum_uuid() {
        // Two distinct agentum sessions in the same workdir must launch
        // claude with distinct --session-id values so their transcripts
        // land in different files. Regression for the cross-talk bug
        // where the agent-tasks panel showed another agent's todos.
        let a = fixture("claude", None, &[]);
        let b = fixture("claude", None, &[]);
        assert_ne!(a.id, b.id);
        let argv_a = ClaudeAdapter.launch(&a).argv;
        let argv_b = ClaudeAdapter.launch(&b).argv;
        let pos_a = argv_a.iter().position(|s| s == "--session-id").unwrap();
        let pos_b = argv_b.iter().position(|s| s == "--session-id").unwrap();
        assert_eq!(argv_a[pos_a + 1], a.id.to_string());
        assert_eq!(argv_b[pos_b + 1], b.id.to_string());
        assert_ne!(argv_a[pos_a + 1], argv_b[pos_b + 1]);
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
    fn codex_translates_yolo_marker_to_its_own_flag() {
        // Both clients ship `--dangerously-skip-permissions` (Claude's
        // spelling) on the wire when YOLO is on. CodexAdapter must
        // translate it — feeding Claude's flag verbatim to codex
        // produces `error: unexpected argument '--dangerously-skip-
        // permissions' found / tip: a similar argument exists:
        // '--dangerously-bypass-approvals-and-sandbox'`. Regression
        // test for v0.6.24.
        let s = fixture("codex", None, &["--dangerously-skip-permissions"]);
        let cmd = CodexAdapter.launch(&s);
        assert_eq!(
            cmd.argv,
            vec!["codex", "--dangerously-bypass-approvals-and-sandbox"]
        );
    }

    #[test]
    fn gemini_translates_yolo_marker_to_yolo() {
        let s = fixture("gemini", None, &["--dangerously-skip-permissions"]);
        let cmd = GeminiAdapter.launch(&s);
        assert_eq!(cmd.argv, vec!["gemini", "--yolo"]);
    }

    #[test]
    fn hermes_drops_yolo_marker_when_unsupported() {
        // HermesAdapter has no yolo_flag; translation drops the marker
        // rather than passing Claude's flag to a binary that doesn't
        // know it. The user effectively gets non-YOLO; preferable to a
        // crash on launch.
        let s = fixture("hermes", None, &["--dangerously-skip-permissions"]);
        let cmd = HermesAdapter.launch(&s);
        assert_eq!(cmd.argv, vec!["hermes", "run", "--workdir", "/tmp/work"]);
    }

    #[test]
    fn non_yolo_flags_pass_through_unchanged() {
        let s = fixture("codex", None, &["--foo", "--bar=baz"]);
        let cmd = CodexAdapter.launch(&s);
        assert_eq!(cmd.argv, vec!["codex", "--foo", "--bar=baz"]);
    }

    #[test]
    fn registry_routes_first_class() {
        for &t in &["claude", "codex", "cursor", "gemini", "hermes"] {
            let a = adapter_for(t);
            assert_eq!(a.name(), t);
        }
        let a = adapter_for("totally-custom");
        assert_eq!(a.name(), "passthrough");
    }

    #[test]
    fn cursor_translates_yolo_marker_to_force() {
        // Cursor-agent's skip-confirmations toggle is `--force`. The TUI
        // and dashboard always wire YOLO as the Claude marker; the
        // adapter must translate it.
        let s = fixture("cursor", None, &["--dangerously-skip-permissions"]);
        let cmd = CursorAdapter.launch(&s);
        assert_eq!(cmd.argv, vec!["cursor-agent", "--force"]);
    }

    #[test]
    fn cursor_argv_uses_cursor_agent_binary() {
        let s = fixture("cursor", Some("auto"), &[]);
        let cmd = CursorAdapter.launch(&s);
        assert_eq!(cmd.argv, vec!["cursor-agent", "--model=auto"]);
    }
}
