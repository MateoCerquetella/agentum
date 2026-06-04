//! The managed agent-status hook: a single POSIX-sh script the server installs
//! per agent. It normalizes each CLI's native lifecycle payload into
//! `{kind: working|done|permission}` and POSTs it to `$AGENTUM_HOOK_URL`, so
//! agents that emit no status in their terminal title still drive the sidebar
//! spinner. See [`crate::AgentHookInstall`] for how each adapter registers it.

/// The script body, embedded at compile time. `$1` carries Codex's argv JSON;
/// Claude-family CLIs pipe their payload on stdin.
pub const HOOK_SCRIPT: &str = include_str!("hook_script.sh");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_maps_working_done_permission_for_both_payload_styles() {
        // working triggers (Claude-family + Codex)
        assert!(HOOK_SCRIPT.contains("UserPromptSubmit"));
        assert!(HOOK_SCRIPT.contains("task_started"));
        // done triggers
        assert!(HOOK_SCRIPT.contains("agent-turn-complete"));
        assert!(HOOK_SCRIPT.contains("Stop"));
        // permission triggers
        assert!(HOOK_SCRIPT.contains("exec_approval_request"));
        // explicit per-event kind (preferred) + payload-parse fallback
        assert!(HOOK_SCRIPT.contains("AGENTUM_HOOK_KIND"));
        // both event field names are read in the fallback
        assert!(HOOK_SCRIPT.contains("hook_event_name"));
        assert!(HOOK_SCRIPT.contains("\"type\""));
        // posts kind + token to the env-provided URL
        assert!(HOOK_SCRIPT.contains("$AGENTUM_HOOK_URL"));
        assert!(HOOK_SCRIPT.contains("X-Agentum-Hook-Token"));
        assert!(HOOK_SCRIPT.contains("\\\"kind\\\""));
    }
}
