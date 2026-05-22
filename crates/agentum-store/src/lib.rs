//! SQLite persistence for agentum. WAL mode, synchronous=NORMAL.
//!
//! All XDG path resolution lives in [`paths`].

use std::path::{Path, PathBuf};
use std::str::FromStr;

use agentum_core::{
    BoardComment, BoardItem, BoardLink, BoardPatch, Channel, Event, LinkKind, Message,
    NewBoardComment, NewBoardItem, NewChannel, NewMessage, NewNote, NewSession, Note, NotePatch,
    ReorderEntry, RequiredField, Session, Status, User,
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
                (id, name, workdir, tool, model, flags, status, created_at, updated_at, card_id)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(new.card_id)
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
            tokens: None,
            cost_usd: None,
            ctx: None,
            last_log: None,
            uptime_seconds: None,
            state: None,
            pinned: false,
            card_id: new.card_id,
        })
    }

    pub async fn list_sessions(&self, status: Option<Status>) -> Result<Vec<Session>> {
        // `pinned DESC` first so favorited rows float to the top of every
        // listing; ties fall back to the creation-order rule everyone
        // already mentally models.
        let rows: Vec<SessionRow> = match status {
            Some(s) => {
                sqlx::query_as::<_, SessionRow>(
                    "SELECT * FROM sessions WHERE status = ? \
                     ORDER BY pinned DESC, created_at DESC",
                )
                .bind(s.as_str())
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, SessionRow>(
                    "SELECT * FROM sessions ORDER BY pinned DESC, created_at DESC",
                )
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.into_iter().map(Session::try_from).collect()
    }

    /// Toggle (or set) the `pinned` flag for a session. Pinned sessions
    /// sort to the top of every list view. Returns the patched row.
    pub async fn patch_session_pinned(&self, id: Uuid, pinned: bool) -> Result<Session> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let affected = sqlx::query("UPDATE sessions SET pinned = ?, updated_at = ? WHERE id = ?")
            .bind(if pinned { 1_i64 } else { 0_i64 })
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

    /// Look up the planner session bound to a goal card via `session.card_id`.
    /// Returns `Some(Session)` when exactly one session references `card_id`,
    /// `None` when none do. Used by the goal-status reconciler (plan 01-04)
    /// to find the planner session to auto-stop on first child arrival (D-07).
    pub async fn get_session_by_card_id(&self, card_id: i64) -> Result<Option<Session>> {
        let row = sqlx::query_as::<_, SessionRow>("SELECT * FROM sessions WHERE card_id = ?")
            .bind(card_id)
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

    /// Patch the session's display name. Empty / whitespace-only inputs
    /// must be rejected by the caller (API does this); on a duplicate
    /// name the underlying UNIQUE index surfaces a `Sqlx` error.
    pub async fn patch_session_name(&self, id: Uuid, new_name: &str) -> Result<Session> {
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let affected = sqlx::query("UPDATE sessions SET name = ?, updated_at = ? WHERE id = ?")
            .bind(new_name)
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

    /// Patch the session's `tool` field. Used by the watchdog when it
    /// detects the user switched the foreground process to a different
    /// adapter (e.g. ran `codex` from a `bash` session) so the sidebar
    /// chip reflects what's actually running, not what was originally
    /// requested. Caller is responsible for rejecting empty inputs.
    pub async fn patch_session_tool(&self, id: Uuid, new_tool: &str) -> Result<Session> {
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let affected = sqlx::query("UPDATE sessions SET tool = ?, updated_at = ? WHERE id = ?")
            .bind(new_tool)
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

    /// Patch session flags (JSON array). Returns the updated session.
    pub async fn patch_session_flags(&self, id: Uuid, flags: &[String]) -> Result<Session> {
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let flags_json = serde_json::to_string(&flags)?;
        let affected = sqlx::query("UPDATE sessions SET flags = ?, updated_at = ? WHERE id = ?")
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
        // Fresh tickets append to the bottom of their column by default.
        // The secondary sort key `created_at ASC` puts the newer row
        // below older rows with priority 0; callers that want a row at
        // the top can pass a negative priority explicitly.
        let priority = new.priority.unwrap_or(0);

        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"INSERT INTO board_items
                (key, title, body, status, lbl, tool, workdir, model, session_id, priority,
                 created_at, updated_at, parent_goal_id)
               VALUES ('', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&new.title)
        .bind(&new.body)
        .bind(&status)
        .bind(&new.lbl)
        .bind(&new.tool)
        .bind(&new.workdir)
        .bind(&new.model)
        .bind(&new.session_id)
        .bind(priority)
        .bind(&now_s)
        .bind(&now_s)
        .bind(new.parent_goal_id)
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
            lbl: new.lbl,
            tool: new.tool,
            workdir: new.workdir,
            model: new.model,
            session_id: new.session_id,
            priority,
            parent_goal_id: new.parent_goal_id,
        })
    }

    pub async fn list_board_items(&self) -> Result<Vec<BoardItem>> {
        // Stable per-column ordering: priority is the primary key
        // (drag-to-reorder writes it), created_at the tiebreaker so
        // fresh rows sit below older rows at the same priority.
        let rows: Vec<BoardItemRow> = sqlx::query_as::<_, BoardItemRow>(
            "SELECT * FROM board_items ORDER BY status ASC, priority ASC, created_at ASC",
        )
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
        let lbl_set = patch.lbl.is_some();
        let lbl_value = patch.lbl.unwrap_or(None);
        let tool_set = patch.tool.is_some();
        let tool_value = patch.tool.unwrap_or(None);
        let workdir_set = patch.workdir.is_some();
        let workdir_value = patch.workdir.unwrap_or(None);
        let model_set = patch.model.is_some();
        let model_value = patch.model.unwrap_or(None);
        let session_id_set = patch.session_id.is_some();
        let session_id_value = patch.session_id.unwrap_or(None);
        // Double-Option: Some(None) → clear, Some(Some(v)) → set, None → leave alone.
        let parent_goal_id_set = patch.parent_goal_id.is_some();
        let parent_goal_id_value: Option<i64> = patch.parent_goal_id.unwrap_or(None);
        let affected = sqlx::query(
            r#"UPDATE board_items SET
                title          = COALESCE(?, title),
                status         = COALESCE(?, status),
                priority       = COALESCE(?, priority),
                body           = CASE WHEN ? = 1 THEN ? ELSE body           END,
                lbl            = CASE WHEN ? = 1 THEN ? ELSE lbl            END,
                tool           = CASE WHEN ? = 1 THEN ? ELSE tool           END,
                workdir        = CASE WHEN ? = 1 THEN ? ELSE workdir        END,
                model          = CASE WHEN ? = 1 THEN ? ELSE model          END,
                session_id     = CASE WHEN ? = 1 THEN ? ELSE session_id     END,
                parent_goal_id = CASE WHEN ? = 1 THEN ? ELSE parent_goal_id END,
                updated_at     = ?
             WHERE id = ?"#,
        )
        .bind(&patch.title)
        .bind(&patch.status)
        .bind(patch.priority)
        .bind(if body_set { 1i32 } else { 0i32 })
        .bind(&body_value)
        .bind(if lbl_set { 1i32 } else { 0i32 })
        .bind(&lbl_value)
        .bind(if tool_set { 1i32 } else { 0i32 })
        .bind(&tool_value)
        .bind(if workdir_set { 1i32 } else { 0i32 })
        .bind(&workdir_value)
        .bind(if model_set { 1i32 } else { 0i32 })
        .bind(&model_value)
        .bind(if session_id_set { 1i32 } else { 0i32 })
        .bind(&session_id_value)
        .bind(if parent_goal_id_set { 1i32 } else { 0i32 })
        .bind(parent_goal_id_value)
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

    /// Release a held claim. CAS-style: succeeds only when the current
    /// `claimed_by` matches `actor`, or `actor` is empty (admin
    /// override). Returns `Ok(Some(item))` on success, `Ok(None)` when
    /// the row is held by a different actor (409), and `NotFound` if
    /// the row doesn't exist.
    pub async fn release_board_item(&self, id: i64, actor: &str) -> Result<Option<BoardItem>> {
        let existing = self
            .get_board_item(id)
            .await?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        if existing.claimed_by.is_none() {
            return Ok(Some(existing));
        }
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let result = if actor.is_empty() {
            sqlx::query(
                "UPDATE board_items SET claimed_by = NULL, updated_at = ?
                 WHERE id = ?",
            )
            .bind(&now_s)
            .bind(id)
            .execute(&self.pool)
            .await?
        } else {
            sqlx::query(
                "UPDATE board_items SET claimed_by = NULL, updated_at = ?
                 WHERE id = ? AND claimed_by = ?",
            )
            .bind(&now_s)
            .bind(id)
            .bind(actor)
            .execute(&self.pool)
            .await?
        };
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_board_item(id).await
    }

    /// List comments for a board item, oldest first so the thread
    /// reads top-to-bottom in the dialog.
    pub async fn list_board_comments(&self, board_id: i64) -> Result<Vec<BoardComment>> {
        let rows: Vec<BoardCommentRow> = sqlx::query_as::<_, BoardCommentRow>(
            "SELECT id, board_id, author, body, created_at FROM board_comments
             WHERE board_id = ? ORDER BY created_at ASC",
        )
        .bind(board_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(BoardComment::try_from).collect()
    }

    /// Append a comment to a ticket. Caller is responsible for any
    /// authorization (the route gates on the bearer token); the store
    /// validates the parent row exists so we don't end up with orphans
    /// even though the FK constraint would catch it later.
    pub async fn create_board_comment(
        &self,
        board_id: i64,
        new: NewBoardComment,
    ) -> Result<BoardComment> {
        if new.author.trim().is_empty() {
            return Err(StoreError::NotFound(format!(
                "board item {board_id}: empty author"
            )));
        }
        if self.get_board_item(board_id).await?.is_none() {
            return Err(StoreError::NotFound(board_id.to_string()));
        }
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let result = sqlx::query(
            "INSERT INTO board_comments (board_id, author, body, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(board_id)
        .bind(&new.author)
        .bind(&new.body)
        .bind(&now_s)
        .execute(&self.pool)
        .await?;
        let id = result.last_insert_rowid();
        Ok(BoardComment {
            id,
            board_id,
            author: new.author,
            body: new.body,
            created_at: now,
        })
    }

    /// True iff at least one row in `board_comments` references this id.
    /// Cheaper than `count_board_comments` for the single-id check the
    /// `done` transition gate needs — `LIMIT 1` short-circuits as soon
    /// as the index hits a matching row.
    pub async fn has_board_comments(&self, board_id: i64) -> Result<bool> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT 1 FROM board_comments WHERE board_id = ? LIMIT 1")
                .bind(board_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.is_some())
    }

    /// Count comments per board id in bulk so the card-foot 💬N chip
    /// stays cheap regardless of ticket count. Returns a map keyed by
    /// board_id; missing ids implicitly have zero comments.
    pub async fn count_board_comments(&self) -> Result<std::collections::HashMap<i64, i64>> {
        let rows: Vec<(i64, i64)> =
            sqlx::query_as("SELECT board_id, COUNT(*) FROM board_comments GROUP BY board_id")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().collect())
    }

    /// Bulk-rewrite priorities for one or more rows in a single
    /// transaction. Used by drag-to-reorder so the affected column
    /// commits atomically — no in-between state where two rows share
    /// a priority value mid-flight.
    pub async fn reorder_board_items(&self, entries: &[ReorderEntry]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let mut tx = self.pool.begin().await?;
        for e in entries {
            sqlx::query("UPDATE board_items SET priority = ?, updated_at = ? WHERE id = ?")
                .bind(e.priority)
                .bind(&now_s)
                .bind(e.id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // ---------- board column rules ----------

    /// Single-column lookup. `None` means no override row exists — the
    /// caller (the gate) falls back to the slice-1 const matrix.
    ///
    /// Per-element deserialisation via `RequiredField::from_missing_key`
    /// rather than serde's all-or-nothing array decode: unknown strings
    /// in the DB (e.g. a variant that was removed in a later version)
    /// are skipped with a warning instead of failing the whole row.
    /// This is the cheap version of JSON-shape versioning the spec
    /// deferred — keep the server up at the cost of dropping fields
    /// from one rule.
    pub async fn get_board_column_rule(&self, column: &str) -> Result<Option<Vec<RequiredField>>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT required_fields FROM board_column_rules WHERE column_name = ?")
                .bind(column)
                .fetch_optional(&self.pool)
                .await?;
        match row {
            Some((json,)) => Ok(Some(parse_rule_json(column, &json)?)),
            None => Ok(None),
        }
    }

    /// All overrides as a map. Empty map when the table is empty. The
    /// merge with the const matrix lives one layer up
    /// (`agentum-server::rules::merged_rule_matrix`) — the store stays
    /// agnostic about which columns are "default".
    pub async fn list_board_column_rules(
        &self,
    ) -> Result<std::collections::BTreeMap<String, Vec<RequiredField>>> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT column_name, required_fields FROM board_column_rules")
                .fetch_all(&self.pool)
                .await?;
        let mut out = std::collections::BTreeMap::new();
        for (col, json) in rows {
            let parsed = parse_rule_json(&col, &json)?;
            out.insert(col, parsed);
        }
        Ok(out)
    }

    /// Upsert by primary key. The caller has already validated the field
    /// list against the wire vocabulary (the route handler parses
    /// `Vec<String>` through `RequiredField::from_missing_key` and rejects
    /// unknown names with 400 before reaching here).
    pub async fn upsert_board_column_rule(
        &self,
        column: &str,
        fields: &[RequiredField],
    ) -> Result<()> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        // Serialise as `[wire-string, …]` so the on-disk shape matches
        // what `RequiredField::from_missing_key` reads back.
        let wire: Vec<&'static str> = fields.iter().map(|f| f.as_missing_key()).collect();
        let json = serde_json::to_string(&wire)?;
        sqlx::query(
            "INSERT INTO board_column_rules (column_name, required_fields, updated_at)
             VALUES (?, ?, ?)
             ON CONFLICT(column_name) DO UPDATE SET
                required_fields = excluded.required_fields,
                updated_at = excluded.updated_at",
        )
        .bind(column)
        .bind(&json)
        .bind(&now_s)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns `Ok(true)` iff a row was actually deleted. The handler
    /// uses the bool to choose 200 vs 404 — REST shape, no extra cost.
    pub async fn delete_board_column_rule(&self, column: &str) -> Result<bool> {
        let affected = sqlx::query("DELETE FROM board_column_rules WHERE column_name = ?")
            .bind(column)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(affected > 0)
    }

    // ---------- board links ----------

    /// Create a directed edge between two board items. The primary key
    /// `(from_card_id, to_card_id, kind)` prevents duplicate edges.
    /// Returns `StoreError::AlreadyExists` on a unique-key collision so
    /// callers can map to 409 rather than 500.
    pub async fn add_board_link(
        &self,
        from_card_id: i64,
        to_card_id: i64,
        kind: LinkKind,
    ) -> Result<BoardLink> {
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let res = sqlx::query(
            "INSERT INTO board_links (from_card_id, to_card_id, kind, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(from_card_id)
        .bind(to_card_id)
        .bind(kind.as_str())
        .bind(&now_s)
        .execute(&self.pool)
        .await;
        if let Err(sqlx::Error::Database(db)) = &res {
            if db.is_unique_violation() {
                return Err(StoreError::AlreadyExists(format!(
                    "board_link {from_card_id} -{kind:?}-> {to_card_id}"
                )));
            }
        }
        res?;
        Ok(BoardLink {
            from_card_id,
            to_card_id,
            kind,
            created_at: now,
        })
    }

    /// All board items whose `parent_goal_id = goal_id`. The partial index
    /// `idx_board_items_parent_goal_id` makes this O(children) not O(table).
    pub async fn list_children_of_goal(&self, goal_id: i64) -> Result<Vec<BoardItem>> {
        let rows: Vec<BoardItemRow> = sqlx::query_as::<_, BoardItemRow>(
            "SELECT * FROM board_items WHERE parent_goal_id = ?
             ORDER BY priority ASC, created_at ASC",
        )
        .bind(goal_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(BoardItem::try_from).collect()
    }

    /// All `board_links` rows where `from_card_id = goal_id`. Used by the
    /// Phase 3 dependency gate to enumerate what a goal's card directly
    /// blocks or parents. The `idx_board_links_to` index covers the
    /// `to_card_id` direction; `from_card_id` uses the PK b-tree prefix.
    pub async fn list_board_links_for_goal(&self, goal_id: i64) -> Result<Vec<BoardLink>> {
        let rows: Vec<(i64, i64, String, String)> = sqlx::query_as(
            "SELECT from_card_id, to_card_id, kind, created_at
             FROM board_links WHERE from_card_id = ?
             ORDER BY created_at ASC",
        )
        .bind(goal_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(from_card_id, to_card_id, kind_str, created_at_str)| {
                Ok(BoardLink {
                    from_card_id,
                    to_card_id,
                    kind: kind_str.parse()?,
                    created_at: OffsetDateTime::parse(&created_at_str, &Rfc3339)?,
                })
            })
            .collect()
    }

    /// Remove a single edge. Returns `true` if a row was deleted, `false`
    /// if the edge did not exist (idempotent — callers choose whether to
    /// surface that as 204 or 404).
    pub async fn delete_board_link(
        &self,
        from_card_id: i64,
        to_card_id: i64,
        kind: LinkKind,
    ) -> Result<bool> {
        let affected = sqlx::query(
            "DELETE FROM board_links
             WHERE from_card_id = ? AND to_card_id = ? AND kind = ?",
        )
        .bind(from_card_id)
        .bind(to_card_id)
        .bind(kind.as_str())
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(affected > 0)
    }

    /// The highest status rank among all children of `goal_id`, or `None`
    /// when the goal has no children. Used by the watchdog goal-status
    /// recomputer to decide whether to advance the goal's own status.
    ///
    /// Status rank follows the natural progression: todo=0, doing=1, done=2.
    /// Any unrecognised status string is treated as rank 0 (defensive).
    pub async fn max_child_status_rank(&self, goal_id: i64) -> Result<Option<i32>> {
        // Rank mapping inline in SQL so it's a single round-trip regardless
        // of child count. CASE WHEN is evaluated by SQLite per-row; no UDF.
        // MAX() over an empty set returns NULL in one row, not zero rows.
        // Use Option<i64> in the tuple to distinguish "no children" (NULL)
        // from "all children are todo" (0).
        let row: (Option<i64>,) = sqlx::query_as(
            "SELECT MAX(CASE status
                          WHEN 'todo'  THEN 0
                          WHEN 'doing' THEN 1
                          WHEN 'done'  THEN 2
                          ELSE 0
                        END)
             FROM board_items WHERE parent_goal_id = ?",
        )
        .bind(goal_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0.map(|v| v as i32))
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

    /// Newest-first list of recent watchdog-eligible events. Filters
    /// for kinds the dashboard's watchdog feed renders (`watchdog.*`
    /// and `session.crashed` / `session.started` etc.). `limit` caps
    /// the result; pass 50 for the default cold-start payload.
    pub async fn list_watchdog_events(&self, limit: i64) -> Result<Vec<Event>> {
        let rows: Vec<EventRow> = sqlx::query_as::<_, EventRow>(
            "SELECT session_id, kind, payload, ts FROM events
             WHERE kind LIKE 'watchdog.%' OR kind LIKE 'session.%'
             ORDER BY ts DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Event::try_from).collect()
    }

    /// One row per session: the most-recent `agent.*` event for each.
    /// Used by the `/api/events` WS handler to bootstrap a fresh client
    /// with the current activity overlay (idle / awaiting input /
    /// working) before live events start streaming. Without this a
    /// dashboard tab opened mid-flight has no way to tell that a
    /// `running` session's agent has already finished its turn —
    /// `agent.finished` only fires once per transition and isn't
    /// replayed on the bus.
    pub async fn latest_agent_event_per_session(&self) -> Result<Vec<Event>> {
        let rows: Vec<EventRow> = sqlx::query_as::<_, EventRow>(
            "SELECT e.session_id, e.kind, e.payload, e.ts FROM events e
             INNER JOIN (
                 SELECT session_id, MAX(id) AS max_id FROM events
                 WHERE session_id IS NOT NULL AND kind LIKE 'agent.%'
                 GROUP BY session_id
             ) latest ON latest.max_id = e.id
             ORDER BY e.ts ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Event::try_from).collect()
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
    /* ---- redesign metrics columns (migration 0007) ---- */
    tokens: Option<i64>,
    cost_usd: Option<f64>,
    ctx: Option<i64>,
    last_log: Option<String>,
    uptime_seconds: Option<i64>,
    state: Option<String>,
    /* ---- migration 0009 ---- */
    #[sqlx(default)]
    pinned: i64,
    /* ---- orchestrator binding (migration 0015) ---- */
    #[sqlx(default)]
    card_id: Option<i64>,
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
struct EventRow {
    session_id: Option<String>,
    kind: String,
    payload: String,
    ts: String,
}

impl TryFrom<EventRow> for Event {
    type Error = StoreError;
    fn try_from(r: EventRow) -> Result<Self> {
        Ok(Event {
            kind: r.kind,
            session_id: r.session_id.as_deref().map(Uuid::parse_str).transpose()?,
            // session_name is not persisted in the events table; the SSE
            // path passes it through directly. Cold-start GETs leave it
            // None and the watchdog projection falls back to session_id.
            session_name: None,
            payload: serde_json::from_str(&r.payload).unwrap_or(serde_json::Value::Null),
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
    /* ---- redesign discriminators (migration 0008) ---- */
    lbl: Option<String>,
    tool: Option<String>,
    /* ---- execution context (migration 0010) ---- */
    workdir: Option<String>,
    model: Option<String>,
    /* ---- session linkage (migration 0011) ---- */
    session_id: Option<String>,
    /* ---- manual ordering (migration 0012) ---- */
    priority: i64,
    /* ---- orchestrator goal binding (migration 0015) ---- */
    #[sqlx(default)]
    parent_goal_id: Option<i64>,
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
            lbl: r.lbl,
            tool: r.tool,
            workdir: r.workdir,
            model: r.model,
            session_id: r.session_id,
            priority: r.priority,
            parent_goal_id: r.parent_goal_id,
        })
    }
}

#[derive(Debug, FromRow)]
struct BoardCommentRow {
    id: i64,
    board_id: i64,
    author: String,
    body: String,
    created_at: String,
}

impl TryFrom<BoardCommentRow> for BoardComment {
    type Error = StoreError;
    fn try_from(r: BoardCommentRow) -> Result<Self> {
        Ok(BoardComment {
            id: r.id,
            board_id: r.board_id,
            author: r.author,
            body: r.body,
            created_at: OffsetDateTime::parse(&r.created_at, &Rfc3339)?,
        })
    }
}

impl TryFrom<SessionRow> for Session {
    type Error = StoreError;
    fn try_from(r: SessionRow) -> Result<Self> {
        let state = r
            .state
            .as_deref()
            .map(agentum_core::SessionState::from_str)
            .transpose()?;
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
            tokens: r.tokens,
            cost_usd: r.cost_usd,
            ctx: r.ctx.map(|c| c as i32),
            last_log: r.last_log,
            uptime_seconds: r.uptime_seconds,
            state,
            pinned: r.pinned != 0,
            card_id: r.card_id,
        })
    }
}

/// Convenience: open the store at the canonical XDG data path.
pub async fn open_default() -> Result<(Store, PathBuf)> {
    let p = paths::data_dir()?.join("db.sqlite");
    let store = Store::open(&p).await?;
    Ok((store, p))
}

/// Decode a `board_column_rules.required_fields` JSON blob to typed
/// variants. Unknown strings are skipped with a warning rather than
/// failing the whole row — see `get_board_column_rule` for why.
fn parse_rule_json(column: &str, json: &str) -> Result<Vec<RequiredField>> {
    let raw: Vec<String> = serde_json::from_str(json)?;
    let mut out = Vec::with_capacity(raw.len());
    for name in raw {
        match RequiredField::from_missing_key(&name) {
            Some(f) => out.push(f),
            None => {
                tracing::warn!(
                    column = column,
                    field = %name,
                    "unknown required-field in board_column_rules; skipping (forward-compat policy)",
                );
            }
        }
    }
    Ok(out)
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
                card_id: None,
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
                model: Some("claude-opus-4-7".into()),
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();
        assert_eq!(item.workdir.as_deref(), Some("/home/me/projects/foo"));
        assert_eq!(item.model.as_deref(), Some("claude-opus-4-7"));

        // Re-listing carries them through the BoardItemRow → BoardItem
        // conversion — guards against a forgotten field mapping.
        let all = s.list_board_items().await.unwrap();
        assert_eq!(all[0].workdir.as_deref(), Some("/home/me/projects/foo"));
        assert_eq!(all[0].model.as_deref(), Some("claude-opus-4-7"));

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
            })
            .await
            .unwrap();
        s.update_status(sess.id, Status::Running).await.unwrap();
        let got = s.get_session_by_id(sess.id).await.unwrap().unwrap();
        assert_eq!(got.status, Status::Running);
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
}
