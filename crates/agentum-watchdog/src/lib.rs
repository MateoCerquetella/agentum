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
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use agentum_core::{Event, LOCAL_HOST_ID, Session, Status};
use agentum_store::Store;
use regex::Regex;
use tokio::sync::{RwLock, broadcast};
use tokio::task::JoinHandle;
use tokio::time::interval;
use uuid::Uuid;

// Two independent background workers built on the same event bus, split into
// their own modules; each reaches back for the crate's shared `Store`/`Event`/
// `emit`/error types via `use super::*`. Their `run_*` entry points are the
// crate's public API (the server's `spawn_background_workers` spawns them).
mod comment_bridge;
mod reconciler;
pub use comment_bridge::run_session_comment_bridge;
pub use reconciler::run_goal_reconciler;

/// The "context low" compact trigger, compiled once for the whole process.
/// Every session-watch task consulted this pattern; compiling it per spawn
/// (once per running session, re-paid on each reconcile that re-spawns a
/// watch task) was needless — the pattern is a fixed valid literal. Mirrors
/// the `LazyLock<Regex>` caches in `agentum-server`'s `usage.rs`.
static CONTEXT_LOW_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Context low.*<\s*50%").expect("context-low regex is valid"));

/// How often each session's pane is sampled for activity / crash
/// signatures. Was 5 s; halved to 1 s so the sidebar dot follows the
/// agent's Working ↔ Idle ↔ AwaitingInput transitions on perceived-
/// instant latency rather than after a full breath. tmux
/// `capture-pane` is a few ms per call — 5× more invocations is still
/// negligible against the value of a snappy "is my agent done yet"
/// indicator.
const TICK: Duration = Duration::from_secs(1);

/// Pane-sample cadence for REMOTE (SSH) sessions. Each tick is an `ssh` exec on
/// the host's ControlMaster; at the local 1 s cadence, N open remote sessions
/// fired N `capture-pane`s/sec at one remote tmux server and N channel opens at
/// the SSH master — which throttled both pane output (~B/s) and keystroke
/// delivery. 3 s cuts that load ~3× while keeping the agent-status dot's lag
/// imperceptible. Local sampling is a cheap process spawn with no channel
/// contention, so it stays at [`TICK`].
const REMOTE_TICK: Duration = Duration::from_secs(3);

const COMPACT_COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// How often to pane-sample a session's host: slower for SSH (each tick is a
/// remote `ssh` exec sharing the host's ControlMaster + tmux server) than local.
fn sample_tick(kind: &agentum_core::HostKind) -> Duration {
    match kind {
        agentum_core::HostKind::Ssh { .. } => REMOTE_TICK,
        agentum_core::HostKind::Local => TICK,
    }
}

/// How long a pane must stay visually quiet (footer hash unchanged) while settled
/// out of the Working state before its sample cadence backs off. Under many
/// agents most panes sit idle, and the dominant per-pane cost is the tmux sample
/// spawn, so halving it for long-quiet panes cuts idle-fleet OS load. Kept well
/// above [`IDLE_AFTER_QUIET`] so a pane is confidently idle before we slow down.
const SAMPLE_BACKOFF_AFTER: Duration = Duration::from_secs(10);

/// The delay before the NEXT pane sample. A Working pane, or one whose footer
/// changed within the last [`SAMPLE_BACKOFF_AFTER`], samples at the base cadence
/// ([`sample_tick`]); a pane that has settled non-Working and stayed quiet longer
/// samples half as often. A resumed agent is still caught within one slow tick,
/// and the live sidebar signal comes from agent hooks / pane byte-flow (not this
/// poll), so the extra latency is invisible in practice while crash detection
/// stays well within a couple of seconds.
fn next_sample_delay(
    kind: &agentum_core::HostKind,
    activity: ActivityState,
    pane_quiet_for: Duration,
) -> Duration {
    let base = sample_tick(kind);
    if activity == ActivityState::Working || pane_quiet_for < SAMPLE_BACKOFF_AFTER {
        base
    } else {
        base * 2
    }
}

/// For agents that don't declare a `busy_signature` (codex, cursor, gemini,
/// hermes — anything other than Claude), we fall back to change-based
/// detection: if the visible footer hasn't changed for this long, the
/// agent is treated as Idle. The pre-fix path classified them as
/// `Unknown` forever and the sidebar dot stayed green with zero
/// `agent.finished` events ever firing.
const IDLE_AFTER_QUIET: Duration = Duration::from_secs(3);

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

        // Spawn watch tasks for sessions we don't already track. Remote
        // (SSH) sessions are no longer skipped: pane sampling is host-aware
        // now (`agentum_tmux::ssh::*` branches on the host kind), so
        // hookless agents (opencode, codex, …) on an SSH host get the same
        // Working/Idle detection as local ones.
        for sess in running {
            let id = sess.id;
            if tasks.contains_key(&id) {
                continue;
            }
            // Resolve the session's host once, up front. The local host row is
            // seeded by migration 0018, so this is almost always `Some`; fall
            // back to a synthesized `Local` host if it's somehow absent (or a
            // remote host row was deleted out from under a running session) so
            // the task still samples *something* rather than silently dropping.
            let host_id = sess.host_id.unwrap_or(LOCAL_HOST_ID);
            let host = match self.store.get_host(host_id).await {
                Ok(Some(h)) => h,
                Ok(None) => {
                    tracing::warn!(
                        name = %sess.name,
                        %id,
                        %host_id,
                        "watchdog: host row missing; falling back to local pane sampling"
                    );
                    local_host_fallback(host_id)
                }
                Err(e) => {
                    tracing::warn!(
                        name = %sess.name,
                        %id,
                        %host_id,
                        error = ?e,
                        "watchdog: get_host failed; skipping watch task this tick"
                    );
                    continue;
                }
            };
            tracing::info!(name = %sess.name, %id, "watchdog: starting watch task");
            let bus = self.bus.clone();
            let store = self.store.clone();
            tasks.insert(id, tokio::spawn(watch_session(sess, host, bus, store)));
        }

        Ok(())
    }
}

/// Synthesize a `Local` host when the store has no row for `id`. Defensive:
/// the local host is seeded by migration 0018, but a deleted remote-host row
/// (or a fresh DB mid-migration) shouldn't leave a running session unwatched.
/// `created_at`/`updated_at` are placeholders — the watchdog only reads
/// `kind` to pick the local-vs-SSH branch.
fn local_host_fallback(id: Uuid) -> agentum_core::Host {
    agentum_core::Host {
        id,
        name: "local".to_string(),
        kind: agentum_core::HostKind::Local,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
        last_seen_at: None,
    }
}

/// One session's watch loop. Returns when the pane is gone or a crash
/// signature is hit (which marks the session crashed and emits an event).
/// Pane sampling is host-aware: `host` is `Local` (tmux run directly) or
/// `Ssh` (tmux run over the session's ssh connection) — see
/// `agentum_tmux::ssh`.
async fn watch_session(
    sess: Session,
    host: agentum_core::Host,
    bus: broadcast::Sender<Event>,
    store: Arc<Store>,
) {
    let target = sess
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&sess.name));

    let adapter = agentum_executor::adapter_for(&sess.tool);
    let compact_cmd = adapter.compact_trigger();
    let crash_sigs = adapter.crash_signatures();
    let busy_sig = adapter.busy_signature();
    let awaiting_sigs = adapter.awaiting_input_signatures();
    let is_agent = adapter.is_agent();

    // Compiled once process-wide (see CONTEXT_LOW_RE), not per session spawn.
    let context_low = &*CONTEXT_LOW_RE;

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
    // Change-based idle detection for adapters without a `busy_signature`.
    // We hash the bottom-20 of the visible viewport and remember when it
    // last changed; classify_activity treats "footer hasn't changed for
    // IDLE_AFTER_QUIET" as Idle for `is_agent` adapters. Claude is
    // unaffected — its busy_signature still drives classification.
    let mut last_bottom_hash: Option<u64> = None;
    let mut last_change_at = Instant::now();
    // Slower sample cadence on SSH hosts: the per-tick `sample_pane` is a remote
    // `ssh` exec, so 1 s × N sessions flooded the host (see REMOTE_TICK).
    // Adaptive sample cadence: the base rate ([`sample_tick`]) while a pane is
    // active, backing off once it has settled quiet (see [`next_sample_delay`]).
    // Replaces the fixed `interval` so an idle fleet stops paying the base-rate
    // sample spawn on every pane. The initial `base` delay stands in for the old
    // drop-first-immediate-tick — don't sample before the pane is alive.
    let mut next_delay = sample_tick(&host.kind);

    loop {
        tokio::time::sleep(next_delay).await;

        // One sample per tick: existence + both captures + foreground command
        // in a single round trip (on SSH hosts, one exec instead of four —
        // the per-call channel churn on the shared ControlMaster was the
        // dominant remote load and competed with interactive keystrokes).
        //
        // Two captures:
        //   `pane`      — 100 lines incl. scrollback; for crash + context-low
        //                 matches that can scroll slightly off-screen and
        //                 still need to fire.
        //   `viewport`  — currently-visible cells only; for activity
        //                 classification, where stale "esc to interrupt"
        //                 text in scrollback would otherwise pin the
        //                 session as Working forever after a turn ended.
        let sample = match agentum_tmux::ssh::sample_pane(&host, &target, 100).await {
            Ok(Some(s)) => s,
            Ok(None) => {
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
                tracing::warn!(target = %target, error = ?e, "pane sample failed");
                continue;
            }
        };
        let pane = sample.pane;
        let viewport = sample.viewport;

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
                    if let Err(e) = agentum_tmux::ssh::send_keys(&host, &target, cmd, true).await {
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

        // Tool drift → `session.tool_changed`. The foreground command rides
        // the combined sample (no extra round trip). We map it to a known
        // adapter id and only commit on the second consecutive observation
        // of the same NEW value, so a brief shell-out (git, ls, …) doesn't
        // get latched as the active tool. The `tool_candidate` slot is reset
        // whenever the observation doesn't match it.
        if let Some(detected) = canonical_tool_from_command(&sample.current_command)
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
        // Classify against the BOTTOM of the visible viewport only.
        // Claude's spinner footer and prompt UI are always anchored
        // to the bottom of the pane; chat output scrolls up. Matching
        // the whole viewport meant generic chat content (an answer
        // that quotes "esc to interrupt" or "Enter to select · ↑/↓
        // to navigate" in code/comments) faked a Working or
        // AwaitingInput state. Trim to the last ~20 lines, which
        // comfortably covers Claude's multi-line menu plus the
        // spinner/input footer without picking up scrolled-by chat.
        let bottom = bottom_lines(&viewport, 20);
        // Hash the footer; update last_change_at whenever it shifts.
        // Used by classify_activity for the no-busy-signature fallback.
        let h = hash_str(bottom);
        if last_bottom_hash != Some(h) {
            last_bottom_hash = Some(h);
            last_change_at = Instant::now();
        }
        let pane_quiet_for = last_change_at.elapsed();
        let next = classify_activity(bottom, busy_sig, awaiting_sigs, is_agent, pane_quiet_for);
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
                // First observation lands the session as already
                // working. Without this emit the session-snapshot
                // replay served to new WS clients falls back to
                // the most recent agent.* row in the events log —
                // which could be a stale `agent.awaiting_input` /
                // `agent.finished` from a previous daemon — and
                // the dashboard's dot shows the wrong colour until
                // the next transition. `initial: true` keeps the
                // toast suppressed.
                (ActivityState::Unknown, ActivityState::Working) => {
                    let ev = Event::new("agent.working")
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

        // Decide how long to wait before the next sample from the state we just
        // observed. `activity` reflects the current sample (it equals `next`
        // whether or not the transition block above ran); `pane_quiet_for` is how
        // long the footer has been unchanged. A settled, long-quiet pane samples
        // half as often — the idle-fleet OS-load win.
        next_delay = next_sample_delay(&host.kind, activity, pane_quiet_for);
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

/// Return the last `n` newline-delimited lines of `text` as a single
/// borrowed slice. Used by `watch_session` to scope the busy/awaiting
/// classifier to the bottom of Claude's pane, so chat content that
/// happens to quote the signature strings can't fake a state.
fn bottom_lines(text: &str, n: usize) -> &str {
    if n == 0 {
        return "";
    }
    let mut count = 0;
    // Walk back from the end looking for the n-th newline before EOF.
    // bytes() is sufficient — we only key on `\n` and return a
    // byte-offset slice, which is always valid UTF-8 since `\n` is a
    // single-byte ASCII boundary.
    for (i, b) in text.bytes().enumerate().rev() {
        if b == b'\n' {
            count += 1;
            if count == n {
                return &text[i + 1..];
            }
        }
    }
    // Fewer than n lines total — return the whole thing.
    text
}

/// Classify a pane snapshot into an [`ActivityState`]. Permission-prompt
/// signatures take precedence over the busy/idle distinction — Claude
/// keeps "esc to interrupt" on screen while a permission box is open,
/// and the user-facing important fact is "you need to answer this".
///
/// When `busy_sig` is `None` but the adapter is an interactive agent
/// (`is_agent == true`), fall back to change-based detection: the
/// footer is treated as Idle once it has been quiet for
/// `IDLE_AFTER_QUIET`. Without this, codex/cursor/gemini/hermes (all
/// of which have no stable spinner marker) stayed pinned at `Unknown`
/// forever — the sidebar dot never flipped off green and no
/// `agent.finished` ever emitted, so no toast/chime ever fired.
fn classify_activity(
    pane: &str,
    busy_sig: Option<&str>,
    awaiting_sigs: &[&str],
    is_agent: bool,
    pane_quiet_for: Duration,
) -> ActivityState {
    if !awaiting_sigs.is_empty() && awaiting_sigs.iter().any(|s| pane.contains(s)) {
        return ActivityState::AwaitingInput;
    }
    match busy_sig {
        Some(s) if pane.contains(s) => ActivityState::Working,
        Some(_) => ActivityState::Idle,
        None if is_agent => {
            if pane_quiet_for >= IDLE_AFTER_QUIET {
                ActivityState::Idle
            } else {
                ActivityState::Working
            }
        }
        // Shells and unknown passthroughs deliberately stay Unknown:
        // an idle bash prompt isn't an "agent finished its turn" event.
        None => ActivityState::Unknown,
    }
}

/// Cheap stable hash of a `&str`. Used by `watch_session` to detect
/// when the visible footer last changed (the input for change-based
/// idle detection). Wrapper around `std::hash::DefaultHasher` so the
/// callsite reads as a single expression.
fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
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
    // The status-rank helpers moved to `reconciler` with their concern; the
    // reconciler/bridge tests below drive the public `run_*` workers (re-exported
    // above) but two tests exercise these pure helpers directly.
    use super::reconciler::{rank_to_status, status_rank};

    #[test]
    fn remote_sessions_sample_slower_than_local() {
        use agentum_core::{HostKind, SshAuth};
        let ssh = HostKind::Ssh {
            user: "u".into(),
            hostname: "h".into(),
            port: 22,
            auth: SshAuth::Agent,
        };
        // Local stays tight; SSH backs off so N sessions don't flood the host.
        assert_eq!(sample_tick(&HostKind::Local), TICK);
        assert_eq!(sample_tick(&ssh), REMOTE_TICK);
        assert!(sample_tick(&ssh) > sample_tick(&HostKind::Local));
    }

    #[test]
    fn idle_quiet_panes_sample_less_often() {
        use agentum_core::HostKind;
        let recent = Duration::from_secs(1);
        let long_quiet = SAMPLE_BACKOFF_AFTER + Duration::from_secs(1);

        // Working always samples at the base cadence, however long it's been quiet.
        assert_eq!(
            next_sample_delay(&HostKind::Local, ActivityState::Working, long_quiet),
            TICK
        );
        // A settled idle/awaiting pane that only just went quiet stays at base —
        // we don't slow down until we're confident it's idle.
        assert_eq!(
            next_sample_delay(&HostKind::Local, ActivityState::Idle, recent),
            TICK
        );
        // Long-quiet idle / awaiting-input panes back off to half the base rate.
        assert_eq!(
            next_sample_delay(&HostKind::Local, ActivityState::Idle, long_quiet),
            TICK * 2
        );
        assert_eq!(
            next_sample_delay(&HostKind::Local, ActivityState::AwaitingInput, long_quiet),
            TICK * 2
        );
        // Backoff is relative to each host's base cadence.
        assert_eq!(
            next_sample_delay(&HostKind::Local, ActivityState::Unknown, long_quiet),
            TICK * 2
        );
    }

    #[test]
    fn status_rank_orders_todo_doing_done() {
        assert_eq!(status_rank("todo"), 0);
        assert_eq!(status_rank("doing"), 1);
        // `review` ranks with `doing` so a goal stays in-progress (not done)
        // while a child awaits verification.
        assert_eq!(status_rank("review"), 1);
        assert_eq!(status_rank("done"), 2);
        assert_eq!(status_rank("unknown_future_status"), -1);
        assert_eq!(status_rank(""), -1);
    }

    #[test]
    fn rank_to_status_round_trip() {
        assert_eq!(rank_to_status(0), Some("todo"));
        assert_eq!(rank_to_status(1), Some("doing"));
        assert_eq!(rank_to_status(2), Some("done"));
        assert_eq!(rank_to_status(-1), None);
        assert_eq!(rank_to_status(3), None);
        assert_eq!(rank_to_status(99), None);
    }

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
        // For busy_sig-driven adapters (Claude), is_agent and
        // pane_quiet_for don't affect the answer. Pass zero/false.
        let quiet = Duration::ZERO;

        assert_eq!(
            classify_activity(
                "...working hard (esc to interrupt)",
                busy,
                &awaiting,
                true,
                quiet,
            ),
            ActivityState::Working,
        );
        assert_eq!(
            classify_activity("> _\n", busy, &awaiting, true, quiet),
            ActivityState::Idle,
        );
        // Permission prompt outranks busy spinner — Claude leaves the
        // spinner up while the prompt is open.
        assert_eq!(
            classify_activity(
                "... esc to interrupt ...\nDo you want to proceed?",
                busy,
                &awaiting,
                true,
                quiet,
            ),
            ActivityState::AwaitingInput,
        );
        // Shell / passthrough (is_agent=false, no busy_sig) stays
        // Unknown — an idle bash prompt isn't a "finished" event.
        assert_eq!(
            classify_activity("anything goes here", None, &[], false, quiet),
            ActivityState::Unknown,
        );
    }

    #[test]
    fn classify_activity_change_based_idle_for_agents_without_busy_sig() {
        // Regression: codex/cursor/gemini/hermes have no `busy_signature`.
        // Before this fix `classify_activity` returned `Unknown` for them
        // forever, so the watchdog never emitted `agent.finished`, the
        // sidebar dot stayed green, and no toast/chime ever fired.
        // Now: with `is_agent=true` and no busy_sig, a footer that has
        // been quiet for >= IDLE_AFTER_QUIET classifies as Idle; an
        // actively-changing footer stays Working.
        let busy = None;
        let awaiting: [&str; 0] = [];
        let just_changed = Duration::from_millis(100);
        let quiet_long = IDLE_AFTER_QUIET + Duration::from_millis(500);

        assert_eq!(
            classify_activity(
                "codex> generating response...",
                busy,
                &awaiting,
                true,
                just_changed,
            ),
            ActivityState::Working,
        );
        assert_eq!(
            classify_activity("codex> ", busy, &awaiting, true, quiet_long,),
            ActivityState::Idle,
        );
        // Same content + same elapsed but is_agent=false → still
        // Unknown (we don't auto-fire on shells).
        assert_eq!(
            classify_activity("$ ", busy, &awaiting, false, quiet_long),
            ActivityState::Unknown,
        );
    }

    #[test]
    fn classify_activity_hookless_passthrough_agent_via_real_adapter() {
        // Root-cause regression for the remote OpenCode "stuck on Idle"
        // bug. OpenCode is a hookless agent that routes through
        // PassthroughAdapter (it's in PASSTHROUGH_PROBED, not FIRST_CLASS),
        // so it has no busy_signature(). Before the fix PassthroughAdapter
        // inherited the default is_agent() == false, so classify_activity
        // hit the `None =>` arm and returned Unknown forever — the session
        // never transitioned to Working or Idle and the sidebar dot showed
        // "Idle" while the agent was visibly streaming output.
        //
        // This test pulls the real adapter values straight from
        // `adapter_for("opencode")` (rather than hardcoding is_agent=true)
        // so it stays coupled to the actual wiring: if someone flips
        // PassthroughAdapter back to is_agent() == false, this fails.
        let adapter = agentum_executor::adapter_for("opencode");
        let busy_sig = adapter.busy_signature();
        let awaiting_sigs = adapter.awaiting_input_signatures();
        let is_agent = adapter.is_agent();
        assert!(
            busy_sig.is_none(),
            "opencode is hookless: no busy_signature"
        );
        assert!(is_agent, "opencode must be treated as an agent");

        // Actively redrawing pane (footer just changed this tick) → Working.
        let just_changed = Duration::from_millis(100);
        assert_eq!(
            classify_activity(
                "Build · Big Pickle\ngenerating tests...",
                busy_sig,
                awaiting_sigs,
                is_agent,
                just_changed,
            ),
            ActivityState::Working,
            "an actively-changing OpenCode pane must classify as Working"
        );

        // Stable pane (footer quiet past the idle threshold) → Idle.
        let quiet_long = IDLE_AFTER_QUIET + Duration::from_millis(500);
        assert_eq!(
            classify_activity(
                "opencode ready\n> ",
                busy_sig,
                awaiting_sigs,
                is_agent,
                quiet_long,
            ),
            ActivityState::Idle,
            "a stable OpenCode pane past the quiet threshold must classify as Idle"
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
            "Enter to select · ↑/↓ to navigate",
        ];
        let menu = "❯ 1. Re-apply both files\n  2. CSP-only fix\n\nEnter to select · ↑/↓ to navigate · Esc to cancel";
        assert_eq!(
            classify_activity(menu, busy, &awaiting, true, Duration::ZERO),
            ActivityState::AwaitingInput,
        );
    }

    #[test]
    fn classify_activity_ignores_prose_mentioning_menu_phrases() {
        // Pre-v0.7.52 Claude awaiting signatures matched the bare
        // strings "Enter to select" and "↑/↓ to navigate"
        // individually, which triggered a false AwaitingInput
        // whenever a code comment, source file, or chat reply
        // mentioned either phrase. The watchdog's own pane caught
        // this — its viewport contained a doc comment quoting
        // "Enter to select · ↑/↓ to navigate" while no actual
        // prompt was open, and the dashboard dot went yellow.
        // Tightening to the structural middle-dot pair stops
        // generic prose from masquerading as a real prompt.
        let busy = Some("esc to interrupt");
        let awaiting = [
            "Do you want to proceed?",
            "Enter to select · ↑/↓ to navigate",
        ];
        // Working pane that QUOTES the menu phrases in prose,
        // not as the footer of an actual menu — spinner is still
        // up, so the right answer is Working.
        let prose = "...working hard (esc to interrupt)\n// see the docs: Enter to select and ↑/↓ to navigate";
        assert_eq!(
            classify_activity(prose, busy, &awaiting, true, Duration::ZERO),
            ActivityState::Working,
        );
    }

    #[test]
    fn bottom_lines_returns_last_n() {
        let s = "a\nb\nc\nd\ne\n";
        assert_eq!(bottom_lines(s, 2), "e\n");
        assert_eq!(bottom_lines(s, 3), "d\ne\n");
        assert_eq!(bottom_lines(s, 10), s); // fewer than n → all
        assert_eq!(bottom_lines("", 5), "");
        assert_eq!(
            bottom_lines("only one line, no newline", 3),
            "only one line, no newline"
        );
    }

    #[test]
    fn bottom_lines_scopes_classifier_to_footer() {
        // The watchdog's own viewport often contains chat output
        // that mentions the signature strings (a doc comment about
        // "Enter to select · ↑/↓ to navigate", an explanation
        // quoting "esc to interrupt"). Those quotations live in
        // the scrolled-up chat region, not the bottom-anchored
        // Claude UI footer. Trimming the classifier's input to
        // the last 20 lines pins matches to the actual prompt
        // surface and lets prose roll past freely.
        let busy = Some("esc to interrupt");
        let awaiting = [
            "Do you want to proceed?",
            "Enter to select · ↑/↓ to navigate",
        ];

        // Realistic shape: chat output near the top quotes the
        // signatures, then a long run of unrelated chat scrolls
        // past, and at the bottom sits Claude's quiet input UI
        // (no spinner, no menu). The trimmed bottom-20 view never
        // touches the signature-quoting chat, so the right answer
        // is Idle.
        let mut pane = String::new();
        pane.push_str(
            "chat about the watchdog: writing about \"esc to interrupt\" and Enter to select · ↑/↓ to navigate\n"
        );
        for i in 0..40 {
            pane.push_str(&format!("plain chat line {i}\n"));
        }
        // Quiet footer at the bottom — no signatures.
        pane.push_str("──────\n❯ \n──────\n  ⏵⏵ bypass permissions on\n");

        let bottom = bottom_lines(&pane, 20);
        assert_eq!(
            classify_activity(bottom, busy, &awaiting, true, Duration::ZERO),
            ActivityState::Idle,
            "bottom-20 of a pane whose chat quotes the signatures must classify as Idle, not Working/Awaiting"
        );

        // Same pane but with the spinner actually live at the
        // bottom — should switch to Working.
        let mut working_pane = pane.clone();
        working_pane.push_str("  ⏵⏵ bypass permissions on · esc to interrupt\n");
        let bottom = bottom_lines(&working_pane, 20);
        assert_eq!(
            classify_activity(bottom, busy, &awaiting, true, Duration::ZERO),
            ActivityState::Working,
        );
    }

    // ---- goal-status reconciler tests (plan 01-04, Task 2) ----

    async fn tmp_store_for_reconciler() -> Arc<agentum_store::Store> {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        std::mem::forget(dir);
        Arc::new(agentum_store::Store::open(&p).await.unwrap())
    }

    async fn make_goal_item(store: &agentum_store::Store) -> agentum_core::BoardItem {
        store
            .create_board_item(agentum_core::NewBoardItem {
                title: "test goal".into(),
                body: None,
                status: None,
                lbl: Some("goal".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap()
    }

    fn make_child_item<'a>(
        store: &'a agentum_store::Store,
        goal_id: i64,
        status: Option<&'a str>,
    ) -> impl std::future::Future<Output = agentum_core::BoardItem> + 'a {
        let status = status.map(|s| s.to_string());
        async move {
            store
                .create_board_item(agentum_core::NewBoardItem {
                    title: "child card".into(),
                    body: None,
                    status,
                    lbl: None,
                    tool: Some("claude".into()),
                    workdir: Some("/tmp".into()),
                    model: None,
                    session_id: None,
                    priority: None,
                    parent_goal_id: Some(goal_id),
                })
                .await
                .unwrap()
        }
    }

    /// Helper: wait up to `timeout_ms` for an event with the given `kind` on `rx`.
    /// Returns `Ok(event)` on success, `Err(())` on timeout.
    async fn try_wait_for_event(
        mut rx: tokio::sync::broadcast::Receiver<agentum_core::Event>,
        kind: &'static str,
        timeout_ms: u64,
    ) -> Result<agentum_core::Event, ()> {
        let deadline = std::time::Duration::from_millis(timeout_ms);
        tokio::time::timeout(deadline, async move {
            loop {
                match rx.recv().await {
                    Ok(ev) if ev.kind == kind => return ev,
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Channel closed; will never receive the event.
                        // Spin forever so the outer timeout fires cleanly.
                        std::future::pending::<()>().await;
                        unreachable!()
                    }
                }
            }
        })
        .await
        .map_err(|_| ())
    }

    /// Like [`try_wait_for_event`] but panics on timeout.
    async fn wait_for_event(
        rx: tokio::sync::broadcast::Receiver<agentum_core::Event>,
        kind: &'static str,
        timeout_ms: u64,
    ) -> agentum_core::Event {
        try_wait_for_event(rx, kind, timeout_ms)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for event '{kind}'"))
    }

    #[tokio::test]
    async fn reconciler_promotes_goal_when_first_child_moves_to_doing() {
        let store = tmp_store_for_reconciler().await;
        let (bus, _rx_keep_alive) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let goal = make_goal_item(&store).await;
        let child = make_child_item(&store, goal.id, None).await;
        let observer = bus.subscribe();

        tokio::spawn(run_goal_reconciler(store.clone(), bus.clone()));

        // Simulate child moving to doing via a PATCH + board.updated event.
        store
            .patch_board_item(
                child.id,
                agentum_core::BoardPatch {
                    status: Some("doing".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let _ = bus.send(agentum_core::Event::new("board.updated").with_payload(
            serde_json::json!({
                "id": child.id,
                "key": child.key,
                "status": "doing",
                "parent_goal_id": goal.id,
            }),
        ));

        let ev = wait_for_event(observer, "goal.status.changed", 500).await;
        assert_eq!(ev.payload["from"], "todo");
        assert_eq!(ev.payload["to"], "doing");
        assert_eq!(ev.payload["goal_id"], goal.id);

        // DB must reflect the new status.
        let updated_goal = store.get_board_item(goal.id).await.unwrap().unwrap();
        assert_eq!(updated_goal.status, "doing");
    }

    #[tokio::test]
    async fn reconciler_demotes_goal_when_last_doing_child_returns_to_todo() {
        let store = tmp_store_for_reconciler().await;
        let (bus, _rx_keep_alive) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let goal = make_goal_item(&store).await;
        // Child starts at doing; goal set to doing to reflect it.
        let child = make_child_item(&store, goal.id, Some("doing")).await;
        store
            .patch_board_item(
                goal.id,
                agentum_core::BoardPatch {
                    status: Some("doing".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let observer = bus.subscribe();
        tokio::spawn(run_goal_reconciler(store.clone(), bus.clone()));

        // Child moves back to todo.
        store
            .patch_board_item(
                child.id,
                agentum_core::BoardPatch {
                    status: Some("todo".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let _ = bus.send(agentum_core::Event::new("board.updated").with_payload(
            serde_json::json!({
                "id": child.id,
                "key": child.key,
                "status": "todo",
                "parent_goal_id": goal.id,
            }),
        ));

        let ev = wait_for_event(observer, "goal.status.changed", 500).await;
        assert_eq!(ev.payload["from"], "doing");
        assert_eq!(ev.payload["to"], "todo");

        let updated = store.get_board_item(goal.id).await.unwrap().unwrap();
        assert_eq!(updated.status, "todo");
    }

    #[tokio::test]
    async fn reconciler_promotes_goal_to_done_when_all_children_done() {
        let store = tmp_store_for_reconciler().await;
        let (bus, _rx_keep_alive) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let goal = make_goal_item(&store).await;
        let child1 = make_child_item(&store, goal.id, Some("done")).await;
        let child2 = make_child_item(&store, goal.id, Some("done")).await;
        // Goal currently at todo; both children already done.

        let observer = bus.subscribe();
        tokio::spawn(run_goal_reconciler(store.clone(), bus.clone()));
        // Yield so the reconciler task starts and calls bus.subscribe() before
        // we send the trigger event. Without this yield, the spawn is only
        // scheduled — the reconciler hasn't entered its receive loop yet and
        // the broadcast event would be delivered to zero reconciler receivers.
        tokio::task::yield_now().await;

        // Trigger via board.updated on child2 (both already done).
        let _ = bus.send(agentum_core::Event::new("board.updated").with_payload(
            serde_json::json!({
                "id": child2.id,
                "key": child2.key,
                "status": "done",
                "parent_goal_id": goal.id,
            }),
        ));
        // Also ensure child1 doesn't change observable.
        let _ = child1.id; // suppress unused warning

        let ev = wait_for_event(observer, "goal.status.changed", 500).await;
        assert_eq!(ev.payload["to"], "done");

        let updated = store.get_board_item(goal.id).await.unwrap().unwrap();
        assert_eq!(updated.status, "done");
    }

    #[tokio::test]
    async fn reconciler_ignores_events_without_parent_goal_id() {
        let store = tmp_store_for_reconciler().await;
        let (bus, _rx_keep_alive) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let observer = bus.subscribe();
        tokio::spawn(run_goal_reconciler(store.clone(), bus.clone()));

        // board.updated with parent_goal_id=null — must be ignored.
        let _ = bus.send(agentum_core::Event::new("board.updated").with_payload(
            serde_json::json!({
                "id": 99,
                "key": "AG-99",
                "status": "doing",
                "parent_goal_id": null,
            }),
        ));

        // Wait 200 ms; no goal.status.changed event should arrive.
        let result = try_wait_for_event(observer, "goal.status.changed", 200).await;
        assert!(
            result.is_err(),
            "should not emit goal.status.changed for orphan-less card"
        );
    }

    #[tokio::test]
    async fn reconciler_emits_planner_first_child_and_idempotent_for_repeat_creates() {
        let store = tmp_store_for_reconciler().await;
        let (bus, _rx_keep_alive) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let goal = make_goal_item(&store).await;

        // Create a planner session bound to the goal.
        let planner_sess = store
            .create_session(agentum_core::NewSession {
                name: "planner-test".into(),
                workdir: "/tmp".into(),
                tool: "claude".into(),
                model: None,
                flags: vec![],
                card_id: Some(goal.id),
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await
            .unwrap();
        // Set its status to Running so the reconciler recognises it.
        store
            .update_status_and_target(planner_sess.id, agentum_core::Status::Running, None)
            .await
            .unwrap();

        let observer = bus.subscribe();
        tokio::spawn(run_goal_reconciler(store.clone(), bus.clone()));

        // First child arrives.
        let child1 = make_child_item(&store, goal.id, None).await;
        let _ = bus.send(agentum_core::Event::new("board.created").with_payload(
            serde_json::json!({
                "id": child1.id,
                "key": child1.key,
                "title": "child 1",
                "parent_goal_id": goal.id,
            }),
        ));

        // Must emit goal.planner.first_child before or alongside goal.status.changed.
        let first_child_ev = wait_for_event(observer, "goal.planner.first_child", 500).await;
        assert_eq!(first_child_ev.payload["goal_id"], goal.id);

        // Second child arrives — must NOT trigger another goal.planner.first_child.
        let observer2 = bus.subscribe();
        let child2 = make_child_item(&store, goal.id, None).await;
        let _ = bus.send(agentum_core::Event::new("board.created").with_payload(
            serde_json::json!({
                "id": child2.id,
                "key": child2.key,
                "title": "child 2",
                "parent_goal_id": goal.id,
            }),
        ));

        // Drain observer2 for 200 ms; confirm no second goal.planner.first_child.
        let second_fire = try_wait_for_event(observer2, "goal.planner.first_child", 200).await;
        assert!(
            second_fire.is_err(),
            "goal.planner.first_child must not fire twice for the same goal"
        );
    }

    #[tokio::test]
    async fn reconciler_skips_recompute_for_goal_with_parent() {
        let store = tmp_store_for_reconciler().await;
        let (bus, _rx_keep_alive) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        // Create a "grandparent" goal (v1 invariant: goals don't have parents,
        // but if one exists the reconciler must skip it).
        let grandparent = make_goal_item(&store).await;
        // Create a "goal" that itself has a parent — this violates v1 depth=1.
        let invalid_goal = store
            .create_board_item(agentum_core::NewBoardItem {
                title: "nested goal (invalid v1)".into(),
                body: None,
                status: None,
                lbl: Some("goal".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: Some(grandparent.id),
            })
            .await
            .unwrap();
        // Create a child under the invalid goal.
        let child = make_child_item(&store, invalid_goal.id, Some("doing")).await;

        let observer = bus.subscribe();
        tokio::spawn(run_goal_reconciler(store.clone(), bus.clone()));

        let _ = bus.send(agentum_core::Event::new("board.updated").with_payload(
            serde_json::json!({
                "id": child.id,
                "key": child.key,
                "status": "doing",
                "parent_goal_id": invalid_goal.id,
            }),
        ));

        // The reconciler warns + skips; no goal.status.changed should fire.
        let result = try_wait_for_event(observer, "goal.status.changed", 200).await;
        assert!(
            result.is_err(),
            "reconciler must not patch a goal that itself has a parent"
        );
        // DB: invalid_goal must still be at todo (no PATCH fired).
        let still_todo = store
            .get_board_item(invalid_goal.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(still_todo.status, "todo");
    }

    // ---- session-comment bridge tests (plan 02-04, Task 1) ----

    async fn make_non_goal_card(store: &agentum_store::Store) -> agentum_core::BoardItem {
        store
            .create_board_item(agentum_core::NewBoardItem {
                title: "task card".into(),
                body: None,
                status: None,
                lbl: None, // not a goal
                tool: Some("claude".into()),
                workdir: Some("/tmp".into()),
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap()
    }

    async fn make_bound_session(
        store: &agentum_store::Store,
        card_id: i64,
    ) -> agentum_core::Session {
        store
            .create_session(agentum_core::NewSession {
                name: format!("sess-{card_id}"),
                workdir: "/tmp".into(),
                tool: "claude".into(),
                model: None,
                flags: vec![],
                card_id: Some(card_id),
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await
            .unwrap()
    }

    async fn wait_for_comment(
        store: &agentum_store::Store,
        card_id: i64,
        timeout_ms: u64,
    ) -> Vec<agentum_core::BoardComment> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            let comments = store.list_board_comments(card_id).await.unwrap();
            if !comments.is_empty() {
                return comments;
            }
            if std::time::Instant::now() >= deadline {
                return comments;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn bridge_inserts_awaiting_input_comment_on_bound_non_goal_card() {
        // Test 1: agent.awaiting_input on a bound non-goal card inserts
        // author="system", body="[system] agent awaiting input".
        let store = tmp_store_for_reconciler().await;
        let (bus, _keep) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let card = make_non_goal_card(&store).await;
        let sess = make_bound_session(&store, card.id).await;

        tokio::spawn(run_session_comment_bridge(store.clone(), bus.clone()));
        tokio::task::yield_now().await;

        let _ = bus.send(
            agentum_core::Event::new("agent.awaiting_input")
                .with_session(sess.id, sess.name.clone()),
        );

        let comments = wait_for_comment(&store, card.id, 500).await;
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author, "system");
        assert_eq!(comments[0].body, "[system] agent awaiting input");
    }

    #[tokio::test]
    async fn bridge_inserts_finished_comment_on_bound_non_goal_card() {
        // Test 2: agent.finished → "[system] agent finished"
        let store = tmp_store_for_reconciler().await;
        let (bus, _keep) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let card = make_non_goal_card(&store).await;
        let sess = make_bound_session(&store, card.id).await;

        tokio::spawn(run_session_comment_bridge(store.clone(), bus.clone()));
        tokio::task::yield_now().await;

        let _ = bus.send(
            agentum_core::Event::new("agent.finished").with_session(sess.id, sess.name.clone()),
        );

        let comments = wait_for_comment(&store, card.id, 500).await;
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author, "system");
        assert_eq!(comments[0].body, "[system] agent finished");
    }

    #[tokio::test]
    async fn bridge_inserts_crashed_comment_with_signature() {
        // Test 3: session.crashed with payload.signature → "[system] session crashed: SIGSEGV"
        let store = tmp_store_for_reconciler().await;
        let (bus, _keep) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let card = make_non_goal_card(&store).await;
        let sess = make_bound_session(&store, card.id).await;

        tokio::spawn(run_session_comment_bridge(store.clone(), bus.clone()));
        tokio::task::yield_now().await;

        let _ = bus.send(
            agentum_core::Event::new("session.crashed")
                .with_session(sess.id, sess.name.clone())
                .with_payload(serde_json::json!({ "signature": "SIGSEGV" })),
        );

        let comments = wait_for_comment(&store, card.id, 500).await;
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].body, "[system] session crashed: SIGSEGV");
    }

    #[tokio::test]
    async fn bridge_inserts_crashed_comment_without_signature_uses_unknown() {
        // Test 4: session.crashed without signature → "[system] session crashed: unknown"
        let store = tmp_store_for_reconciler().await;
        let (bus, _keep) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let card = make_non_goal_card(&store).await;
        let sess = make_bound_session(&store, card.id).await;

        tokio::spawn(run_session_comment_bridge(store.clone(), bus.clone()));
        tokio::task::yield_now().await;

        let _ = bus.send(
            agentum_core::Event::new("session.crashed")
                .with_session(sess.id, sess.name.clone())
                .with_payload(serde_json::json!({})),
        );

        let comments = wait_for_comment(&store, card.id, 500).await;
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].body, "[system] session crashed: unknown");
    }

    #[tokio::test]
    async fn bridge_trims_signature_to_80_chars() {
        // Test 5: 200-char signature trimmed to ≤80 chars
        let store = tmp_store_for_reconciler().await;
        let (bus, _keep) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let card = make_non_goal_card(&store).await;
        let sess = make_bound_session(&store, card.id).await;

        tokio::spawn(run_session_comment_bridge(store.clone(), bus.clone()));
        tokio::task::yield_now().await;

        let long_sig: String = "X".repeat(200);
        let _ = bus.send(
            agentum_core::Event::new("session.crashed")
                .with_session(sess.id, sess.name.clone())
                .with_payload(serde_json::json!({ "signature": long_sig })),
        );

        let comments = wait_for_comment(&store, card.id, 500).await;
        assert_eq!(comments.len(), 1);
        let prefix = "[system] session crashed: ";
        assert!(comments[0].body.starts_with(prefix));
        let sig_part = &comments[0].body[prefix.len()..];
        assert_eq!(
            sig_part.chars().count(),
            80,
            "signature must be trimmed to 80 chars"
        );
    }

    #[tokio::test]
    async fn bridge_skips_goal_card_events() {
        // Test 6: events on sessions bound to goal cards are skipped
        let store = tmp_store_for_reconciler().await;
        let (bus, _keep) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let goal_card = make_goal_item(&store).await; // lbl="goal"
        let sess = make_bound_session(&store, goal_card.id).await;

        tokio::spawn(run_session_comment_bridge(store.clone(), bus.clone()));
        tokio::task::yield_now().await;

        let _ = bus.send(
            agentum_core::Event::new("agent.finished").with_session(sess.id, sess.name.clone()),
        );

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let comments = store.list_board_comments(goal_card.id).await.unwrap();
        assert!(
            comments.is_empty(),
            "goal-card events must not produce comments"
        );
    }

    #[tokio::test]
    async fn bridge_skips_unbound_sessions() {
        // Test 7: session with no card_id → no comment
        let store = tmp_store_for_reconciler().await;
        let (bus, _keep) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let unbound_sess = store
            .create_session(agentum_core::NewSession {
                name: "unbound".into(),
                workdir: "/tmp".into(),
                tool: "claude".into(),
                model: None,
                flags: vec![],
                card_id: None, // no binding
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await
            .unwrap();

        tokio::spawn(run_session_comment_bridge(store.clone(), bus.clone()));
        tokio::task::yield_now().await;

        let _ = bus.send(
            agentum_core::Event::new("agent.finished")
                .with_session(unbound_sess.id, unbound_sess.name.clone()),
        );

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        // No card exists for this session, so no comments to check.
        // Just verifying no panic occurred in the bridge task.
    }

    #[tokio::test]
    async fn bridge_dedupes_identical_back_to_back_events() {
        // Test 8: two back-to-back agent.finished → only ONE comment
        let store = tmp_store_for_reconciler().await;
        let (bus, _keep) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let card = make_non_goal_card(&store).await;
        let sess = make_bound_session(&store, card.id).await;

        tokio::spawn(run_session_comment_bridge(store.clone(), bus.clone()));
        tokio::task::yield_now().await;

        let _ = bus.send(
            agentum_core::Event::new("agent.finished").with_session(sess.id, sess.name.clone()),
        );
        let _ = bus.send(
            agentum_core::Event::new("agent.finished").with_session(sess.id, sess.name.clone()),
        );

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let comments = store.list_board_comments(card.id).await.unwrap();
        assert_eq!(
            comments.len(),
            1,
            "back-to-back identical events must produce only one comment"
        );
    }

    #[tokio::test]
    async fn bridge_advances_doing_card_to_review_on_finish() {
        // The user-facing "update each task as it progresses" behaviour: a card
        // an agent is actively building (`doing`) advances to `review` when the
        // agent finishes its turn — but NOT on awaiting_input (still building),
        // and only ever out of `doing`.
        let store = tmp_store_for_reconciler().await;
        let (bus, _keep) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        // A started card sits in `doing`, bound to its agent session.
        let card = make_non_goal_card(&store).await;
        store
            .patch_board_item(
                card.id,
                agentum_core::BoardPatch {
                    status: Some("doing".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let sess = make_bound_session(&store, card.id).await;

        tokio::spawn(run_session_comment_bridge(store.clone(), bus.clone()));
        tokio::task::yield_now().await;

        // awaiting_input is mid-task → the card must stay in `doing`.
        let _ = bus.send(
            agentum_core::Event::new("agent.awaiting_input")
                .with_session(sess.id, sess.name.clone()),
        );
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(
            store.get_board_item(card.id).await.unwrap().unwrap().status,
            "doing",
            "awaiting_input must not advance the card out of Building"
        );

        // finished → the card advances `doing` → `review`.
        let _ = bus.send(
            agentum_core::Event::new("agent.finished").with_session(sess.id, sess.name.clone()),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            let st = store.get_board_item(card.id).await.unwrap().unwrap().status;
            if st == "review" || std::time::Instant::now() > deadline {
                assert_eq!(st, "review", "agent.finished must advance doing → review");
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn bridge_dedupe_resets_on_different_kind() {
        // Test 9: agent.finished then agent.awaiting_input → both comments inserted
        let store = tmp_store_for_reconciler().await;
        let (bus, _keep) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let card = make_non_goal_card(&store).await;
        let sess = make_bound_session(&store, card.id).await;

        tokio::spawn(run_session_comment_bridge(store.clone(), bus.clone()));
        tokio::task::yield_now().await;

        let _ = bus.send(
            agentum_core::Event::new("agent.finished").with_session(sess.id, sess.name.clone()),
        );
        // Wait for first comment before sending second event to avoid race.
        let first = wait_for_comment(&store, card.id, 500).await;
        assert_eq!(first.len(), 1);

        let _ = bus.send(
            agentum_core::Event::new("agent.awaiting_input")
                .with_session(sess.id, sess.name.clone()),
        );

        // Wait for second comment.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let comments = loop {
            let c = store.list_board_comments(card.id).await.unwrap();
            if c.len() >= 2 || std::time::Instant::now() >= deadline {
                break c;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        assert_eq!(comments.len(), 2, "different kind must bypass dedupe");
        assert!(comments.iter().any(|c| c.body == "[system] agent finished"));
        assert!(
            comments
                .iter()
                .any(|c| c.body == "[system] agent awaiting input")
        );
    }

    #[tokio::test]
    async fn bridge_recovers_from_bus_lag_and_continues() {
        // Test 10: bus lag does not kill the bridge; a valid event after lag
        // still inserts a comment.
        //
        // Design: use a small-capacity channel so flooding it triggers Lagged
        // on the bridge's receiver. The bridge must log a warning and continue
        // processing subsequent events rather than panicking or exiting.
        let store = tmp_store_for_reconciler().await;
        // Use channel capacity=4 so we can overflow it easily.
        let (bus, _keep) = tokio::sync::broadcast::channel::<agentum_core::Event>(4);

        let card = make_non_goal_card(&store).await;
        let sess = make_bound_session(&store, card.id).await;

        tokio::spawn(run_session_comment_bridge(store.clone(), bus.clone()));
        // Yield so the bridge task starts and calls bus.subscribe() before we
        // send the flood — we need it subscribed before overflow.
        tokio::task::yield_now().await;

        // Flood the channel with irrelevant events beyond capacity to trigger
        // Lagged on the bridge's receiver.
        for _ in 0..20 {
            let _ = bus.send(agentum_core::Event::new("host.metrics"));
        }

        // After the flood, send a real event. The bridge should log the lag
        // warning from RecvError::Lagged then pick up this event.
        let _ = bus.send(
            agentum_core::Event::new("agent.finished").with_session(sess.id, sess.name.clone()),
        );

        let comments = wait_for_comment(&store, card.id, 1000).await;
        assert_eq!(
            comments.len(),
            1,
            "bridge must continue after bus lag and still handle the real event"
        );
    }

    #[tokio::test]
    async fn bridge_ignores_irrelevant_event_kinds() {
        // Test 11: board.created and host.metrics are dropped silently
        let store = tmp_store_for_reconciler().await;
        let (bus, _keep) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let card = make_non_goal_card(&store).await;
        let sess = make_bound_session(&store, card.id).await;

        tokio::spawn(run_session_comment_bridge(store.clone(), bus.clone()));
        tokio::task::yield_now().await;

        let _ = bus.send(
            agentum_core::Event::new("board.created").with_session(sess.id, sess.name.clone()),
        );
        let _ = bus.send(agentum_core::Event::new("host.metrics"));

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let comments = store.list_board_comments(card.id).await.unwrap();
        assert!(
            comments.is_empty(),
            "irrelevant event kinds must not produce comments"
        );
    }

    #[tokio::test]
    async fn reconciler_recomputes_on_child_deletion() {
        let store = tmp_store_for_reconciler().await;
        let (bus, _rx_keep_alive) = tokio::sync::broadcast::channel::<agentum_core::Event>(64);

        let goal = make_goal_item(&store).await;
        let child1 = make_child_item(&store, goal.id, Some("done")).await;
        let child2 = make_child_item(&store, goal.id, Some("done")).await;
        // Pre-set goal to done.
        store
            .patch_board_item(
                goal.id,
                agentum_core::BoardPatch {
                    status: Some("done".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let observer = bus.subscribe();
        tokio::spawn(run_goal_reconciler(store.clone(), bus.clone()));

        // Delete child1 from DB, emit board.deleted.
        store.delete_board_item(child1.id).await.unwrap();
        let _ = bus.send(agentum_core::Event::new("board.deleted").with_payload(
            serde_json::json!({
                "id": child1.id,
                "parent_goal_id": goal.id,
            }),
        ));

        // child2 still done → goal stays done; no goal.status.changed.
        let no_change = try_wait_for_event(bus.subscribe(), "goal.status.changed", 200).await;
        assert!(
            no_change.is_err(),
            "goal must stay done while at least one done child remains"
        );
        let still_done = store.get_board_item(goal.id).await.unwrap().unwrap();
        assert_eq!(still_done.status, "done");

        // Now delete child2; goal must drop to todo (max-of-empty → todo).
        store.delete_board_item(child2.id).await.unwrap();
        let _ = bus.send(agentum_core::Event::new("board.deleted").with_payload(
            serde_json::json!({
                "id": child2.id,
                "parent_goal_id": goal.id,
            }),
        ));

        let ev = wait_for_event(observer, "goal.status.changed", 500).await;
        assert_eq!(ev.payload["from"], "done");
        assert_eq!(ev.payload["to"], "todo");

        let dropped = store.get_board_item(goal.id).await.unwrap().unwrap();
        assert_eq!(dropped.status, "todo");
    }
}
