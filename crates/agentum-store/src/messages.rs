//! Messages: append-only chat rows scoped to a [`Channel`](agentum_core::Channel).

use crate::{Result, Store, StoreError};
use agentum_core::{Message, NewMessage};
use sqlx::FromRow;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

impl Store {
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
