//! Settings: a generic string key/value store (migration 0021) used for small
//! bits of persisted config. Booleans are encoded as `"1"`/`"0"`.

use crate::{Result, Store};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

impl Store {
    /// Read a setting's raw string value, or `None` when the key was never set.
    pub async fn setting_get(&self, key: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.0))
    }

    /// Upsert a setting's raw string value.
    pub async fn setting_set(&self, key: &str, value: &str) -> Result<()> {
        let now_s = OffsetDateTime::now_utc().format(&Rfc3339)?;
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value)
        .bind(&now_s)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Read a boolean setting (stored as `"1"`/`"0"`), falling back to `default`
    /// when unset or unparseable.
    pub async fn setting_get_bool(&self, key: &str, default: bool) -> Result<bool> {
        Ok(self
            .setting_get(key)
            .await?
            // Why: fall back to `default` for any value that isn't a canonical
            // "1"/"0" — not just when the key is absent. `setting_set_bool`
            // only ever writes the canonical pair, but a value written by a
            // different code path (or a future non-boolean reuse of the key)
            // should not silently read as `false`, which `== "1"` would do.
            .map(|v| match v.as_str() {
                "1" => true,
                "0" => false,
                _ => default,
            })
            .unwrap_or(default))
    }

    /// Upsert a boolean setting as `"1"`/`"0"`.
    pub async fn setting_set_bool(&self, key: &str, value: bool) -> Result<()> {
        self.setting_set(key, if value { "1" } else { "0" }).await
    }
}
