//! Cached per-session agent task state backed by Claude Code transcripts.
//!
//! Transcript state and transcript observation intentionally have different
//! lifetimes. Historical reads synchronously consume the appended JSONL suffix
//! without creating a directory or watcher. Only a live read attaches a
//! filesystem observer, and dropping that observer aborts its Tokio consumer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentum_core::transcript::{self, AgentTaskState};
use agentum_core::{Event, Session, Status};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::json;
use time::OffsetDateTime;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Whether a read should only refresh cached state or should also keep that
/// state current through filesystem notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObservationMode {
    Live,
    SnapshotOnly,
}

type NotifyCallback = Box<dyn FnMut(notify::Result<notify::Event>) + Send + 'static>;

trait ObserverGuard: Send {}
impl<T: Send> ObserverGuard for T {}

trait ObserverFactory: Send + Sync {
    fn create(
        &self,
        project_dir: &Path,
        callback: NotifyCallback,
    ) -> notify::Result<Box<dyn ObserverGuard>>;
}

struct NotifyObserverFactory;

impl ObserverFactory for NotifyObserverFactory {
    fn create(
        &self,
        project_dir: &Path,
        callback: NotifyCallback,
    ) -> notify::Result<Box<dyn ObserverGuard>> {
        let mut watcher = RecommendedWatcher::new(callback, Config::default())?;
        watcher.watch(project_dir, RecursiveMode::NonRecursive)?;
        Ok(Box::new(watcher))
    }
}

/// Owns every live-observation resource. The consumer is aborted before the
/// watcher guard is released, so a late callback cannot keep a task alive.
struct Observer {
    consumer: JoinHandle<()>,
    _guard: Box<dyn ObserverGuard>,
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.consumer.abort();
    }
}

/// Parser state can remain cached after live observation is retired.
struct Slot {
    workdir: PathBuf,
    state: AgentTaskState,
    pinned_path: PathBuf,
    transcript_path: PathBuf,
    cursor: u64,
    pending_tasks: HashMap<String, OffsetDateTime>,
    observer: Option<Observer>,
}

#[derive(Clone)]
pub struct TranscriptStore {
    inner: Arc<Mutex<HashMap<Uuid, Slot>>>,
    bus: broadcast::Sender<Event>,
    observer_factory: Arc<dyn ObserverFactory>,
}

impl TranscriptStore {
    pub fn new(bus: broadcast::Sender<Event>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            bus,
            observer_factory: Arc::new(NotifyObserverFactory),
        }
    }

    /// Atomically create/reuse passive state, synchronously consume complete
    /// appended transcript lines, and (for a live Claude session) attach the
    /// one observer owned by the slot.
    pub(crate) fn read(
        &self,
        id: Uuid,
        workdir: PathBuf,
        tool: &str,
        mode: ObservationMode,
    ) -> AgentTaskState {
        if tool != "claude" {
            self.forget(id);
            return AgentTaskState::default();
        }

        let Some(project_dir) = transcript::project_dir_for(&workdir) else {
            tracing::debug!(?workdir, "transcript: no home or relative workdir");
            return AgentTaskState::default();
        };
        let Some(pinned_path) = transcript::transcript_path_for(&workdir, id) else {
            return AgentTaskState::default();
        };

        let mut emit = false;
        let state = {
            let Ok(mut guard) = self.inner.lock() else {
                return AgentTaskState::default();
            };

            let replace = guard.get(&id).is_some_and(|slot| slot.workdir != workdir);
            if replace {
                guard.remove(&id);
            }
            let slot = guard
                .entry(id)
                .or_insert_with(|| passive_slot(workdir, &project_dir, pinned_path));

            match mode {
                ObservationMode::Live if slot.observer.is_none() => {
                    match self.create_observer(id, &project_dir) {
                        Some(observer) => {
                            slot.observer = Some(observer);
                            // Preserve the existing initial update notification.
                            emit = true;
                        }
                        None => {
                            // A later live read retries because the slot remains passive.
                        }
                    }
                }
                ObservationMode::SnapshotOnly => {
                    slot.observer.take();
                }
                ObservationMode::Live => {}
            }

            emit |= refresh_slot(slot);
            slot.state.clone()
        };

        if emit {
            self.emit_updated(id);
        }
        state
    }

    /// Clear state and advance the selected transcript cursor to EOF. This is
    /// session-aware so reset-before-first-read cannot replay old content.
    pub(crate) fn reset(&self, id: Uuid, workdir: PathBuf, tool: &str) {
        if tool != "claude" {
            return;
        }
        let Some(project_dir) = transcript::project_dir_for(&workdir) else {
            return;
        };
        let Some(pinned_path) = transcript::transcript_path_for(&workdir, id) else {
            return;
        };

        let cleared = {
            let Ok(mut guard) = self.inner.lock() else {
                return;
            };
            let replace = guard.get(&id).is_some_and(|slot| slot.workdir != workdir);
            if replace {
                guard.remove(&id);
            }
            let slot = guard
                .entry(id)
                .or_insert_with(|| passive_slot(workdir, &project_dir, pinned_path));
            promote_pinned(slot);
            slot.state = AgentTaskState::default();
            slot.pending_tasks.clear();
            slot.cursor = std::fs::metadata(&slot.transcript_path)
                .map(|meta| meta.len())
                .unwrap_or(0);
            true
        };
        if cleared {
            self.emit_updated(id);
        }
    }

    /// Drop live observation but retain the parsed snapshot.
    pub(crate) fn stop_observing(&self, id: Uuid) {
        if let Ok(mut guard) = self.inner.lock()
            && let Some(slot) = guard.get_mut(&id)
        {
            slot.observer.take();
        }
    }

    /// Retire observers that are no longer backed by a running Claude session
    /// at the same workdir. This operation only drops; it never reads or starts.
    pub fn retain_observers(&self, running: &[Session]) {
        let running: HashMap<Uuid, &Session> = running.iter().map(|s| (s.id, s)).collect();
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        for (id, slot) in guard.iter_mut() {
            let keep = running.get(id).is_some_and(|session| {
                session.status == Status::Running
                    && session.tool == "claude"
                    && Path::new(&session.workdir) == slot.workdir
            });
            if !keep {
                slot.observer.take();
            }
        }
    }

    /// Remove both observation and cached parser state.
    pub(crate) fn forget(&self, id: Uuid) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(&id);
        }
    }

    fn create_observer(&self, id: Uuid, project_dir: &Path) -> Option<Observer> {
        if !project_dir.exists()
            && let Err(error) = std::fs::create_dir_all(project_dir)
        {
            tracing::debug!(%error, dir = %project_dir.display(), "transcript: cannot create project dir");
            return None;
        }

        let (tx, mut rx) = mpsc::channel::<()>(1);
        let callback: NotifyCallback = Box::new(move |result| {
            let Ok(event) = result else {
                return;
            };
            if is_relevant(&event.kind) {
                // Capacity one coalesces bursts: Full means a refresh is queued.
                let _ = tx.try_send(());
            }
        });
        let guard = match self.observer_factory.create(project_dir, callback) {
            Ok(guard) => guard,
            Err(error) => {
                tracing::warn!(%error, dir = %project_dir.display(), "transcript: watch failed");
                return None;
            }
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("transcript: live read outside a Tokio runtime");
            return None;
        };
        let store = self.clone();
        let consumer = runtime.spawn(async move {
            while rx.recv().await.is_some() {
                store.refresh(id);
            }
        });
        Some(Observer {
            consumer,
            _guard: guard,
        })
    }

    fn refresh(&self, id: Uuid) {
        let changed = {
            let Ok(mut guard) = self.inner.lock() else {
                return;
            };
            guard.get_mut(&id).is_some_and(refresh_slot)
        };
        if changed {
            self.emit_updated(id);
        }
    }

    fn emit_updated(&self, id: Uuid) {
        let _ = self.bus.send(
            Event::new("agent_tasks.updated").with_payload(json!({ "session_id": id.to_string() })),
        );
    }

    #[cfg(test)]
    pub(crate) fn with_counting_factory(bus: broadcast::Sender<Event>) -> (Self, ObserverCounts) {
        let counts = ObserverCounts::default();
        (
            Self {
                inner: Arc::new(Mutex::new(HashMap::new())),
                bus,
                observer_factory: Arc::new(CountingObserverFactory {
                    counts: counts.clone(),
                }),
            },
            counts,
        )
    }

    #[cfg(test)]
    pub(crate) fn cache_count(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn observing_count(&self) -> usize {
        self.inner
            .lock()
            .map(|g| g.values().filter(|slot| slot.observer.is_some()).count())
            .unwrap_or(0)
    }
}

fn passive_slot(workdir: PathBuf, project_dir: &Path, pinned_path: PathBuf) -> Slot {
    let transcript_path = if pinned_path.exists() {
        pinned_path.clone()
    } else {
        transcript::latest_transcript_excluding(project_dir, Some(&pinned_path))
            .unwrap_or_else(|| pinned_path.clone())
    };
    Slot {
        workdir,
        state: AgentTaskState::default(),
        pinned_path,
        transcript_path,
        cursor: 0,
        pending_tasks: HashMap::new(),
        observer: None,
    }
}

fn promote_pinned(slot: &mut Slot) {
    if slot.transcript_path != slot.pinned_path && slot.pinned_path.exists() {
        slot.transcript_path = slot.pinned_path.clone();
        slot.cursor = 0;
        slot.state = AgentTaskState::default();
        slot.pending_tasks.clear();
    }
}

/// Apply complete newly appended lines, preserving the cursor at the beginning
/// of a partial trailing line until a later append completes it.
fn refresh_slot(slot: &mut Slot) -> bool {
    promote_pinned(slot);
    let Ok(metadata) = std::fs::metadata(&slot.transcript_path) else {
        return false;
    };
    let len = metadata.len();
    if len < slot.cursor {
        slot.cursor = 0;
        slot.state = AgentTaskState::default();
        slot.pending_tasks.clear();
    }
    if len == slot.cursor {
        return false;
    }

    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(&slot.transcript_path) else {
        return false;
    };
    if file.seek(SeekFrom::Start(slot.cursor)).is_err() {
        return false;
    }
    let mut buf = Vec::with_capacity((len - slot.cursor) as usize);
    if file.read_to_end(&mut buf).is_err() {
        return false;
    }
    let Some(end) = buf.iter().rposition(|&byte| byte == b'\n').map(|i| i + 1) else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(&buf[..end]) else {
        return false;
    };
    for line in text.lines() {
        transcript::apply_line(&mut slot.state, &mut slot.pending_tasks, line);
    }
    slot.cursor += end as u64;
    true
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

#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct ObserverCounts {
    created: Arc<std::sync::atomic::AtomicUsize>,
    dropped: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl ObserverCounts {
    pub(crate) fn created(&self) -> usize {
        self.created.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn dropped(&self) -> usize {
        self.dropped.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
struct CountingObserverFactory {
    counts: ObserverCounts,
}

#[cfg(test)]
impl ObserverFactory for CountingObserverFactory {
    fn create(
        &self,
        _project_dir: &Path,
        _callback: NotifyCallback,
    ) -> notify::Result<Box<dyn ObserverGuard>> {
        self.counts
            .created
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Box::new(CountingObserverGuard {
            counts: self.counts.clone(),
        }))
    }
}

#[cfg(test)]
struct CountingObserverGuard {
    counts: ObserverCounts,
}

#[cfg(test)]
impl Drop for CountingObserverGuard {
    fn drop(&mut self) {
        self.counts
            .dropped
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const TODO_A: &str = r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"t1","name":"TodoWrite","input":{"todos":[{"content":"a","status":"pending"}]}}]}}"#;
    const TODO_B: &str = r#"{"type":"assistant","timestamp":"2025-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"t2","name":"TodoWrite","input":{"todos":[{"content":"b","status":"pending"}]}}]}}"#;

    fn fixture() -> (tempfile::TempDir, PathBuf, Uuid, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let workdir = root.path().join("workspace");
        std::fs::create_dir_all(&workdir).unwrap();
        let id = Uuid::new_v4();
        let path = transcript::transcript_path_for(&workdir, id).unwrap();
        (root, workdir, id, path)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn repeated_and_concurrent_live_reads_create_exactly_one_observer() {
        let (_root, workdir, id, path) = fixture();
        let project_dir = path.parent().unwrap();
        let (bus, mut rx) = broadcast::channel(16);
        let (store, counts) = TranscriptStore::with_counting_factory(bus);
        let barrier = Arc::new(tokio::sync::Barrier::new(16));
        let mut joins = Vec::new();
        for _ in 0..16 {
            let store = store.clone();
            let workdir = workdir.clone();
            let barrier = barrier.clone();
            joins.push(tokio::spawn(async move {
                barrier.wait().await;
                store.read(id, workdir, "claude", ObservationMode::Live)
            }));
        }
        for join in joins {
            join.await.unwrap();
        }
        assert_eq!(counts.created(), 1);
        assert_eq!(store.observing_count(), 1);
        let event = rx.try_recv().expect("first live read emits update");
        assert_eq!(event.kind, "agent_tasks.updated");
        assert_eq!(event.payload, json!({ "session_id": id.to_string() }));
        store.stop_observing(id);
        assert_eq!(counts.dropped(), 1);
        let _ = std::fs::remove_dir_all(project_dir);
    }

    #[test]
    fn historical_reads_refresh_appended_complete_lines_without_observation() {
        let (_root, workdir, id, path) = fixture();
        let project_dir = path.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{TODO_A}\n")).unwrap();
        let (bus, _) = broadcast::channel(16);
        let (store, counts) = TranscriptStore::with_counting_factory(bus);
        let first = store.read(id, workdir.clone(), "claude", ObservationMode::SnapshotOnly);
        assert_eq!(first.todos[0].content, "a");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(file, "{TODO_B}").unwrap();
        let partial = store.read(id, workdir.clone(), "claude", ObservationMode::SnapshotOnly);
        assert_eq!(partial.todos[0].content, "a");
        writeln!(file).unwrap();
        let complete = store.read(id, workdir, "claude", ObservationMode::SnapshotOnly);
        assert_eq!(complete.todos[0].content, "b");
        assert_eq!(counts.created(), 0);
        let _ = std::fs::remove_dir_all(project_dir);
    }

    #[test]
    fn reset_before_first_read_never_resurrects_pre_reset_tasks() {
        let (_root, workdir, id, path) = fixture();
        let project_dir = path.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{TODO_A}\n")).unwrap();
        let (bus, _) = broadcast::channel(16);
        let (store, counts) = TranscriptStore::with_counting_factory(bus);
        store.reset(id, workdir.clone(), "claude");
        let empty = store.read(id, workdir.clone(), "claude", ObservationMode::SnapshotOnly);
        assert!(empty.is_empty());
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{TODO_B}").unwrap();
        let post_reset = store.read(id, workdir, "claude", ObservationMode::SnapshotOnly);
        assert_eq!(post_reset.todos.len(), 1);
        assert_eq!(post_reset.todos[0].content, "b");
        assert_eq!(counts.created(), 0);
        let _ = std::fs::remove_dir_all(project_dir);
    }

    #[test]
    fn pinned_transcript_promotes_over_legacy_fallback() {
        let (_root, workdir, id, pinned) = fixture();
        let project_dir = pinned.parent().unwrap();
        std::fs::create_dir_all(project_dir).unwrap();
        let legacy = project_dir.join(format!("{}.jsonl", Uuid::new_v4()));
        std::fs::write(&legacy, format!("{TODO_A}\n")).unwrap();
        let (bus, _) = broadcast::channel(16);
        let (store, counts) = TranscriptStore::with_counting_factory(bus);
        let fallback = store.read(id, workdir.clone(), "claude", ObservationMode::SnapshotOnly);
        assert_eq!(fallback.todos[0].content, "a");
        std::fs::write(&pinned, format!("{TODO_B}\n")).unwrap();
        let promoted = store.read(id, workdir, "claude", ObservationMode::SnapshotOnly);
        assert_eq!(promoted.todos[0].content, "b");
        assert_eq!(counts.created(), 0);
        let _ = std::fs::remove_dir_all(project_dir);
    }

    #[tokio::test]
    async fn lifecycle_operations_drop_without_starting_observers() {
        let (_root, workdir, id, path) = fixture();
        let project_dir = path.parent().unwrap().to_path_buf();
        let (bus, _) = broadcast::channel(16);
        let (store, counts) = TranscriptStore::with_counting_factory(bus);
        store.read(id, workdir.clone(), "claude", ObservationMode::Live);
        assert_eq!(counts.created(), 1);

        let mut running: Session = serde_json::from_value(serde_json::json!({
            "id": id, "name": "s", "workdir": workdir.to_string_lossy(), "tool": "claude",
            "model": null, "flags": [], "status": "running", "tmux_target": null,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "last_activity_at": null
        }))
        .unwrap();
        store.retain_observers(std::slice::from_ref(&running));
        assert_eq!(store.observing_count(), 1);
        running.tool = "codex".into();
        store.retain_observers(std::slice::from_ref(&running));
        assert_eq!(counts.dropped(), 1);
        assert_eq!(store.cache_count(), 1);

        store.read(id, workdir.clone(), "claude", ObservationMode::Live);
        assert_eq!(counts.created(), 2);
        running.tool = "claude".into();
        running.status = Status::Stopped;
        store.retain_observers(std::slice::from_ref(&running));
        assert_eq!(counts.dropped(), 2);

        store.read(id, workdir, "claude", ObservationMode::Live);
        assert_eq!(counts.created(), 3);
        store.retain_observers(&[]);
        assert_eq!(counts.dropped(), 3);
        store.forget(id);
        assert_eq!(store.cache_count(), 0);
        assert_eq!(counts.created(), 3, "retirement must never create");
        let _ = std::fs::remove_dir_all(project_dir);
    }

    #[tokio::test]
    async fn non_claude_read_forgets_prior_claude_cache_and_observer() {
        let (_root, workdir, id, path) = fixture();
        let project_dir = path.parent().unwrap().to_path_buf();
        let (bus, _) = broadcast::channel(16);
        let (store, counts) = TranscriptStore::with_counting_factory(bus);
        store.read(id, workdir.clone(), "claude", ObservationMode::Live);
        assert_eq!(store.cache_count(), 1);
        assert_eq!(store.observing_count(), 1);

        let empty = store.read(id, workdir, "codex", ObservationMode::SnapshotOnly);
        assert!(empty.is_empty());
        assert_eq!(store.cache_count(), 0);
        assert_eq!(store.observing_count(), 0);
        assert_eq!(counts.dropped(), 1);
        let _ = std::fs::remove_dir_all(project_dir);
    }

    #[test]
    fn non_claude_read_and_reset_create_nothing() {
        let (_root, workdir, id, path) = fixture();
        let (bus, _) = broadcast::channel(16);
        let (store, counts) = TranscriptStore::with_counting_factory(bus);
        assert!(
            store
                .read(id, workdir.clone(), "codex", ObservationMode::Live)
                .is_empty()
        );
        store.reset(id, workdir, "codex");
        assert_eq!(store.cache_count(), 0);
        assert_eq!(counts.created(), 0);
        assert!(!path.parent().unwrap().exists());
    }
}
