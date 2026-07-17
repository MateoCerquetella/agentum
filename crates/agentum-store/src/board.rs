//! Board: the kanban surface — items (cards), threaded comments, goal links,
//! per-column validation rules, and external-tracker (GitHub/Linear) sync.
//! `BoardItemRow`/`BoardCommentRow` mirror the `board_items`/`board_comments`
//! tables; `parse_rule_json` decodes a column rule's required-field blob.

use crate::{Result, Store, StoreError};
use agentum_core::{
    BoardComment, BoardItem, BoardLink, BoardPatch, LinkKind, NewBoardComment, NewBoardItem,
    ReorderEntry, RequiredField, TrackerBinding,
};
use sqlx::FromRow;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

impl Store {
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

        // If the payload binds to an existing session, validate it inside the
        // tx so the dual-write (board_items.session_id ↔ sessions.card_id)
        // is atomic. Without this, creating a card pre-bound to a session
        // would leave sessions.card_id NULL — the watchdog→comment bridge
        // and the bound-session panel both read sessions.card_id, so the
        // binding would be invisible to half the system.
        //
        // Returns AlreadyExists (HTTP 409) when the target session is
        // already bound to a different card.
        if let Some(sid) = &new.session_id {
            let row: Option<(String, Option<i64>)> =
                sqlx::query_as("SELECT id, card_id FROM sessions WHERE id = ?")
                    .bind(sid)
                    .fetch_optional(&mut *tx)
                    .await?;
            match row {
                None => return Err(StoreError::NotFound(format!("session {sid}"))),
                Some((_, Some(other))) => {
                    return Err(StoreError::AlreadyExists(format!(
                        "session {sid} already bound to card {other}"
                    )));
                }
                Some((_, None)) => { /* fall through to the dual-write below */ }
            }
        }

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

        // Reverse leg of the dual-write — the existence + ownership check
        // above guarantees this UPDATE touches exactly one row that's
        // currently unbound, so a races-free single-statement update is
        // safe inside the same tx.
        if let Some(sid) = &new.session_id {
            sqlx::query("UPDATE sessions SET card_id = ?, updated_at = ? WHERE id = ?")
                .bind(id)
                .bind(&now_s)
                .bind(sid)
                .execute(&mut *tx)
                .await?;
        }

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
            // Native cards carry no external link; the tracker-sync paths
            // (`upsert_board_item_by_external_url`, `upsert_external_card`) are
            // the only writers of these.
            external_url: None,
            external_provider: None,
            external_id: None,
            external_synced_at: None,
        })
    }

    /// Idempotent tracker sync: upsert a card keyed on `external_url`. If a
    /// card already mirrors that issue, refresh its mutable mirror fields
    /// (title/body/status/lbl/provider) in place; otherwise insert a fresh
    /// card carrying the external link. This is what makes "fold the
    /// GitHub/Linear Tasks view into the Board as a sync source" (#48)
    /// re-runnable without duplicating cards — the external issue is the
    /// source of truth for these fields, so a re-sync overwrites them.
    ///
    /// Self-contained (does not go through `create_board_item`) so the common
    /// `NewBoardItem` path stays free of external-tracker concerns.
    pub async fn upsert_board_item_by_external_url(
        &self,
        external_url: &str,
        external_provider: Option<&str>,
        title: &str,
        body: Option<&str>,
        status: &str,
        lbl: Option<&str>,
    ) -> Result<BoardItem> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;

        if let Some(existing) = self.board_item_by_external_url(external_url).await? {
            sqlx::query(
                r#"UPDATE board_items
                   SET title = ?, body = ?, status = ?, lbl = ?, external_provider = ?,
                       updated_at = ?
                   WHERE id = ?"#,
            )
            .bind(title)
            .bind(body)
            .bind(status)
            .bind(lbl)
            .bind(external_provider)
            .bind(&now_s)
            .bind(existing.id)
            .execute(&self.pool)
            .await?;
            return self
                .get_board_item(existing.id)
                .await?
                .ok_or_else(|| StoreError::NotFound(format!("board item {}", existing.id)));
        }

        // Insert a new external card. Mirrors create_board_item's key dance
        // (empty key → derive AG-<id> post-insert) but writes the external link.
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"INSERT INTO board_items
                (key, title, body, status, lbl, priority, created_at, updated_at,
                 external_url, external_provider)
               VALUES ('', ?, ?, ?, ?, 0, ?, ?, ?, ?)"#,
        )
        .bind(title)
        .bind(body)
        .bind(status)
        .bind(lbl)
        .bind(&now_s)
        .bind(&now_s)
        .bind(external_url)
        .bind(external_provider)
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

        self.get_board_item(id)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("board item {id}")))
    }

    /// Look up a board card by the external issue URL it mirrors (the sync
    /// dedupe key). `None` when no card mirrors that issue yet.
    pub async fn board_item_by_external_url(&self, url: &str) -> Result<Option<BoardItem>> {
        let row =
            sqlx::query_as::<_, BoardItemRow>("SELECT * FROM board_items WHERE external_url = ?")
                .bind(url)
                .fetch_optional(&self.pool)
                .await?;
        row.map(BoardItem::try_from).transpose()
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

    // ---------- board ↔ external tracker sync (spec 016a) ----------

    /// Create-or-update a card that mirrors an external issue, matched by
    /// `(external_provider, external_id)`. Idempotent: re-syncing the same
    /// issue refreshes the existing row in place (returns `created = false`)
    /// instead of inserting a duplicate. Deliberately bypasses
    /// `create_board_item` so the session dual-write path stays untouched.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_external_card(
        &self,
        provider: &str,
        external_id: &str,
        title: &str,
        body: Option<&str>,
        url: &str,
        status: &str,
        synced_at: &str,
    ) -> Result<(BoardItem, bool)> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let mut tx = self.pool.begin().await?;

        let existing: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM board_items WHERE external_provider = ? AND external_id = ?",
        )
        .bind(provider)
        .bind(external_id)
        .fetch_optional(&mut *tx)
        .await?;

        let (id, created) = match existing {
            Some(id) => {
                sqlx::query(
                    r#"UPDATE board_items SET
                        title = ?, body = ?, status = ?, external_url = ?,
                        external_synced_at = ?, updated_at = ?
                       WHERE id = ?"#,
                )
                .bind(title)
                .bind(body)
                .bind(status)
                .bind(url)
                .bind(synced_at)
                .bind(&now_s)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                (id, false)
            }
            None => {
                let res = sqlx::query(
                    r#"INSERT INTO board_items
                        (key, title, body, status, lbl, priority, created_at, updated_at,
                         external_provider, external_id, external_url, external_synced_at)
                       VALUES ('', ?, ?, ?, 'feat', 0, ?, ?, ?, ?, ?, ?)"#,
                )
                .bind(title)
                .bind(body)
                .bind(status)
                .bind(&now_s)
                .bind(&now_s)
                .bind(provider)
                .bind(external_id)
                .bind(url)
                .bind(synced_at)
                .execute(&mut *tx)
                .await?;
                let id = res.last_insert_rowid();
                sqlx::query("UPDATE board_items SET key = ? WHERE id = ?")
                    .bind(format!("AG-{id}"))
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                (id, true)
            }
        };
        tx.commit().await?;

        let item = self
            .get_board_item(id)
            .await?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        Ok((item, created))
    }

    /// `(card id, external_id, status)` for every card mirroring `provider`.
    /// The sync engine reconciles incoming issues against this.
    pub async fn list_external_refs(&self, provider: &str) -> Result<Vec<(i64, String, String)>> {
        let rows: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT id, external_id, status FROM board_items \
             WHERE external_provider = ? AND external_id IS NOT NULL",
        )
        .bind(provider)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Create the board↔tracker binding, or refresh `updated_at` if the same
    /// `(provider, project)` is bound again (idempotent re-bind).
    pub async fn create_tracker_binding(
        &self,
        provider: &str,
        project: &str,
    ) -> Result<TrackerBinding> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        sqlx::query(
            r#"INSERT INTO board_tracker_bindings (provider, project, created_at, updated_at)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(provider, project) DO UPDATE SET updated_at = excluded.updated_at"#,
        )
        .bind(provider)
        .bind(project)
        .bind(&now_s)
        .bind(&now_s)
        .execute(&self.pool)
        .await?;

        let row: (i64, String, String, String, String) = sqlx::query_as(
            "SELECT id, provider, project, created_at, updated_at \
             FROM board_tracker_bindings WHERE provider = ? AND project = ?",
        )
        .bind(provider)
        .bind(project)
        .fetch_one(&self.pool)
        .await?;
        Ok(TrackerBinding {
            id: row.0,
            provider: row.1,
            project: row.2,
            created_at: OffsetDateTime::parse(&row.3, &Rfc3339)?,
            updated_at: OffsetDateTime::parse(&row.4, &Rfc3339)?,
        })
    }

    pub async fn list_tracker_bindings(&self) -> Result<Vec<TrackerBinding>> {
        let rows: Vec<(i64, String, String, String, String)> = sqlx::query_as(
            "SELECT id, provider, project, created_at, updated_at \
             FROM board_tracker_bindings ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(TrackerBinding {
                    id: r.0,
                    provider: r.1,
                    project: r.2,
                    created_at: OffsetDateTime::parse(&r.3, &Rfc3339)?,
                    updated_at: OffsetDateTime::parse(&r.4, &Rfc3339)?,
                })
            })
            .collect()
    }

    pub async fn delete_tracker_binding(&self, id: i64) -> Result<()> {
        let affected = sqlx::query("DELETE FROM board_tracker_bindings WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Stamp a card's external link by id (spec 016b push-back). Used when a
    /// native card is pushed to a tracker and gets a freshly-created issue, so
    /// the next pull reconciles it instead of re-creating.
    pub async fn set_card_external_link(&self, id: i64, url: &str, provider: &str) -> Result<()> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let affected = sqlx::query(
            "UPDATE board_items SET external_url = ?, external_provider = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(url)
        .bind(provider)
        .bind(&now_s)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
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
        // `UPDATE … RETURNING *` returns the patched row in one round trip
        // instead of UPDATE-then-SELECT. This is the reconciler's per-event
        // write path (board.updated/created/deleted), so halving its statement
        // count matters. `RETURNING *` yields the same columns as
        // `get_board_item`'s `SELECT *`, mapped by name into `BoardItemRow`.
        let row = sqlx::query_as::<_, BoardItemRow>(
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
             WHERE id = ?
             RETURNING *"#,
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
        .fetch_optional(&self.pool)
        .await?;
        row.map(BoardItem::try_from)
            .transpose()?
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
        // One round trip: the CAS `WHERE … claimed_by IS NULL` still gates the
        // update, and `RETURNING *` hands back the claimed row (or no row on a
        // conflict → `None`), replacing the follow-up `get_board_item`.
        let row = sqlx::query_as::<_, BoardItemRow>(
            "UPDATE board_items SET claimed_by = ?, updated_at = ?
             WHERE id = ? AND claimed_by IS NULL
             RETURNING *",
        )
        .bind(claimed_by)
        .bind(&now_s)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(BoardItem::try_from).transpose()
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
        // `RETURNING *` collapses the release UPDATE + re-fetch into one round
        // trip. `None` (no row matched the CAS) maps to the same conflict
        // signal the old `rows_affected() == 0` produced.
        let row = if actor.is_empty() {
            sqlx::query_as::<_, BoardItemRow>(
                "UPDATE board_items SET claimed_by = NULL, updated_at = ?
                 WHERE id = ?
                 RETURNING *",
            )
            .bind(&now_s)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, BoardItemRow>(
                "UPDATE board_items SET claimed_by = NULL, updated_at = ?
                 WHERE id = ? AND claimed_by = ?
                 RETURNING *",
            )
            .bind(&now_s)
            .bind(id)
            .bind(actor)
            .fetch_optional(&self.pool)
            .await?
        };
        row.map(BoardItem::try_from).transpose()
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
            // `review` ranks with `doing` (1): from the GOAL's perspective a child
            // awaiting verification is still in-progress — the goal is only `done`
            // once every child is `done` (2). Without this arm `review` hit ELSE→0
            // and a goal whose children were all in review wrongly rolled back to
            // `todo`. Ranking it 1 (not a new top rank) keeps `done`=2 and the
            // reconciler's rank→status map unchanged.
            "SELECT MAX(CASE status
                          WHEN 'todo'   THEN 0
                          WHEN 'doing'  THEN 1
                          WHEN 'review' THEN 1
                          WHEN 'done'   THEN 2
                          ELSE 0
                        END)
             FROM board_items WHERE parent_goal_id = ?",
        )
        .bind(goal_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0.map(|v| v as i32))
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
    /* ---- external tracker link (migration 0022) ---- */
    #[sqlx(default)]
    external_url: Option<String>,
    #[sqlx(default)]
    external_provider: Option<String>,
    /* ---- two-way sync identity + marker (migration 0023) ---- */
    #[sqlx(default)]
    external_id: Option<String>,
    #[sqlx(default)]
    external_synced_at: Option<String>,
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
            external_url: r.external_url,
            external_provider: r.external_provider,
            external_id: r.external_id,
            external_synced_at: r.external_synced_at,
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
