//! Channels: 1:1 communication links between two sessions. The pair is
//! canonicalized (`a_session < b_session`) so (A,B) and (B,A) collapse to
//! one row.

use crate::{Result, Store, StoreError};
use agentum_core::{Channel, NewChannel};
use sqlx::FromRow;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

impl Store {
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
