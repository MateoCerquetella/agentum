//! SQLite persistence for agentum. WAL mode, synchronous=NORMAL.
//!
//! All XDG path resolution lives in [`paths`].

use std::path::{Path, PathBuf};

use agentum_core::{BoardItem, NewSession, Session, Status, User};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

pub mod orchestration;
pub mod paths;

// Domain method blocks split out of this file for cohesion. Each adds an
// `impl Store` block (and its private row types) for one table/concern; Rust
// lets inherent impls span modules within the crate, and child modules can
// reach the crate-root `Store`'s private `pool` field.
mod board;
mod channels;
mod events;
mod hosts;
mod messages;
mod notes;
mod sessions;
mod settings;

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

    // ---------- card-session binding ----------

    /// Atomically create a session and bind it to a board card in one transaction.
    ///
    /// This is the **atomic** card-claim primitive — invoked from the PATCH→doing
    /// auto-spawn path (Phase 2 plan 03). The caller is responsible for resolving
    /// `tool` and `workdir` BEFORE calling (the helper does not implement Phase 2
    /// D-02's fall-through policy). This method is the symmetric inverse of
    /// `transfer_card_binding(card_id, None)` — claim creates AND binds in one
    /// transaction, whereas transfer rebinds-or-unbinds an existing session.
    ///
    /// Returns `(BoardItem, Session)` with both rows reflecting the committed state.
    /// Errors:
    /// - `NotFound` if the card does not exist.
    /// - `AlreadyExists` if the card already has a `session_id` (HTTP 409 via
    ///   existing `From<StoreError> for ApiError` impl).
    /// - `AlreadyExists` if the session name collides with an existing session name.
    pub async fn claim_card(
        &self,
        card_id: i64,
        mut new: NewSession,
    ) -> Result<(BoardItem, Session)> {
        agentum_core::validate_name(&new.name)?;

        let mut tx = self.pool.begin().await?;

        // Step 1: Check the card exists and is unbound.
        let row: Option<(i64, Option<String>)> =
            sqlx::query_as("SELECT id, session_id FROM board_items WHERE id = ?")
                .bind(card_id)
                .fetch_optional(&mut *tx)
                .await?;

        let (_, existing_sid) = match row {
            Some(r) => r,
            None => return Err(StoreError::NotFound(format!("board item {card_id}"))),
        };

        if let Some(sid) = existing_sid {
            return Err(StoreError::AlreadyExists(format!(
                "card {card_id} already bound to session {sid}"
            )));
        }

        // Step 2: Force the card binding on the new session.
        new.card_id = Some(card_id);

        let session_id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let flags = serde_json::to_string(&new.flags)?;
        let status = Status::Idle;

        // Step 3: INSERT the new session row.
        let res = sqlx::query(
            r#"INSERT INTO sessions
                (id, name, workdir, tool, model, flags, status, created_at, updated_at, card_id)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(session_id.to_string())
        .bind(&new.name)
        .bind(&new.workdir)
        .bind(&new.tool)
        .bind(&new.model)
        .bind(&flags)
        .bind(status.as_str())
        .bind(&now_s)
        .bind(&now_s)
        .bind(new.card_id)
        .execute(&mut *tx)
        .await;

        if let Err(sqlx::Error::Database(ref db)) = res {
            if db.is_unique_violation() {
                return Err(StoreError::AlreadyExists(new.name));
            }
        }
        res?;

        // Step 4: UPDATE the card's session_id.
        sqlx::query("UPDATE board_items SET session_id = ?, updated_at = ? WHERE id = ?")
            .bind(session_id.to_string())
            .bind(&now_s)
            .bind(card_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        // Step 5: Reload both rows from the committed state.
        let item = self
            .get_board_item(card_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("board item {card_id}")))?;
        let session = self
            .get_session_by_id(session_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(session_id.to_string()))?;

        Ok((item, session))
    }

    /// Atomically rebind or unbind a card's session link in one transaction.
    ///
    /// Implements the **3-step atomic transfer** from CONTEXT D-11:
    /// 1. Clear `card_id` on the old session (if any).
    /// 2. Set `card_id` on the new session (if `new_session_id` is `Some`).
    /// 3. Set `session_id` on the card to the new value (or NULL to unbind).
    ///
    /// The unbind branch (`new_session_id == None`) skips step 2.
    ///
    /// Returns `AlreadyExists` if `new_session_id` is already bound to a
    /// *different* card — the route layer maps that to HTTP 409.
    ///
    /// Note: the previous session's tmux pane is NOT touched by design (D-12:
    /// crash leaves binding intact; the user navigates to the dead pane).
    pub async fn transfer_card_binding(
        &self,
        card_id: i64,
        new_session_id: Option<Uuid>,
    ) -> Result<BoardItem> {
        let mut tx = self.pool.begin().await?;

        // Step 1: Fetch the card; capture its current session_id.
        let row: Option<(i64, Option<String>)> =
            sqlx::query_as("SELECT id, session_id FROM board_items WHERE id = ?")
                .bind(card_id)
                .fetch_optional(&mut *tx)
                .await?;

        let (_, old_sid_str) = match row {
            Some(r) => r,
            None => return Err(StoreError::NotFound(format!("board item {card_id}"))),
        };

        // Step 2: If rebinding, verify the target session exists and is free.
        if let Some(new) = new_session_id {
            let sess_row: Option<(String, Option<i64>)> =
                sqlx::query_as("SELECT id, card_id FROM sessions WHERE id = ?")
                    .bind(new.to_string())
                    .fetch_optional(&mut *tx)
                    .await?;

            match sess_row {
                None => {
                    return Err(StoreError::NotFound(format!("session {new}")));
                }
                Some((_, Some(existing_card))) if existing_card != card_id => {
                    return Err(StoreError::AlreadyExists(format!(
                        "session {new} already bound to card {existing_card}"
                    )));
                }
                _ => {}
            }
        }

        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;

        // Step 3: Clear the old session's card_id (if there was one).
        if let Some(ref old_sid) = old_sid_str {
            sqlx::query("UPDATE sessions SET card_id = NULL, updated_at = ? WHERE id = ?")
                .bind(&now_s)
                .bind(old_sid)
                .execute(&mut *tx)
                .await?;
        }

        // Step 4: Set the new session's card_id (if rebinding).
        if let Some(new) = new_session_id {
            sqlx::query("UPDATE sessions SET card_id = ?, updated_at = ? WHERE id = ?")
                .bind(card_id)
                .bind(&now_s)
                .bind(new.to_string())
                .execute(&mut *tx)
                .await?;
        }

        // Step 5: Update the card's session_id.
        sqlx::query("UPDATE board_items SET session_id = ?, updated_at = ? WHERE id = ?")
            .bind(new_session_id.map(|u| u.to_string()))
            .bind(&now_s)
            .bind(card_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        self.get_board_item(card_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("board item {card_id}")))
    }

    // ------------- users + auth sessions -------------

    pub async fn count_users(&self) -> Result<i64> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    pub async fn create_user(&self, username: &str, pw_hash: &str) -> Result<User> {
        let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, pw_hash, created_at) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(username)
        .bind(pw_hash)
        .bind(&now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                StoreError::AlreadyExists(username.to_string())
            }
            _ => StoreError::Sqlx(e),
        })?;
        Ok(User {
            id,
            username: username.to_string(),
            created_at: OffsetDateTime::parse(&now, &Rfc3339)?,
        })
    }

    pub async fn get_user_by_username(&self, username: &str) -> Result<Option<(User, String)>> {
        let row: Option<(i64, String, String, String)> = sqlx::query_as(
            "SELECT id, username, pw_hash, created_at FROM users WHERE username = ?",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some((id, username, pw_hash, created_at)) => Ok(Some((
                User {
                    id,
                    username,
                    created_at: OffsetDateTime::parse(&created_at, &Rfc3339)?,
                },
                pw_hash,
            ))),
            None => Ok(None),
        }
    }

    pub async fn get_user_by_id(&self, id: i64) -> Result<Option<User>> {
        let row: Option<(i64, String, String)> =
            sqlx::query_as("SELECT id, username, created_at FROM users WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        match row {
            Some((id, username, created_at)) => Ok(Some(User {
                id,
                username,
                created_at: OffsetDateTime::parse(&created_at, &Rfc3339)?,
            })),
            None => Ok(None),
        }
    }

    pub async fn create_auth_session(
        &self,
        user_id: i64,
        token: &str,
        ttl: time::Duration,
    ) -> Result<()> {
        let now = OffsetDateTime::now_utc();
        let expires = now + ttl;
        let now_s = now.format(&Rfc3339)?;
        let exp_s = expires.format(&Rfc3339)?;
        sqlx::query(
            "INSERT INTO auth_sessions (token, user_id, created_at, last_used_at, expires_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(token)
        .bind(user_id)
        .bind(&now_s)
        .bind(&now_s)
        .bind(&exp_s)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Look up the user behind a session token and bump `last_used_at`.
    /// Returns `None` for unknown tokens AND for expired ones — expired
    /// rows are deleted as a side effect so the table self-heals.
    ///
    /// `slide_ttl`, when `Some`, refreshes `expires_at` to `now + ttl` on
    /// each touch (sliding expiration). Use `None` for absolute expiry.
    pub async fn touch_auth_session(
        &self,
        token: &str,
        slide_ttl: Option<time::Duration>,
    ) -> Result<Option<User>> {
        let row: Option<(i64, Option<String>)> =
            sqlx::query_as("SELECT user_id, expires_at FROM auth_sessions WHERE token = ?")
                .bind(token)
                .fetch_optional(&self.pool)
                .await?;
        let Some((uid, expires_at)) = row else {
            return Ok(None);
        };

        // Treat NULL expires_at as "infinite" for forward compat (the migration
        // backfills, but a future cleanup might null it). The current default
        // is "if missing, accept" rather than reject — flip if you'd rather be
        // strict.
        let now = OffsetDateTime::now_utc();
        if let Some(exp_s) = expires_at.as_deref() {
            match OffsetDateTime::parse(exp_s, &Rfc3339) {
                Ok(exp) if exp <= now => {
                    let _ = sqlx::query("DELETE FROM auth_sessions WHERE token = ?")
                        .bind(token)
                        .execute(&self.pool)
                        .await;
                    return Ok(None);
                }
                Ok(_) => {}
                Err(_) => {
                    // Malformed timestamp — treat as expired and clean up.
                    let _ = sqlx::query("DELETE FROM auth_sessions WHERE token = ?")
                        .bind(token)
                        .execute(&self.pool)
                        .await;
                    return Ok(None);
                }
            }
        }

        let now_s = now.format(&Rfc3339)?;
        if let Some(ttl) = slide_ttl {
            let new_exp = (now + ttl).format(&Rfc3339)?;
            let _ = sqlx::query(
                "UPDATE auth_sessions SET last_used_at = ?, expires_at = ? WHERE token = ?",
            )
            .bind(&now_s)
            .bind(&new_exp)
            .bind(token)
            .execute(&self.pool)
            .await;
        } else {
            let _ = sqlx::query("UPDATE auth_sessions SET last_used_at = ? WHERE token = ?")
                .bind(&now_s)
                .bind(token)
                .execute(&self.pool)
                .await;
        }
        self.get_user_by_id(uid).await
    }

    pub async fn delete_auth_session(&self, token: &str) -> Result<()> {
        sqlx::query("DELETE FROM auth_sessions WHERE token = ?")
            .bind(token)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete every auth_session row whose `expires_at` is in the past.
    /// Returns the number of rows deleted. Cheap to call on a timer.
    pub async fn sweep_expired_auth_sessions(&self) -> Result<u64> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let res = sqlx::query(
            "DELETE FROM auth_sessions WHERE expires_at IS NOT NULL AND expires_at <= ?",
        )
        .bind(&now_s)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn list_users(&self) -> Result<Vec<User>> {
        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT id, username, created_at FROM users ORDER BY id")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|(id, username, created_at)| {
                Ok(User {
                    id,
                    username,
                    created_at: OffsetDateTime::parse(&created_at, &Rfc3339)?,
                })
            })
            .collect()
    }

    pub async fn delete_user_by_username(&self, username: &str) -> Result<bool> {
        let affected = sqlx::query("DELETE FROM users WHERE username = ?")
            .bind(username)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(affected > 0)
    }

    pub async fn wipe_users(&self) -> Result<()> {
        sqlx::query("DELETE FROM auth_sessions")
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM users").execute(&self.pool).await?;
        Ok(())
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
    // Domain constructors used by tests that share this module's
    // `tmp_store`/`make_session` helpers. The non-test `use agentum_core::{…}`
    // no longer pulls these in (their methods moved to the per-domain
    // submodules: notes/channels/messages/hosts/…), so import them here.
    use agentum_core::{
        BoardPatch, HostKind, LOCAL_HOST_ID, LinkKind, NewBoardComment, NewBoardItem, NewChannel,
        NewHost, NewMessage, NewNote, NotePatch, ReorderEntry, RequiredField, SshAuth,
    };

    async fn tmp_store() -> Store {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        // dir is dropped at end of test; sqlx pool keeps file alive only while open.
        // Leak the tempdir handle to keep it alive for the test duration.
        std::mem::forget(dir);
        Store::open(&p).await.unwrap()
    }

    #[tokio::test]
    async fn upsert_by_external_url_is_idempotent_and_updates_in_place() {
        let s = tmp_store().await;

        // First sync of an issue inserts a card carrying its external link.
        let first = s
            .upsert_board_item_by_external_url(
                "https://github.com/o/r/issues/7",
                Some("github"),
                "Add CSV export",
                Some("body v1"),
                "todo",
                Some("github"),
            )
            .await
            .unwrap();
        assert_eq!(
            first.external_url.as_deref(),
            Some("https://github.com/o/r/issues/7")
        );
        assert_eq!(first.external_provider.as_deref(), Some("github"));
        assert_eq!(first.title, "Add CSV export");

        // Re-syncing the SAME issue must update the same row, not duplicate it.
        let second = s
            .upsert_board_item_by_external_url(
                "https://github.com/o/r/issues/7",
                Some("github"),
                "Add CSV export (renamed)",
                Some("body v2"),
                "doing",
                Some("github"),
            )
            .await
            .unwrap();
        assert_eq!(second.id, first.id, "re-sync must hit the same card");
        assert_eq!(
            second.title, "Add CSV export (renamed)",
            "tracker wins on re-sync"
        );
        assert_eq!(second.status, "doing");

        // Exactly one card exists for that issue.
        let all = s.list_board_items().await.unwrap();
        let mirrors: Vec<_> = all
            .iter()
            .filter(|c| c.external_url.as_deref() == Some("https://github.com/o/r/issues/7"))
            .collect();
        assert_eq!(mirrors.len(), 1, "external sync must not duplicate cards");

        // A different issue is a distinct card.
        let other = s
            .upsert_board_item_by_external_url(
                "https://linear.app/t/issue/ABC-1",
                Some("linear"),
                "Linear thing",
                None,
                "todo",
                Some("linear"),
            )
            .await
            .unwrap();
        assert_ne!(other.id, first.id);
        assert_eq!(s.list_board_items().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn upsert_external_card_is_idempotent_on_re_sync() {
        let s = tmp_store().await;
        // First sync creates the card.
        let (a, created) = s
            .upsert_external_card(
                "github",
                "42",
                "Add login",
                Some("body"),
                "https://gh/o/r/issues/42",
                "todo",
                "2026-06-22T00:00:00Z",
            )
            .await
            .unwrap();
        assert!(created, "first upsert creates");
        assert_eq!(a.external_provider.as_deref(), Some("github"));
        assert_eq!(a.external_id.as_deref(), Some("42"));
        assert_eq!(a.status, "todo");

        // Re-syncing the same issue (edited, closed) updates in place.
        let (b, created2) = s
            .upsert_external_card(
                "github",
                "42",
                "Add login (v2)",
                Some("body2"),
                "https://gh/o/r/issues/42",
                "done",
                "2026-06-22T01:00:00Z",
            )
            .await
            .unwrap();
        assert!(!created2, "second upsert updates, not creates");
        assert_eq!(b.id, a.id, "same card id — no duplicate");
        assert_eq!(b.title, "Add login (v2)");
        assert_eq!(b.status, "done");
        assert_eq!(
            b.external_synced_at.as_deref(),
            Some("2026-06-22T01:00:00Z")
        );

        // Exactly one external card exists for the provider.
        let refs = s.list_external_refs("github").await.unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0], (a.id, "42".to_string(), "done".to_string()));
    }

    #[tokio::test]
    async fn tracker_binding_roundtrip_and_rebind() {
        let s = tmp_store().await;
        let b = s.create_tracker_binding("github", "o/r").await.unwrap();
        assert_eq!(b.provider, "github");
        assert_eq!(b.project, "o/r");

        // Re-binding the same repo refreshes the same row (no duplicate).
        let b2 = s.create_tracker_binding("github", "o/r").await.unwrap();
        assert_eq!(b2.id, b.id);
        assert_eq!(s.list_tracker_bindings().await.unwrap().len(), 1);

        // A different repo is a separate binding.
        let _c = s.create_tracker_binding("github", "o/other").await.unwrap();
        assert_eq!(s.list_tracker_bindings().await.unwrap().len(), 2);

        // Delete is idempotent-aware: second delete is NotFound.
        s.delete_tracker_binding(b.id).await.unwrap();
        assert_eq!(s.list_tracker_bindings().await.unwrap().len(), 1);
        assert!(
            s.delete_tracker_binding(b.id).await.is_err(),
            "second delete must be NotFound"
        );
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
    async fn board_create_and_claim_cas() {
        let s = tmp_store().await;
        let item = s
            .create_board_item(NewBoardItem {
                title: "ship phase 7".into(),
                body: Some("kanban + atomic claim".into()),
                status: None,
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();
        assert!(item.key.starts_with("AG-"));
        assert_eq!(item.status, "todo");
        assert!(item.claimed_by.is_none());

        // First claim wins.
        let won = s.claim_board_item(item.id, "actor-A").await.unwrap();
        assert!(won.is_some(), "first claim should succeed");
        assert_eq!(won.as_ref().unwrap().claimed_by.as_deref(), Some("actor-A"));

        // Second claim by anyone else loses.
        let lost = s.claim_board_item(item.id, "actor-B").await.unwrap();
        assert!(lost.is_none(), "second claim should be rejected");

        // Even the same actor can't re-claim.
        let again = s.claim_board_item(item.id, "actor-A").await.unwrap();
        assert!(again.is_none(), "re-claim by same actor should also fail");

        // Listing returns it.
        let all = s.list_board_items().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn board_release_is_cas_safe() {
        let s = tmp_store().await;
        let item = s
            .create_board_item(NewBoardItem {
                title: "release me".into(),
                body: None,
                status: None,
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();

        // Release on an unclaimed row is a no-op success — the user
        // pressed "unclaim" on something nobody held, no point 4xx-ing.
        let noop = s
            .release_board_item(item.id, "actor-A")
            .await
            .unwrap()
            .expect("unclaimed release should succeed");
        assert!(noop.claimed_by.is_none());

        // Claim, then a foreign actor's release should fail.
        s.claim_board_item(item.id, "actor-A").await.unwrap();
        let denied = s.release_board_item(item.id, "actor-B").await.unwrap();
        assert!(denied.is_none(), "foreign release must be rejected");

        // The holder can release.
        let released = s
            .release_board_item(item.id, "actor-A")
            .await
            .unwrap()
            .expect("holder release should succeed");
        assert!(released.claimed_by.is_none());

        // Admin override (empty actor) clears any claim.
        s.claim_board_item(item.id, "actor-C").await.unwrap();
        let admin = s
            .release_board_item(item.id, "")
            .await
            .unwrap()
            .expect("admin override should succeed");
        assert!(admin.claimed_by.is_none());
    }

    #[tokio::test]
    async fn board_patch_and_clear_body() {
        let s = tmp_store().await;
        let item = s
            .create_board_item(NewBoardItem {
                title: "x".into(),
                body: Some("orig".into()),
                status: None,
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();

        // status-only patch leaves body alone
        let patched = s
            .patch_board_item(
                item.id,
                BoardPatch {
                    status: Some("doing".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(patched.status, "doing");
        assert_eq!(patched.body.as_deref(), Some("orig"));

        // explicit body=null clears it
        let cleared = s
            .patch_board_item(
                item.id,
                BoardPatch {
                    body: Some(None),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(cleared.body.is_none());
    }

    #[tokio::test]
    async fn board_priority_orders_within_column() {
        let s = tmp_store().await;
        // Three rows in todo, created in this order.
        let a = s
            .create_board_item(NewBoardItem {
                title: "a".into(),
                body: None,
                status: None,
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: Some(10),
                parent_goal_id: None,
            })
            .await
            .unwrap();
        let b = s
            .create_board_item(NewBoardItem {
                title: "b".into(),
                body: None,
                status: None,
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: Some(0),
                parent_goal_id: None,
            })
            .await
            .unwrap();
        let c = s
            .create_board_item(NewBoardItem {
                title: "c".into(),
                body: None,
                status: None,
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: Some(5),
                parent_goal_id: None,
            })
            .await
            .unwrap();

        // listed sort: priority ASC within the same status.
        let all = s.list_board_items().await.unwrap();
        let titles: Vec<_> = all.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["b", "c", "a"]);

        // reorder rewrites priorities in one transaction.
        s.reorder_board_items(&[
            ReorderEntry {
                id: a.id,
                priority: 1,
            },
            ReorderEntry {
                id: b.id,
                priority: 2,
            },
            ReorderEntry {
                id: c.id,
                priority: 3,
            },
        ])
        .await
        .unwrap();
        let after = s.list_board_items().await.unwrap();
        let titles: Vec<_> = after.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn board_comments_roundtrip() {
        let s = tmp_store().await;
        let item = s
            .create_board_item(NewBoardItem {
                title: "with comments".into(),
                body: None,
                status: None,
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();

        let c1 = s
            .create_board_comment(
                item.id,
                NewBoardComment {
                    author: "actor-A".into(),
                    body: "first".into(),
                },
            )
            .await
            .unwrap();
        // small sleep to guarantee a distinct created_at ordering.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let c2 = s
            .create_board_comment(
                item.id,
                NewBoardComment {
                    author: "actor-B".into(),
                    body: "second".into(),
                },
            )
            .await
            .unwrap();

        let list = s.list_board_comments(item.id).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, c1.id);
        assert_eq!(list[1].id, c2.id);
        assert_eq!(list[1].author, "actor-B");

        let counts = s.count_board_comments().await.unwrap();
        assert_eq!(counts.get(&item.id).copied(), Some(2));

        // Empty author rejected; missing parent rejected.
        let bad = s
            .create_board_comment(
                item.id,
                NewBoardComment {
                    author: "".into(),
                    body: "x".into(),
                },
            )
            .await;
        assert!(bad.is_err());
        let orphan = s
            .create_board_comment(
                99_999,
                NewBoardComment {
                    author: "actor-Z".into(),
                    body: "x".into(),
                },
            )
            .await;
        assert!(orphan.is_err());
    }

    #[tokio::test]
    async fn has_board_comments_smoke() {
        let s = tmp_store().await;
        let item = s
            .create_board_item(NewBoardItem {
                title: "gate sentinel".into(),
                body: None,
                status: None,
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();

        // Empty thread => false.
        assert!(!s.has_board_comments(item.id).await.unwrap());

        // After a single insert => true. `LIMIT 1` should short-circuit
        // before scanning the whole index.
        s.create_board_comment(
            item.id,
            NewBoardComment {
                author: "actor-A".into(),
                body: "explains why we're closing this".into(),
            },
        )
        .await
        .unwrap();
        assert!(s.has_board_comments(item.id).await.unwrap());

        // Sibling row's comments don't bleed across — id-scoping is
        // what the `done` gate relies on.
        let other = s
            .create_board_item(NewBoardItem {
                title: "other".into(),
                body: None,
                status: None,
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();
        assert!(!s.has_board_comments(other.id).await.unwrap());
    }

    #[tokio::test]
    async fn board_workdir_and_model_roundtrip() {
        let s = tmp_store().await;
        let item = s
            .create_board_item(NewBoardItem {
                title: "wire kanban".into(),
                body: None,
                status: None,
                lbl: None,
                tool: Some("claude".into()),
                workdir: Some("/home/me/projects/foo".into()),
                model: Some("claude-opus-4-8".into()),
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();
        assert_eq!(item.workdir.as_deref(), Some("/home/me/projects/foo"));
        assert_eq!(item.model.as_deref(), Some("claude-opus-4-8"));

        // Re-listing carries them through the BoardItemRow → BoardItem
        // conversion — guards against a forgotten field mapping.
        let all = s.list_board_items().await.unwrap();
        assert_eq!(all[0].workdir.as_deref(), Some("/home/me/projects/foo"));
        assert_eq!(all[0].model.as_deref(), Some("claude-opus-4-8"));

        // Patch can swap workdir and clear model.
        let patched = s
            .patch_board_item(
                item.id,
                BoardPatch {
                    workdir: Some(Some("/home/me/projects/bar".into())),
                    model: Some(None),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(patched.workdir.as_deref(), Some("/home/me/projects/bar"));
        assert!(patched.model.is_none());
    }

    #[tokio::test]
    async fn board_column_rule_crud_smoke() {
        let s = tmp_store().await;

        // Empty DB: lookup returns None, list returns empty map.
        assert!(s.get_board_column_rule("doing").await.unwrap().is_none());
        assert!(s.list_board_column_rules().await.unwrap().is_empty());

        // Upsert a row, then read it back through both single + list.
        s.upsert_board_column_rule("doing", &[RequiredField::Title, RequiredField::Lbl])
            .await
            .unwrap();
        let one = s.get_board_column_rule("doing").await.unwrap().unwrap();
        assert_eq!(one, vec![RequiredField::Title, RequiredField::Lbl]);
        let map = s.list_board_column_rules().await.unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get("doing"),
            Some(&vec![RequiredField::Title, RequiredField::Lbl])
        );

        // Re-upsert overrides the previous value (PRIMARY KEY conflict).
        s.upsert_board_column_rule(
            "doing",
            &[
                RequiredField::Title,
                RequiredField::Lbl,
                RequiredField::Workdir,
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            s.get_board_column_rule("doing").await.unwrap().unwrap(),
            vec![
                RequiredField::Title,
                RequiredField::Lbl,
                RequiredField::Workdir
            ]
        );

        // Delete returns true once, false on the second call.
        assert!(s.delete_board_column_rule("doing").await.unwrap());
        assert!(!s.delete_board_column_rule("doing").await.unwrap());
        assert!(s.get_board_column_rule("doing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn board_column_rule_unknown_field_is_skipped() {
        // Forward-compat: an unknown wire string in the JSON column
        // gets skipped with a warning rather than failing the row.
        // Insert raw JSON via the pool to simulate a row written by a
        // future version that introduced a new variant we don't know.
        let s = tmp_store().await;
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        sqlx::query(
            "INSERT INTO board_column_rules (column_name, required_fields, updated_at)
             VALUES (?, ?, ?)",
        )
        .bind("review")
        .bind(r#"["title","totally_new_variant","lbl"]"#)
        .bind(&now_s)
        .execute(s.pool())
        .await
        .unwrap();

        let parsed = s.get_board_column_rule("review").await.unwrap().unwrap();
        assert_eq!(parsed, vec![RequiredField::Title, RequiredField::Lbl]);
    }

    #[tokio::test]
    async fn board_column_rule_empty_array_persists() {
        // Empty required_fields is a valid configuration (spec decision 3
        // — synonym for "no gate"). The row exists with `[]` and a
        // subsequent lookup returns Some(empty Vec), not None.
        let s = tmp_store().await;
        s.upsert_board_column_rule("doing", &[]).await.unwrap();
        let v = s.get_board_column_rule("doing").await.unwrap().unwrap();
        assert!(v.is_empty());
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

    /// End-to-end smoke for the Phase 1 orchestrator schema additions:
    /// - parent_goal_id column on board_items
    /// - board_links table with FK + ON DELETE CASCADE
    /// - add_board_link / list_children_of_goal / list_board_links_for_goal
    ///   / delete_board_link / max_child_status_rank
    #[tokio::test]
    async fn links_and_parent_round_trip() {
        let s = tmp_store().await;

        // 1. Create a goal card.
        let goal = s
            .create_board_item(NewBoardItem {
                title: "Goal: ship planner".into(),
                body: None,
                status: Some("todo".into()),
                lbl: Some("goal".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();
        assert!(goal.parent_goal_id.is_none());

        // 2. Create two child cards referencing the goal.
        let child1 = s
            .create_board_item(NewBoardItem {
                title: "Child 1".into(),
                body: None,
                status: None,
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: Some(goal.id),
            })
            .await
            .unwrap();
        assert_eq!(child1.parent_goal_id, Some(goal.id));

        let child2 = s
            .create_board_item(NewBoardItem {
                title: "Child 2".into(),
                body: None,
                status: None,
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: Some(goal.id),
            })
            .await
            .unwrap();

        // 3. Add a ParentOf link from goal → child1.
        let link_a = s
            .add_board_link(goal.id, child1.id, LinkKind::ParentOf)
            .await
            .unwrap();
        assert_eq!(link_a.from_card_id, goal.id);
        assert_eq!(link_a.to_card_id, child1.id);
        assert_eq!(link_a.kind, LinkKind::ParentOf);

        // 4. Add a Blocks link from child1 → child2.
        s.add_board_link(child1.id, child2.id, LinkKind::Blocks)
            .await
            .unwrap();

        // 5. Duplicate add_board_link returns AlreadyExists.
        let dup = s
            .add_board_link(goal.id, child1.id, LinkKind::ParentOf)
            .await;
        assert!(
            matches!(dup, Err(StoreError::AlreadyExists(_))),
            "duplicate edge must be rejected"
        );

        // 6. list_children_of_goal returns both children.
        let children = s.list_children_of_goal(goal.id).await.unwrap();
        assert_eq!(children.len(), 2);
        let child_ids: std::collections::HashSet<_> = children.iter().map(|c| c.id).collect();
        assert!(child_ids.contains(&child1.id));
        assert!(child_ids.contains(&child2.id));

        // 7. list_board_links_for_goal returns only goal → child1
        //    (child1 → child2 is a different from_card_id).
        let links = s.list_board_links_for_goal(goal.id).await.unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].to_card_id, child1.id);
        assert_eq!(links[0].kind, LinkKind::ParentOf);

        // 8. All children are still todo → max_child_status_rank == Some(0).
        let rank_before = s.max_child_status_rank(goal.id).await.unwrap();
        assert_eq!(rank_before, Some(0));

        // 9. Patch child1 to "doing"; max rank should advance to 1.
        s.patch_board_item(
            child1.id,
            BoardPatch {
                status: Some("doing".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let rank_after = s.max_child_status_rank(goal.id).await.unwrap();
        assert_eq!(rank_after, Some(1));

        // 10. Detach child1 from goal via double-Option None.
        let detached = s
            .patch_board_item(
                child1.id,
                BoardPatch {
                    parent_goal_id: Some(None),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(
            detached.parent_goal_id.is_none(),
            "detaching should clear parent_goal_id"
        );

        // After detach, only child2 remains as a child of goal.
        let children_after = s.list_children_of_goal(goal.id).await.unwrap();
        assert_eq!(children_after.len(), 1);
        assert_eq!(children_after[0].id, child2.id);

        // 11. delete_board_link: first call returns true, second false.
        let deleted = s
            .delete_board_link(goal.id, child1.id, LinkKind::ParentOf)
            .await
            .unwrap();
        assert!(deleted, "first delete should report row removed");
        let again = s
            .delete_board_link(goal.id, child1.id, LinkKind::ParentOf)
            .await
            .unwrap();
        assert!(!again, "second delete should report no-op");
    }

    #[tokio::test]
    async fn get_session_by_card_id_returns_some_then_none() {
        let s = tmp_store().await;

        // Create a goal board item to use as the card_id target.
        let goal = s
            .create_board_item(NewBoardItem {
                title: "goal: build auth".into(),
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
            .unwrap();

        // Create a session bound to that goal card.
        let sess = s
            .create_session(NewSession {
                name: "planner-auth".into(),
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

        // get_session_by_card_id must find the session by its card_id.
        let found = s.get_session_by_card_id(goal.id).await.unwrap();
        assert!(found.is_some(), "expected to find session by card_id");
        assert_eq!(found.unwrap().id, sess.id);

        // A card_id that no session references returns None, not an error.
        let missing = s.get_session_by_card_id(9999).await.unwrap();
        assert!(missing.is_none(), "unknown card_id must return None");
    }

    // ---------- claim_card tests ----------

    fn make_card_new_session(name: &str) -> NewSession {
        NewSession {
            name: name.into(),
            workdir: "/tmp".into(),
            tool: "claude".into(),
            model: None,
            flags: vec![],
            card_id: None,
            worktree_path: None,
            worktree_branch: None,
            worktree_base_ref: None,
        }
    }

    async fn make_card(s: &Store, title: &str) -> BoardItem {
        s.create_board_item(NewBoardItem {
            title: title.into(),
            body: None,
            status: None,
            lbl: None,
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

    /// Test 1 (happy path): claim_card on an unbound card atomically
    /// inserts a session, sets card.session_id and session.card_id.
    #[tokio::test]
    async fn claim_card_happy_path() {
        let s = tmp_store().await;
        let card = make_card(&s, "claim-me").await;

        let (item, session) = s
            .claim_card(card.id, make_card_new_session("planner-1"))
            .await
            .unwrap();

        // card.session_id = session uuid.
        assert_eq!(
            item.session_id.as_deref(),
            Some(session.id.to_string().as_str()),
            "card.session_id must equal the new session id"
        );
        // session.card_id = card id.
        assert_eq!(
            session.card_id,
            Some(card.id),
            "session.card_id must equal the card id"
        );

        // Reload both rows to confirm the commit survived.
        let reloaded_card = s.get_board_item(card.id).await.unwrap().unwrap();
        let reloaded_sess = s.get_session_by_id(session.id).await.unwrap().unwrap();
        assert_eq!(reloaded_card.session_id, item.session_id);
        assert_eq!(reloaded_sess.card_id, Some(card.id));
    }

    /// Test 2 (already-bound conflict): claim_card on a card whose
    /// session_id IS NOT NULL returns AlreadyExists. DB rows are unchanged.
    #[tokio::test]
    async fn claim_card_already_bound_returns_already_exists() {
        let s = tmp_store().await;
        let card = make_card(&s, "bound-card").await;

        // First claim succeeds.
        let (_, first_sess) = s
            .claim_card(card.id, make_card_new_session("planner-first"))
            .await
            .unwrap();

        // Second claim must fail.
        let err = s
            .claim_card(card.id, make_card_new_session("planner-second"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::AlreadyExists(_)),
            "expected AlreadyExists, got {err:?}"
        );

        // The card still points at the first session.
        let reloaded = s.get_board_item(card.id).await.unwrap().unwrap();
        assert_eq!(
            reloaded.session_id.as_deref(),
            Some(first_sess.id.to_string().as_str()),
            "card must still reference the first session"
        );
        // No second session was created.
        let all_sessions = s.list_sessions(None).await.unwrap();
        assert_eq!(all_sessions.len(), 1, "only one session must exist");
    }

    /// Test 3 (no such card): claim_card(99999, …) returns NotFound.
    /// No session row is created.
    #[tokio::test]
    async fn claim_card_no_such_card_returns_not_found() {
        let s = tmp_store().await;

        let err = s
            .claim_card(99999, make_card_new_session("orphan"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );

        // No sessions were created.
        let sessions = s.list_sessions(None).await.unwrap();
        assert!(sessions.is_empty(), "no session must be created");
    }

    /// Test 4 (atomic rollback): a name collision on the session INSERT
    /// rolls back the card UPDATE — card.session_id stays NULL.
    #[tokio::test]
    async fn claim_card_session_name_collision_rolls_back_card() {
        let s = tmp_store().await;

        // Pre-create a session with the colliding name.
        s.create_session(make_card_new_session("clash"))
            .await
            .unwrap();

        let card = make_card(&s, "atomic-card").await;

        let err = s
            .claim_card(card.id, make_card_new_session("clash"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::AlreadyExists(_)),
            "expected AlreadyExists on name collision, got {err:?}"
        );

        // The card must still be unbound.
        let reloaded = s.get_board_item(card.id).await.unwrap().unwrap();
        assert!(
            reloaded.session_id.is_none(),
            "card.session_id must remain NULL after rollback"
        );
    }

    // ---------- transfer_card_binding tests ----------

    /// Test 1 (unbind happy path): transfer_card_binding(card, None)
    /// clears both card.session_id and session.card_id in one tx.
    #[tokio::test]
    async fn transfer_card_binding_unbind_clears_both_sides() {
        let s = tmp_store().await;
        let card = make_card(&s, "unbind-me").await;

        // Bind via claim_card.
        let (_, sess) = s
            .claim_card(card.id, make_card_new_session("bound-sess"))
            .await
            .unwrap();

        // Unbind.
        let unbound = s.transfer_card_binding(card.id, None).await.unwrap();
        assert!(
            unbound.session_id.is_none(),
            "card.session_id must be NULL after unbind"
        );

        // Reload both sides.
        let reloaded_sess = s.get_session_by_id(sess.id).await.unwrap().unwrap();
        assert!(
            reloaded_sess.card_id.is_none(),
            "session.card_id must be NULL after unbind"
        );
    }

    /// Test 2 (rebind from A to B): card bound to session A, session B
    /// is unbound. After transfer: card → B, A.card_id = NULL, B.card_id = card.
    #[tokio::test]
    async fn transfer_card_binding_rebind_updates_all_three_rows() {
        let s = tmp_store().await;
        let card = make_card(&s, "rebind-me").await;

        // Bind via claim_card → session A.
        let (_, sess_a) = s
            .claim_card(card.id, make_card_new_session("sess-a"))
            .await
            .unwrap();

        // Create session B (unbound).
        let sess_b = s
            .create_session(make_card_new_session("sess-b"))
            .await
            .unwrap();

        // Rebind card to B.
        let rebound = s
            .transfer_card_binding(card.id, Some(sess_b.id))
            .await
            .unwrap();

        assert_eq!(
            rebound.session_id.as_deref(),
            Some(sess_b.id.to_string().as_str()),
            "card.session_id must point to sess-b"
        );

        let reload_a = s.get_session_by_id(sess_a.id).await.unwrap().unwrap();
        assert!(
            reload_a.card_id.is_none(),
            "sess-a.card_id must be NULL after rebind"
        );

        let reload_b = s.get_session_by_id(sess_b.id).await.unwrap().unwrap();
        assert_eq!(
            reload_b.card_id,
            Some(card.id),
            "sess-b.card_id must equal the card id"
        );
    }

    /// Test 3 (rebind to already-bound session): session B is already bound
    /// to card C. transfer returns AlreadyExists; all rows unchanged.
    #[tokio::test]
    async fn transfer_card_binding_conflict_on_already_bound_session() {
        let s = tmp_store().await;
        let card_c = make_card(&s, "card-c").await;
        let card_target = make_card(&s, "card-target").await;

        // Bind sess-b to card_c.
        let (_, sess_b) = s
            .claim_card(card_c.id, make_card_new_session("sess-b"))
            .await
            .unwrap();

        // Bind card_target to sess-a.
        let (_, sess_a) = s
            .claim_card(card_target.id, make_card_new_session("sess-a"))
            .await
            .unwrap();

        // Attempt to rebind card_target to sess-b (already bound to card_c).
        let err = s
            .transfer_card_binding(card_target.id, Some(sess_b.id))
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::AlreadyExists(_)),
            "expected AlreadyExists, got {err:?}"
        );

        // All rows must be unchanged.
        let reload_card_target = s.get_board_item(card_target.id).await.unwrap().unwrap();
        assert_eq!(
            reload_card_target.session_id.as_deref(),
            Some(sess_a.id.to_string().as_str()),
            "card_target must still reference sess-a"
        );
        let reload_card_c = s.get_board_item(card_c.id).await.unwrap().unwrap();
        assert_eq!(
            reload_card_c.session_id.as_deref(),
            Some(sess_b.id.to_string().as_str()),
            "card_c must still reference sess-b"
        );
    }

    /// Test 4 (no such card): transfer_card_binding(99999, Some(sid))
    /// returns NotFound.
    #[tokio::test]
    async fn transfer_card_binding_no_such_card_returns_not_found() {
        let s = tmp_store().await;
        let sess = s
            .create_session(make_card_new_session("orphan-sess"))
            .await
            .unwrap();

        let err = s
            .transfer_card_binding(99999, Some(sess.id))
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
    }

    /// Test 5 (no such session in rebind): transfer_card_binding(card, Some(bad_uuid))
    /// returns NotFound; no partial mutation.
    #[tokio::test]
    async fn transfer_card_binding_no_such_session_returns_not_found() {
        let s = tmp_store().await;
        let card = make_card(&s, "orphan-card").await;
        let phantom = Uuid::new_v4();

        let err = s
            .transfer_card_binding(card.id, Some(phantom))
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );

        // Card must still be unbound.
        let reload = s.get_board_item(card.id).await.unwrap().unwrap();
        assert!(reload.session_id.is_none(), "card must remain unbound");
    }

    /// Test 6 (unbind on already-unbound card): idempotent no-op.
    #[tokio::test]
    async fn transfer_card_binding_unbind_already_unbound_is_noop() {
        let s = tmp_store().await;
        let card = make_card(&s, "noop-card").await;

        let result = s.transfer_card_binding(card.id, None).await.unwrap();
        assert!(
            result.session_id.is_none(),
            "unbound card stays unbound after no-op transfer"
        );
    }

    // --- create_board_item dual-write (existing-session picker) -----

    /// `create_board_item` with `session_id = Some(_)` must dual-write:
    /// the card row gets `session_id`, AND the matching `sessions.card_id`
    /// gets the new card id, atomically in one tx.
    ///
    /// This is the contract the dashboard's existing-session picker
    /// relies on — without the reverse-leg UPDATE, the watchdog→comment
    /// bridge and the bound-session panel can't find the binding.
    #[tokio::test]
    async fn create_board_item_with_session_id_dual_writes_both_sides() {
        let s = tmp_store().await;

        // Seed an unbound session.
        let sess = s
            .create_session(NewSession {
                name: "existing-pane".into(),
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
        assert!(sess.card_id.is_none(), "freshly created session is unbound");

        // Create a card that pre-binds to it.
        let card = s
            .create_board_item(NewBoardItem {
                title: "attach me".into(),
                body: None,
                status: None,
                lbl: Some("feat".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: Some(sess.id.to_string()),
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();

        // Forward leg: card.session_id = session uuid.
        assert_eq!(
            card.session_id.as_deref(),
            Some(sess.id.to_string().as_str())
        );

        // Reverse leg: sessions.card_id = card.id (the bug this test pins
        // against — the previous implementation skipped this UPDATE).
        let reloaded = s.get_session_by_id(sess.id).await.unwrap().unwrap();
        assert_eq!(
            reloaded.card_id,
            Some(card.id),
            "sessions.card_id must equal the new card id (reverse leg of dual-write)"
        );
    }

    /// `create_board_item` with `session_id` pointing to a session that's
    /// already bound to another card returns AlreadyExists — preventing
    /// silent rebind-via-create. The card row must NOT be inserted (tx
    /// rolls back).
    #[tokio::test]
    async fn create_board_item_with_session_already_bound_returns_already_exists() {
        let s = tmp_store().await;

        let other_card = make_card(&s, "owner-card").await;
        let sess = s
            .create_session(NewSession {
                name: "owned-pane".into(),
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
        // Bind sess to other_card via the canonical helper.
        let _ = s
            .transfer_card_binding(other_card.id, Some(sess.id))
            .await
            .unwrap();

        // Now try to create a new card pointing at the same session.
        let err = s
            .create_board_item(NewBoardItem {
                title: "thief".into(),
                body: None,
                status: None,
                lbl: Some("feat".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: Some(sess.id.to_string()),
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::AlreadyExists(_)),
            "expected AlreadyExists, got {err:?}"
        );

        // The original binding is intact.
        let reloaded = s.get_session_by_id(sess.id).await.unwrap().unwrap();
        assert_eq!(reloaded.card_id, Some(other_card.id));
    }

    /// `create_board_item` with `session_id` pointing to a non-existent
    /// session returns NotFound. No card row is created (tx rolls back).
    #[tokio::test]
    async fn create_board_item_with_unknown_session_id_returns_not_found() {
        let s = tmp_store().await;
        let missing = Uuid::new_v4();
        let err = s
            .create_board_item(NewBoardItem {
                title: "ghost".into(),
                body: None,
                status: None,
                lbl: Some("feat".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: Some(missing.to_string()),
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
    }
}
