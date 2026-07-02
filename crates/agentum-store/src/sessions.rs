//! Sessions: the core `(name, workdir, tool, model, flags)` records the daemon
//! spawns into tmux panes, plus their status / worktree-isolation /
//! provisioned-endpoint lifecycle. `SessionRow` mirrors the heavily-migrated
//! `sessions` table.

use crate::{Result, Store, StoreError};
use agentum_core::{LOCAL_HOST_ID, NewSession, Session, Status};
use sqlx::FromRow;
use std::str::FromStr;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

impl Store {
    pub async fn create_session(&self, new: NewSession) -> Result<Session> {
        self.create_session_on_host(new, None).await
    }

    pub async fn create_session_on_host(
        &self,
        new: NewSession,
        host_id: Option<Uuid>,
    ) -> Result<Session> {
        agentum_core::validate_name(&new.name)?;

        let id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let flags = serde_json::to_string(&new.flags)?;
        let status = Status::Idle;
        let host_id = host_id.unwrap_or(LOCAL_HOST_ID);

        // Worktree fields are resolved by the server *before* calling
        // create_session: by the time we hit the store, they're either
        // all None (no isolation) or all Some (server already ran
        // `git worktree add`).
        let wt_path = new.worktree_path.clone();
        let wt_branch = new.worktree_branch.clone();
        let wt_base = new.worktree_base_ref.clone();

        let res = sqlx::query(
            r#"
            INSERT INTO sessions
                (id, name, workdir, tool, model, flags, status, created_at, updated_at, card_id,
                 worktree_path, worktree_branch, worktree_base_ref, host_id)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(wt_path.as_deref())
        .bind(wt_branch.as_deref())
        .bind(wt_base.as_deref())
        .bind(host_id.to_string())
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
            host_id: Some(host_id),
            host_label: None,
            host_kind: None,
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
            worktree_path: wt_path,
            worktree_branch: wt_branch,
            worktree_base_ref: wt_base,
            // A freshly-created session has not been launched yet, so it has no
            // provisioned endpoint and nothing to reconnect. Recorded at spawn.
            provisioned_api_base: None,
            provisioned_token_hash: None,
            provisioned_needs_reconnect: false,
        })
    }

    /// Null out the three worktree columns after the server has called
    /// `git worktree remove`. Idempotent — a session without a worktree
    /// no-ops cleanly.
    pub async fn clear_session_worktree(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE sessions SET worktree_path = NULL, worktree_branch = NULL, worktree_base_ref = NULL, updated_at = ? WHERE id = ?",
        )
        .bind(OffsetDateTime::now_utc().format(&Rfc3339)?)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_sessions(&self, status: Option<Status>) -> Result<Vec<Session>> {
        // `pinned DESC` first so favorited rows float to the top of every
        // listing; ties fall back to the creation-order rule everyone
        // already mentally models.
        let rows: Vec<SessionRow> = match status {
            Some(s) => {
                sqlx::query_as::<_, SessionRow>(
                    "SELECT s.*, h.name AS host_label, h.kind AS host_kind \
                     FROM sessions s LEFT JOIN hosts h ON h.id = s.host_id \
                     WHERE s.status = ? \
                     ORDER BY s.pinned DESC, s.created_at DESC",
                )
                .bind(s.as_str())
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, SessionRow>(
                    "SELECT s.*, h.name AS host_label, h.kind AS host_kind \
                     FROM sessions s LEFT JOIN hosts h ON h.id = s.host_id \
                     ORDER BY s.pinned DESC, s.created_at DESC",
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
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT s.*, h.name AS host_label, h.kind AS host_kind \
             FROM sessions s LEFT JOIN hosts h ON h.id = s.host_id \
             WHERE s.name = ?",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        row.map(Session::try_from).transpose()
    }

    pub async fn get_session_by_id(&self, id: Uuid) -> Result<Option<Session>> {
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT s.*, h.name AS host_label, h.kind AS host_kind \
             FROM sessions s LEFT JOIN hosts h ON h.id = s.host_id \
             WHERE s.id = ?",
        )
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
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT s.*, h.name AS host_label, h.kind AS host_kind \
             FROM sessions s LEFT JOIN hosts h ON h.id = s.host_id \
             WHERE s.card_id = ?",
        )
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

    /// Record what a session was provisioned with — the live `api_base` URL +
    /// hex token hash its MCP config/env were written against. Also clears the
    /// `provisioned_needs_reconnect` flag, since by definition the session is now
    /// current. Called at spawn (Local only) and after the boot drift scan
    /// rewrites a session. NotFound when the row is gone.
    pub async fn set_session_provisioned(
        &self,
        id: Uuid,
        api_base: Option<&str>,
        token_hash: Option<&str>,
    ) -> Result<()> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let affected = sqlx::query(
            "UPDATE sessions SET provisioned_api_base = ?, provisioned_token_hash = ?, \
             provisioned_needs_reconnect = 0, updated_at = ? WHERE id = ?",
        )
        .bind(api_base)
        .bind(token_hash)
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

    /// Mark a session "endpoint drifted / needs reconnect". Set by the boot drift
    /// scan after it rewrites a live session's config+env to the current endpoint:
    /// the running agent only re-reads its MCP config at launch, so it must
    /// reconnect to pick up the change. The flag survives a restart (it's a
    /// column) and rides along in the session JSON so the UI can surface it.
    /// NotFound when the row is gone.
    pub async fn flag_session_needs_reconnect(&self, id: Uuid) -> Result<()> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let affected = sqlx::query(
            "UPDATE sessions SET provisioned_needs_reconnect = 1, updated_at = ? WHERE id = ?",
        )
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
}

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
    #[sqlx(default)]
    host_id: Option<String>,
    #[sqlx(default)]
    host_label: Option<String>,
    #[sqlx(default)]
    host_kind: Option<String>,
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
    /* ---- worktree isolation (migration 0016) ---- */
    #[sqlx(default)]
    worktree_path: Option<String>,
    #[sqlx(default)]
    worktree_branch: Option<String>,
    #[sqlx(default)]
    worktree_base_ref: Option<String>,
    /* ---- agent hooks (migration 0017) ---- */
    // Columns are persisted by the migration but consumed via the in-memory
    // `AppState.hook_tokens` map for now; reads from these row fields land
    // in a follow-up that survives daemon restarts.
    #[sqlx(default)]
    #[allow(dead_code)]
    hook_token: Option<String>,
    #[sqlx(default)]
    #[allow(dead_code)]
    hook_events_enabled: i64,
    /* ---- provisioned endpoint drift tracking (migration 0023) ---- */
    #[sqlx(default)]
    provisioned_api_base: Option<String>,
    #[sqlx(default)]
    provisioned_token_hash: Option<String>,
    #[sqlx(default)]
    provisioned_needs_reconnect: i64,
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
            host_id: r.host_id.as_deref().map(Uuid::parse_str).transpose()?,
            host_label: r.host_label,
            host_kind: r.host_kind,
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
            worktree_path: r.worktree_path,
            worktree_branch: r.worktree_branch,
            worktree_base_ref: r.worktree_base_ref,
            provisioned_api_base: r.provisioned_api_base,
            provisioned_token_hash: r.provisioned_token_hash,
            provisioned_needs_reconnect: r.provisioned_needs_reconnect != 0,
        })
    }
}
