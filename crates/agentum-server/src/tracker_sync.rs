//! Session-lifecycle → tracker status sync (spec 012, F2–F4).
//!
//! Two thin layers that drive an item's status through the workspace's session
//! lifecycle by *calling* the one existing write seam
//! [`crate::task_sink::apply_tracker_transition`] (which spec 010 already taught
//! to move the Projects Status column for a bound repo) — this module writes no
//! label/Projects/Linear code of its own (invariant #1).
//!
//! - **Session-start reactor** (F2): a bus subscriber that, on `session.started`
//!   in a bound worktree, fires `InProgress`. Never inline in the launch path
//!   (invariant #2) — it hangs off the lifecycle bus the watchdog already emits.
//! - **PR/merge poller** (F3/F4): a bounded, backed-off `gh` loop that drives
//!   `InReview` on the first non-draft PR and `Done` on merge. No webhooks
//!   (invariant #6) → poll only.
//!
//! Every transition is idempotent, best-effort, and never-halt (invariant #3):
//! a failed transition logs and the session/poll proceeds. Advancement is guarded
//! by the pure monotonic [`next_phase_write`] (invariant #4) so status never
//! regresses (a reopened Done workspace does not drag the card back), the
//! session-start `InProgress` converges with the harness's own `InProgress`, and
//! the poller's `Done` is a restart-safe terminal (the persisted `tracker_phase`
//! excludes a merged workspace from the next tick).

use std::sync::Arc;

use agentum_core::Event;
use agentum_store::Store;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::task_sink::{TrackerPhase, apply_tracker_transition, parse_tracker_phase};

/// The canonical monotonic rank of a pipeline phase (spec 012 §4):
///
/// ```text
/// Todo(0) < InProgress(1) < InReview(2) < ReadyToTest(3) < Done(4)
/// ```
///
/// The `InReview(2)` slot is reserved for F3 so adding that variant never shifts
/// the others. Chosen so `InReview`'s nearest-earlier mapped phase is
/// `InProgress` (the spec's Projects fallback) and a gated run's `ReadyToTest`
/// (unit-green) can never regress to `InReview` when a PR opens. `Done(4)` always
/// wins — merge is terminal.
fn phase_rank(phase: TrackerPhase) -> i8 {
    match phase {
        TrackerPhase::Todo => 0,
        TrackerPhase::InProgress => 1,
        // TrackerPhase::InReview => 2,  // reserved (F3)
        TrackerPhase::ReadyToTest => 3,
        TrackerPhase::Done => 4,
    }
}

/// The lowercase wire form of a phase — the value persisted as a worktree's
/// `tracker_phase` and re-parsed by [`parse_tracker_phase`] on the next event.
/// The exact inverse of `parse_tracker_phase` (round-trips).
pub(crate) fn tracker_phase_wire(phase: TrackerPhase) -> &'static str {
    match phase {
        TrackerPhase::Todo => "todo",
        TrackerPhase::InProgress => "in_progress",
        TrackerPhase::ReadyToTest => "ready_to_test",
        TrackerPhase::Done => "done",
    }
}

/// The monotonic-forward guard (invariant #4). Returns `Some(target)` only when
/// `target` ranks strictly above the worktree's persisted phase; `None` when the
/// item is already at or past `target` (idempotent / no-thrash / no regress).
///
/// An absent or unparseable `current` ranks below `Todo`, so a first transition
/// always advances. Because the guard reads the *persisted* phase, a session
/// re-start, reconnect, or extra tab is a no-op, the session-start `InProgress`
/// converges with the harness's own `InProgress`, and a merged workspace's `Done`
/// is a restart-safe terminal.
pub(crate) fn next_phase_write(
    current: Option<&str>,
    target: TrackerPhase,
) -> Option<TrackerPhase> {
    let current_rank = current
        .and_then(parse_tracker_phase)
        .map(phase_rank)
        .unwrap_or(-1);
    if current_rank < phase_rank(target) {
        Some(target)
    } else {
        None
    }
}

/// Resolve a worktree's persisted bind coords into `(provider, tracker_url)`, or
/// `None` for an unbound worktree (invariant #5, fail-closed). A provider outside
/// the supported set or an empty URL yields no binding — never a fabricated one.
pub(crate) fn resolve_binding(
    provider: Option<&str>,
    url: Option<&str>,
) -> Option<(String, String)> {
    let provider = provider.map(str::trim).filter(|p| !p.is_empty())?;
    if !matches!(provider, "github" | "linear") {
        return None;
    }
    let url = url.map(str::trim).filter(|u| !u.is_empty())?;
    Some((provider.to_string(), url.to_string()))
}

/// The session-start reactor's pure decision (AC 5–7): given a worktree's bind
/// coords + its persisted phase, what transition (if any) should a session start
/// fire? `None` for an unbound worktree (silent no-op) or one already ≥
/// `InProgress` (converges with the harness, blocks a Done→InProgress regress).
pub(crate) fn session_start_decision(
    provider: Option<&str>,
    url: Option<&str>,
    current_phase: Option<&str>,
) -> Option<(String, String, TrackerPhase)> {
    let (provider, url) = resolve_binding(provider, url)?;
    let target = next_phase_write(current_phase, TrackerPhase::InProgress)?;
    Some((provider, url, target))
}

/// The tracker id `apply_tracker_transition` needs per provider. The GitHub arm
/// ignores it (it parses `owner/repo` + number from the URL), so the URL doubles
/// as an inert id; the Linear arm uses the item identifier, which the worktree
/// persists as `linked_linear_issue` (falling back to the URL string).
fn tracker_id_for(provider: &str, url: &str, linked_linear_issue: Option<&str>) -> String {
    match provider {
        "linear" => linked_linear_issue
            .map(str::to_string)
            .unwrap_or_else(|| url.to_string()),
        _ => url.to_string(),
    }
}

/// React to one `session.started` event: map the session to its worktree by
/// workdir, and — if bound and not already advanced — fire `InProgress` and
/// persist the phase. Best-effort/never-halt (invariant #3): every miss is a
/// quiet return, every transport failure logs and is dropped.
async fn react_to_session_start(store: &Store, session_id: Uuid) {
    let Ok(Some(session)) = store.get_session_by_id(session_id).await else {
        return;
    };
    let workdir = session.workdir;
    let Some(worktree) = crate::routes::worktrees::find_tracker_worktree_by_path(&workdir) else {
        return; // a plain, non-registered workdir — silent no-op (AC 7)
    };
    let Some((provider, url, target)) = session_start_decision(
        worktree.tracker_provider.as_deref(),
        worktree.tracker_url.as_deref(),
        worktree.tracker_phase.as_deref(),
    ) else {
        return; // unbound, or already ≥ InProgress (converges / no regress)
    };
    let tracker_id = tracker_id_for(&provider, &url, worktree.linked_linear_issue.as_deref());
    match apply_tracker_transition(store, &provider, &tracker_id, Some(&url), target).await {
        Ok(result) => {
            tracing::info!(
                workdir = %workdir,
                provider = %provider,
                ?target,
                ?result,
                "session-start tracker transition"
            );
            // Persist the phase so the guard dedupes re-starts and the poller's
            // terminal-stop survives a reboot. A registry miss is a no-op.
            if let Err(e) = crate::routes::worktrees::persist_tracker_progress(
                &worktree.id,
                Some(tracker_phase_wire(target)),
                None,
            ) {
                tracing::warn!(error = %e, "persisting tracker_phase failed (non-fatal)");
            }
        }
        Err(e) => {
            tracing::warn!(workdir = %workdir, error = %e, "session-start tracker transition failed (non-fatal)");
        }
    }
}

/// The session-start reactor loop (F2): subscribe to the lifecycle bus and, on
/// each `session.started`, drive `InProgress` for a bound worktree. Runs forever
/// (until the bus closes at shutdown); a lagged receiver is skipped, never fatal.
/// Spawned at server boot alongside the other background workers.
pub async fn run_session_start_reactor(store: Arc<Store>, bus: broadcast::Sender<Event>) {
    let mut rx = bus.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) => {
                if event.kind == "session.started" {
                    if let Some(session_id) = event.session_id {
                        react_to_session_start(&store, session_id).await;
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_phase_write_is_monotonic_and_idempotent() {
        // A first transition always advances (absent → below Todo).
        assert_eq!(
            next_phase_write(None, TrackerPhase::InProgress),
            Some(TrackerPhase::InProgress)
        );
        assert_eq!(
            next_phase_write(Some("todo"), TrackerPhase::InProgress),
            Some(TrackerPhase::InProgress)
        );
        // Idempotent: re-firing the same phase is a no-op (converges with harness).
        assert_eq!(
            next_phase_write(Some("in_progress"), TrackerPhase::InProgress),
            None
        );
        // No regress: Done never drags back to InProgress on a session reopen.
        assert_eq!(
            next_phase_write(Some("done"), TrackerPhase::InProgress),
            None
        );
        // Done always advances from any earlier phase, and is terminal.
        assert_eq!(
            next_phase_write(Some("in_progress"), TrackerPhase::Done),
            Some(TrackerPhase::Done)
        );
        assert_eq!(next_phase_write(Some("done"), TrackerPhase::Done), None);
        // An unparseable persisted phase ranks below everything → advances.
        assert_eq!(
            next_phase_write(Some("garbage"), TrackerPhase::InProgress),
            Some(TrackerPhase::InProgress)
        );
    }

    #[test]
    fn tracker_phase_wire_round_trips_through_parse() {
        for phase in [
            TrackerPhase::Todo,
            TrackerPhase::InProgress,
            TrackerPhase::ReadyToTest,
            TrackerPhase::Done,
        ] {
            assert_eq!(parse_tracker_phase(tracker_phase_wire(phase)), Some(phase));
        }
    }

    #[test]
    fn resolve_binding_is_fail_closed() {
        // A fully-formed github/linear bind resolves.
        assert_eq!(
            resolve_binding(Some("github"), Some("https://github.com/o/r/issues/1")),
            Some((
                "github".to_string(),
                "https://github.com/o/r/issues/1".to_string()
            ))
        );
        assert_eq!(
            resolve_binding(Some("linear"), Some("ENG-9")),
            Some(("linear".to_string(), "ENG-9".to_string()))
        );
        // Fail-closed: no provider, an unsupported provider, or an empty URL binds nothing.
        assert_eq!(resolve_binding(None, Some("https://x")), None);
        assert_eq!(resolve_binding(Some("gitlab"), Some("https://x")), None);
        assert_eq!(resolve_binding(Some("github"), None), None);
        assert_eq!(resolve_binding(Some("github"), Some("   ")), None);
        assert_eq!(resolve_binding(Some(""), Some("https://x")), None);
    }

    #[test]
    fn session_start_fires_inprogress_for_bound_worktree() {
        let decision = session_start_decision(
            Some("github"),
            Some("https://github.com/o/r/issues/1"),
            None,
        );
        assert_eq!(
            decision,
            Some((
                "github".to_string(),
                "https://github.com/o/r/issues/1".to_string(),
                TrackerPhase::InProgress
            ))
        );
    }

    #[test]
    fn session_start_is_no_op_for_unbound_worktree() {
        // No provider/url at all → nothing to drive.
        assert_eq!(session_start_decision(None, None, None), None);
        // A provider but no URL (partial bind) → fail-closed, no transition.
        assert_eq!(session_start_decision(Some("github"), None, None), None);
    }

    #[test]
    fn session_start_converges_with_harness_inprogress_no_thrash() {
        // Already InProgress (e.g. the harness fired it) → no duplicate.
        assert_eq!(
            session_start_decision(
                Some("github"),
                Some("https://github.com/o/r/issues/1"),
                Some("in_progress")
            ),
            None
        );
        // Already Done → a re-opened session never regresses the card.
        assert_eq!(
            session_start_decision(
                Some("github"),
                Some("https://github.com/o/r/issues/1"),
                Some("done")
            ),
            None
        );
    }

    #[test]
    fn tracker_id_for_uses_identifier_for_linear_url_for_github() {
        assert_eq!(
            tracker_id_for("github", "https://github.com/o/r/issues/1", Some("ENG-9")),
            "https://github.com/o/r/issues/1"
        );
        assert_eq!(
            tracker_id_for("linear", "https://linear.app/x", Some("ENG-9")),
            "ENG-9"
        );
        // Linear with no persisted identifier falls back to the URL string.
        assert_eq!(tracker_id_for("linear", "ENG-42", None), "ENG-42");
    }
}
