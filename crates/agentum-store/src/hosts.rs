//! Hosts: the SSH-host catalog (migration 0018). The `local` host is an
//! immutable pseudo-entry; SSH hosts carry connection + auth fields. Sessions
//! reference a host by `host_id`.

use crate::{Result, Store, StoreError};
use agentum_core::{Host, HostKind, LOCAL_HOST_ID, NewHost, SshAuth};
use sqlx::FromRow;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

impl Store {
    pub async fn list_hosts(&self) -> Result<Vec<Host>> {
        let rows: Vec<HostRow> =
            sqlx::query_as("SELECT * FROM hosts ORDER BY kind = 'local' DESC, name ASC")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter().map(Host::try_from).collect()
    }

    pub async fn get_host(&self, id: Uuid) -> Result<Option<Host>> {
        let row = sqlx::query_as::<_, HostRow>("SELECT * FROM hosts WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(Host::try_from).transpose()
    }

    pub async fn create_host(&self, new: NewHost) -> Result<Host> {
        let id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        let now_s = now.format(&Rfc3339)?;
        let parts = host_kind_parts(&new.kind);
        let res = sqlx::query(
            r#"
            INSERT INTO hosts
                (id, name, kind, user, hostname, port, auth_kind, key_path, secret, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(id.to_string())
        .bind(&new.name)
        .bind(parts.kind)
        .bind(parts.user)
        .bind(parts.hostname)
        .bind(parts.port.map(i64::from))
        .bind(parts.auth_kind)
        .bind(parts.key_path)
        .bind(parts.secret)
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
        Ok(Host {
            id,
            name: new.name,
            kind: new.kind,
            created_at: now,
            updated_at: now,
            last_seen_at: None,
        })
    }

    /// Edit an existing SSH host in place. Rewrites every connection field
    /// (name, user, hostname, port, auth + its secret) from `new` and bumps
    /// `updated_at`, preserving `id`, `created_at`, and `last_seen_at` so the
    /// row keeps its identity (sessions reference `host_id`) and its sidebar
    /// dot history. The local pseudo-host is immutable. Returns the refreshed
    /// [`Host`], `NotFound` if the id doesn't exist (or is the immutable local
    /// host — mirrors `delete_host`, which treats it as "no such editable
    /// host"), or `AlreadyExists` when a rename collides with another host.
    pub async fn update_host(&self, id: Uuid, new: NewHost) -> Result<Host> {
        if id == LOCAL_HOST_ID {
            return Err(StoreError::NotFound(
                "the local host is not editable".into(),
            ));
        }
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let parts = host_kind_parts(&new.kind);
        let res = sqlx::query(
            r#"
            UPDATE hosts SET
                name = ?, kind = ?, user = ?, hostname = ?, port = ?,
                auth_kind = ?, key_path = ?, secret = ?, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&new.name)
        .bind(parts.kind)
        .bind(parts.user)
        .bind(parts.hostname)
        .bind(parts.port.map(i64::from))
        .bind(parts.auth_kind)
        .bind(parts.key_path)
        .bind(parts.secret)
        .bind(&now_s)
        .bind(id.to_string())
        .execute(&self.pool)
        .await;
        if let Err(sqlx::Error::Database(db)) = &res {
            if db.is_unique_violation() {
                return Err(StoreError::AlreadyExists(new.name));
            }
        }
        let affected = res?.rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.get_host(id)
            .await?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub async fn update_host_seen(&self, id: Uuid) -> Result<()> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        sqlx::query("UPDATE hosts SET last_seen_at = ?, updated_at = ? WHERE id = ?")
            .bind(&now_s)
            .bind(&now_s)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_host(&self, id: Uuid) -> Result<bool> {
        if id == LOCAL_HOST_ID {
            return Ok(false);
        }
        let in_use: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE host_id = ?")
            .bind(id.to_string())
            .fetch_one(&self.pool)
            .await?;
        if in_use > 0 {
            return Err(StoreError::AlreadyExists(format!(
                "host has {in_use} session(s)"
            )));
        }
        let affected = sqlx::query("DELETE FROM hosts WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(affected > 0)
    }
}

struct HostKindParts<'a> {
    kind: &'static str,
    user: Option<&'a str>,
    hostname: Option<&'a str>,
    port: Option<u16>,
    auth_kind: Option<&'static str>,
    key_path: Option<&'a str>,
    /// SSH password, only set for `SshAuth::Password`. Persisted to the
    /// `hosts.secret` column (migration 0019).
    secret: Option<&'a str>,
}

fn host_kind_parts(kind: &HostKind) -> HostKindParts<'_> {
    match kind {
        HostKind::Local => HostKindParts {
            kind: "local",
            user: None,
            hostname: None,
            port: None,
            auth_kind: None,
            key_path: None,
            secret: None,
        },
        HostKind::Ssh {
            user,
            hostname,
            port,
            auth,
        } => {
            let base = HostKindParts {
                kind: "ssh",
                user: Some(user.as_str()),
                hostname: Some(hostname.as_str()),
                port: Some(*port),
                auth_kind: Some("agent"),
                key_path: None,
                secret: None,
            };
            match auth {
                SshAuth::Agent => base,
                SshAuth::Key { path } => HostKindParts {
                    auth_kind: Some("key"),
                    key_path: Some(path.as_str()),
                    ..base
                },
                SshAuth::Password { password } => HostKindParts {
                    auth_kind: Some("password"),
                    secret: Some(password.as_str()),
                    ..base
                },
            }
        }
    }
}

#[derive(Debug, FromRow)]
struct HostRow {
    id: String,
    name: String,
    kind: String,
    user: Option<String>,
    hostname: Option<String>,
    port: Option<i64>,
    auth_kind: Option<String>,
    key_path: Option<String>,
    #[sqlx(default)]
    secret: Option<String>,
    created_at: String,
    updated_at: String,
    last_seen_at: Option<String>,
}

impl TryFrom<HostRow> for Host {
    type Error = StoreError;
    fn try_from(r: HostRow) -> Result<Self> {
        let kind = match r.kind.as_str() {
            "local" => HostKind::Local,
            "ssh" => {
                let auth = match r.auth_kind.as_deref() {
                    Some("key") => SshAuth::Key {
                        path: r.key_path.unwrap_or_default(),
                    },
                    Some("password") => SshAuth::Password {
                        password: r.secret.unwrap_or_default(),
                    },
                    _ => SshAuth::Agent,
                };
                HostKind::Ssh {
                    user: r.user.unwrap_or_default(),
                    hostname: r.hostname.unwrap_or_default(),
                    port: r.port.unwrap_or(22) as u16,
                    auth,
                }
            }
            other => return Err(StoreError::NotFound(format!("unknown host kind: {other}"))),
        };
        Ok(Host {
            id: Uuid::parse_str(&r.id)?,
            name: r.name,
            kind,
            created_at: OffsetDateTime::parse(&r.created_at, &Rfc3339)?,
            updated_at: OffsetDateTime::parse(&r.updated_at, &Rfc3339)?,
            last_seen_at: r
                .last_seen_at
                .as_deref()
                .map(|s| OffsetDateTime::parse(s, &Rfc3339))
                .transpose()?,
        })
    }
}
