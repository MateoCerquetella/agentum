//! SQLite persistence for agentum. WAL mode, synchronous=NORMAL.
//!
//! All XDG path resolution lives in [`paths`].

use std::path::{Path, PathBuf};
use std::str::FromStr;

use agentum_core::{
    BoardItem, BoardPatch, Channel, Event, Message, NewBoardItem, NewChannel, NewMessage, NewNote,
    NewSession, Note, NotePatch, Session, Status, User,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{FromRow, SqlitePool};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

pub mod paths;

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

    pub async fn create_session(&self, new: NewSession) -> Result<Session> {
        agentum_core::validate_name(&new.name)?;

        let id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let flags = serde_json::to_string(&new.flags)?;
        let status = Status::Idle;

        let res = sqlx::query(
            r#"
            INSERT INTO sessions
                (id, name, workdir, tool, model, flags, status, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(id.to_string())
        .bind(&new.name)
        .bind(&new.workdir)
        .bind(&new.tool)
        .bind(&new.model)
        .bind(&flags)
        .bind(status.as_str())
        .bind(&now_s)
        .bind(&now_s)
        .execute(&self.pool)
        .await;

        if let Err(sqlx::Error::Database(db)) = &res {
            if db.is_unique_violation() {
                return Err(StoreError::AlreadyExists(new.name));
            }
        }
        res?;

        Ok(Session {
            id,
            name: new.name,
            workdir: new.workdir,
            tool: new.tool,
            model: new.model,
            flags: new.flags,
            status,
            tmux_target: None,
            created_at: now,
            updated_at: now,
            last_activity_at: None,
        })
    }

    pub async fn list_sessions(&self, status: Option<Status>) -> Result<Vec<Session>> {
        let rows: Vec<SessionRow> = match status {
            Some(s) => {
                sqlx::query_as::<_, SessionRow>(
                    "SELECT * FROM sessions WHERE status = ? ORDER BY created_at DESC",
                )
                .bind(s.as_str())
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, SessionRow>("SELECT * FROM sessions ORDER BY created_at DESC")
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        rows.into_iter().map(Session::try_from).collect()
    }

    pub async fn get_session_by_name(&self, name: &str) -> Result<Option<Session>> {
        let row = sqlx::query_as::<_, SessionRow>("SELECT * FROM sessions WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        row.map(Session::try_from).transpose()
    }

    pub async fn get_session_by_id(&self, id: Uuid) -> Result<Option<Session>> {
        let row = sqlx::query_as::<_, SessionRow>("SELECT * FROM sessions WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(Session::try_from).transpose()
    }

    pub async fn update_status(&self, id: Uuid, status: Status) -> Result<()> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let affected = sqlx::query("UPDATE sessions SET status = ?, updated_at = ? WHERE id = ?")
            .bind(status.as_str())
            .bind(now_s)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Atomically flip status + tmux_target (use `None` to clear).
    pub async fn update_status_and_target(
        &self,
        id: Uuid,
        status: Status,
        tmux_target: Option<&str>,
    ) -> Result<()> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let affected = sqlx::query(
            "UPDATE sessions SET status = ?, tmux_target = ?, updated_at = ? WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(tmux_target)
        .bind(now_s)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Patch session flags (JSON array). Returns the updated session.
    pub async fn patch_session_flags(&self, id: Uuid, flags: &[String]) -> Result<Session> {
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let flags_json = serde_json::to_string(&flags)?;
        let affected = sqlx::query(
            "UPDATE sessions SET flags = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&flags_json)
        .bind(now_s)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.get_session_by_id(id)
            .await?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    // ---------- board ----------

    pub async fn create_board_item(&self, new: NewBoardItem) -> Result<BoardItem> {
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let status = new.status.unwrap_or_else(|| "todo".to_string());

        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "INSERT INTO board_items (key, title, body, status, created_at, updated_at)
             VALUES ('', ?, ?, ?, ?, ?)",
        )
        .bind(&new.title)
        .bind(&new.body)
        .bind(&status)
        .bind(&now_s)
        .bind(&now_s)
        .execute(&mut *tx)
        .await?;
        let id = result.last_insert_rowid();
        let key = format!("AG-{id}");
        sqlx::query("UPDATE board_items SET key = ? WHERE id = ?")
            .bind(&key)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        Ok(BoardItem {
            id,
            key,
            title: new.title,
            body: new.body,
            status,
            claimed_by: None,
            created_at: now,
            updated_at: now,
        })
    }

    pub async fn list_board_items(&self) -> Result<Vec<BoardItem>> {
        let rows: Vec<BoardItemRow> =
            sqlx::query_as::<_, BoardItemRow>("SELECT * FROM board_items ORDER BY created_at ASC")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter().map(BoardItem::try_from).collect()
    }

    pub async fn get_board_item(&self, id: i64) -> Result<Option<BoardItem>> {
        let row = sqlx::query_as::<_, BoardItemRow>("SELECT * FROM board_items WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(BoardItem::try_from).transpose()
    }

    pub async fn patch_board_item(&self, id: i64, patch: BoardPatch) -> Result<BoardItem> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let body_set = patch.body.is_some();
        let body_value = patch.body.unwrap_or(None);
        let affected = sqlx::query(
            "UPDATE board_items SET
                title  = COALESCE(?, title),
                status = COALESCE(?, status),
                body   = CASE WHEN ? = 1 THEN ? ELSE body END,
                updated_at = ?
             WHERE id = ?",
        )
        .bind(&patch.title)
        .bind(&patch.status)
        .bind(if body_set { 1i32 } else { 0i32 })
        .bind(&body_value)
        .bind(&now_s)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.get_board_item(id)
            .await?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub async fn delete_board_item(&self, id: i64) -> Result<()> {
        let affected = sqlx::query("DELETE FROM board_items WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Atomic CAS claim: succeeds only if `claimed_by` is currently NULL.
    /// Returns the updated row on success, `None` on conflict (caller maps
    /// to 409).
    pub async fn claim_board_item(&self, id: i64, claimed_by: &str) -> Result<Option<BoardItem>> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let result = sqlx::query(
            "UPDATE board_items SET claimed_by = ?, updated_at = ?
             WHERE id = ? AND claimed_by IS NULL",
        )
        .bind(claimed_by)
        .bind(&now_s)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_board_item(id).await
    }

    // ---------- notes ----------

    pub async fn create_note(&self, new: NewNote) -> Result<Note> {
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let tags_json = serde_json::to_string(&new.tags)?;
        let result = sqlx::query(
            "INSERT INTO notes (title, body, tags, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&new.title)
        .bind(&new.body)
        .bind(&tags_json)
        .bind(&now_s)
        .bind(&now_s)
        .execute(&self.pool)
        .await?;
        Ok(Note {
            id: result.last_insert_rowid(),
            title: new.title,
            body: new.body,
            tags: new.tags,
            created_at: now,
            updated_at: now,
        })
    }

    pub async fn list_notes(&self) -> Result<Vec<Note>> {
        let rows: Vec<NoteRow> =
            sqlx::query_as::<_, NoteRow>("SELECT * FROM notes ORDER BY updated_at DESC")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter().map(Note::try_from).collect()
    }

    pub async fn get_note(&self, id: i64) -> Result<Option<Note>> {
        let row = sqlx::query_as::<_, NoteRow>("SELECT * FROM notes WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(Note::try_from).transpose()
    }

    pub async fn patch_note(&self, id: i64, patch: NotePatch) -> Result<Note> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let tags_json = match &patch.tags {
            Some(t) => Some(serde_json::to_string(t)?),
            None => None,
        };
        let affected = sqlx::query(
            "UPDATE notes SET
                title = COALESCE(?, title),
                body  = COALESCE(?, body),
                tags  = COALESCE(?, tags),
                updated_at = ?
             WHERE id = ?",
        )
        .bind(&patch.title)
        .bind(&patch.body)
        .bind(&tags_json)
        .bind(&now_s)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.get_note(id)
            .await?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub async fn delete_note(&self, id: i64) -> Result<()> {
        let affected = sqlx::query("DELETE FROM notes WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    // ---------- channels ----------

    /// Create a 1:1 channel between two sessions. The pair is canonicalized
    /// (`a_session < b_session`) so (A,B) and (B,A) collapse to one row.
    pub async fn create_channel(&self, new: NewChannel) -> Result<Channel> {
        if new.a_session == new.b_session {
            return Err(StoreError::Core(agentum_core::CoreError::InvalidName(
                "channel sessions must differ".into(),
            )));
        }
        let (a, b) = if new.a_session < new.b_session {
            (new.a_session, new.b_session)
        } else {
            (new.b_session, new.a_session)
        };
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let res =
            sqlx::query("INSERT INTO channels (a_session, b_session, created_at) VALUES (?, ?, ?)")
                .bind(a.to_string())
                .bind(b.to_string())
                .bind(&now_s)
                .execute(&self.pool)
                .await;
        if let Err(sqlx::Error::Database(db)) = &res {
            if db.is_unique_violation() {
                return Err(StoreError::AlreadyExists(format!(
                    "channel between {a} and {b}"
                )));
            }
        }
        let id = res?.last_insert_rowid();
        Ok(Channel {
            id,
            a_session: a,
            b_session: b,
            created_at: now,
        })
    }

    pub async fn list_channels(&self) -> Result<Vec<Channel>> {
        let rows: Vec<ChannelRow> =
            sqlx::query_as::<_, ChannelRow>("SELECT * FROM channels ORDER BY created_at ASC")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter().map(Channel::try_from).collect()
    }

    pub async fn get_channel(&self, id: i64) -> Result<Option<Channel>> {
        let row = sqlx::query_as::<_, ChannelRow>("SELECT * FROM channels WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(Channel::try_from).transpose()
    }

    pub async fn delete_channel(&self, id: i64) -> Result<()> {
        let affected = sqlx::query("DELETE FROM channels WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    // ---------- messages ----------

    pub async fn append_message(&self, channel_id: i64, msg: NewMessage) -> Result<Message> {
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let res =
            sqlx::query("INSERT INTO messages (channel_id, sender, body, ts) VALUES (?, ?, ?, ?)")
                .bind(channel_id)
                .bind(&msg.sender)
                .bind(&msg.body)
                .bind(&now_s)
                .execute(&self.pool)
                .await?;
        Ok(Message {
            id: res.last_insert_rowid(),
            channel_id,
            sender: msg.sender,
            body: msg.body,
            ts: now,
        })
    }

    pub async fn list_messages(&self, channel_id: i64, limit: i64) -> Result<Vec<Message>> {
        // Most recent `limit` messages, returned oldest-first for chat UI.
        let rows: Vec<MessageRow> = sqlx::query_as::<_, MessageRow>(
            "SELECT * FROM (
                SELECT * FROM messages WHERE channel_id = ? ORDER BY ts DESC LIMIT ?
             ) ORDER BY ts ASC",
        )
        .bind(channel_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Message::try_from).collect()
    }

    /// Persist an event row. Best-effort (failures should not break callers).
    pub async fn insert_event(&self, ev: &Event) -> Result<()> {
        let payload = serde_json::to_string(&ev.payload)?;
        let ts = ev.ts.format(&Rfc3339)?;
        sqlx::query("INSERT INTO events (session_id, kind, payload, ts) VALUES (?, ?, ?, ?)")
            .bind(ev.session_id.map(|u| u.to_string()))
            .bind(&ev.kind)
            .bind(payload)
            .bind(ts)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn count_by_status(&self, status: Status) -> Result<i64> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE status = ?")
            .bind(status.as_str())
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    pub async fn delete_session(&self, id: Uuid) -> Result<()> {
        let affected = sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
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

#[derive(Debug, FromRow)]
struct SessionRow {
    id: String,
    name: String,
    workdir: String,
    tool: String,
    model: Option<String>,
    flags: String,
    status: String,
    tmux_target: Option<String>,
    created_at: String,
    updated_at: String,
    last_activity_at: Option<String>,
}

#[derive(Debug, FromRow)]
struct NoteRow {
    id: i64,
    title: String,
    body: String,
    tags: String,
    created_at: String,
    updated_at: String,
}

impl TryFrom<NoteRow> for Note {
    type Error = StoreError;
    fn try_from(r: NoteRow) -> Result<Self> {
        Ok(Note {
            id: r.id,
            title: r.title,
            body: r.body,
            tags: serde_json::from_str(&r.tags)?,
            created_at: OffsetDateTime::parse(&r.created_at, &Rfc3339)?,
            updated_at: OffsetDateTime::parse(&r.updated_at, &Rfc3339)?,
        })
    }
}

#[derive(Debug, FromRow)]
struct ChannelRow {
    id: i64,
    a_session: String,
    b_session: String,
    created_at: String,
}

impl TryFrom<ChannelRow> for Channel {
    type Error = StoreError;
    fn try_from(r: ChannelRow) -> Result<Self> {
        Ok(Channel {
            id: r.id,
            a_session: Uuid::parse_str(&r.a_session)?,
            b_session: Uuid::parse_str(&r.b_session)?,
            created_at: OffsetDateTime::parse(&r.created_at, &Rfc3339)?,
        })
    }
}

#[derive(Debug, FromRow)]
struct MessageRow {
    id: i64,
    channel_id: i64,
    sender: String,
    body: String,
    ts: String,
}

impl TryFrom<MessageRow> for Message {
    type Error = StoreError;
    fn try_from(r: MessageRow) -> Result<Self> {
        Ok(Message {
            id: r.id,
            channel_id: r.channel_id,
            sender: r.sender,
            body: r.body,
            ts: OffsetDateTime::parse(&r.ts, &Rfc3339)?,
        })
    }
}

#[derive(Debug, FromRow)]
struct BoardItemRow {
    id: i64,
    key: String,
    title: String,
    body: Option<String>,
    status: String,
    claimed_by: Option<String>,
    created_at: String,
    updated_at: String,
}

impl TryFrom<BoardItemRow> for BoardItem {
    type Error = StoreError;
    fn try_from(r: BoardItemRow) -> Result<Self> {
        Ok(BoardItem {
            id: r.id,
            key: r.key,
            title: r.title,
            body: r.body,
            status: r.status,
            claimed_by: r.claimed_by,
            created_at: OffsetDateTime::parse(&r.created_at, &Rfc3339)?,
            updated_at: OffsetDateTime::parse(&r.updated_at, &Rfc3339)?,
        })
    }
}

impl TryFrom<SessionRow> for Session {
    type Error = StoreError;
    fn try_from(r: SessionRow) -> Result<Self> {
        Ok(Session {
            id: Uuid::parse_str(&r.id)?,
            name: r.name,
            workdir: r.workdir,
            tool: r.tool,
            model: r.model,
            flags: serde_json::from_str(&r.flags)?,
            status: Status::from_str(&r.status)?,
            tmux_target: r.tmux_target,
            created_at: OffsetDateTime::parse(&r.created_at, &Rfc3339)?,
            updated_at: OffsetDateTime::parse(&r.updated_at, &Rfc3339)?,
            last_activity_at: r
                .last_activity_at
                .as_deref()
                .map(|s| OffsetDateTime::parse(s, &Rfc3339))
                .transpose()?,
        })
    }
}

/// Convenience: open the store at the canonical XDG data path.
pub async fn open_default() -> Result<(Store, PathBuf)> {
    let p = paths::data_dir()?.join("db.sqlite");
    let store = Store::open(&p).await?;
    Ok((store, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn tmp_store() -> Store {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        // dir is dropped at end of test; sqlx pool keeps file alive only while open.
        // Leak the tempdir handle to keep it alive for the test duration.
        std::mem::forget(dir);
        Store::open(&p).await.unwrap()
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
        };
        s.create_session(new.clone()).await.unwrap();
        let err = s.create_session(new).await.unwrap_err();
        assert!(matches!(err, StoreError::AlreadyExists(_)));
    }

    #[tokio::test]
    async fn board_create_and_claim_cas() {
        let s = tmp_store().await;
        let item = s
            .create_board_item(NewBoardItem {
                title: "ship phase 7".into(),
                body: Some("kanban + atomic claim".into()),
                status: None,
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
    async fn board_patch_and_clear_body() {
        let s = tmp_store().await;
        let item = s
            .create_board_item(NewBoardItem {
                title: "x".into(),
                body: Some("orig".into()),
                status: None,
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
            })
            .await
            .unwrap();
        s.update_status(sess.id, Status::Running).await.unwrap();
        let got = s.get_session_by_id(sess.id).await.unwrap().unwrap();
        assert_eq!(got.status, Status::Running);
    }
}
