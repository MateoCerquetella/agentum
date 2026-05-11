//! In-memory store for per-session agent task state, populated by
//! tailing each session's Claude Code JSONL transcript.
//!
//! Lifecycle: `TranscriptStore::ensure_started` is called every time a
//! session is mentioned in a TUI request (`get_one`, `start`, …). It's
//! idempotent — calling it twice for the same session id is cheap and
//! leaves a single watcher running. The watcher itself uses `notify` to
//! get coarse "directory changed" events; on each event we re-read the
//! deterministic per-session transcript path
//! (`<project_dir>/<agentum-session-id>.jsonl`), parse the newly
//! appended slice, update the cached state, and broadcast an
//! `agent_tasks.updated` event.
//!
//! Why per-session pinning? Claude Code names transcripts
//! `<session-uuid>.jsonl`, and `ClaudeAdapter::launch` passes
//! `--session-id <agentum-session-uuid>` so the agentum id *is* the
//! file stem. Earlier this code picked the most-recently-mtimed
//! `*.jsonl` in the project dir, which cross-pollinated todos when
//! multiple agents shared a workdir.
//!
//! Non-Claude tools (codex, gemini, opencode, …) write transcripts in
//! tool-specific shapes and locations and aren't supported here yet —
//! `ensure_started` short-circuits for them so we don't materialize an
//! empty `~/.claude/projects/<enc-cwd>/` for sessions that never run
//! claude.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agentum_core::Event;
use agentum_core::transcript::{self, AgentTaskState};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::json;
use time::OffsetDateTime;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Per-session bookkeeping kept inside the store. Wrapped in `Mutex`
/// because the watcher thread (sync, supplied by `notify`) and the
/// Axum handler thread both touch it.
struct Slot {
    workdir: PathBuf,
    state: AgentTaskState,
    /// Deterministic path of this session's JSONL transcript
    /// (`<project_dir>/<agentum-session-id>.jsonl`). Always set, even
    /// when the file doesn't exist yet — claude materializes it on
    /// the first turn. We *promote* to this path the moment it
    /// appears on disk, swapping out any fallback we picked at slot
    /// creation time so the panel snaps to the agent's own transcript
    /// the instant claude starts writing.
    pinned_path: PathBuf,
    /// What we're actually reading right now. Equals `pinned_path`
    /// when claude has begun writing; otherwise it's a best-guess
    /// fallback (the latest `*.jsonl` in the project dir at the
    /// moment we resolved). `refresh` re-checks for `pinned_path` on
    /// every tick so a slot that started with a stale fallback
    /// recovers automatically.
    transcript_path: PathBuf,
    /// Bytes already consumed from `transcript_path`. Lets each event
    /// apply only the newly appended slice instead of re-parsing the
    /// whole transcript.
    cursor: u64,
    /// Pending `Task` tool dispatches awaiting their `tool_result`.
    /// Keyed by tool_use id.
    pending_tasks: HashMap<String, OffsetDateTime>,
    /// Active notify watcher for this session. Dropped when the
    /// store is removed; keeps watcher alive otherwise.
    _watcher: RecommendedWatcher,
}

#[derive(Clone)]
pub struct TranscriptStore {
    inner: Arc<Mutex<HashMap<Uuid, Slot>>>,
    bus: broadcast::Sender<Event>,
}

impl TranscriptStore {
    pub fn new(bus: broadcast::Sender<Event>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            bus,
        }
    }

    /// Snapshot the current state for a session. `None` when the store
    /// has never seen this session id.
    pub fn snapshot(&self, id: Uuid) -> Option<AgentTaskState> {
        self.inner.lock().ok()?.get(&id).map(|s| s.state.clone())
    }

    /// Clear the cached plan/todos/tasks for this session **and** fast-
    /// forward the file cursor to the current end-of-file so anything
    /// already in the transcript is treated as consumed. Used by the
    /// TUI when it sees the user run `/clear` (or `\clear`) in the
    /// agent pane — the agent itself wipes its conversation context,
    /// and the plan/todo panel needs to mirror that even though the
    /// transcript file stays append-only.
    ///
    /// No-op if the slot doesn't exist yet (nothing cached → nothing
    /// to wipe). Broadcasts an `agent_tasks.updated` so every connected
    /// client refetches and lands on the empty state in lockstep.
    pub fn reset(&self, id: Uuid) {
        let cleared;
        {
            let Ok(mut guard) = self.inner.lock() else {
                return;
            };
            let Some(slot) = guard.get_mut(&id) else {
                return;
            };
            slot.state = AgentTaskState::default();
            slot.pending_tasks.clear();
            // Cursor → current file length so the next refresh() only
            // sees bytes appended after the reset. Without this the
            // FS watcher's first refresh would re-parse the entire
            // file from offset 0 and rebuild the cleared state.
            if let Ok(meta) = std::fs::metadata(&slot.transcript_path) {
                slot.cursor = meta.len();
            }
            cleared = true;
        }
        if cleared {
            let _ = self.bus.send(
                Event::new("agent_tasks.updated")
                    .with_payload(json!({ "session_id": id.to_string() })),
            );
        }
    }

    /// Begin watching this session's transcript file if we aren't
    /// already. Idempotent. Performs an initial parse so callers see
    /// data without having to wait for the next FS event.
    ///
    /// `tool` is the session's tool name. Only `claude` has a
    /// supported transcript shape today; other tools short-circuit so
    /// we don't materialize an empty `~/.claude/projects/...` for
    /// codex/gemini/opencode sessions and don't pretend the panel has
    /// data for them.
    pub fn ensure_started(&self, id: Uuid, workdir: PathBuf, tool: &str) {
        if tool != "claude" {
            // Codex writes `~/.codex/sessions/YYYY/MM/DD/rollout-…-<uuid>.jsonl`
            // and opencode is SQLite-backed; both also lack a launch-
            // time session-id flag, so the agentum→tool id mapping
            // story is open. Until those parsers exist, no-op.
            return;
        }

        {
            let guard = match self.inner.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            if guard.contains_key(&id) {
                return;
            }
        }

        let Some(project_dir) = transcript::project_dir_for(&workdir) else {
            tracing::debug!(
                ?workdir,
                "transcript: no $HOME or relative workdir; skipping watcher"
            );
            return;
        };
        let Some(pinned_path) = transcript::transcript_path_for(&workdir, id) else {
            return;
        };
        // Resolve the actual file to read.
        //
        // Post-v0.6.25 sessions: claude was launched with
        // `--session-id <agentum-uuid>`, so `pinned_path` materializes
        // on the first turn and is the only one we ever read. Pinning
        // is what stops two agents in the same workdir from
        // cross-pollinating todos.
        //
        // Pre-v0.6.25 sessions: claude wrote to its own random UUID,
        // and `pinned_path` will never appear. Without a fallback the
        // Plan / Todos / Tasks panels stay empty until the user kills
        // the session and recreates it. We accept the cross-
        // pollination risk for pre-pin sessions in exchange for a
        // working panel — the alternative is a silent dead-end UI.
        let transcript_path = if pinned_path.exists() {
            pinned_path.clone()
        } else {
            transcript::latest_transcript_excluding(&project_dir, Some(&pinned_path))
                .unwrap_or_else(|| pinned_path.clone())
        };

        // Make the directory if it doesn't exist yet — Claude Code creates
        // it on first turn. notify barfs on missing paths otherwise.
        if !project_dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&project_dir) {
                tracing::debug!(error = %e, dir = %project_dir.display(), "transcript: cannot create project dir");
                return;
            }
        }

        // Channel: notify watcher → tokio task that mutates the store.
        // We watch the project directory (not the file directly)
        // because claude creates the JSONL on its first turn — the file
        // doesn't exist yet at watcher-setup time.
        let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "transcript: failed to create watcher");
                return;
            }
        };
        if let Err(e) = watcher.watch(&project_dir, RecursiveMode::NonRecursive) {
            tracing::warn!(error = %e, dir = %project_dir.display(), "transcript: watch failed");
            return;
        }

        // Populate immediately from whatever's already on disk so the
        // first GET returns data even if no FS events have fired yet.
        // First run usually has nothing — claude hasn't started writing
        // yet — and that's fine.
        let (state, cursor, pending) = if transcript_path.exists() {
            parse_full(&transcript_path)
        } else {
            (AgentTaskState::default(), 0, HashMap::new())
        };

        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(
                id,
                Slot {
                    workdir,
                    state,
                    pinned_path,
                    transcript_path,
                    cursor,
                    pending_tasks: pending,
                    _watcher: watcher,
                },
            );
        }

        // Spawn the consumer that drains the notify channel forever.
        // Note: we don't filter on event paths here — any change in the
        // project dir triggers a refresh, and refresh() reads only our
        // pinned file. notify's path payload isn't reliable across
        // platforms so the cheap re-stat is safer than path matching.
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            for evt in rx {
                let Ok(evt) = evt else { continue };
                if !is_relevant(&evt.kind) {
                    continue;
                }
                store.refresh(id);
            }
        });

        // Kick a broadcast right after start so listening clients
        // (including the TUI that just switched selection) get the
        // initial snapshot without waiting for the agent to type
        // anything.
        let _ = self.bus.send(
            Event::new("agent_tasks.updated").with_payload(json!({ "session_id": id.to_string() })),
        );
    }

    /// Apply any newly-appended bytes from this session's pinned
    /// transcript file. Holds the store lock only long enough to mutate
    /// the slot.
    fn refresh(&self, id: Uuid) {
        let emit;

        {
            let mut guard = match self.inner.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let Some(slot) = guard.get_mut(&id) else {
                return;
            };

            // Promote to the pinned (agentum-UUID) transcript the
            // moment claude creates it. Pre-pin sessions or sessions
            // whose first turn lands AFTER agentum's slot was
            // initialised would otherwise stay locked on whatever
            // fallback we picked at slot creation — usually a stale
            // cross-pollinated transcript from another agent in the
            // same workdir. Wiping cursor + state forces a clean
            // re-parse from the new file.
            if slot.transcript_path != slot.pinned_path && slot.pinned_path.exists() {
                slot.transcript_path = slot.pinned_path.clone();
                slot.cursor = 0;
                slot.state = AgentTaskState::default();
                slot.pending_tasks.clear();
            }

            let path = slot.transcript_path.clone();

            // Read only the newly appended slice. If the file shrank
            // (someone truncated it), restart from offset 0. The file
            // may also not exist yet — claude creates it on the first
            // turn — so a missing file just means "nothing to do."
            let metadata = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => return,
            };
            let len = metadata.len();
            if len < slot.cursor {
                slot.cursor = 0;
                slot.state = AgentTaskState::default();
                slot.pending_tasks.clear();
            }
            if len == slot.cursor {
                return; // unchanged — no need to emit
            }

            use std::io::{Read, Seek, SeekFrom};
            let mut file = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(_) => return,
            };
            if file.seek(SeekFrom::Start(slot.cursor)).is_err() {
                return;
            }
            let mut buf = Vec::with_capacity((len - slot.cursor) as usize);
            if file.read_to_end(&mut buf).is_err() {
                return;
            }
            // Don't advance the cursor past a partial trailing line — wait
            // for the next event so we apply complete JSON objects only.
            let last_newline = buf.iter().rposition(|&b| b == b'\n');
            let end = match last_newline {
                Some(idx) => idx + 1,
                None => 0,
            };
            if end == 0 {
                return;
            }
            let consumed = &buf[..end];
            let text = match std::str::from_utf8(consumed) {
                Ok(s) => s,
                Err(_) => return,
            };
            for line in text.lines() {
                transcript::apply_line(&mut slot.state, &mut slot.pending_tasks, line);
            }
            slot.cursor += end as u64;
            emit = true;
            // Workdir is captured in the slot for future debugging.
            let _ = &slot.workdir;
        }

        if emit {
            let _ = self.bus.send(
                Event::new("agent_tasks.updated")
                    .with_payload(json!({ "session_id": id.to_string() })),
            );
        }
    }
}

fn is_relevant(kind: &EventKind) -> bool {
    use notify::event::ModifyKind;
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Any)
            | EventKind::Modify(ModifyKind::Name(_))
            | EventKind::Modify(ModifyKind::Other)
    )
}

fn parse_full(path: &std::path::Path) -> (AgentTaskState, u64, HashMap<String, OffsetDateTime>) {
    let mut state = AgentTaskState::default();
    let mut pending = HashMap::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return (state, 0, pending);
    };
    for line in content.lines() {
        transcript::apply_line(&mut state, &mut pending, line);
    }
    let cursor = content.len() as u64;
    (state, cursor, pending)
}
