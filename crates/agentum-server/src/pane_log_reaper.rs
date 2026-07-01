//! Boot-time reaper for orphaned pane logs.
//!
//! `tmux pipe-pane` appends every session's raw output stream to
//! `cache_dir/sessions/<uuid>.log` for the life of the session (see
//! [`agentum_store::paths::pane_log`]), and nothing ever deleted them — a
//! long-lived install accumulates unbounded cache from sessions deleted
//! months ago (a ratatui agent redraw stream runs to hundreds of MB).
//!
//! Deliberately conservative — this must never touch a log the daemon still
//! cares about:
//! - only files whose stem is a well-formed UUID with NO session row at all
//!   (a stopped-but-present session may be restarted, re-arming the same
//!   path; live logs are tailed by the streaming path),
//! - only files untouched for [`MIN_ORPHAN_AGE`] (a session mid-create has a
//!   log on disk moments before/after its DB row lands — mtime age removes
//!   that race entirely),
//! - and never when the session listing itself failed (absence must be
//!   proven, not assumed).

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

/// A log must be this stale (mtime) before it is eligible. Generous on
/// purpose: the reaper runs once per boot, so there is no rush, and a live
/// session's log is written continuously — an hour of silence plus no DB row
/// means the session is truly gone.
const MIN_ORPHAN_AGE: Duration = Duration::from_secs(60 * 60);

/// Resolve the log dir + known session ids, then sweep on a blocking thread.
/// Call once from `spawn_background_workers`.
pub(crate) async fn reap_orphan_pane_logs(store: std::sync::Arc<agentum_store::Store>) {
    let Ok(dir) = agentum_store::paths::pane_log_dir() else {
        return;
    };
    let sessions = match store.list_sessions(None).await {
        Ok(sessions) => sessions,
        Err(e) => {
            tracing::warn!(error = ?e, "pane-log reaper: session listing failed; skipping sweep");
            return;
        }
    };
    let known: HashSet<String> = sessions.iter().map(|s| s.id.to_string()).collect();
    let swept =
        tokio::task::spawn_blocking(move || reap_orphan_pane_logs_in(&dir, &known, MIN_ORPHAN_AGE))
            .await
            .unwrap_or((0, 0));
    if swept.0 > 0 {
        tracing::info!(
            files = swept.0,
            bytes = swept.1,
            "reaped orphaned pane logs from cache"
        );
    }
}

/// The sweep itself, factored pure-ish (explicit dir/ids/age) so it is unit
/// testable without `AGENTUM_HOME` or a live store. Returns (files, bytes)
/// removed. Non-UUID and non-`.log` entries are never touched — the dir is
/// ours, but a stray foreign file shouldn't be collateral.
fn reap_orphan_pane_logs_in(dir: &Path, known: &HashSet<String>, min_age: Duration) -> (u64, u64) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // Missing dir = nothing ever logged here; nothing to do.
        Err(_) => return (0, 0),
    };
    let now = std::time::SystemTime::now();
    let mut files = 0u64;
    let mut bytes = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if uuid::Uuid::parse_str(stem).is_err() || known.contains(stem) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let stale = meta
            .modified()
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age >= min_age);
        if !stale {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            files += 1;
            bytes += meta.len();
        }
    }
    (files, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_log(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn reaps_only_stale_unknown_uuid_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let known_id = uuid::Uuid::new_v4().to_string();
        let orphan_id = uuid::Uuid::new_v4().to_string();
        let known_log = write_log(dir, &format!("{known_id}.log"), "keep: session exists");
        let orphan_log = write_log(dir, &format!("{orphan_id}.log"), "reap: no session row");
        let foreign = write_log(dir, "not-a-uuid.log", "keep: not ours to judge");
        let non_log = write_log(dir, &format!("{orphan_id}.txt"), "keep: wrong extension");

        let known: HashSet<String> = [known_id].into_iter().collect();

        // With a min-age they're all too fresh — nothing reaped (the
        // mid-create race guard).
        let (files, _) = reap_orphan_pane_logs_in(dir, &known, Duration::from_secs(3600));
        assert_eq!(files, 0);
        assert!(orphan_log.exists());

        // Zero min-age: only the stale-eligible orphan goes.
        let (files, bytes) = reap_orphan_pane_logs_in(dir, &known, Duration::ZERO);
        assert_eq!(files, 1);
        assert!(bytes > 0);
        assert!(!orphan_log.exists(), "orphan must be removed");
        assert!(known_log.exists(), "known session's log must survive");
        assert!(foreign.exists(), "non-UUID files must survive");
        assert!(non_log.exists(), "non-.log files must survive");
    }

    #[test]
    fn missing_dir_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("never-created");
        let (files, bytes) = reap_orphan_pane_logs_in(&missing, &HashSet::new(), Duration::ZERO);
        assert_eq!((files, bytes), (0, 0));
    }
}
