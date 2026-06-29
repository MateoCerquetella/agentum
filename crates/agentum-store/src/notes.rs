//! Notes: simple titled/bodied scratch items with JSON tags.

use crate::{Result, Store, StoreError};
use agentum_core::{NewNote, Note, NotePatch};
use sqlx::FromRow;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

impl Store {
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
