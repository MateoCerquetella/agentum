//! Tool-adapter abstraction.
//!
//! Every supported AI CLI (Claude, Codex, Gemini, Hermes, …) implements
//! [`ToolAdapter`]. The rest of agentum talks to the trait — never to a
//! specific binary — so a `Session` row is a tool-agnostic identity that
//! becomes a concrete invocation only at spawn time.
//!
//! Unknown tools fall through to [`PassthroughAdapter`]: agentum trusts
//! whatever's on PATH and forwards `--model=…` plus the user-provided flags.

use std::borrow::Cow;

use agentum_core::Session;

mod adapters;

pub use adapters::{
    AgentAdapter, ClaudeAdapter, CodexAdapter, CursorAdapter, GeminiAdapter, HermesAdapter,
    PassthroughAdapter, TerminalAdapter,
};

/// What tmux actually launches: argv plus per-session environment overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl LaunchCommand {
    pub fn argv_only(argv: Vec<String>) -> Self {
        Self {
            argv,
            env: Vec::new(),
        }
    }
}

/// Host-specific facts needed to build a command for an SSH-owned session.
///
/// These values must be resolved on the target host. In particular, using the
/// daemon's `$SHELL` or checking its local Claude transcript directory is wrong
/// when the session's workdir lives on another machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteLaunchContext<'a> {
    /// The target host's resolved `${SHELL:-/bin/sh}` command. [`Cow`] lets a
    /// server call site borrow a preflight result or move an owned value.
    pub shell: Cow<'a, str>,
    /// Whether the deterministic Claude transcript exists on the target host.
    pub claude_transcript_exists: bool,
}

impl RemoteLaunchContext<'_> {
    /// A defensive POSIX fallback for a remote account with an empty `$SHELL`.
    pub fn shell(&self) -> &str {
        if self.shell.trim().is_empty() {
            "/bin/sh"
        } else {
            &self.shell
        }
    }
}

/// Inputs for wiring one or more streamable-HTTP MCP servers into an agent
/// **at launch** (agentum's own MCP server, the shared Playwright server, …).
///
/// MCP servers are read only at agent-CLI startup (Claude Code / Codex have no
/// in-session reload), so the launch site must (a) ensure each HTTP server is up
/// → [`Self::servers`] and (b) for tools that load MCP from a file, pre-write a
/// combined config → [`Self::config_file`]. Each adapter turns these into the
/// right startup flags and environment via [`ToolAdapter::apply_mcp`] — the
/// only tool-specific part. Server processes may be shared per machine/host;
/// bearer credentials and their environment handles are session/launch scoped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpProvision {
    /// Every streamable-HTTP MCP server to register with this agent. Tools that
    /// take MCP config on the command line (Codex `-c`) emit one block per entry.
    pub servers: Vec<McpServer>,
    /// Path to the combined `{ "mcpServers": { … } }` file the launch site
    /// already wrote (holding *all* of [`Self::servers`]) — used by tools that
    /// load MCP from a file (Claude `--mcp-config <file>`).
    pub config_file: std::path::PathBuf,
}

/// One streamable-HTTP MCP server to wire into an agent at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    /// Logical key under `mcpServers` (Claude) / `mcp_servers.<name>` (Codex).
    pub name: String,
    /// Streamable-HTTP endpoint, e.g. `http://127.0.0.1:8931/mcp`.
    pub url: String,
    /// Optional bearer token sent as `Authorization: Bearer <token>` on every
    /// request to this server. `Some` for agentum's own MCP (which requires it);
    /// `None` for servers that don't authenticate (e.g. a local Playwright).
    pub auth_token: Option<String>,
    /// Launch-scoped environment variable holding [`Self::auth_token`].
    ///
    /// Authenticated servers must receive a fresh, unpredictable, shell-safe
    /// name from the provisioning layer. Codex references this name from argv
    /// and receives the token only through its child environment. Keeping the
    /// name on the provisioned server prevents the argv and environment halves
    /// from independently deriving (and potentially disagreeing on) a name.
    /// Unauthenticated servers use `None`.
    pub auth_env_var: Option<String>,
}

/// A first-class tool integration. Trait methods are deliberately small so a
/// new adapter is a ~30-line file.
pub trait ToolAdapter: Send + Sync {
    /// Stable identifier — should match the `--tool` value users pass.
    fn name(&self) -> &'static str;

    /// Build the argv (and any env) tmux should spawn for this session.
    fn launch(&self, session: &Session) -> LaunchCommand;

    /// Build a launch command using facts resolved on an SSH target.
    ///
    /// Most tools have no host-specific argv and use [`Self::launch`]. Claude
    /// and terminal override this so remote transcript state and the remote
    /// login shell never get confused with state on the daemon machine.
    fn launch_remote(
        &self,
        session: &Session,
        _context: &RemoteLaunchContext<'_>,
    ) -> LaunchCommand {
        self.launch(session)
    }

    /// Extra startup args that register the provisioned MCP servers.
    ///
    /// Default: none — most tools (and shells) get no browser MCP. First-class
    /// agents that support launch-time MCP override this:
    /// - Claude Code: `--mcp-config <file>` (additive; we deliberately do NOT
    ///   pass `--strict-mcp-config`, which would disable the user's own servers).
    /// - Codex: repeated `-c mcp_servers.playwright.*` overrides (no file flag).
    ///
    /// Returns args to append to [`LaunchCommand::argv`]. Launch sites should
    /// normally call [`Self::apply_mcp`] so any accompanying environment is
    /// included too; they remain responsible for having started the servers
    /// and written `p.config_file`.
    fn mcp_args(&self, _p: &McpProvision) -> Vec<String> {
        Vec::new()
    }

    /// Per-process environment needed by [`Self::mcp_args`].
    ///
    /// Authentication secrets belong here, never in argv. The default is
    /// empty; Codex overrides it for authenticated streamable-HTTP servers.
    fn mcp_env(&self, _p: &McpProvision) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Apply all launch-time MCP configuration to one command.
    ///
    /// Keeping argv and environment application together prevents a caller
    /// from adding Codex's `bearer_token_env_var` reference but forgetting the
    /// referenced child environment value.
    fn apply_mcp(&self, launch: &mut LaunchCommand, p: &McpProvision) {
        launch.argv.extend(self.mcp_args(p));
        launch.env.extend(self.mcp_env(p));
    }

    /// Tool-specific watchdog "compact context" command, if any. Watchdog
    /// sends this verbatim followed by Enter when context-low signatures appear.
    fn compact_trigger(&self) -> Option<&'static str> {
        None
    }

    /// Substrings that, if seen in pane output, should mark the session
    /// crashed (and trigger watchdog auto-restart). Empty by default.
    fn crash_signatures(&self) -> &'static [&'static str] {
        &[]
    }

    /// Substring that means "this agent is currently working on a turn"
    /// (i.e. the spinner / 'esc to interrupt' line is on screen). When
    /// the watchdog observes a Working→!Working transition it emits
    /// `agent.finished`. Tools that don't have a stable busy marker
    /// return `None` and opt out of finished-notifications.
    fn busy_signature(&self) -> Option<&'static str> {
        None
    }

    /// Substrings that, when present in the visible pane, mean the agent
    /// is blocked waiting for the user to approve an action (a permission
    /// prompt, a plan-mode confirmation, etc.). Watchdog emits
    /// `agent.awaiting_input` on the !Awaiting→Awaiting transition.
    fn awaiting_input_signatures(&self) -> &'static [&'static str] {
        &[]
    }

    /// The tool-specific flag that enables "YOLO" / skip-permissions mode.
    /// Returns `None` for tools that have no such flag (or where we
    /// haven't verified the spelling).
    ///
    /// agentum's YOLO toggle is tool-agnostic at the UI layer — both the
    /// TUI and dashboard send the canonical Claude marker
    /// `--dangerously-skip-permissions` through the session's flags
    /// list. Each adapter's `launch` translates that marker to its own
    /// `yolo_flag` (Claude: identity; Codex: `--dangerously-bypass-
    /// approvals-and-sandbox`; tools with `None`: marker is dropped).
    /// This keeps a single source of truth for per-tool YOLO semantics.
    fn yolo_flag(&self) -> Option<&'static str> {
        None
    }

    /// Whether this adapter represents an interactive coding-agent CLI
    /// (Claude, Codex, Cursor, Gemini, …) as opposed to a plain shell
    /// or an unknown passthrough binary. The watchdog uses this to
    /// decide whether to apply change-based activity detection (idle =
    /// no pane output for ~3 s) when [`busy_signature`] is `None`.
    /// Without this fallback, every agent except Claude stayed pinned
    /// at `ActivityState::Unknown` forever and the sidebar `●` never
    /// flipped to the muted `◌` idle dot — see the v0.7.68 fix.
    ///
    /// Shells and passthroughs deliberately return `false`: an idle
    /// `bash` prompt isn't an "agent finished its turn" event and
    /// shouldn't fire a toast.
    fn is_agent(&self) -> bool {
        false
    }
}

/// The canonical "user wants YOLO" marker as it travels through
/// `Session::flags`. Both the TUI and the dashboard push this exact
/// string when the YOLO checkbox is on. Every adapter's `launch`
/// translates it via [`translate_yolo_marker`] to the tool-specific
/// flag (or drops it for tools without one).
pub const YOLO_MARKER: &str = "--dangerously-skip-permissions";

/// Walk `flags` and substitute any [`YOLO_MARKER`] for the adapter's
/// own `yolo_flag()` value. Markers are dropped entirely when the
/// adapter has no YOLO flag, so a YOLO toggle on a tool that doesn't
/// support it is silently a no-op rather than passing Claude's flag
/// to a binary that rejects it.
///
/// Order is preserved; non-marker flags pass through unchanged.
pub fn translate_yolo_marker(flags: &[String], yolo_flag: Option<&str>) -> Vec<String> {
    flags
        .iter()
        .filter_map(|f| {
            if f == YOLO_MARKER {
                yolo_flag.map(|s| s.to_string())
            } else {
                Some(f.clone())
            }
        })
        .collect()
}

/// Pick the right adapter for a tool name. Always returns something — unknown
/// tools get a passthrough.
pub fn adapter_for(tool: &str) -> Box<dyn ToolAdapter> {
    match tool {
        "claude" => Box::new(ClaudeAdapter),
        "codex" => Box::new(CodexAdapter),
        "cursor" => Box::new(CursorAdapter),
        "agent" => Box::new(AgentAdapter),
        "gemini" => Box::new(GeminiAdapter),
        "hermes" => Box::new(HermesAdapter),
        "terminal" => Box::new(TerminalAdapter),
        other => Box::new(PassthroughAdapter::new(other.to_string())),
    }
}

/// Names of the first-class executors agentum ships with built-in support for.
/// Each entry has a matching adapter in [`adapter_for`] with hand-tuned
/// argv, YOLO flag, and watchdog signatures.
pub const FIRST_CLASS: &[&str] = &["claude", "codex", "cursor", "agent", "gemini", "hermes"];

/// Names that the dashboard / TUI agent picker shows but that route
/// through [`PassthroughAdapter`] instead of a hand-tuned adapter.
/// They still belong in the availability probe so the picker can grey
/// them out when the binary isn't installed; their YOLO toggle is a
/// no-op until someone verifies the right flag spelling.
pub const PASSTHROUGH_PROBED: &[&str] = &["opencode", "aider"];

/// Every tool name the `/api/agents` probe should report. Order matters
/// — both surfaces render entries in this sequence so the picker stays
/// stable across releases.
pub fn probed_tools() -> impl Iterator<Item = &'static str> {
    FIRST_CLASS.iter().chain(PASSTHROUGH_PROBED.iter()).copied()
}

/// Tool name → expected binary name on `PATH`. Used by the agent-availability
/// probe so the TUI / dashboard can gate selection on whether the user
/// actually has the CLI installed (e.g. `cursor` ↦ `cursor-agent`).
///
/// Only the names that disagree with the tool id need an entry; for the
/// rest the tool id *is* the binary name.
pub fn binary_for(tool: &str) -> &str {
    match tool {
        "cursor" => "cursor-agent",
        other => other,
    }
}
