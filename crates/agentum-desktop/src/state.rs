use std::{collections::HashMap, io::Write, sync::Arc};

use anyhow::Context;
use notify::RecommendedWatcher;
use parking_lot::Mutex;
use portable_pty::{Child, MasterPty};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

// Cap on the retained window. The renderer asks for ~5000 scrollback rows;
// 2 MiB of raw bytes comfortably covers that for a wide terminal while bounding
// per-pane memory. It also matches the renderer's own backlog cap, so a single
// hidden pane never holds more here than it would have queued itself.
pub const MAIN_BUFFER_CAP_BYTES: usize = 2 * 1024 * 1024;

// Retained raw PTY output so the renderer can rebuild a hidden pane's screen
// via pty_get_main_buffer_snapshot. The renderer deliberately stops writing
// output to xterm while a pane is hidden (to avoid backlog jank) on the
// assumption that main keeps the bytes; without this buffer that restore
// always failed and surfaced the "main recovery was unavailable" warning.
pub struct PtyOutputBuffer {
    // Most-recent window of raw bytes, front-trimmed to MAIN_BUFFER_CAP_BYTES.
    pub bytes: Vec<u8>,
    // Every byte ever emitted, even past trims. The snapshot reports this as
    // `seq` so the renderer can drop live chunks already covered by the
    // snapshot and keep byte order across the trimmed window boundary.
    pub total: u64,
    pub cols: u16,
    pub rows: u16,
}

impl PtyOutputBuffer {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            bytes: Vec::new(),
            total: 0,
            cols,
            rows,
        }
    }

    // Append a raw output chunk, trim the retained window to the cap from the
    // front, and return the running byte total (the snapshot `seq`) through
    // this chunk. Trimming the front keeps the most recent bytes — the ones
    // that reconstruct the current screen — while `total` keeps counting so the
    // seq the renderer dedupes against stays monotonic across the trim.
    pub fn push(&mut self, chunk: &[u8]) -> u64 {
        self.bytes.extend_from_slice(chunk);
        self.total += chunk.len() as u64;
        if self.bytes.len() > MAIN_BUFFER_CAP_BYTES {
            let excess = self.bytes.len() - MAIN_BUFFER_CAP_BYTES;
            self.bytes.drain(0..excess);
        }
        self.total
    }
}

pub struct PtyHandle {
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    // Shared so the reader thread can wait() for the real exit code on EOF while
    // pty_kill / introspection still reach the child from the command handlers.
    pub child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    // Shared so the reader thread can append output while the snapshot/resize
    // command handlers read dimensions and bytes back out.
    pub output: Arc<Mutex<PtyOutputBuffer>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceState {
    pub workspace_root: Option<String>,
    pub active_project: Option<String>,
    pub active_session_id: Option<String>,
    pub healthy: bool,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            workspace_root: None,
            active_project: None,
            active_session_id: None,
            healthy: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct RuntimeStateData {
    pub workspace: WorkspaceState,
}

pub struct AppState {
    pub ptys: Arc<Mutex<HashMap<String, PtyHandle>>>,
    pub settings_db: Arc<Mutex<Connection>>,
    pub watchers: Arc<Mutex<HashMap<String, RecommendedWatcher>>>,
    pub runtime: Arc<Mutex<RuntimeStateData>>,
}

impl AppState {
    pub fn new() -> anyhow::Result<Self> {
        let base_dir = dirs::data_local_dir()
            .or_else(dirs::data_dir)
            .unwrap_or(std::env::current_dir().context("failed to resolve current directory")?);
        let app_dir = base_dir.join("Agentum");
        std::fs::create_dir_all(&app_dir).context("failed to create app data directory")?;

        let connection = Connection::open(app_dir.join("settings.sqlite3"))
            .context("failed to open settings database")?;
        connection
            .execute_batch(
                "                PRAGMA journal_mode = WAL;
                CREATE TABLE IF NOT EXISTS settings (
                  key TEXT PRIMARY KEY,
                  value TEXT NOT NULL
                );",
            )
            .context("failed to initialize settings database")?;

        Ok(Self {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            settings_db: Arc::new(Mutex::new(connection)),
            watchers: Arc::new(Mutex::new(HashMap::new())),
            runtime: Arc::new(Mutex::new(RuntimeStateData::default())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_under_cap_retains_all_bytes_and_counts_seq() {
        let mut buf = PtyOutputBuffer::new(80, 24);
        assert_eq!(buf.push(b"hello "), 6);
        assert_eq!(buf.push(b"world"), 11);
        assert_eq!(buf.bytes, b"hello world");
        assert_eq!(buf.total, 11);
    }

    #[test]
    fn push_over_cap_keeps_most_recent_window_but_seq_keeps_counting() {
        let mut buf = PtyOutputBuffer::new(80, 24);
        // Fill exactly to the cap, then push one more byte.
        buf.push(&vec![b'a'; MAIN_BUFFER_CAP_BYTES]);
        let seq = buf.push(b"Z");
        // The window is trimmed back to the cap, dropping the oldest byte and
        // keeping the newest ("Z") so the current screen reconstructs.
        assert_eq!(buf.bytes.len(), MAIN_BUFFER_CAP_BYTES);
        assert_eq!(buf.bytes.last(), Some(&b'Z'));
        // seq counts every byte ever pushed, even the trimmed one.
        assert_eq!(seq, MAIN_BUFFER_CAP_BYTES as u64 + 1);
        assert_eq!(buf.total, MAIN_BUFFER_CAP_BYTES as u64 + 1);
    }

    #[test]
    fn push_chunk_larger_than_cap_keeps_only_the_tail() {
        let mut buf = PtyOutputBuffer::new(80, 24);
        let mut chunk = vec![b'x'; MAIN_BUFFER_CAP_BYTES];
        chunk.extend_from_slice(b"TAIL");
        let seq = buf.push(&chunk);
        assert_eq!(buf.bytes.len(), MAIN_BUFFER_CAP_BYTES);
        assert!(buf.bytes.ends_with(b"TAIL"));
        assert_eq!(seq, (MAIN_BUFFER_CAP_BYTES + 4) as u64);
    }
}
