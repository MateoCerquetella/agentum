//! Built-in [`ToolAdapter`](super::ToolAdapter) implementations.

use std::path::Path;

use agentum_core::{Session, transcript};

use crate::{LaunchCommand, McpProvision, ToolAdapter, translate_yolo_marker};

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

/// Does Claude already have a transcript on disk for this session in
/// its workdir? Used by `ClaudeAdapter::launch` to pick between
/// `--session-id` (first launch — claim the id) and `--resume`
/// (restart — continue the existing conversation). Defensive against
/// the path-resolution helper returning `None` (missing `$HOME` or
/// non-absolute workdir): in that case we conservatively report
/// "doesn't exist" so we keep the original `--session-id` behaviour.
fn claude_transcript_exists(session: &Session) -> bool {
    transcript::transcript_path_for(Path::new(&session.workdir), session.id)
        .as_deref()
        .map(Path::exists)
        .unwrap_or(false)
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
        //
        // On *restart* the transcript already exists; `--session-id`
        // would then crash with `Error: Session ID <X> is already in
        // use`. Detect that case and switch to `--resume <id>`, which
        // continues the same transcript instead of trying to claim the
        // ID fresh. Stop/start cycles, orphan-tmux respawns, and
        // daemon restarts all funnel through `start()` with the same
        // agentum UUID, so without this every re-launch crashed.
        if claude_transcript_exists(session) {
            argv.push("--resume".to_string());
        } else {
            argv.push("--session-id".to_string());
        }
        argv.push(session.id.to_string());
        push_user_flags(&mut argv, session, self.yolo_flag());
        LaunchCommand::argv_only(argv)
    }

    // Claude loads MCP servers from a file at startup; point it at the
    // pre-written combined config (agentum + playwright + …). Additive — we
    // deliberately omit `--strict-mcp-config` so the user's own MCP servers
    // stay available.
    fn mcp_args(&self, p: &McpProvision) -> Vec<String> {
        vec![
            "--mcp-config".to_string(),
            p.config_file.display().to_string(),
        ]
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

    // Two distinct families of "agent is blocked on the user" UI in
    // Claude Code, both detected here:
    //
    //   1. Yes/no permission prompts ("Do you want to proceed?" etc.)
    //      — the original signatures, kept for older Claude versions
    //      where the wording is still on screen.
    //
    //   2. Interactive menus (plan mode, multi-choice subagent picks,
    //      route-selection during reviews). Claude renders these with
    //      a footer reading
    //      `Enter to select · ↑/↓ to navigate · Esc to cancel`
    //      and the input field suppressed. The yes/no signatures
    //      don't match these because the first option isn't "Yes" —
    //      it's whatever the agent's first proposal is. Match the
    //      footer's `Enter to select` substring, which is unique to
    //      these menus (the normal prompt doesn't render it).
    //
    // Without (2), running `agentum-check-linear`-style flows the
    // watchdog stays in Idle, the TUI never gets `agent.awaiting_input`,
    // no toast fires, and the sidebar dot stays green while the agent
    // is actually blocked on the user.
    fn awaiting_input_signatures(&self) -> &'static [&'static str] {
        // Substrings unique to Claude Code's actual prompt UI. The
        // multi-choice menu's footer reads
        // `Enter to select · ↑/↓ to navigate · Esc to cancel` as a
        // single line — match the structural pair so generic prose
        // (a code comment quoting "Enter to select", a help page
        // mentioning ↑/↓ navigation) can't masquerade as a real
        // pending prompt. Yes/no permission prompts keep their
        // sentence-form signatures.
        &[
            "Do you want to proceed?",
            "Do you want to make this edit",
            "Do you want to create",
            "❯ 1. Yes",
            "Enter to select · ↑/↓ to navigate",
        ]
    }

    fn is_agent(&self) -> bool {
        true
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

    // Codex has no `--mcp-config`; inject each server with `-c` TOML overrides at
    // launch. Values are quoted so the URL parses as a TOML string. One block per
    // server (agentum, playwright, …); a server with an `auth_token` also gets a
    // `bearer_token` override so Codex authenticates to it.
    fn mcp_args(&self, p: &McpProvision) -> Vec<String> {
        let mut args = Vec::with_capacity(p.servers.len() * 6);
        for s in &p.servers {
            args.push("-c".to_string());
            args.push(format!("mcp_servers.{}.type=\"http\"", s.name));
            args.push("-c".to_string());
            args.push(format!("mcp_servers.{}.url=\"{}\"", s.name, s.url));
            if let Some(token) = &s.auth_token {
                args.push("-c".to_string());
                args.push(format!("mcp_servers.{}.bearer_token=\"{}\"", s.name, token));
            }
        }
        args
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

    fn is_agent(&self) -> bool {
        true
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

    fn is_agent(&self) -> bool {
        true
    }
}

// ---------- agent ----------

/// Cursor's renamed CLI as of the Jan 2026 release: the binary is `agent`
/// and it ships agent-mode + plan-mode + cloud handoff in one entry point.
/// Same product as `CursorAdapter` (the older `cursor-agent` binary stays
/// around as a back-compat alias) — kept as a distinct adapter so the
/// picker can probe the new binary independently and users who only have
/// the new spelling installed still get a first-class entry.
pub struct AgentAdapter;

impl ToolAdapter for AgentAdapter {
    fn name(&self) -> &'static str {
        "agent"
    }

    fn launch(&self, session: &Session) -> LaunchCommand {
        let mut argv = vec!["agent".to_string()];
        push_model(&mut argv, session);
        push_user_flags(&mut argv, session, self.yolo_flag());
        LaunchCommand::argv_only(argv)
    }

    // Same skip-confirmations spelling as cursor-agent — the underlying
    // product is identical, only the entry-point name changed.
    fn yolo_flag(&self) -> Option<&'static str> {
        Some("--force")
    }

    fn is_agent(&self) -> bool {
        true
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

    fn is_agent(&self) -> bool {
        true
    }
}

// ---------- hermes ----------

pub struct HermesAdapter;

impl ToolAdapter for HermesAdapter {
    fn name(&self) -> &'static str {
        "hermes"
    }

    fn launch(&self, session: &Session) -> LaunchCommand {
        // `hermes chat` is the interactive entry. The pre-0.9 `hermes run`
        // subcommand and its `--workdir` flag were both removed; the pane's
        // tmux cwd already pins the working directory like every other
        // adapter, so we just exec `hermes chat [--model=…] [user flags]`.
        let mut argv = vec!["hermes".to_string(), "chat".to_string()];
        push_model(&mut argv, session);
        push_user_flags(&mut argv, session, self.yolo_flag());
        LaunchCommand::argv_only(argv)
    }

    fn yolo_flag(&self) -> Option<&'static str> {
        Some("--yolo")
    }

    fn is_agent(&self) -> bool {
        true
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

    // Hookless agents that route through this catch-all (opencode, aider —
    // see `PASSTHROUGH_PROBED`) still need change-based Working/Idle
    // detection: they have no first-class adapter, so `busy_signature()`
    // is `None`, and the watchdog only applies its no-busy-signature
    // fallback when `is_agent()` is true. Without this, an actively
    // rendering remote OpenCode pane classified as `Unknown` forever —
    // never `Working`, never `Idle` — so the sidebar dot showed "Idle"
    // while the agent was visibly streaming output. A truly unknown
    // binary (a one-off shell command typed as a "tool") stays `false`
    // and `Unknown`, preserving the "don't auto-fire on shells" rule.
    fn is_agent(&self) -> bool {
        crate::PASSTHROUGH_PROBED.contains(&self.tool.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{McpServer, adapter_for};
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
            host_id: None,
            host_label: None,
            host_kind: None,
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
            card_id: None,
            worktree_path: None,
            worktree_branch: None,
            worktree_base_ref: None,
        }
    }

    #[test]
    fn claude_argv() {
        let s = fixture(
            "claude",
            Some("opus-4-8"),
            &["--dangerously-skip-permissions"],
        );
        let cmd = ClaudeAdapter.launch(&s);
        assert_eq!(
            cmd.argv,
            vec![
                "claude".to_string(),
                "--model=opus-4-8".to_string(),
                "--session-id".to_string(),
                s.id.to_string(),
                "--dangerously-skip-permissions".to_string(),
            ]
        );
        assert_eq!(ClaudeAdapter.compact_trigger(), Some("/compact"));
    }

    fn provision() -> McpProvision {
        McpProvision {
            servers: vec![McpServer {
                name: "playwright".to_string(),
                url: "http://127.0.0.1:8931/mcp".to_string(),
                auth_token: None,
            }],
            config_file: std::path::PathBuf::from("/tmp/agentum/playwright-mcp.json"),
        }
    }

    /// Two servers: agentum (token-guarded) + playwright (none) → Codex must emit
    /// a `-c` block for each, plus a `bearer_token` for agentum.
    fn provision_two() -> McpProvision {
        McpProvision {
            servers: vec![
                McpServer {
                    name: "agentum".to_string(),
                    url: "http://127.0.0.1:8822/mcp".to_string(),
                    auth_token: Some("secret-tok".to_string()),
                },
                McpServer {
                    name: "playwright".to_string(),
                    url: "http://127.0.0.1:8931/mcp".to_string(),
                    auth_token: None,
                },
            ],
            config_file: std::path::PathBuf::from("/tmp/agentum/mcp.json"),
        }
    }

    #[test]
    fn claude_mcp_args_point_at_the_config_file_additively() {
        // Additive: no `--strict-mcp-config`, so the user's own MCP servers survive.
        let args = ClaudeAdapter.mcp_args(&provision());
        assert_eq!(
            args,
            vec![
                "--mcp-config".to_string(),
                "/tmp/agentum/playwright-mcp.json".to_string(),
            ]
        );
        assert!(!args.iter().any(|a| a == "--strict-mcp-config"));
    }

    #[test]
    fn codex_mcp_args_inject_http_server_via_config_overrides() {
        let args = CodexAdapter.mcp_args(&provision());
        assert_eq!(
            args,
            vec![
                "-c".to_string(),
                "mcp_servers.playwright.type=\"http\"".to_string(),
                "-c".to_string(),
                "mcp_servers.playwright.url=\"http://127.0.0.1:8931/mcp\"".to_string(),
            ]
        );
    }

    #[test]
    fn codex_mcp_args_emit_one_block_per_server() {
        // N servers in order; the token-guarded agentum server also gets a
        // `bearer_token` override, the unauthenticated playwright one doesn't.
        let args = CodexAdapter.mcp_args(&provision_two());
        assert_eq!(
            args,
            vec![
                "-c".to_string(),
                "mcp_servers.agentum.type=\"http\"".to_string(),
                "-c".to_string(),
                "mcp_servers.agentum.url=\"http://127.0.0.1:8822/mcp\"".to_string(),
                "-c".to_string(),
                "mcp_servers.agentum.bearer_token=\"secret-tok\"".to_string(),
                "-c".to_string(),
                "mcp_servers.playwright.type=\"http\"".to_string(),
                "-c".to_string(),
                "mcp_servers.playwright.url=\"http://127.0.0.1:8931/mcp\"".to_string(),
            ]
        );
    }

    #[test]
    fn claude_mcp_args_are_one_config_file_regardless_of_server_count() {
        // Claude reads all servers from the single combined config file.
        let args = ClaudeAdapter.mcp_args(&provision_two());
        assert_eq!(
            args,
            vec![
                "--mcp-config".to_string(),
                "/tmp/agentum/mcp.json".to_string()
            ]
        );
    }

    #[test]
    fn tools_without_browser_mcp_get_no_args_by_default() {
        let p = provision();
        for tool in [
            "cursor", "gemini", "hermes", "terminal", "agent", "opencode",
        ] {
            assert!(
                adapter_for(tool).mcp_args(&p).is_empty(),
                "{tool} must not inject browser MCP by default"
            );
        }
    }

    #[test]
    fn claude_restart_uses_resume_when_transcript_exists() {
        // Repro for the v0.7.45 crash where every restart of a Claude
        // session died with `Error: Session ID <X> is already in use`.
        // Stop/start, orphan-tmux respawn, and daemon-restart respawn
        // all funnel through `start()` with the same agentum UUID, so
        // pinning `--session-id <X>` a second time was guaranteed to
        // hit Claude's collision check. The fix: when the transcript
        // already lives at the deterministic project-dir path, switch
        // to `--resume <X>` (continue the conversation) instead of
        // `--session-id <X>` (claim a fresh id).
        //
        // Filesystem-touching: lays down a sentinel transcript under a
        // tempdir-rooted HOME, runs the adapter, then restores HOME.
        use std::path::PathBuf;
        let unique = format!(
            "agentum-executor-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        );
        let fake_home = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&fake_home).unwrap();

        let saved = std::env::var_os("HOME");
        // SAFETY: this crate's tests don't otherwise mutate HOME, so no
        // intra-binary race. Restored on the happy path below; a panic
        // here will leave HOME pointing at a missing tempdir, which is
        // acceptable for a test failure (the assertion message is what
        // we care about, not subsequent test isolation).
        unsafe {
            std::env::set_var("HOME", &fake_home);
        }

        let workdir = "/tmp/work";
        let session = fixture("claude", None, &[]);
        let session = Session {
            workdir: workdir.into(),
            ..session
        };
        let enc = workdir.replace('/', "-");
        let project_dir: PathBuf = fake_home.join(".claude").join("projects").join(enc);
        std::fs::create_dir_all(&project_dir).unwrap();
        let transcript_path = project_dir.join(format!("{}.jsonl", session.id));
        std::fs::write(&transcript_path, b"{}\n").unwrap();

        let argv = ClaudeAdapter.launch(&session).argv;

        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&fake_home);

        assert!(
            argv.iter().any(|s| s == "--resume"),
            "expected --resume in argv when transcript exists: {argv:?}"
        );
        assert!(
            !argv.iter().any(|s| s == "--session-id"),
            "did not expect --session-id (Claude rejects it on a known id): {argv:?}"
        );
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
    fn hermes_launches_chat_with_model() {
        // Regression: pre-v0.7.21 the adapter launched `hermes run
        // --workdir <dir>`, but Hermes 0.9 dropped both the `run`
        // subcommand and the `--workdir` flag, so panes died with
        // "invalid choice: 'run'". The pane's tmux cwd already pins
        // the workdir like every other adapter.
        let s = fixture("hermes", Some("hermes-3"), &[]);
        let cmd = HermesAdapter.launch(&s);
        assert_eq!(cmd.argv, vec!["hermes", "chat", "--model=hermes-3"]);
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
    fn hermes_translates_yolo_marker_to_yolo() {
        // Hermes 0.9 exposes a top-level `--yolo` flag; the adapter
        // translates Claude's wire marker into it.
        let s = fixture("hermes", None, &["--dangerously-skip-permissions"]);
        let cmd = HermesAdapter.launch(&s);
        assert_eq!(cmd.argv, vec!["hermes", "chat", "--yolo"]);
    }

    #[test]
    fn non_yolo_flags_pass_through_unchanged() {
        let s = fixture("codex", None, &["--foo", "--bar=baz"]);
        let cmd = CodexAdapter.launch(&s);
        assert_eq!(cmd.argv, vec!["codex", "--foo", "--bar=baz"]);
    }

    #[test]
    fn registry_routes_first_class() {
        for &t in &["claude", "codex", "cursor", "agent", "gemini", "hermes"] {
            let a = adapter_for(t);
            assert_eq!(a.name(), t);
        }
        let a = adapter_for("totally-custom");
        assert_eq!(a.name(), "passthrough");
    }

    #[test]
    fn agent_argv_uses_agent_binary() {
        let s = fixture("agent", Some("auto"), &[]);
        let cmd = AgentAdapter.launch(&s);
        assert_eq!(cmd.argv, vec!["agent", "--model=auto"]);
    }

    #[test]
    fn agent_translates_yolo_marker_to_force() {
        // Cursor renamed the binary to `agent` in Jan 2026 but kept the
        // same `--force` skip-confirmations flag. Both surfaces still
        // wire YOLO as the Claude marker; the adapter must translate.
        let s = fixture("agent", None, &["--dangerously-skip-permissions"]);
        let cmd = AgentAdapter.launch(&s);
        assert_eq!(cmd.argv, vec!["agent", "--force"]);
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

    #[test]
    fn passthrough_probed_agents_report_is_agent() {
        // opencode / aider are hookless coding agents that route through
        // PassthroughAdapter (they're in PASSTHROUGH_PROBED, not FIRST_CLASS).
        // They must report is_agent() == true so the watchdog applies its
        // change-based Working/Idle detection — otherwise classify_activity
        // pins them at Unknown forever and the sidebar dot shows "Idle"
        // while the agent is visibly working. Regression for the remote
        // OpenCode "stuck on Idle" bug.
        for &tool in crate::PASSTHROUGH_PROBED {
            let a = adapter_for(tool);
            assert!(
                a.is_agent(),
                "passthrough-probed agent {tool:?} must report is_agent() == true"
            );
        }
    }

    #[test]
    fn unknown_passthrough_binary_is_not_agent() {
        // A truly unknown binary (a one-off shell command typed as a "tool")
        // must stay is_agent() == false so the watchdog leaves it Unknown and
        // never auto-fires an agent.finished/idle for what may be a shell.
        let a = adapter_for("some-random-binary");
        assert!(!a.is_agent());
    }
}
