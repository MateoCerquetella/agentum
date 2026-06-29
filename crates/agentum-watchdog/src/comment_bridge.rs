//! Watchdog → board-comment bridge: a background worker that turns agent
//! lifecycle events (`agent.*` / `session.crashed`) into `[system]` comments on
//! the bound card's thread, with dedupe + bus-lag recovery. `use super::*` pulls
//! in the parent crate's shared types/helpers; the handler is `pub(super)` for
//! the central test module.

use super::*;

/// Bridge from watchdog/agent events onto the bound card's comment thread.
///
/// CONTEXT D-05: separate bus-subscriber task (not folded into watch_session),
///   sibling to run_goal_reconciler.
/// CONTEXT D-06: body templates + 80-char signature cap.
/// CONTEXT D-07: in-memory HashMap<session_id, &'static str> dedupe;
///   skip identical back-to-back inserts.
/// CONTEXT D-08: skip events on goal cards (lbl == "goal") — the goal
///   reconciler already surfaces those state changes via goal.status.changed.
/// CONTEXT D-09: RecvError::Lagged logs warn and continues; no resync.
pub async fn run_session_comment_bridge(
    store: Arc<Store>,
    bus: tokio::sync::broadcast::Sender<Event>,
) {
    use tokio::sync::broadcast::error::RecvError;
    let mut rx = bus.subscribe();
    // Last comment kind per session; dedupes back-to-back identical inserts
    // (defense-in-depth against bus-lag double-fires — D-07).
    let mut last_kind: std::collections::HashMap<Uuid, &'static str> =
        std::collections::HashMap::new();

    loop {
        let ev = match rx.recv().await {
            Ok(ev) => ev,
            Err(RecvError::Closed) => break,
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(
                    lagged = n,
                    "session_comment_bridge: bus lagged; will resume on next event"
                );
                continue;
            }
        };

        // Filter to the three observable agent events ONLY (D-06).
        let kind: &'static str = match ev.kind.as_str() {
            "agent.awaiting_input" => "awaiting_input",
            "agent.finished" => "finished",
            "session.crashed" => "crashed",
            _ => continue,
        };

        if let Err(e) = handle_session_event(&store, &bus, &ev, kind, &mut last_kind).await {
            tracing::warn!(
                error = ?e, kind = %ev.kind,
                "session_comment_bridge: handle failed"
            );
        }
    }
}

/// Core dispatch for a single agent/session event.
///
/// Resolves session → card → applies goal-card filter → dedupe → inserts
/// the `[system]` comment.
pub(super) async fn handle_session_event(
    store: &Store,
    bus: &broadcast::Sender<Event>,
    ev: &Event,
    kind: &'static str,
    last_kind: &mut std::collections::HashMap<Uuid, &'static str>,
) -> Result<(), WatchdogError> {
    let Some(session_id) = ev.session_id else {
        return Ok(());
    };

    // Resolve the bound card (if any).
    let session = match store.get_session_by_id(session_id).await? {
        Some(s) => s,
        None => return Ok(()), // concurrent session delete; benign
    };
    let Some(card_id) = session.card_id else {
        return Ok(());
    };

    // Goal-card filter (CONTEXT D-08): skip planner sessions bound to goal
    // cards — the goal-status reconciler already surfaces those state changes.
    let card = match store.get_board_item(card_id).await? {
        Some(c) => c,
        None => return Ok(()), // card was deleted; benign
    };
    if card.lbl.as_deref() == Some("goal") {
        return Ok(());
    }

    // In-memory dedupe: skip identical back-to-back (session_id, kind) pairs
    // to guard against bus-lag double-fires (CONTEXT D-07).
    if last_kind.get(&session_id) == Some(&kind) {
        return Ok(());
    }
    last_kind.insert(session_id, kind);

    // Compose the body template (CONTEXT D-06 + UI-SPEC §Copywriting Contract).
    let body = match kind {
        "awaiting_input" => "[system] agent awaiting input".to_string(),
        "finished" => "[system] agent finished".to_string(),
        "crashed" => {
            let raw_sig = ev
                .payload
                .get("signature")
                .and_then(|v| v.as_str())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("unknown");
            // 80-char cap prevents comment-body amplification (T-02-15).
            let sig: String = raw_sig.chars().take(80).collect();
            format!("[system] session crashed: {sig}")
        }
        // SAFETY: kind is one of the three literals matched above.
        _ => unreachable!("kind matched one of the three literals above"),
    };

    store
        .create_board_comment(
            card_id,
            agentum_core::NewBoardComment {
                author: "system".to_string(),
                body,
            },
        )
        .await?;

    // Auto-advance the card's column to track the agent lifecycle — the user-
    // facing "update each task as it progresses" behaviour. A card an agent is
    // actively building sits in `doing`; when the agent finishes its turn the
    // work is ready for a human/verify pass, so move it to `review`. We do NOT
    // move on `awaiting_input` (the agent is mid-task, paused for input — still
    // Building) or `crashed` (left in place, with the system comment above), and
    // we only ever transition OUT of `doing`, so a manual move (straight to
    // `done`, or back to `todo`) is never clobbered. Emitting `board.updated`
    // with `parent_goal_id` both refreshes the board UI (it's a board-relevant
    // kind) and lets the goal-status reconciler roll the change up to the goal.
    if kind == "finished" && card.status == "doing" {
        let updated = store
            .patch_board_item(
                card_id,
                agentum_core::BoardPatch {
                    status: Some("review".to_string()),
                    ..Default::default()
                },
            )
            .await?;
        let _ = bus.send(Event::new("board.updated").with_payload(serde_json::json!({
            "id": updated.id,
            "status": updated.status,
            "parent_goal_id": updated.parent_goal_id,
        })));
    }

    Ok(())
}
