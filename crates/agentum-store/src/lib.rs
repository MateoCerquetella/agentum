//! SQLite persistence for agentum. WAL mode, synchronous=NORMAL.
//!
//! All XDG path resolution lives in [`paths`].

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

pub mod orchestration;
pub mod paths;
pub mod sdd;
pub mod sdd_browser_evidence;
pub mod sdd_delivery;
pub mod sdd_integrations;
pub mod sdd_remote_projection;
pub mod sdd_remote_worker;
pub mod sdd_runtime;

// Domain method blocks split out of this file for cohesion. Each adds an
// `impl Store` block (and its private row types) for one table/concern; Rust
// lets inherent impls span modules within the crate, and child modules can
// reach the crate-root `Store`'s private `pool` field.
mod channels;
mod events;
mod hosts;
mod messages;
mod notes;
mod project_trackers;
pub use project_trackers::ProjectTrackerWrite;
mod sessions;
mod settings;
mod users;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    TimeFormat(#[from] time::error::Format),
    #[error(transparent)]
    TimeParse(#[from] time::error::Parse),
    #[error(transparent)]
    Uuid(#[from] uuid::Error),
    #[error(transparent)]
    Core(#[from] agentum_core::CoreError),
    #[error(transparent)]
    Path(#[from] paths::PathError),
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("session already exists: {0}")]
    AlreadyExists(String),
    #[error("stale aggregate revision: expected {expected}, current {current}")]
    StaleRevision { expected: i64, current: i64 },
    #[error("approval is no longer pending or its digest does not match")]
    ApprovalInvalid,
    #[error("an artifact author cannot approve their own work")]
    SelfApproval,
    #[error("invalid command: {0}")]
    InvalidCommand(String),
    #[error("idempotency key was already used for a different request in scope {0}")]
    IdempotencyConflict(String),
    #[error("repository artifact manifest conflicts with its registered identity: {0}")]
    ArtifactSetConflict(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Open (or create) a database at the given file path and run pending migrations.
    pub async fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let opts = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        // The DB holds Argon2id password hashes and live bearer tokens.
        // Force 0600 on the file + WAL/SHM sidecars so a permissive umask
        // doesn't leak them to other local accounts.
        restrict_db_perms(db_path);

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// Best-effort 0600 on the SQLite file and its WAL/SHM sidecars. Logs a
/// warning on failure rather than aborting boot — on weird filesystems
/// (NFS, FAT) chmod may fail but the server should still come up.
#[cfg(unix)]
fn restrict_db_perms(db_path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let candidates = [
        db_path.to_path_buf(),
        db_path.with_extension(
            db_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!("{e}-wal"))
                .unwrap_or_else(|| "sqlite-wal".to_string()),
        ),
        db_path.with_extension(
            db_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!("{e}-shm"))
                .unwrap_or_else(|| "sqlite-shm".to_string()),
        ),
    ];
    for p in &candidates {
        if !p.exists() {
            continue;
        }
        match std::fs::metadata(p) {
            Ok(m) => {
                let mut perm = m.permissions();
                perm.set_mode(0o600);
                if let Err(e) = std::fs::set_permissions(p, perm) {
                    tracing::warn!(path = %p.display(), error = %e, "could not chmod 0600");
                }
            }
            Err(e) => {
                tracing::warn!(path = %p.display(), error = %e, "stat failed");
            }
        }
    }
}

#[cfg(not(unix))]
fn restrict_db_perms(_db_path: &Path) {}

/// Convenience: open the store at the canonical XDG data path.
pub async fn open_default() -> Result<(Store, PathBuf)> {
    let p = paths::data_dir()?.join("db.sqlite");
    let store = Store::open(&p).await?;
    Ok((store, p))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Domain types these tests construct. Every `Store` method now lives in a
    // per-domain submodule, so this central test module pulls in the fixtures it
    // needs directly.
    use agentum_core::{
        HostKind, LOCAL_HOST_ID, NewChannel, NewHost, NewMessage, NewNote, NewSession, NotePatch,
        Session, SshAuth, Status,
    };
    use time::OffsetDateTime;
    use uuid::Uuid;

    async fn tmp_store() -> Store {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        // dir is dropped at end of test; sqlx pool keeps file alive only while open.
        // Leak the tempdir handle to keep it alive for the test duration.
        std::mem::forget(dir);
        Store::open(&p).await.unwrap()
    }

    #[tokio::test]
    async fn settings_roundtrip_and_bool_default() {
        let s = tmp_store().await;
        // Unset key → None, and the bool reader returns the caller's default.
        assert_eq!(s.setting_get("orchestration.enabled").await.unwrap(), None);
        assert!(
            !s.setting_get_bool("orchestration.enabled", false)
                .await
                .unwrap()
        );
        assert!(
            s.setting_get_bool("orchestration.enabled", true)
                .await
                .unwrap()
        );

        // Set true, then flip false — upsert overwrites in place.
        s.setting_set_bool("orchestration.enabled", true)
            .await
            .unwrap();
        assert_eq!(
            s.setting_get("orchestration.enabled")
                .await
                .unwrap()
                .as_deref(),
            Some("1")
        );
        assert!(
            s.setting_get_bool("orchestration.enabled", false)
                .await
                .unwrap()
        );

        s.setting_set_bool("orchestration.enabled", false)
            .await
            .unwrap();
        assert!(
            !s.setting_get_bool("orchestration.enabled", true)
                .await
                .unwrap()
        );
    }

    /// Historical internal-board tables remain as inert compatibility storage:
    /// opening the current store and doing ordinary session work must neither
    /// reject nor rewrite rows created by older Agentum versions.
    #[tokio::test]
    async fn legacy_board_rows_survive_reopen_and_normal_store_work_is_inert() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy-board.sqlite");
        let store = Store::open(&path).await.unwrap();

        sqlx::query(
            "INSERT INTO board_items
             (key, title, body, status, claimed_by, created_at, updated_at, lbl, tool,
              workdir, model, session_id, priority, parent_goal_id, external_url,
              external_provider, external_id, external_synced_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("AG-legacy")
        .bind("Retained legacy card")
        .bind("historical body")
        .bind("doing")
        .bind("old-agent")
        .bind("2024-01-01T00:00:00Z")
        .bind("2024-01-02T00:00:00Z")
        .bind("feat")
        .bind("claude")
        .bind("/tmp/legacy")
        .bind("legacy-model")
        .bind(Option::<String>::None)
        .bind(7_i64)
        .bind(Option::<i64>::None)
        .bind("https://example.test/issues/7")
        .bind("github")
        .bind("7")
        .bind("2024-01-02T00:00:00Z")
        .execute(store.pool())
        .await
        .unwrap();

        let snapshot = legacy_board_snapshot(&store).await;
        store.pool().close().await;

        let reopened = Store::open(&path).await.unwrap();
        reopened
            .create_session(NewSession {
                name: "normal-session".into(),
                workdir: "/tmp".into(),
                tool: "codex".into(),
                model: None,
                flags: vec![],
                card_id: None,
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await
            .unwrap();

        assert_eq!(legacy_board_snapshot(&reopened).await, snapshot);
    }

    async fn legacy_board_snapshot(store: &Store) -> String {
        sqlx::query_scalar(
            "SELECT json_object(
                'id', id, 'key', key, 'title', title, 'body', body, 'status', status,
                'claimed_by', claimed_by, 'created_at', created_at, 'updated_at', updated_at,
                'lbl', lbl, 'tool', tool, 'workdir', workdir, 'model', model,
                'session_id', session_id, 'priority', priority, 'parent_goal_id', parent_goal_id,
                'external_url', external_url, 'external_provider', external_provider,
                'external_id', external_id, 'external_synced_at', external_synced_at)
             FROM board_items WHERE key = 'AG-legacy'",
        )
        .fetch_one(store.pool())
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn create_and_list() {
        let s = tmp_store().await;
        let sess = s
            .create_session(NewSession {
                name: "alpha".into(),
                workdir: "/tmp".into(),
                tool: "claude".into(),
                model: None,
                flags: vec!["--foo".into()],
                card_id: None,
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await
            .unwrap();
        assert_eq!(sess.status, Status::Idle);

        let all = s.list_sessions(None).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "alpha");
        assert_eq!(all[0].flags, vec!["--foo"]);
    }

    #[tokio::test]
    async fn unique_name() {
        let s = tmp_store().await;
        let new = NewSession {
            name: "dup".into(),
            workdir: "/tmp".into(),
            tool: "claude".into(),
            model: None,
            flags: vec![],
            card_id: None,
            worktree_path: None,
            worktree_branch: None,
            worktree_base_ref: None,
        };
        s.create_session(new.clone()).await.unwrap();
        let err = s.create_session(new).await.unwrap_err();
        assert!(matches!(err, StoreError::AlreadyExists(_)));
    }

    #[tokio::test]
    async fn update_host_rewrites_fields_in_place() {
        let s = tmp_store().await;
        let created = s
            .create_host(NewHost {
                name: "box".into(),
                kind: HostKind::Ssh {
                    user: "me".into(),
                    hostname: "old.local".into(),
                    port: 22,
                    auth: SshAuth::Agent,
                },
            })
            .await
            .unwrap();

        // Edit every connection field, including a switch to password auth.
        let updated = s
            .update_host(
                created.id,
                NewHost {
                    name: "box-renamed".into(),
                    kind: HostKind::Ssh {
                        user: "root".into(),
                        hostname: "new.local".into(),
                        port: 2222,
                        auth: SshAuth::Password {
                            password: "hunter2".into(),
                        },
                    },
                },
            )
            .await
            .unwrap();

        // Same row (id + created_at preserved); fields swapped.
        assert_eq!(updated.id, created.id);
        assert_eq!(updated.created_at, created.created_at);
        assert_eq!(updated.name, "box-renamed");
        assert_eq!(
            updated.kind,
            HostKind::Ssh {
                user: "root".into(),
                hostname: "new.local".into(),
                port: 2222,
                auth: SshAuth::Password {
                    password: "hunter2".into(),
                },
            }
        );

        // Persisted, not just returned.
        let reloaded = s.get_host(created.id).await.unwrap().unwrap();
        assert_eq!(reloaded.name, "box-renamed");
        assert_eq!(reloaded.kind, updated.kind);
    }

    #[tokio::test]
    async fn update_host_rename_collision_is_conflict() {
        let s = tmp_store().await;
        s.create_host(NewHost {
            name: "alpha".into(),
            kind: HostKind::Ssh {
                user: "me".into(),
                hostname: "a.local".into(),
                port: 22,
                auth: SshAuth::Agent,
            },
        })
        .await
        .unwrap();
        let beta = s
            .create_host(NewHost {
                name: "beta".into(),
                kind: HostKind::Ssh {
                    user: "me".into(),
                    hostname: "b.local".into(),
                    port: 22,
                    auth: SshAuth::Agent,
                },
            })
            .await
            .unwrap();

        // Renaming beta → alpha collides with the existing host name.
        let err = s
            .update_host(
                beta.id,
                NewHost {
                    name: "alpha".into(),
                    kind: HostKind::Ssh {
                        user: "me".into(),
                        hostname: "b.local".into(),
                        port: 22,
                        auth: SshAuth::Agent,
                    },
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, StoreError::AlreadyExists(_)));
    }

    #[tokio::test]
    async fn update_host_unknown_id_is_not_found() {
        let s = tmp_store().await;
        let err = s
            .update_host(
                Uuid::new_v4(),
                NewHost {
                    name: "ghost".into(),
                    kind: HostKind::Ssh {
                        user: "me".into(),
                        hostname: "g.local".into(),
                        port: 22,
                        auth: SshAuth::Agent,
                    },
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn update_host_rejects_local_host() {
        let s = tmp_store().await;
        // The local pseudo-host is immutable; editing it is "no such
        // editable host" (NotFound), mirroring delete_host's guard.
        let err = s
            .update_host(
                LOCAL_HOST_ID,
                NewHost {
                    name: "nope".into(),
                    kind: HostKind::Ssh {
                        user: "me".into(),
                        hostname: "x.local".into(),
                        port: 22,
                        auth: SshAuth::Agent,
                    },
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn note_crud_and_patch_persists() {
        let s = tmp_store().await;
        let note = s
            .create_note(NewNote {
                title: "spec".into(),
                body: "hello".into(),
                tags: vec!["draft".into()],
            })
            .await
            .unwrap();
        assert_eq!(note.tags, vec!["draft"]);

        let updated = s
            .patch_note(
                note.id,
                NotePatch {
                    body: Some("hello world".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.body, "hello world");
        assert_eq!(updated.title, "spec"); // untouched
        assert_eq!(updated.tags, vec!["draft"]); // untouched

        // Reload to confirm persistence.
        let again = s.get_note(note.id).await.unwrap().unwrap();
        assert_eq!(again.body, "hello world");
    }

    async fn make_session(s: &Store, name: &str) -> Session {
        s.create_session(NewSession {
            name: name.into(),
            workdir: "/tmp".into(),
            tool: "bash".into(),
            model: None,
            flags: vec![],
            card_id: None,
            worktree_path: None,
            worktree_branch: None,
            worktree_base_ref: None,
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn channel_dedupes_and_sorts_pair() {
        let s = tmp_store().await;
        let sa = make_session(&s, "alpha").await;
        let sb = make_session(&s, "beta").await;
        let (lo, hi) = if sa.id < sb.id {
            (sa.id, sb.id)
        } else {
            (sb.id, sa.id)
        };

        let ch = s
            .create_channel(NewChannel {
                a_session: sa.id,
                b_session: sb.id,
            })
            .await
            .unwrap();
        assert_eq!(ch.a_session, lo);
        assert_eq!(ch.b_session, hi);

        // Reverse order should collide on the unique pair.
        let dup = s
            .create_channel(NewChannel {
                a_session: sb.id,
                b_session: sa.id,
            })
            .await;
        assert!(matches!(dup, Err(StoreError::AlreadyExists(_))));
    }

    #[tokio::test]
    async fn messages_appended_and_listed_oldest_first() {
        let s = tmp_store().await;
        let sa = make_session(&s, "ma").await;
        let sb = make_session(&s, "mb").await;
        let ch = s
            .create_channel(NewChannel {
                a_session: sa.id,
                b_session: sb.id,
            })
            .await
            .unwrap();
        s.append_message(
            ch.id,
            NewMessage {
                sender: "a".into(),
                body: "first".into(),
            },
        )
        .await
        .unwrap();
        s.append_message(
            ch.id,
            NewMessage {
                sender: "b".into(),
                body: "second".into(),
            },
        )
        .await
        .unwrap();
        let list = s.list_messages(ch.id, 10).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].body, "first");
        assert_eq!(list[1].body, "second");
    }

    #[tokio::test]
    async fn update_status_flow() {
        let s = tmp_store().await;
        let sess = s
            .create_session(NewSession {
                name: "flow".into(),
                workdir: "/tmp".into(),
                tool: "claude".into(),
                model: None,
                flags: vec![],
                card_id: None,
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await
            .unwrap();
        s.update_status(sess.id, Status::Running).await.unwrap();
        let got = s.get_session_by_id(sess.id).await.unwrap().unwrap();
        assert_eq!(got.status, Status::Running);
    }

    #[tokio::test]
    async fn provisioned_endpoint_round_trips_and_flag_toggles() {
        // S3: the migration-0023 columns persist + read back, the setter clears
        // the flag, and the flag setter raises it. Also implicitly verifies the
        // migration applies cleanly (tmp_store runs all migrations on open).
        let s = tmp_store().await;
        let sess = make_session(&s, "drift").await;

        // Fresh session: nothing recorded, not flagged.
        assert_eq!(sess.provisioned_api_base, None);
        assert_eq!(sess.provisioned_token_hash, None);
        assert!(!sess.provisioned_needs_reconnect);

        // Record an endpoint → reads back; flag stays clear.
        s.set_session_provisioned(sess.id, Some("http://127.0.0.1:8822"), Some("hashA"))
            .await
            .unwrap();
        let got = s.get_session_by_id(sess.id).await.unwrap().unwrap();
        assert_eq!(
            got.provisioned_api_base.as_deref(),
            Some("http://127.0.0.1:8822")
        );
        assert_eq!(got.provisioned_token_hash.as_deref(), Some("hashA"));
        assert!(!got.provisioned_needs_reconnect);

        // Flag it → reads back true, recorded endpoint untouched.
        s.flag_session_needs_reconnect(sess.id).await.unwrap();
        let got = s.get_session_by_id(sess.id).await.unwrap().unwrap();
        assert!(got.provisioned_needs_reconnect);
        assert_eq!(
            got.provisioned_api_base.as_deref(),
            Some("http://127.0.0.1:8822")
        );

        // Re-provisioning to a new endpoint clears the flag (the session is current
        // again) and overwrites base+hash.
        s.set_session_provisioned(sess.id, Some("http://127.0.0.1:60102"), Some("hashB"))
            .await
            .unwrap();
        let got = s.get_session_by_id(sess.id).await.unwrap().unwrap();
        assert_eq!(
            got.provisioned_api_base.as_deref(),
            Some("http://127.0.0.1:60102")
        );
        assert_eq!(got.provisioned_token_hash.as_deref(), Some("hashB"));
        assert!(!got.provisioned_needs_reconnect);

        // Both methods are NotFound on a missing row.
        let ghost = Uuid::new_v4();
        assert!(matches!(
            s.set_session_provisioned(ghost, None, None).await,
            Err(StoreError::NotFound(_))
        ));
        assert!(matches!(
            s.flag_session_needs_reconnect(ghost).await,
            Err(StoreError::NotFound(_))
        ));
    }

    /// `prune_events` drops aged history but must never drop a session's
    /// newest `agent.*` row — that row seeds the cold-start activity overlay
    /// for clients connecting to /api/events, however old it is.
    #[tokio::test]
    async fn prune_events_keeps_latest_agent_row_per_session() {
        let s = tmp_store().await;
        let sess = make_session(&s, "prune-events").await;

        let old = |days: i64, kind: &str| agentum_core::Event {
            kind: kind.into(),
            session_id: Some(sess.id),
            session_name: None,
            payload: serde_json::json!({}),
            ts: OffsetDateTime::now_utc() - time::Duration::days(days),
        };
        // Two aged agent rows: only the NEWER of the two is protected.
        s.insert_event(&old(40, "agent.working")).await.unwrap();
        s.insert_event(&old(35, "agent.finished")).await.unwrap();
        // Aged non-agent history: prunable.
        s.insert_event(&old(40, "watchdog.compact")).await.unwrap();
        // Fresh row: inside the window, untouched.
        s.insert_event(&old(0, "session.started")).await.unwrap();

        let pruned = s.prune_events(30).await.unwrap();
        assert_eq!(pruned, 2, "the stale agent.working + watchdog rows");

        // The overlay still knows the session's last agent state.
        let latest = s.latest_agent_event_per_session().await.unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].kind, "agent.finished");

        // The watchdog feed keeps only the fresh row.
        let feed = s.list_watchdog_events(50).await.unwrap();
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].kind, "session.started");
    }
}
