//! Tool-adapter abstraction.
//!
//! Every supported AI CLI (Claude, Codex, Gemini, Hermes, …) implements
//! [`ToolAdapter`]. The rest of agentum talks to the trait — never to a
//! specific binary — so a `Session` row is a tool-agnostic identity that
//! becomes a concrete invocation only at spawn time.
//!
//! Unknown tools fall through to [`PassthroughAdapter`]: agentum trusts
//! whatever's on PATH and forwards `--model=…` plus the user-provided flags.

use agentum_core::Session;

mod adapters;

pub use adapters::{
    ClaudeAdapter, CodexAdapter, CursorAdapter, GeminiAdapter, HermesAdapter, PassthroughAdapter,
    TerminalAdapter,
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

/// A first-class tool integration. Trait methods are deliberately small so a
/// new adapter is a ~30-line file.
pub trait ToolAdapter: Send + Sync {
    /// Stable identifier — should match the `--tool` value users pass.
    fn name(&self) -> &'static str;

    /// Build the argv (and any env) tmux should spawn for this session.
    fn launch(&self, session: &Session) -> LaunchCommand;

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
        "gemini" => Box::new(GeminiAdapter),
        "hermes" => Box::new(HermesAdapter),
        "terminal" => Box::new(TerminalAdapter),
        other => Box::new(PassthroughAdapter::new(other.to_string())),
    }
}

/// Names of the first-class executors agentum ships with built-in support for.
/// Each entry has a matching adapter in [`adapter_for`] with hand-tuned
/// argv, YOLO flag, and watchdog signatures.
pub const FIRST_CLASS: &[&str] = &["claude", "codex", "cursor", "gemini", "hermes"];

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
