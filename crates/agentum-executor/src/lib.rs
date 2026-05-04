//! Tool-adapter abstraction (PRD §3 + §12 Phase 2b).
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
    ClaudeAdapter, CodexAdapter, GeminiAdapter, HermesAdapter, PassthroughAdapter,
};

/// What tmux actually launches: argv plus per-session environment overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl LaunchCommand {
    pub fn argv_only(argv: Vec<String>) -> Self {
        Self { argv, env: Vec::new() }
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
}

/// Pick the right adapter for a tool name. Always returns something — unknown
/// tools get a passthrough.
pub fn adapter_for(tool: &str) -> Box<dyn ToolAdapter> {
    match tool {
        "claude" => Box::new(ClaudeAdapter),
        "codex" => Box::new(CodexAdapter),
        "gemini" => Box::new(GeminiAdapter),
        "hermes" => Box::new(HermesAdapter),
        other => Box::new(PassthroughAdapter::new(other.to_string())),
    }
}

/// Names of the four first-class executors agentum ships with built-in support for.
pub const FIRST_CLASS: &[&str] = &["claude", "codex", "gemini", "hermes"];
