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

/// How often each session's pane is sampled for activity / crash
/// signatures. Was 5 s; halved to 1 s so the sidebar dot follows the
/// agent's Working ↔ Idle ↔ AwaitingInput transitions on perceived-
/// instant latency rather than after a full breath. tmux
/// `capture-pane` is a few ms per call — 5× more invocations is still
/// negligible against the value of a snappy "is my agent done yet"
/// indicator.
const TICK: Duration = Duration::from_secs(1);
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
    let busy_sig = adapter.busy_signature();
    let awaiting_sigs = adapter.awaiting_input_signatures();

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
    // Per-session activity state, derived purely from pane substrings.
    // We start in `Unknown` so the first observation never fires a
    // notification — the user already knows the agent is wherever it
    // is when they spawn agentum or the watchdog restarts. Transitions
    // we care about:
    //   Working → Idle             → emit `agent.finished`
    //   (Working|Idle) → Awaiting  → emit `agent.awaiting_input`
    let mut activity = ActivityState::Unknown;
    // Working → Idle commits on the first Idle observation. The
    // pre-v0.7.50 debounce (require two consecutive Idle ticks) was
    // patching a flicker caused by `capture-pane -p -S -100` matching
    // stale "esc to interrupt" text in scrollback — when the user
    // backspaced in the queued-input line the pane snapshot briefly
    // dropped the footer for one tick. v0.7.50 fixed the upstream
    // by switching `classify_activity` to a viewport-only capture
    // (`capture_pane_visible`), and the flicker doesn't reproduce
    // against that. Keeping the debounce after the upstream fix made
    // the watchdog miss `agent.finished` whenever the user replied
    // within ~2 s of a turn ending (faster than two ticks), so the
    // sidebar dot stayed green and no toast fired. The debounce is
    // gone; the watchdog now reacts on the first idle tick.
    // Track the tool we last persisted so we don't spam UPDATE for every
    // tick. Seeded from the session record. The candidate slot debounces
    // brief shell-outs (git, ls) so they don't get latched as the
    // foreground tool — we require two consecutive observations of the
    // same recognised adapter before committing.
    let mut current_tool = sess.tool.clone();
    let mut tool_candidate: Option<String> = None;
    let mut tick = interval(TICK);
    // Drop the immediate first tick so we don't fire before the pane is alive.
    tick.tick().await;

    loop {
        tick.tick().await;

        match agentum_tmux::has_session(&target).await {
            Ok(true) => {}
            Ok(false) => {
                // Pane is gone. Distinguish "user killed it" from "it
                // crashed": if the DB already reflects Stopped (set by the
                // /stop or /kill API route), the disappearance was
                // intentional — exit silently rather than emit a misleading
                // `session.crashed` toast and overwrite the status.
                if intentionally_stopped(&store, sess.id).await {
                    return;
                }
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

        // Two captures per tick:
        //   `pane`      — 100 lines incl. scrollback; for crash + context-low
        //                 matches that can scroll slightly off-screen and
        //                 still need to fire.
        //   `viewport`  — currently-visible cells only; for activity
        //                 classification, where stale "esc to interrupt"
        //                 text in scrollback would otherwise pin the
        //                 session as Working forever after a turn ended.
        //                 That was the v0.7.47-and-earlier bug where the
        //                 sidebar dot stayed green long after Claude
        //                 finished — the scrollback retained the spinner
        //                 footer from the prior turn, so `pane.contains
        //                 ("esc to interrupt")` kept matching.
        let pane = match agentum_tmux::capture_pane(&target, 100).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(target = %target, error = ?e, "capture_pane failed");
                continue;
            }
        };
        let viewport = match agentum_tmux::capture_pane_visible(&target).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(target = %target, error = ?e, "capture_pane_visible failed");
                continue;
            }
        };

        // Crash signatures first — exiting wins over compacting.
        if let Some(sig) = crash_sigs.iter().find(|s| pane.contains(*s)) {
            // Same intentional-stop guard as the pane_exited branch: a
            // crash signature seen during a /stop or /kill flow is just
            // residue from the dying process, not a real crash.
            if intentionally_stopped(&store, sess.id).await {
                return;
            }
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

        // Tool drift → `session.tool_changed`. Cheap: one extra
        // `tmux display-message` per tick. We map the foreground command
        // to a known adapter id and only commit on the second
        // consecutive observation of the same NEW value, so a brief
        // shell-out (git, ls, …) doesn't get latched as the active tool.
        // The `tool_candidate` slot is reset whenever the observation
        // doesn't match it.
        if let Ok(cmd) = agentum_tmux::pane_current_command(&target).await
            && let Some(detected) = canonical_tool_from_command(&cmd)
            && detected != current_tool
        {
            if tool_candidate.as_deref() == Some(detected) {
                // Second observation in a row — commit.
                match store.patch_session_tool(sess.id, detected).await {
                    Ok(updated) => {
                        let prev = std::mem::replace(&mut current_tool, detected.to_string());
                        tool_candidate = None;
                        let ev = Event::new("session.tool_changed")
                            .with_session(updated.id, &updated.name)
                            .with_payload(serde_json::json!({
                                "tool": updated.tool,
                                "previous_tool": prev,
                            }));
                        let _ = emit(&bus, &store, ev).await;
                    }
                    Err(e) => {
                        tracing::warn!(
                            session = %sess.name,
                            tool = %detected,
                            error = ?e,
                            "watchdog: patch_session_tool failed"
                        );
                    }
                }
            } else {
                tool_candidate = Some(detected.to_string());
            }
        } else {
            // Either we couldn't query tmux, the foreground process
            // isn't a recognised adapter, or it matches the stored
            // tool — clear any pending candidate so we don't latch on
            // a one-off mismatch.
            tool_candidate = None;
        }

        // Activity-state transitions → agent.finished / agent.awaiting_input
        // / agent.input_resolved. Only fires for adapters that declared a
        // busy_signature or any awaiting_input_signatures — others stay in
        // `Unknown` forever and never emit.
        //
        // Unknown→Idle / Unknown→AwaitingInput fire with `initial: true`
        // in the payload so clients can update the sidebar dot (idle /
        // attention) without firing a toast — a user who restarts the
        // daemon (or reconnects the dashboard) onto an already-finished
        // session needs the visual state to match reality, but doesn't
        // want a flurry of "X finished" toasts for events that already
        // happened before they tuned in.
        let next = classify_activity(&viewport, busy_sig, awaiting_sigs);
        if next != activity {
            match (activity, next) {
                (ActivityState::Working, ActivityState::Idle) => {
                    let ev = Event::new("agent.finished").with_session(sess.id, &sess.name);
                    let _ = emit(&bus, &store, ev).await;
                }
                // First observation lands the session as idle. Silent
                // toast (clients gate on `initial`) but the dot still
                // updates because the event itself fires.
                (ActivityState::Unknown, ActivityState::Idle) => {
                    let ev = Event::new("agent.finished")
                        .with_session(sess.id, &sess.name)
                        .with_payload(serde_json::json!({"initial": true}));
                    let _ = emit(&bus, &store, ev).await;
                }
                // First observation lands the session as already
                // blocked on a prompt — flip the attention dot without
                // a "needs input" toast (the agent has been waiting
                // since before the user tuned in; toasting now would
                // be misleadingly stale).
                (ActivityState::Unknown, ActivityState::AwaitingInput) => {
                    let ev = Event::new("agent.awaiting_input")
                        .with_session(sess.id, &sess.name)
                        .with_payload(serde_json::json!({"initial": true}));
                    let _ = emit(&bus, &store, ev).await;
                }
                // Idle → Working: the agent picked up a new turn after
                // sitting at the prompt. Without this event the TUI keeps
                // the session pinned in its idle set and the sidebar dot
                // stays grey while the agent is visibly working. No
                // toast: a quietly-resumed agent isn't notification-
                // worthy on its own.
                (ActivityState::Idle, ActivityState::Working) => {
                    let ev = Event::new("agent.working").with_session(sess.id, &sess.name);
                    let _ = emit(&bus, &store, ev).await;
                }
                (prev, ActivityState::AwaitingInput)
                    if prev != ActivityState::AwaitingInput && prev != ActivityState::Unknown =>
                {
                    let ev = Event::new("agent.awaiting_input").with_session(sess.id, &sess.name);
                    let _ = emit(&bus, &store, ev).await;
                }
                // Leaving AwaitingInput → user has answered (or the prompt
                // was dismissed). Lets clients clear any "needs input" UI
                // (yellow status dot, attention badges) without waiting for
                // a separate finished/working event. Payload carries the
                // resolved state so a single event distinguishes "answered
                // and back to working" from "prompt dismissed, now idle" —
                // the TUI uses this to swap the dot between active green
                // and muted "sleeping" without having to wait for a
                // follow-up `agent.finished`.
                (ActivityState::AwaitingInput, next_state)
                    if next_state != ActivityState::AwaitingInput
                        && next_state != ActivityState::Unknown =>
                {
                    let resolved = match next_state {
                        ActivityState::Working => "working",
                        ActivityState::Idle => "idle",
                        // Unreachable per the guard above, but keeps the
                        // match exhaustive without a panic if the variant
                        // set ever grows.
                        _ => "unknown",
                    };
                    let ev = Event::new("agent.input_resolved")
                        .with_session(sess.id, &sess.name)
                        .with_payload(serde_json::json!({"state": resolved}));
                    let _ = emit(&bus, &store, ev).await;
                }
                _ => {}
            }
            activity = next;
        }
    }
}

/// `true` if the session's current persisted status is `Stopped`, meaning
/// the API `/stop` or `/kill` route already retired it. Used to suppress
/// the `session.crashed` event the watchdog would otherwise fire on the
/// next tick when it sees the pane gone — that disappearance is the user's
/// own doing, not an actual crash.
async fn intentionally_stopped(store: &Store, id: Uuid) -> bool {
    matches!(
        store.get_session_by_id(id).await,
        Ok(Some(s)) if s.status == Status::Stopped
    )
}

/// Map a tmux `pane_current_command` value to the adapter id we store
/// on the session. Returns `None` for shells / unknown binaries so
/// briefly running `git`, `ls`, `bash` etc. doesn't trigger a tool
/// flip — only first-class adapter binaries swap the chip.
///
/// Conservative on purpose: matches both the bare binary names and
/// common npm/yarn wrapper variants (`claude-code`, `codex-cli`).
/// Add aliases here as we discover them in the wild.
fn canonical_tool_from_command(cmd: &str) -> Option<&'static str> {
    let c = cmd.trim();
    match c {
        "claude" | "claude-code" => Some("claude"),
        "codex" | "codex-cli" => Some("codex"),
        "gemini" | "gemini-cli" => Some("gemini"),
        "hermes" | "hermes-cli" => Some("hermes"),
        _ => None,
    }
}

/// Pane-derived activity state. `Unknown` is the seed value before any
/// observation; transitions out of `Unknown` never emit so the watchdog
/// stays quiet on startup / reconnect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActivityState {
    Unknown,
    Working,
    Idle,
    AwaitingInput,
}

/// Classify a pane snapshot into an [`ActivityState`]. Permission-prompt
/// signatures take precedence over the busy/idle distinction — Claude
/// keeps "esc to interrupt" on screen while a permission box is open,
/// and the user-facing important fact is "you need to answer this".
fn classify_activity(pane: &str, busy_sig: Option<&str>, awaiting_sigs: &[&str]) -> ActivityState {
    if !awaiting_sigs.is_empty() && awaiting_sigs.iter().any(|s| pane.contains(s)) {
        return ActivityState::AwaitingInput;
    }
    match busy_sig {
        Some(s) if pane.contains(s) => ActivityState::Working,
        Some(_) => ActivityState::Idle,
        // Adapter opted out — keep Unknown so we never emit spurious
        // finished/awaiting events for tools without stable markers.
        None => ActivityState::Unknown,
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

    #[test]
    fn classify_activity_states() {
        let busy = Some("esc to interrupt");
        let awaiting = ["Do you want to proceed?", "❯ 1. Yes"];

        assert_eq!(
            classify_activity("...working hard (esc to interrupt)", busy, &awaiting),
            ActivityState::Working,
        );
        assert_eq!(
            classify_activity("> _\n", busy, &awaiting),
            ActivityState::Idle,
        );
        // Permission prompt outranks busy spinner — Claude leaves the
        // spinner up while the prompt is open.
        assert_eq!(
            classify_activity(
                "... esc to interrupt ...\nDo you want to proceed?",
                busy,
                &awaiting
            ),
            ActivityState::AwaitingInput,
        );
        // Adapter without a busy_signature stays in Unknown — no
        // spurious finished/awaiting toasts for tools we don't know.
        assert_eq!(
            classify_activity("anything goes here", None, &[]),
            ActivityState::Unknown,
        );
    }

#[test]
    fn classify_activity_multichoice_menu() {
        // Regression: Claude Code multi-choice menus (plan mode,
        // subagent picks) show `Enter to select · ↑/↓ to navigate`
        // instead of a "Do you want to proceed?" yes/no. The
        // first option starts with arbitrary text, not "Yes", so the
        // legacy signatures miss it and the watchdog stayed in Idle
        // — no notification, no yellow attention dot.
        let busy = Some("esc to interrupt");
        let awaiting = [
            "Do you want to proceed?",
            "❯ 1. Yes",
            "Enter to select",
            "↑/↓ to navigate",
        ];
        let menu = "❯ 1. Re-apply both files\n  2. CSP-only fix\n\nEnter to select · ↑/↓ to navigate · Esc to cancel";
        assert_eq!(
            classify_activity(menu, busy, &awaiting),
            ActivityState::AwaitingInput,
        );
    }
}
