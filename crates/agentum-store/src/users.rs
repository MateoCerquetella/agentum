//! Users + auth sessions: the credential store (Argon2id hashes) and bearer
//! token lifecycle (sliding + absolute expiry, swept periodically).

use crate::{Result, Store, StoreError};
use agentum_core::User;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

impl Store {
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
