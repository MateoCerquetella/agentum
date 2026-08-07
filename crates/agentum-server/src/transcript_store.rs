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
use agentum_core::transcript::{self, AgentTaskSnapshot, AgentTaskSnapshotStatus, AgentTaskState};
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
    /// the first turn.
    pinned_path: PathBuf,
    /// What we're actually reading right now. Kept separately so a future
    /// explicitly identified legacy source can be supported without changing
    /// cursor bookkeeping; current sessions always use `pinned_path`.
    transcript_path: PathBuf,
    /// Bytes already consumed from `transcript_path`. Lets each event
    /// apply only the newly appended slice instead of re-parsing the
    /// whole transcript.
    cursor: u64,
    /// Stable file identity used to distinguish replacement from append.
    /// Claude occasionally atomically replaces a transcript during recovery;
    /// size alone cannot detect a replacement whose new file is as large as
    /// the old one.
    file_identity: Option<u64>,
    /// Pending `Task` tool dispatches awaiting their `tool_result`.
    /// Keyed by tool_use id.
    pending_tasks: HashMap<String, OffsetDateTime>,
    /// Active notify watcher for this session. Dropped when the
    /// store is removed; keeps watcher alive otherwise.
    _watcher: RecommendedWatcher,
}

#[derive(Default)]
struct RemoteCursor {
    seen_len: usize,
    seen_hash: u64,
    reset_at: usize,
}

#[derive(Clone)]
pub struct TranscriptStore {
    inner: Arc<Mutex<HashMap<Uuid, Slot>>>,
    remote_cursors: Arc<Mutex<HashMap<Uuid, RemoteCursor>>>,
    bus: broadcast::Sender<Event>,
}

impl TranscriptStore {
    pub fn new(bus: broadcast::Sender<Event>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            remote_cursors: Arc::new(Mutex::new(HashMap::new())),
            bus,
        }
    }

    /// Snapshot the current state for a session. `None` when the store
    /// has never seen this session id.
    pub fn snapshot(&self, id: Uuid) -> Option<AgentTaskState> {
        self.inner.lock().ok()?.get(&id).map(|s| s.state.clone())
    }

    /// Source-qualified local snapshot for the HTTP API. The distinction
    /// between a missing transcript and a readable transcript with no task
    /// records is observable in the TUI instead of both becoming `{}`.
    pub fn snapshot_with_status(&self, id: Uuid, tool: &str) -> AgentTaskSnapshot {
        if tool != "claude" {
            let mut snap = AgentTaskSnapshot::new(
                AgentTaskState::default(),
                AgentTaskSnapshotStatus::Unsupported,
                tool,
            );
            snap.detail = Some(format!(
                "task transcript parser is not available for {tool}"
            ));
            return snap;
        }

        let Ok(guard) = self.inner.lock() else {
            let mut snap = AgentTaskSnapshot::new(
                AgentTaskState::default(),
                AgentTaskSnapshotStatus::ReadError,
                tool,
            );
            snap.detail = Some("transcript cache lock is unavailable".to_string());
            return snap;
        };
        let Some(slot) = guard.get(&id) else {
            return AgentTaskSnapshot::new(
                AgentTaskState::default(),
                AgentTaskSnapshotStatus::WaitingForTranscript,
                tool,
            );
        };
        let metadata = match std::fs::metadata(&slot.transcript_path) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                let mut snap = AgentTaskSnapshot::new(
                    slot.state.clone(),
                    AgentTaskSnapshotStatus::ReadError,
                    tool,
                );
                snap.transcript_path = Some(slot.transcript_path.to_string_lossy().into_owned());
                snap.detail = Some(error.to_string());
                return snap;
            }
        };
        if metadata.is_some()
            && let Err(error) = std::fs::File::open(&slot.transcript_path)
        {
            let mut snap = AgentTaskSnapshot::new(
                slot.state.clone(),
                AgentTaskSnapshotStatus::ReadError,
                tool,
            );
            snap.transcript_path = Some(slot.transcript_path.to_string_lossy().into_owned());
            snap.detail = Some(error.to_string());
            return snap;
        }
        let status = if metadata.is_none() {
            AgentTaskSnapshotStatus::WaitingForTranscript
        } else if slot.state.is_empty() {
            AgentTaskSnapshotStatus::Empty
        } else {
            AgentTaskSnapshotStatus::Current
        };
        let mut snap = AgentTaskSnapshot::new(slot.state.clone(), status, tool);
        snap.transcript_path = Some(slot.transcript_path.to_string_lossy().into_owned());
        snap.updated_at_ms = metadata
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);
        snap
    }

    /// Clear the cached plan/todos/tasks for this session **and** fast-
    /// forward the file cursor to the current end-of-file so anything
    /// already in the transcript is treated as consumed. Used by the
    /// TUI when it sees the user run `/clear` (or `\clear`) in the
    /// agent pane — the agent itself wipes its conversation context,
    /// and the plan/todo panel needs to mirror that even though the
    /// transcript file stays append-only.
    ///
    /// SSH snapshots do not have local watcher slots, so their most recently
    /// observed byte length is fast-forwarded separately. Broadcasts an
    /// `agent_tasks.updated` so every connected client lands on the empty
    /// state in lockstep.
    pub fn reset(&self, id: Uuid) {
        let mut cleared = false;
        if let Ok(mut guard) = self.inner.lock()
            && let Some(slot) = guard.get_mut(&id)
        {
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
        if let Ok(mut cursors) = self.remote_cursors.lock()
            && let Some(cursor) = cursors.get_mut(&id)
        {
            cursor.reset_at = cursor.seen_len;
            cleared = true;
        }
        if cleared {
            let _ = self.bus.send(
                Event::new("agent_tasks.updated")
                    .with_payload(json!({ "session_id": id.to_string() })),
            );
        }
    }

    /// Return the byte offset after the last explicit reset for an SSH
    /// transcript. Each GET still parses the complete post-reset suffix, so a
    /// partial final line or missed event converges on the next bounded poll.
    /// Prefix hashing detects replacement even when the replacement is not
    /// smaller than the prior file.
    pub fn remote_parse_start(&self, id: Uuid, content: &[u8]) -> usize {
        use std::hash::{Hash, Hasher};

        let Ok(mut cursors) = self.remote_cursors.lock() else {
            return 0;
        };
        let cursor = cursors.entry(id).or_default();
        let prefix_len = cursor.seen_len.min(content.len());
        let mut prefix_hasher = std::collections::hash_map::DefaultHasher::new();
        content[..prefix_len].hash(&mut prefix_hasher);
        let prefix_hash = prefix_hasher.finish();
        if content.len() < cursor.seen_len
            || (cursor.seen_len > 0 && prefix_hash != cursor.seen_hash)
        {
            cursor.reset_at = 0;
        }
        let start = cursor.reset_at.min(content.len());
        let mut full_hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut full_hasher);
        cursor.seen_len = content.len();
        cursor.seen_hash = full_hasher.finish();
        start
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
        // Claude is launched with
        // `--session-id <agentum-uuid>`, so `pinned_path` materializes
        // on the first turn and is the only one we ever read. Pinning
        // is what stops two agents in the same workdir from
        // cross-pollinating todos.
        // Never fall back to a different JSONL in the same workdir. That old
        // heuristic made a newly selected session briefly display another
        // agent's plan/todos until its own UUID-pinned transcript appeared.
        // Older unpinned sessions now remain explicitly in the waiting state
        // instead of risking cross-session leakage.
        let transcript_path = pinned_path.clone();

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
        let file_identity = std::fs::metadata(&transcript_path)
            .ok()
            .and_then(|metadata| file_identity(&metadata));

        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(
                id,
                Slot {
                    workdir,
                    state,
                    pinned_path,
                    transcript_path,
                    cursor,
                    file_identity,
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
                slot.file_identity = None;
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
            let identity = file_identity(&metadata);
            if slot.file_identity.is_some() && identity.is_some() && slot.file_identity != identity
            {
                slot.cursor = 0;
                slot.state = AgentTaskState::default();
                slot.pending_tasks.clear();
            }
            slot.file_identity = identity;
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

    /// Reconcile a requested slot even if its filesystem notification was
    /// coalesced or lost. The TUI's bounded polling path calls the GET route,
    /// which invokes this method before returning the snapshot.
    pub fn reconcile(&self, id: Uuid) {
        self.refresh(id);
    }
}

#[cfg(unix)]
fn file_identity(metadata: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.ino())
}

#[cfg(not(unix))]
fn file_identity(_metadata: &std::fs::Metadata) -> Option<u64> {
    None
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
    // Only consume newline-terminated records. A half-written final JSON
    // object must remain behind the cursor so reconciliation can parse it
    // after Claude completes the write.
    let complete_len = content.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    for line in content[..complete_len].lines() {
        transcript::apply_line(&mut state, &mut pending, line);
    }
    let cursor = complete_len as u64;
    (state, cursor, pending)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn plan_line(plan: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{{"content":[{{"type":"tool_use","id":"p1","name":"ExitPlanMode","input":{{"plan":"{plan}"}}}}]}}}}"#
        )
    }

    fn insert_slot(store: &TranscriptStore, id: Uuid, path: PathBuf) {
        let (state, cursor, pending_tasks) = parse_full(&path);
        let file_identity = std::fs::metadata(&path)
            .ok()
            .and_then(|metadata| super::file_identity(&metadata));
        let (tx, _rx) = std::sync::mpsc::channel();
        let watcher = RecommendedWatcher::new(tx, Config::default()).unwrap();
        store.inner.lock().unwrap().insert(
            id,
            Slot {
                workdir: path.parent().unwrap().to_path_buf(),
                state,
                pinned_path: path.clone(),
                transcript_path: path,
                cursor,
                file_identity,
                pending_tasks,
                _watcher: watcher,
            },
        );
    }

    #[test]
    fn initial_parse_leaves_partial_trailing_record_for_reconciliation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let line = plan_line("partial becomes current");
        std::fs::write(&path, &line).unwrap();

        let (state, cursor, _) = parse_full(&path);
        assert!(state.is_empty());
        assert_eq!(cursor, 0);

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file).unwrap();
        let (state, cursor, _) = parse_full(&path);
        assert_eq!(state.plan.as_deref(), Some("partial becomes current"));
        assert_eq!(cursor, line.len() as u64 + 1);
    }

    #[test]
    fn reconcile_recovers_missed_append_and_atomic_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, format!("{}\n", plan_line("first"))).unwrap();
        let (bus, _) = broadcast::channel(8);
        let store = TranscriptStore::new(bus);
        let id = Uuid::new_v4();
        insert_slot(&store, id, path.clone());

        let todo = r#"{"type":"assistant","timestamp":"2025-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"c1","name":"TaskCreate","input":{"subject":"caught up"}}]}}"#;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{todo}").unwrap();
        store.reconcile(id);
        let snap = store.snapshot_with_status(id, "claude");
        assert_eq!(snap.state.todos[0].content, "caught up");

        let replacement = dir.path().join("replacement.jsonl");
        std::fs::write(&replacement, format!("{}\n", plan_line("replacement"))).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        store.reconcile(id);
        let snap = store.snapshot_with_status(id, "claude");
        assert_eq!(snap.state.plan.as_deref(), Some("replacement"));
        assert!(snap.state.todos.is_empty());
    }

    #[test]
    fn unsupported_tools_are_not_reported_as_empty_claude_state() {
        let (bus, _) = broadcast::channel(1);
        let store = TranscriptStore::new(bus);
        let snapshot = store.snapshot_with_status(Uuid::new_v4(), "codex");
        assert_eq!(snapshot.status, AgentTaskSnapshotStatus::Unsupported);
        assert_eq!(snapshot.tool, "codex");
        assert!(snapshot.detail.unwrap().contains("not available"));
    }

    #[test]
    fn remote_reset_fast_forwards_and_replacement_clears_the_cutoff() {
        let (bus, _) = broadcast::channel(4);
        let store = TranscriptStore::new(bus);
        let id = Uuid::new_v4();
        let before = b"old transcript\n";
        assert_eq!(store.remote_parse_start(id, before), 0);

        store.reset(id);
        assert_eq!(store.remote_parse_start(id, before), before.len());
        let mut appended = before.to_vec();
        appended.extend_from_slice(b"new record\n");
        assert_eq!(store.remote_parse_start(id, &appended), before.len());

        let replacement = b"replacement file with a different prefix\n";
        assert_eq!(store.remote_parse_start(id, replacement), 0);
    }
}
