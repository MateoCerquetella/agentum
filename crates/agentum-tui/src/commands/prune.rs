//! `agentum prune` — remove dead sessions the control plane is still tracking.
//!
//! The "zombie" problem: crashed agents leave Session rows (and their stale
//! tmux panes) behind, cluttering `ls` / the sidebar. This prunes them.
//!
//! Safety: prune only ever acts on sessions agentum already tracks in its store
//! and only those in a terminal state (`crashed`, or `stopped` with `--stopped`).
//! Running/idle sessions are never touched, and tmux sessions a user started
//! outside agentum are never in the store, so they can't be killed here. Dry-run
//! is the default — nothing is removed without `--yes`.

use agentum_core::{Session, Status};
use anyhow::Result;

/// The safety-critical predicate, kept pure so it can be exhaustively tested
/// without a live store. Crashed is always prunable; stopped only with the
/// opt-in flag; running/idle are NEVER prunable (the explicit arm makes the
/// compiler force a decision if a new Status variant is ever added).
pub fn is_prunable(status: &Status, include_stopped: bool) -> bool {
    match status {
        Status::Crashed => true,
        Status::Stopped => include_stopped,
        Status::Running | Status::Idle => false,
    }
}

/// Select the sessions a prune would remove, preserving store order.
pub fn prunable(sessions: &[Session], include_stopped: bool) -> Vec<&Session> {
    sessions
        .iter()
        .filter(|s| is_prunable(&s.status, include_stopped))
        .collect()
}

pub async fn run(yes: bool, include_stopped: bool) -> Result<()> {
    let (store, _) = super::open_store().await?;
    let sessions = store.list_sessions(None).await?;
    let targets = prunable(&sessions, include_stopped);

    if targets.is_empty() {
        let scope = if include_stopped {
            "crashed/stopped"
        } else {
            "crashed"
        };
        println!("no {scope} sessions to prune");
        return Ok(());
    }

    if !yes {
        println!(
            "Would prune {} dead session(s) — dry run, pass --yes to remove:",
            targets.len()
        );
        for s in &targets {
            let loc = s.host_label.as_deref().unwrap_or("local");
            println!(
                "  {:<8}  {}  ({} @ {})",
                s.status.as_str(),
                s.name,
                s.tool,
                loc
            );
        }
        return Ok(());
    }

    let mut removed = 0u32;
    for s in targets {
        // Best-effort tmux kill: a crashed pane is usually already gone, and a
        // remote session's pane lives on its host (not the local tmux server),
        // so we only attempt a local kill and ignore failures. The store row is
        // the thing that clutters the UI — deleting it is the real cleanup.
        if s.host_id.is_none() {
            let target = s
                .tmux_target
                .clone()
                .unwrap_or_else(|| agentum_tmux::target_for(&s.name));
            let _ = agentum_tmux::kill_session(&target).await;
        }
        store.delete_session(s.id).await?;
        println!("pruned      {}", s.name);
        removed += 1;
    }
    println!("\nremoved {removed} dead session(s)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crashed_is_always_prunable() {
        assert!(is_prunable(&Status::Crashed, false));
        assert!(is_prunable(&Status::Crashed, true));
    }

    #[test]
    fn running_and_idle_are_never_prunable() {
        for status in [Status::Running, Status::Idle] {
            assert!(!is_prunable(&status, false), "{status:?} must never prune");
            assert!(!is_prunable(&status, true), "{status:?} must never prune");
        }
    }

    #[test]
    fn stopped_only_prunable_with_flag() {
        assert!(!is_prunable(&Status::Stopped, false));
        assert!(is_prunable(&Status::Stopped, true));
    }
}
