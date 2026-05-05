//! Per-session watchdog.
//!
//! Spawned by the server. Reconciles the running-task set against the DB
//! every tick: a session that becomes `running` gets its own watch task,
//! one that leaves `running` has its task aborted. Each task captures the
//! pane every 5 s and applies the rule table:
//!
//! | Pattern (last 100 lines)            | Action                  | Cooldown |
//! |-------------------------------------|-------------------------|----------|
//! | `Context low.*<\s*50%`              | send `/compact` + Enter | 5 min    |
//! | crash signature OR pane exited      | mark crashed, emit      | n/a      |
//!
//! Crash signatures come from the executor adapter, so each first-class
//! tool can declare its own (Claude has `redacted_thinking`, etc.).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentum_core::{Event, Session, Status};
use agentum_store::Store;
use regex::Regex;
use tokio::sync::{RwLock, broadcast};
use tokio::task::JoinHandle;
use tokio::time::interval;
use uuid::Uuid;

const TICK: Duration = Duration::from_secs(5);
const COMPACT_COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// How long the orchestrator waits between reconcile passes. Visible for
/// integration tests that want a faster cadence.
pub const RECONCILE_TICK: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum WatchdogError {
    #[error(transparent)]
    Store(#[from] agentum_store::StoreError),
    #[error(transparent)]
    Tmux(#[from] agentum_tmux::TmuxError),
}

/// Orchestrator. Holds the broadcast bus + a map of in-flight per-session
/// task handles.
pub struct Watchdog {
    bus: broadcast::Sender<Event>,
    store: Arc<Store>,
    tasks: Arc<RwLock<HashMap<Uuid, JoinHandle<()>>>>,
}

impl Watchdog {
    pub fn new(bus: broadcast::Sender<Event>, store: Arc<Store>) -> Self {
        Self {
            bus,
            store,
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Run forever. Spawn this with `tokio::spawn`.
    pub async fn run(self) {
        let mut tick = interval(RECONCILE_TICK);
        // First tick fires immediately; subsequent fire on cadence.
        loop {
            tick.tick().await;
            if let Err(e) = self.reconcile().await {
                tracing::warn!(error = ?e, "watchdog reconcile failed");
            }
        }
    }

    async fn reconcile(&self) -> Result<(), WatchdogError> {
        let running = self.store.list_sessions(Some(Status::Running)).await?;
        let running_ids: std::collections::HashSet<Uuid> = running.iter().map(|s| s.id).collect();

        let mut tasks = self.tasks.write().await;

        // Drop tracker for sessions that are no longer running.
        let mut to_remove = Vec::new();
        for (id, handle) in tasks.iter() {
            if !running_ids.contains(id) || handle.is_finished() {
                handle.abort();
                to_remove.push(*id);
            }
        }
        for id in to_remove {
            tasks.remove(&id);
            tracing::debug!(%id, "watchdog: dropped finished/non-running task");
        }

        // Spawn watch tasks for sessions we don't already track.
        for sess in running {
            let id = sess.id;
            tasks.entry(id).or_insert_with(|| {
                tracing::info!(name = %sess.name, %id, "watchdog: starting watch task");
                let bus = self.bus.clone();
                let store = self.store.clone();
                tokio::spawn(watch_session(sess, bus, store))
            });
        }

        Ok(())
    }
}

/// One session's watch loop. Returns when the pane is gone or a crash
/// signature is hit (which marks the session crashed and emits an event).
async fn watch_session(sess: Session, bus: broadcast::Sender<Event>, store: Arc<Store>) {
    let target = sess
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&sess.name));

    let adapter = agentum_executor::adapter_for(&sess.tool);
    let compact_cmd = adapter.compact_trigger();
    let crash_sigs = adapter.crash_signatures();

    let context_low = match Regex::new(r"Context low.*<\s*50%") {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "context-low regex compile failed");
            return;
        }
    };

    let _ = emit(
        &bus,
        &store,
        Event::new("session.started").with_session(sess.id, &sess.name),
    )
    .await;

    let mut last_compact: Option<Instant> = None;
    let mut tick = interval(TICK);
    // Drop the immediate first tick so we don't fire before the pane is alive.
    tick.tick().await;

    loop {
        tick.tick().await;

        match agentum_tmux::has_session(&target).await {
            Ok(true) => {}
            Ok(false) => {
                let _ = store
                    .update_status_and_target(sess.id, Status::Crashed, None)
                    .await;
                let ev = Event::new("session.crashed")
                    .with_session(sess.id, &sess.name)
                    .with_payload(serde_json::json!({"reason": "pane_exited"}));
                let _ = emit(&bus, &store, ev).await;
                return;
            }
            Err(e) => {
                tracing::warn!(target = %target, error = ?e, "has_session check failed");
                continue;
            }
        }

        let pane = match agentum_tmux::capture_pane(&target, 100).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(target = %target, error = ?e, "capture_pane failed");
                continue;
            }
        };

        // Crash signatures first — exiting wins over compacting.
        if let Some(sig) = crash_sigs.iter().find(|s| pane.contains(*s)) {
            tracing::warn!(name = %sess.name, signature = sig, "crash signature matched");
            let _ = store
                .update_status_and_target(sess.id, Status::Crashed, None)
                .await;
            let ev = Event::new("session.crashed")
                .with_session(sess.id, &sess.name)
                .with_payload(serde_json::json!({"signature": sig}));
            let _ = emit(&bus, &store, ev).await;
            return;
        }

        // Context-low → /compact (cooldown 5 min)
        if let Some(cmd) = compact_cmd {
            if context_low.is_match(&pane) {
                let now = Instant::now();
                let due = last_compact
                    .map(|t| now.duration_since(t) >= COMPACT_COOLDOWN)
                    .unwrap_or(true);
                if due {
                    last_compact = Some(now);
                    if let Err(e) = agentum_tmux::send_keys(&target, cmd, true).await {
                        tracing::warn!(error = ?e, "watchdog: send_keys /compact failed");
                    }
                    let ev = Event::new("watchdog.compact")
                        .with_session(sess.id, &sess.name)
                        .with_payload(serde_json::json!({
                            "trigger": "context_low",
                            "command": cmd,
                        }));
                    let _ = emit(&bus, &store, ev).await;
                }
            }
        }
    }
}

/// Broadcast + persist. Failures on either are logged but don't break the loop.
async fn emit(bus: &broadcast::Sender<Event>, store: &Store, ev: Event) -> Result<(), ()> {
    if let Err(e) = store.insert_event(&ev).await {
        tracing::warn!(error = ?e, "could not persist event");
    }
    // Send returns Err only if there are zero subscribers — that's fine, the
    // event is still in the persisted log.
    let _ = bus.send(ev);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_low_regex() {
        // Regex: must have `<` followed by `50%` (with optional ws).
        let re = Regex::new(r"Context low.*<\s*50%").unwrap();
        assert!(re.is_match("Context low: <50%"));
        assert!(re.is_match("Context low: < 50%"));
        assert!(re.is_match("WARNING — Context low: about <50% remaining"));
        assert!(!re.is_match("Context low: 45%")); // no `<` → doesn't fire
        assert!(!re.is_match("context is fine"));
        assert!(!re.is_match("Context low: 80%"));
    }

    #[test]
    fn cooldown_window() {
        // 5 minute window
        assert_eq!(COMPACT_COOLDOWN, Duration::from_secs(300));
    }
}
