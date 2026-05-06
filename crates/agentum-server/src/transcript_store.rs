//! In-memory store for per-session agent task state, populated by
//! tailing each session's Claude Code JSONL transcript.
//!
//! Lifecycle: `TranscriptStore::start_for_session` is called every time
//! a session is mentioned in a TUI request (`get_one`, `start`, …). It's
//! idempotent — calling it twice for the same session id is cheap and
//! leaves a single watcher running. The watcher itself uses `notify` to
//! get coarse "directory changed" events; on each event we re-pick the
//! latest `*.jsonl` for the workdir, read what's been appended since
//! our last read, parse it, update the cached state, and broadcast an
//! `agent_tasks.updated` event.
//!
//! Why not async per-file watchers? Claude can rotate transcripts (it
//! creates a fresh file per session), so we'd have to re-resolve which
//! file is "current" anyway. Watching the project directory once and
//! polling `latest_transcript` on every event sidesteps the file-rotation
//! race and keeps the design boring.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agentum_core::transcript::{self, AgentTaskState};
use agentum_core::Event;
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
    /// Path of the JSONL file we're currently following — `None` until
    /// the agent's first turn lands one in the project dir.
    current_path: Option<PathBuf>,
    /// Bytes already consumed from `current_path`. Lets each event apply
    /// only the newly appended slice instead of re-parsing the whole
    /// transcript.
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
        self.inner
            .lock()
            .ok()?
            .get(&id)
            .map(|s| s.state.clone())
    }

    /// Begin watching this session's transcript directory if we aren't
    /// already. Idempotent. Performs an initial parse so callers see
    /// data without having to wait for the next FS event.
    pub fn ensure_started(&self, id: Uuid, workdir: PathBuf) {
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
            tracing::debug!(?workdir, "transcript: no $HOME or relative workdir; skipping watcher");
            return;
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
        let initial_path = transcript::latest_transcript(&project_dir);
        let (state, cursor, pending) = match initial_path.as_deref() {
            Some(path) => parse_full(path),
            None => (AgentTaskState::default(), 0, HashMap::new()),
        };

        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(
                id,
                Slot {
                    workdir,
                    state,
                    current_path: initial_path,
                    cursor,
                    pending_tasks: pending,
                    _watcher: watcher,
                },
            );
        }

        // Spawn the consumer that drains the notify channel forever.
        let store = self.clone();
        let project_dir_for_task = project_dir.clone();
        tokio::task::spawn_blocking(move || {
            for evt in rx {
                let Ok(evt) = evt else { continue };
                if !is_relevant(&evt.kind) {
                    continue;
                }
                store.refresh(id, &project_dir_for_task);
            }
        });

        // Kick a broadcast right after start so listening clients
        // (including the TUI that just switched selection) get the
        // initial snapshot without waiting for the agent to type
        // anything.
        let _ = self.bus.send(
            Event::new("agent_tasks.updated")
                .with_payload(json!({ "session_id": id.to_string() })),
        );
    }

    /// Re-resolve the latest transcript and apply any newly-appended
    /// bytes. Holds the store lock only long enough to mutate the slot.
    fn refresh(&self, id: Uuid, project_dir: &std::path::Path) {
        let emit;

        {
            let mut guard = match self.inner.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let Some(slot) = guard.get_mut(&id) else {
                return;
            };

            let latest = transcript::latest_transcript(project_dir);
            // File rotated (or first event after watcher start picks up
            // a new file). Reset cursor + replay the whole thing.
            if latest != slot.current_path {
                slot.current_path = latest.clone();
                slot.cursor = 0;
                slot.state = AgentTaskState::default();
                slot.pending_tasks.clear();
            }

            let Some(path) = slot.current_path.clone() else {
                return;
            };

            // Read only the newly appended slice. If the file shrank
            // (someone truncated it), restart from offset 0.
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
