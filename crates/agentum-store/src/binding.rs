//! Card ↔ session binding: the atomic dual-write that ties a board card to the
//! agent session working it. `claim_card` inserts a session and binds it in one
//! transaction; `transfer_card_binding` rebinds or unbinds, keeping both sides
//! (`board_items.session_id` ↔ `sessions.card_id`) consistent.

use crate::{Result, Store, StoreError};
use agentum_core::{BoardItem, NewSession, Session, Status};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

impl Store {
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
}
