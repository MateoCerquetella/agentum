//! SQLite persistence for agentum. WAL mode, synchronous=NORMAL.
//!
//! All XDG path resolution lives in [`paths`].

use std::path::{Path, PathBuf};
use std::str::FromStr;

use agentum_core::{Event, NewSession, Session, Status};
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
                sqlx::query_as::<_, SessionRow>(
                    "SELECT * FROM sessions ORDER BY created_at DESC",
                )
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
        let affected = sqlx::query(
            "UPDATE sessions SET status = ?, updated_at = ? WHERE id = ?",
        )
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

    /// Persist an event row. Best-effort (failures should not break callers).
    pub async fn insert_event(&self, ev: &Event) -> Result<()> {
        let payload = serde_json::to_string(&ev.payload)?;
        let ts = ev.ts.format(&Rfc3339)?;
        sqlx::query(
            "INSERT INTO events (session_id, kind, payload, ts) VALUES (?, ?, ?, ?)",
        )
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
    created_at: String,
    updated_at: String,
    last_activity_at: Option<String>,
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
