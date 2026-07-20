//! Watchdog → tracker attention worker (spec 014 F4).
//!
//! A bus subscriber (the `comment_bridge` shape: event filter, per-key dedupe,
//! lag-tolerant loop) that turns a session's crash or a SUSTAINED
//! awaiting-input into the existing `status/blocked` escalation on the bound
//! issue — via [`crate::task_sink::apply_blocked_transition`], never new write
//! mechanics — and clears it on recovery by re-applying the persisted
//! pipeline phase verbatim through [`crate::task_sink::apply_tracker_transition`]
//! (idempotent, rank-equal: it can never advance or regress the phase; any
//! pipeline edit's remove-set drops `status/blocked` for free).
//!
//! Lives in the SERVER crate (not agentum-watchdog): the worker calls the
//! task_sink seam + the worktree registry helpers — a watchdog-crate worker
//! calling server code would be a dependency cycle. It consumes the watchdog's
//! *events* (kind strings on the shared bus), exactly like `tracker_sync`'s
//! session-start reactor.
//!
//! Never-halt (012 invariant #3): every write is best-effort and bounded by
//! `run_gh`'s 30 s timeout; failures log and drop. State is in-memory and
//! resets on server restart (accepted residual — a pre-restart blocked label
//! clears on the next real phase transition).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentum_core::Event;
use agentum_store::Store;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::routes::worktrees::{TrackerWorktree, find_tracker_worktree_by_path};
use crate::task_sink::{
    TrackerEmit, apply_blocked_transition, apply_tracker_transition, parse_tracker_phase,
};

/// Sweep-timer granularity. One coarse tick, no per-session spawned timers — a
/// 10-minute threshold does not need sub-30s precision, and a single tick
/// keeps the worker O(awaiting-sessions) with zero cancellation machinery.
const ATTENTION_SWEEP: Duration = Duration::from_secs(30);

/// A NEW blocked episode for the same worktree inside this window re-applies
/// the label (idempotent) but suppresses the duplicate comment (PM D2 / AC 10
/// crash-loop guard). A named constant, not user config.
const BLOCKED_COMMENT_COOLDOWN: Duration = Duration::from_secs(3600);

/// Sustained-awaiting threshold (PM D1): continuously awaiting input for this
/// long flags the issue. `AGENTUM_ATTENTION_AFTER_SECS`, default 600 (10 min).
fn attention_after() -> Duration {
    let secs = std::env::var("AGENTUM_ATTENTION_AFTER_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(600);
    Duration::from_secs(secs)
}

/// What a new episode should write (the pure decision, AC 9/10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fire {
    /// Episode already active — one signal per episode, nothing to do.
    Skip,
    /// Fresh episode, outside the comment cooldown: label + one comment.
    LabelAndComment,
    /// Fresh episode INSIDE the cooldown of the last comment: re-apply the
    /// (idempotent) label, suppress the duplicate comment.
    LabelOnly,
}

/// One worktree's blocked-episode state. Keyed by WORKTREE (the issue is the
/// write target): two sessions in one workspace share one episode, so a
/// double-crash can't double-comment.
#[derive(Debug, Default)]
struct Episode {
    active: bool,
    last_comment_at: Option<Instant>,
}

/// The worker's in-memory state. Pure decision core (no IO, no time mocking —
/// every method takes `now`), unit-tested below.
#[derive(Debug, Default)]
struct Ledger {
    /// session → when it entered awaiting_input. Cleared on
    /// agent.working / agent.input_resolved / agent.finished /
    /// session.started, and once a sweep handles the session.
    awaiting_since: HashMap<Uuid, Instant>,
    episodes: HashMap<String, Episode>,
}

/// Has this awaiting session crossed the attention threshold?
fn due(awaiting_since: Instant, now: Instant, threshold: Duration) -> bool {
    now.duration_since(awaiting_since) >= threshold
}

impl Ledger {
    fn note_awaiting(&mut self, session: Uuid, now: Instant) {
        self.awaiting_since.entry(session).or_insert(now);
    }

    fn clear_awaiting(&mut self, session: &Uuid) {
        self.awaiting_since.remove(session);
    }

    /// Sessions whose awaiting has persisted past the threshold at `now`.
    fn due_sessions(&self, now: Instant, threshold: Duration) -> Vec<Uuid> {
        self.awaiting_since
            .iter()
            .filter(|(_, since)| due(**since, now, threshold))
            .map(|(id, _)| *id)
            .collect()
    }

    /// Start (or refuse to restart) a blocked episode for a worktree.
    fn begin_episode(&mut self, worktree_id: &str, now: Instant, cooldown: Duration) -> Fire {
        let ep = self.episodes.entry(worktree_id.to_string()).or_default();
        if ep.active {
            return Fire::Skip;
        }
        ep.active = true;
        match ep.last_comment_at {
            Some(at) if now.duration_since(at) < cooldown => Fire::LabelOnly,
            _ => {
                ep.last_comment_at = Some(now);
                Fire::LabelAndComment
            }
        }
    }

    /// End an episode on recovery. `true` ⇒ something was actually flagged and
    /// the caller re-applies the phase (a transient answered prompt — no
    /// active episode — clears nothing and writes nothing).
    fn end_episode(&mut self, worktree_id: &str) -> bool {
        match self.episodes.get_mut(worktree_id) {
            Some(ep) if ep.active => {
                ep.active = false;
                true
            }
            _ => false,
        }
    }

    /// Cheap guard so the chatty recovery kinds (`agent.working` fires every
    /// activity transition) skip the session+registry resolve entirely while
    /// nothing is flagged.
    fn any_active_episode(&self) -> bool {
        self.episodes.values().any(|e| e.active)
    }
}

/// The attention worker loop. Spawned at server boot beside the poller.
pub async fn run_tracker_attention_worker(store: Arc<Store>, bus: broadcast::Sender<Event>) {
    let mut rx = bus.subscribe();
    let mut ledger = Ledger::default();
    let threshold = attention_after();
    let mut sweep = tokio::time::interval(ATTENTION_SWEEP);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(event) => handle_event(&store, &bus, &mut ledger, &event).await,
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // comment_bridge's D-09: log + continue, never die on lag.
                    tracing::warn!(skipped, "tracker-attention bus receiver lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = sweep.tick() => sweep_due(&store, &bus, &mut ledger, threshold).await,
        }
    }
}

/// The IO shell for one bus event (mirrors `tracker_sync::react_to_session_start`).
async fn handle_event(
    store: &Store,
    bus: &broadcast::Sender<Event>,
    ledger: &mut Ledger,
    event: &Event,
) {
    let now = Instant::now();
    match event.kind.as_str() {
        // Cheap bookkeeping only — no registry read on chatty streams; the
        // sweep does the resolve once a session is actually due.
        "agent.awaiting_input" => {
            if let Some(id) = event.session_id {
                ledger.note_awaiting(id, now);
            }
        }
        // A finished turn clears the awaiting timer only, NOT an episode
        // (AC 10 lists the exact clear conditions; "finished" after a blocked
        // episode is not "recovered").
        "agent.finished" => {
            if let Some(id) = event.session_id {
                ledger.clear_awaiting(&id);
            }
        }
        // Immediate escalation — no threshold for a crash (AC 8).
        "session.crashed" => {
            let Some(id) = event.session_id else { return };
            ledger.clear_awaiting(&id);
            let Some((session, wt)) = resolve_bound_github(store, id).await else {
                return; // unbound / non-github — silent no-op (fail-closed)
            };
            let fire = ledger.begin_episode(&wt.id, now, BLOCKED_COMMENT_COOLDOWN);
            let gate_tail = event
                .payload
                .get("signature")
                .and_then(|v| v.as_str())
                .unwrap_or("(no crash signature)")
                .to_string();
            fire_blocked(
                store,
                bus,
                &wt,
                fire,
                &session.name,
                "session crash",
                &gate_tail,
            )
            .await;
        }
        // Recovery: clear the awaiting timer, and if an episode was active,
        // re-apply the persisted phase (which drops `status/blocked`).
        "agent.working" | "agent.input_resolved" | "session.started" => {
            let Some(id) = event.session_id else { return };
            ledger.clear_awaiting(&id);
            if !ledger.any_active_episode() {
                return; // nothing flagged anywhere — skip the resolve
            }
            let Some((_session, wt)) = resolve_bound_github(store, id).await else {
                return;
            };
            if ledger.end_episode(&wt.id) {
                clear_blocked(store, bus, &wt).await;
            }
        }
        _ => {}
    }
}

/// One sweep tick: escalate every session whose awaiting persisted past the
/// threshold (AC 9). Each due session is handled once — its timer is dropped;
/// the per-worktree episode dedupes any sibling's concurrent fire.
async fn sweep_due(
    store: &Store,
    bus: &broadcast::Sender<Event>,
    ledger: &mut Ledger,
    threshold: Duration,
) {
    let now = Instant::now();
    for id in ledger.due_sessions(now, threshold) {
        ledger.clear_awaiting(&id);
        let Some((session, wt)) = resolve_bound_github(store, id).await else {
            continue; // unbound — silent no-op (fail-closed)
        };
        let fire = ledger.begin_episode(&wt.id, now, BLOCKED_COMMENT_COOLDOWN);
        let gate_tail = format!(
            "agent has been awaiting input for over {} minutes",
            threshold.as_secs() / 60
        );
        fire_blocked(
            store,
            bus,
            &wt,
            fire,
            &session.name,
            "awaiting input",
            &gate_tail,
        )
        .await;
    }
}

/// Session → its bound GITHUB worktree, or None (fail-closed). The attention
/// signal is GitHub-label-only (spec scope), so linear/board binds never
/// start an episode — and therefore never get a clear re-apply either.
async fn resolve_bound_github(
    store: &Store,
    session_id: Uuid,
) -> Option<(agentum_core::Session, TrackerWorktree)> {
    let session = store.get_session_by_id(session_id).await.ok()??;
    let wt = find_tracker_worktree_by_path(&session.workdir).or_else(|| {
        session
            .worktree_path
            .as_deref()
            .and_then(find_tracker_worktree_by_path)
    })?;
    if wt.tracker_provider.as_deref() != Some("github") {
        return None;
    }
    wt.tracker_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())?;
    Some((session, wt))
}

/// One best-effort blocked write through the existing seam. `Fire::Skip` never
/// touches `gh`; the URL doubles as the (inert, GitHub-ignored) tracker id,
/// exactly like `tracker_sync`.
async fn fire_blocked(
    store: &Store,
    bus: &broadcast::Sender<Event>,
    wt: &TrackerWorktree,
    fire: Fire,
    feature_name: &str,
    gate_label: &str,
    gate_tail: &str,
) {
    if fire == Fire::Skip {
        return;
    }
    let Some(url) = wt.tracker_url.as_deref() else {
        return;
    };
    let with_comment = fire == Fire::LabelAndComment;
    match apply_blocked_transition(
        store,
        "github",
        url,
        Some(url),
        feature_name,
        gate_label,
        1,
        gate_tail,
        with_comment,
        TrackerEmit {
            bus,
            worktree_id: Some(&wt.id),
        },
    )
    .await
    {
        Ok(result) => {
            tracing::info!(worktree = %wt.id, gate_label, with_comment, ?result, "attention blocked write");
        }
        Err(e) => {
            tracing::warn!(worktree = %wt.id, error = %e, "attention blocked write failed (non-fatal)");
        }
    }
}

/// The recovery clear (PM D2): re-apply the worktree's PERSISTED phase
/// verbatim through the pipeline seam — intentionally bypassing
/// `next_phase_write` (rank-equal is the point), so it can never advance or
/// regress. The `gh issue edit` remove-set drops `status/blocked` for free,
/// and the resulting `tracker.phase_changed` clears the chip (AC 11). No
/// persisted phase ⇒ skip — never fabricate one.
async fn clear_blocked(store: &Store, bus: &broadcast::Sender<Event>, wt: &TrackerWorktree) {
    let Some(phase) = wt.tracker_phase.as_deref().and_then(parse_tracker_phase) else {
        return;
    };
    let Some(url) = wt.tracker_url.as_deref() else {
        return;
    };
    match apply_tracker_transition(
        store,
        "github",
        url,
        Some(url),
        phase,
        TrackerEmit {
            bus,
            worktree_id: Some(&wt.id),
        },
    )
    .await
    {
        Ok(result) => {
            tracing::info!(worktree = %wt.id, ?phase, ?result, "attention clear (phase re-apply)");
        }
        Err(e) => {
            tracing::warn!(worktree = %wt.id, error = %e, "attention clear failed (non-fatal)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COOLDOWN: Duration = Duration::from_secs(3600);
    const THRESHOLD: Duration = Duration::from_secs(600);

    #[test]
    fn due_respects_the_threshold() {
        let start = Instant::now();
        assert!(!due(start, start, THRESHOLD));
        assert!(!due(start, start + Duration::from_secs(599), THRESHOLD));
        assert!(due(start, start + THRESHOLD, THRESHOLD));
        assert!(due(start, start + Duration::from_secs(3600), THRESHOLD));
    }

    #[test]
    fn transient_prompt_answered_before_threshold_fires_nothing() {
        let mut ledger = Ledger::default();
        let id = Uuid::new_v4();
        let t0 = Instant::now();
        ledger.note_awaiting(id, t0);
        // Answered 2 minutes in — the timer clears...
        ledger.clear_awaiting(&id);
        // ...so a later sweep finds nothing due.
        assert!(
            ledger
                .due_sessions(t0 + Duration::from_secs(3600), THRESHOLD)
                .is_empty()
        );
        // And with no episode ever begun, recovery clears nothing.
        assert!(!ledger.end_episode("r1::/p"));
    }

    #[test]
    fn note_awaiting_keeps_the_first_timestamp() {
        let mut ledger = Ledger::default();
        let id = Uuid::new_v4();
        let t0 = Instant::now();
        ledger.note_awaiting(id, t0);
        // A repeated awaiting event must not reset the clock (the whole point
        // of "continuously awaiting").
        ledger.note_awaiting(id, t0 + Duration::from_secs(500));
        assert_eq!(
            ledger.due_sessions(t0 + THRESHOLD, THRESHOLD),
            vec![id],
            "due from the FIRST awaiting timestamp"
        );
    }

    #[test]
    fn one_fire_per_episode() {
        let mut ledger = Ledger::default();
        let now = Instant::now();
        assert_eq!(
            ledger.begin_episode("r1::/p", now, COOLDOWN),
            Fire::LabelAndComment
        );
        // Still active: a sibling session's crash / another due sweep is a Skip.
        assert_eq!(ledger.begin_episode("r1::/p", now, COOLDOWN), Fire::Skip);
        assert_eq!(
            ledger.begin_episode("r1::/p", now + Duration::from_secs(60), COOLDOWN),
            Fire::Skip
        );
    }

    #[test]
    fn crash_loop_inside_cooldown_relabels_without_comment() {
        let mut ledger = Ledger::default();
        let t0 = Instant::now();
        assert_eq!(
            ledger.begin_episode("r1::/p", t0, COOLDOWN),
            Fire::LabelAndComment
        );
        // Recovered...
        assert!(ledger.end_episode("r1::/p"));
        // ...crashed again 30 minutes later: label yes, comment no (AC 10).
        assert_eq!(
            ledger.begin_episode("r1::/p", t0 + Duration::from_secs(1800), COOLDOWN),
            Fire::LabelOnly
        );
        assert!(ledger.end_episode("r1::/p"));
        // Past the cooldown the comment comes back.
        assert_eq!(
            ledger.begin_episode("r1::/p", t0 + Duration::from_secs(3601), COOLDOWN),
            Fire::LabelAndComment
        );
    }

    #[test]
    fn end_episode_gates_the_clear() {
        let mut ledger = Ledger::default();
        let now = Instant::now();
        // No episode → no clear write.
        assert!(!ledger.end_episode("r1::/p"));
        ledger.begin_episode("r1::/p", now, COOLDOWN);
        // First recovery clears...
        assert!(ledger.end_episode("r1::/p"));
        // ...and a duplicate recovery event doesn't re-clear (no write churn).
        assert!(!ledger.end_episode("r1::/p"));
    }

    #[test]
    fn episodes_are_per_worktree_not_per_session() {
        let mut ledger = Ledger::default();
        let now = Instant::now();
        assert_eq!(
            ledger.begin_episode("r1::/p", now, COOLDOWN),
            Fire::LabelAndComment
        );
        // A second worktree gets its own episode (and its own comment).
        assert_eq!(
            ledger.begin_episode("r2::/q", now, COOLDOWN),
            Fire::LabelAndComment
        );
    }

    #[test]
    fn any_active_episode_gates_the_recovery_resolve() {
        let mut ledger = Ledger::default();
        assert!(!ledger.any_active_episode());
        ledger.begin_episode("r1::/p", Instant::now(), COOLDOWN);
        assert!(ledger.any_active_episode());
        ledger.end_episode("r1::/p");
        assert!(!ledger.any_active_episode());
    }

    #[test]
    fn attention_after_default_is_ten_minutes() {
        // Hermetic only when the env var is unset — guard rather than mutate.
        if std::env::var("AGENTUM_ATTENTION_AFTER_SECS").is_err() {
            assert_eq!(attention_after(), Duration::from_secs(600));
        }
    }
}
