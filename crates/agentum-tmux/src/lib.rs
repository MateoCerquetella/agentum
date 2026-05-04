//! tmux subprocess adapter. Real implementation lands in PRD phase 2.
//!
//! This stub exists so the workspace compiles end-to-end during phase 1.

#[derive(Debug, thiserror::Error)]
pub enum TmuxError {
    #[error("tmux adapter not yet implemented (phase 2)")]
    NotImplemented,
}

/// Returns the tmux session name for an agentum session id.
pub fn target_for(name: &str) -> String {
    format!("agentum-{name}")
}
