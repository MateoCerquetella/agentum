//! Goal-status reconciler: a background worker that keeps a parent goal's status
//! equal to `max(child statuses)` and fires the planner's first-child auto-stop.
//! Driven by board events off the shared bus. `use super::*` pulls in the parent
//! crate's `Store`/`Event`/`emit`/error types; the handlers are `pub(super)` so
//! the central test module can drive them.

use super::*;

/// Subscribes to the broadcast bus and reconciles goal statuses against their
/// children per CONTEXT D-03 (`goal.status = max(child statuses)`).
///
/// Spawned alongside `Watchdog::run` in `agentum-server::serve()`. Handles:
/// - `board.created` / `board.updated` / `board.deleted` events with a
///   `parent_goal_id` payload — recomputes the parent goal's status via a
///   single `max_child_status_rank` SQL call and patches if the rank differs.
/// - D-07 planner auto-stop: on the *first* `board.created` event for each
///   goal, emits `goal.planner.first_child` and calls `graceful_stop` on the
///   planner session bound via `session.card_id = goal.id`.
///
/// The `planner_stopped` HashSet is in-memory only: a daemon restart resets
/// it, which may cause a duplicate `graceful_stop` call on already-dead
/// planner panes. Those calls log a warning and are otherwise harmless.
pub async fn run_goal_reconciler(store: Arc<Store>, bus: tokio::sync::broadcast::Sender<Event>) {
    let mut rx = bus.subscribe();
    // Tracks which goal ids have already had their planner session stopped so
    // we never fire the auto-stop twice per daemon lifetime (D-07 idempotency).
    let mut planner_stopped: std::collections::HashSet<i64> = std::collections::HashSet::new();

    loop {
        let ev = match rx.recv().await {
            Ok(ev) => ev,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                // The bus dropped events we couldn't consume fast enough. The
                // next event triggers a fresh `max_child_status_rank` read, so
                // the goal status will converge on the next child transition
                // even if we missed intermediate events here (T-04-02).
                tracing::warn!(
                    lagged = n,
                    "goal reconciler lagged; will recompute on next event"
                );
                continue;
            }
        };
        if !matches!(
            ev.kind.as_str(),
            "board.created" | "board.updated" | "board.deleted"
        ) {
            continue;
        }
        if let Err(e) = handle_board_event(&store, &bus, &ev, &mut planner_stopped).await {
            tracing::warn!(error = ?e, kind = %ev.kind, "goal reconcile failed");
        }
    }
}

/// Core dispatch for a single board event. Extracts the parent goal id,
/// applies depth-1 guard, fires the planner auto-stop on first child, then
/// patches the goal's status when the computed rank diverges.
async fn handle_board_event(
    store: &Store,
    bus: &tokio::sync::broadcast::Sender<Event>,
    ev: &Event,
    planner_stopped: &mut std::collections::HashSet<i64>,
) -> Result<(), WatchdogError> {
    let Some(goal_id) = extract_parent_goal_id(store, ev).await? else {
        return Ok(());
    };

    let goal = match store.get_board_item(goal_id).await? {
        Some(item) => item,
        // Goal row deleted concurrently; nothing to reconcile.
        None => return Ok(()),
    };

    // Depth-1 invariant (CONTEXT D-03 + PATTERNS.md): goals don't have
    // parents in v1. If this goal row itself has a parent_goal_id, the data
    // is in an unexpected state. Log a warning and skip rather than silently
    // cascading writes up an unbounded tree.
    if goal.parent_goal_id.is_some() {
        tracing::warn!(
            goal_id,
            "v1 depth-1 invariant violated: goal has a parent; skipping recompute"
        );
        return Ok(());
    }

    // D-07 planner auto-stop — only on the first board.created child observed
    // for this goal per daemon lifetime. `HashSet::insert` returns true only
    // on the first insertion, ensuring idempotency.
    if ev.kind == "board.created" && planner_stopped.insert(goal_id) {
        if let Some(session) = store.get_session_by_card_id(goal_id).await? {
            if matches!(session.status, agentum_core::Status::Running) {
                // Emit the event BEFORE the tmux call so tests without a real
                // tmux fixture (and downstream observers like the dashboard)
                // still observe "first child arrived" even when the stop fails.
                let _ = bus.send(Event::new("goal.planner.first_child").with_payload(
                    serde_json::json!({
                        "goal_id": goal_id,
                        "planner_session_id": session.id.to_string(),
                    }),
                ));
                let target = agentum_tmux::target_for(&session.name);
                // 5-second timeout mirrors the sessions route's GRACEFUL_STOP_TIMEOUT.
                if let Err(e) = agentum_tmux::graceful_stop(&target, Duration::from_secs(5)).await {
                    // Best-effort: the event already fired; log the failure but
                    // don't propagate it — the goal-status recompute below is
                    // the important part and must not be skipped.
                    tracing::warn!(
                        error = %e,
                        session = %session.name,
                        "planner graceful_stop failed; pane may have already exited"
                    );
                }
            }
        }
    }

    // Recompute goal status (D-03 invariant). Single SQL call returns the
    // MAX rank across all children, or NULL when no children exist.
    let rank = store.max_child_status_rank(goal_id).await?;
    let target_status = match rank {
        // No children: max of empty set → todo (D-03 "empty-children" rule).
        None | Some(0) => "todo",
        Some(1) => "doing",
        Some(2) => "done",
        // Negative ranks come from the SQL ELSE -1 arm (unrecognised status
        // strings from future migrations or legacy data). Do NOT silently
        // demote the goal — a bad child status is not a signal to move the
        // goal backwards. Log and skip.
        Some(r) if r < 0 => {
            tracing::warn!(
                goal_id,
                rank = r,
                "child has unrecognised status string; skipping goal recompute"
            );
            return Ok(());
        }
        Some(other) => {
            tracing::warn!(
                goal_id,
                rank = other,
                "unexpected child status rank; skipping goal recompute"
            );
            return Ok(());
        }
    };

    // Skip the PATCH when the goal is already at the right status. Keeps the
    // bus quiet and prevents spurious `goal.status.changed` events on
    // repeated identical child events.
    let current_rank = status_rank(&goal.status);
    let target_rank = status_rank(target_status);
    if current_rank == target_rank {
        return Ok(());
    }

    // Write directly through `patch_board_item`, bypassing `enforce_transition`.
    // The watchdog is the sole auto-writer of goal status; goals are not
    // required to have workdir/tool so the normal gate would reject them.
    let patch = agentum_core::BoardPatch {
        status: Some(target_status.to_string()),
        ..Default::default()
    };
    store.patch_board_item(goal_id, patch).await?;
    let _ = bus.send(
        Event::new("goal.status.changed").with_payload(serde_json::json!({
            "goal_id": goal_id,
            "from": goal.status,
            "to": target_status,
        })),
    );
    Ok(())
}

/// Extract the `parent_goal_id` from the event payload.
///
/// For `board.created` and `board.updated`, the payload includes
/// `parent_goal_id` set by the route handler (plan 01-03). For
/// `board.deleted`, the payload also includes `parent_goal_id` (plan 01-03
/// Task 1 step 5b extends the delete handler). Falls back to a DB lookup for
/// `board.updated` events whose payload lacks the field (defensive, shouldn't
/// happen with plan 01-03 in place). For `board.deleted` without the field,
/// there is no fallback — the row is gone — so returns `None`.
async fn extract_parent_goal_id(
    store: &Store,
    ev: &Event,
) -> Result<Option<i64>, WatchdogError> {
    // Fast path: payload carries parent_goal_id directly.
    if let Some(v) = ev.payload.get("parent_goal_id") {
        return Ok(v.as_i64());
    }

    // Deleted rows can't be re-fetched; accept absence as "no parent".
    if ev.kind == "board.deleted" {
        return Ok(None);
    }

    // Fallback for board.updated/created without the field: DB lookup.
    if let Some(id) = ev.payload.get("id").and_then(|v| v.as_i64()) {
        if let Some(item) = store.get_board_item(id).await? {
            return Ok(item.parent_goal_id);
        }
    }
    Ok(None)
}

/// Rank ordering used by D-03's invariant: goal.status = max(child statuses).
/// Returns -1 for any status string not in the canonical set; the caller
/// must treat negative ranks as "unknown / skip recompute" rather than
/// silently treating them as todo.
pub(crate) fn status_rank(s: &str) -> i32 {
    match s {
        "todo" => 0,
        "doing" => 1,
        // `review` ranks with `doing` for goal rollup — a child awaiting
        // verification keeps the goal in-progress, not done (mirrors the
        // `max_child_status_rank` SQL CASE).
        "review" => 1,
        "done" => 2,
        _ => -1,
    }
}

/// Inverse of [`status_rank`]. Returns `None` for ranks outside [0, 2].
/// Available to tests and to any future consumer that needs to convert
/// a DB-origin i32 rank back to the canonical status string.
#[allow(dead_code)]
pub(crate) fn rank_to_status(r: i32) -> Option<&'static str> {
    match r {
        0 => Some("todo"),
        1 => Some("doing"),
        2 => Some("done"),
        _ => None,
    }
}
