//! Boot-time repair for hijacked pane pipes (issue #244).
//!
//! A tmux pane has exactly ONE `pipe-pane` slot. The external-tmux attach
//! route used to let an agentum-MANAGED (`agentum-*`) session acquire a
//! second, EXTERNAL-flagged session row bound to the same pane. The duplicate
//! could never stream (`pipe_pane`'s `#{pane_pipe}` guard leaves the managed
//! session's live pipe in place), and detaching it ran `unpipe_pane` on the shared pane —
//! silently freezing the real session: output stops, keystrokes never echo.
//!
//! The attach route no longer creates such duplicates and the detach paths
//! now skip shared panes, but installs poisoned before the fix still carry
//! the duplicate rows and disarmed pipes. This one-shot boot sweep heals
//! them:
//!
//! 1. Delete every EXTERNAL-flagged row whose (host, resolved target)
//!    collides with a non-external row. Record-only — the pane, the tmux
//!    session, and the surviving row are untouched.
//! 2. Re-arm `pipe-pane` for local managed sessions that plausibly own a
//!    live pane (Running/Idle), so a pipe disarmed by the old bug resumes
//!    feeding the session log without waiting for the next stream connect
//!    (which also re-arms, lazily, since the same fix).

use std::sync::Arc;

use agentum_core::{EXTERNAL_TMUX_FLAG, LOCAL_HOST_ID, Session, Status};

/// The tmux target a row streams from: the stored one, or derived from the
/// session name exactly the way spawn derives it. Mirrors
/// `routes::sessions::tmux_target` (private to that module).
fn resolved_target(s: &Session) -> String {
    s.tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&s.name))
}

fn is_external(s: &Session) -> bool {
    s.flags.iter().any(|f| f == EXTERNAL_TMUX_FLAG)
}

/// Pure decision core, unit-testable without a store or tmux: the ids of
/// external-binding rows that duplicate a non-external row's pane on the
/// same host.
fn poisoned_external_bindings(all: &[Session]) -> Vec<uuid::Uuid> {
    all.iter()
        .filter(|ext| is_external(ext))
        .filter(|ext| {
            let target = resolved_target(ext);
            let host = ext.host_id.unwrap_or(LOCAL_HOST_ID);
            all.iter().any(|owner| {
                !is_external(owner)
                    && owner.id != ext.id
                    && owner.host_id.unwrap_or(LOCAL_HOST_ID) == host
                    && resolved_target(owner) == target
            })
        })
        .map(|ext| ext.id)
        .collect()
}

/// Run the sweep once. Call from `spawn_background_workers`. Both halves are
/// best-effort: a failure is logged and never blocks boot.
pub(crate) async fn repair_pane_bindings(store: Arc<agentum_store::Store>) {
    let all = match store.list_sessions(None).await {
        Ok(all) => all,
        Err(e) => {
            tracing::warn!(error = ?e, "pane repair: session listing failed; skipping sweep");
            return;
        }
    };

    let poisoned = poisoned_external_bindings(&all);
    for id in &poisoned {
        match store.delete_session(*id).await {
            Ok(()) => tracing::info!(session = %id, "removed duplicate external pane binding"),
            Err(e) => tracing::warn!(session = %id, error = ?e,
                "could not remove duplicate external pane binding"),
        }
    }

    // Re-arm local managed pipes. `pipe_pane` probes `#{pane_pipe}` and skips
    // healthy panes (a blind `-o` re-arm would TOGGLE their pipes off — issue
    // #270); dead/missing targets just fail and are ignored. External bindings
    // are excluded: their panes are user-owned and heal lazily at stream connect.
    for s in &all {
        if is_external(s)
            || s.host_id.is_some_and(|h| h != LOCAL_HOST_ID)
            || !matches!(s.status, Status::Running | Status::Idle)
        {
            continue;
        }
        let Ok(log) = agentum_store::paths::pane_log(&s.id.to_string()) else {
            continue;
        };
        let _ = agentum_tmux::pipe_pane(&resolved_target(s), &log).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `Session` through serde so the helper stays valid as optional
    /// fields are added to the struct.
    fn sess(name: &str, external: bool, target: Option<&str>, host: Option<&str>) -> Session {
        serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(),
            "name": name,
            "workdir": "/tmp",
            "tool": if external { "terminal" } else { "claude" },
            "model": null,
            "flags": if external { vec![EXTERNAL_TMUX_FLAG.to_string()] } else { vec![] },
            "status": "running",
            "tmux_target": target,
            "host_id": host,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "last_activity_at": null,
        }))
        .unwrap()
    }

    #[test]
    fn external_binding_on_managed_pane_is_poisoned() {
        // The exact shape observed live: a managed row and an external row
        // sharing one agentum-* target on the local host.
        let owner = sess(
            "finish-the-loop",
            false,
            Some("agentum-finish-the-loop"),
            None,
        );
        let dup = sess(
            "agentum-finish-the-loop",
            true,
            Some("agentum-finish-the-loop"),
            None,
        );
        let ids = poisoned_external_bindings(&[owner, dup.clone()]);
        assert_eq!(ids, vec![dup.id]);
    }

    #[test]
    fn managed_row_with_cleared_target_still_claims_derived_pane() {
        // Stop clears a managed row's tmux_target; the derived
        // `agentum-<name>` target must still shield the pane.
        let owner = sess("alpha", false, None, None);
        let dup = sess("agentum-alpha", true, Some("agentum-alpha"), None);
        let ids = poisoned_external_bindings(&[owner, dup.clone()]);
        assert_eq!(ids, vec![dup.id]);
    }

    #[test]
    fn legit_external_binding_survives() {
        // A user-owned tmux session with no managed sibling: keep it.
        let owner = sess("alpha", false, Some("agentum-alpha"), None);
        let ext = sess("my-tmux", true, Some("my-tmux"), None);
        assert!(poisoned_external_bindings(&[owner, ext]).is_empty());
    }

    #[test]
    fn same_target_on_different_hosts_is_not_a_collision() {
        // Target names repeat across hosts; only same-host pairs share a pane.
        let owner = sess("alpha", false, Some("agentum-alpha"), None);
        let ext = sess(
            "agentum-alpha",
            true,
            Some("agentum-alpha"),
            Some("4bfb2ccf-cdd0-4a82-8793-5d87906da5e0"),
        );
        assert!(poisoned_external_bindings(&[owner, ext]).is_empty());
    }

    #[test]
    fn external_pair_without_owner_is_left_alone() {
        // Two external rows on one pane is an attach-route invariant breach,
        // but deleting either would guess wrong — leave them for the user.
        let a = sess("ext-a", true, Some("shared"), None);
        let b = sess("ext-b", true, Some("shared"), None);
        assert!(poisoned_external_bindings(&[a, b]).is_empty());
    }
}
