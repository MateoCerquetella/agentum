//! Durable metadata for secure SDD integrations.
//!
//! Tokens and device private keys never enter SQLite. The database stores only
//! sanitized connection metadata and one-time OAuth redemption state.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::sdd::now;
use crate::{Result, Store, StoreError};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddIntegrationConnectionRecord {
    pub connection_id: String,
    pub provider: String,
    pub external_account_id: String,
    pub display_name: String,
    pub selected_site_id: Option<String>,
    pub metadata_json: String,
    pub credential_revision: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddOauthFlowRecord {
    pub flow_id: String,
    pub provider: String,
    pub request_id: String,
    #[serde(skip_serializing)]
    pub state_hash: String,
    #[serde(skip_serializing)]
    pub redemption_id: String,
    pub authorization_url: String,
    #[serde(skip_serializing)]
    pub device_key_ref: String,
    pub connection_id: Option<String>,
    pub status: String,
    pub revision: i64,
    pub expires_at: String,
    pub created_at: String,
    pub updated_at: String,
    pub redeemed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewSddOauthFlow<'a> {
    pub flow_id: &'a str,
    pub provider: &'a str,
    pub request_id: &'a str,
    pub state_hash: &'a str,
    pub redemption_id: &'a str,
    pub authorization_url: &'a str,
    pub device_key_ref: &'a str,
    pub expires_at: &'a str,
}

#[derive(Debug, Clone)]
pub struct UpsertSddIntegrationConnection<'a> {
    pub connection_id: &'a str,
    pub provider: &'a str,
    pub external_account_id: &'a str,
    pub display_name: &'a str,
    pub selected_site_id: Option<&'a str>,
    pub metadata_json: &'a str,
    pub credential_revision: i64,
}

impl Store {
    /// Insert or replace a non-brokered integration connection with a strict
    /// credential-revision CAS. Secret publication happens in the caller's
    /// secure vault; SQLite receives sanitized metadata only.
    pub async fn sdd_upsert_integration_connection(
        &self,
        connection: UpsertSddIntegrationConnection<'_>,
        expected_revision: i64,
    ) -> Result<()> {
        if expected_revision < 0 || connection.credential_revision != expected_revision + 1 {
            return Err(StoreError::InvalidCommand(
                "integration credential revision must advance exactly once".into(),
            ));
        }
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let current: Option<(String, i64)> = sqlx::query_as(
            "SELECT provider, credential_revision FROM sdd_integration_connections
             WHERE connection_id = ?",
        )
        .bind(connection.connection_id)
        .fetch_optional(&mut *tx)
        .await?;
        match current {
            None if expected_revision == 0 => {
                sqlx::query(
                    "INSERT INTO sdd_integration_connections
                     (connection_id, provider, external_account_id, display_name,
                      selected_site_id, metadata_json, credential_revision, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(connection.connection_id)
                .bind(connection.provider)
                .bind(connection.external_account_id)
                .bind(connection.display_name)
                .bind(connection.selected_site_id)
                .bind(connection.metadata_json)
                .bind(connection.credential_revision)
                .bind(&at)
                .bind(&at)
                .execute(&mut *tx)
                .await?;
            }
            Some((provider, current_revision))
                if provider == connection.provider && current_revision == expected_revision =>
            {
                let changed = sqlx::query(
                    "UPDATE sdd_integration_connections SET external_account_id = ?,
                     display_name = ?, selected_site_id = ?, metadata_json = ?,
                     credential_revision = ?, updated_at = ?
                     WHERE connection_id = ? AND provider = ? AND credential_revision = ?",
                )
                .bind(connection.external_account_id)
                .bind(connection.display_name)
                .bind(connection.selected_site_id)
                .bind(connection.metadata_json)
                .bind(connection.credential_revision)
                .bind(&at)
                .bind(connection.connection_id)
                .bind(connection.provider)
                .bind(expected_revision)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                if changed != 1 {
                    return Err(StoreError::StaleRevision {
                        expected: expected_revision,
                        current: current_revision,
                    });
                }
            }
            Some((_, current_revision)) => {
                return Err(StoreError::StaleRevision {
                    expected: expected_revision,
                    current: current_revision,
                });
            }
            None => {
                return Err(StoreError::StaleRevision {
                    expected: expected_revision,
                    current: 0,
                });
            }
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn sdd_create_oauth_flow(&self, input: NewSddOauthFlow<'_>) -> Result<()> {
        let at = now()?;
        sqlx::query(
            "INSERT INTO sdd_oauth_flows
             (flow_id, provider, request_id, state_hash, redemption_id, authorization_url,
              device_key_ref, status, revision, expires_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, 'pending', 1, ?, ?, ?)",
        )
        .bind(input.flow_id)
        .bind(input.provider)
        .bind(input.request_id)
        .bind(input.state_hash)
        .bind(input.redemption_id)
        .bind(input.authorization_url)
        .bind(input.device_key_ref)
        .bind(input.expires_at)
        .bind(&at)
        .bind(&at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn sdd_oauth_flow_by_request(
        &self,
        provider: &str,
        request_id: &str,
    ) -> Result<Option<SddOauthFlowRecord>> {
        Ok(
            sqlx::query_as("SELECT * FROM sdd_oauth_flows WHERE provider = ? AND request_id = ?")
                .bind(provider)
                .bind(request_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn sdd_oauth_flow(&self, flow_id: &str) -> Result<Option<SddOauthFlowRecord>> {
        Ok(
            sqlx::query_as("SELECT * FROM sdd_oauth_flows WHERE flow_id = ?")
                .bind(flow_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Claim one pending redemption. Concurrent/replayed redemption requests
    /// fail their revision/status compare-and-swap before contacting the broker.
    pub async fn sdd_claim_oauth_redemption(
        &self,
        flow_id: &str,
        expected_revision: i64,
    ) -> Result<SddOauthFlowRecord> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let flow: SddOauthFlowRecord =
            sqlx::query_as("SELECT * FROM sdd_oauth_flows WHERE flow_id = ?")
                .bind(flow_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StoreError::NotFound(format!("OAuth flow {flow_id}")))?;
        if flow.revision != expected_revision {
            return Err(StoreError::StaleRevision {
                expected: expected_revision,
                current: flow.revision,
            });
        }
        if flow.status != "pending" {
            return Err(StoreError::InvalidCommand(format!(
                "OAuth flow cannot be redeemed from {}",
                flow.status
            )));
        }
        if flow.expires_at <= at {
            sqlx::query(
                "UPDATE sdd_oauth_flows SET status = 'expired', revision = revision + 1,
                 updated_at = ? WHERE flow_id = ? AND revision = ? AND status = 'pending'",
            )
            .bind(&at)
            .bind(flow_id)
            .bind(expected_revision)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Err(StoreError::InvalidCommand("OAuth flow expired".into()));
        }
        let changed = sqlx::query(
            "UPDATE sdd_oauth_flows SET status = 'redeeming', revision = revision + 1,
             updated_at = ? WHERE flow_id = ? AND revision = ? AND status = 'pending'",
        )
        .bind(&at)
        .bind(flow_id)
        .bind(expected_revision)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StoreError::StaleRevision {
                expected: expected_revision,
                current: flow.revision,
            });
        }
        tx.commit().await?;
        self.sdd_oauth_flow(flow_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("OAuth flow {flow_id}")))
    }

    pub async fn sdd_mark_oauth_sync_pending(
        &self,
        flow_id: &str,
        expected_revision: i64,
    ) -> Result<()> {
        let at = now()?;
        let changed = sqlx::query(
            "UPDATE sdd_oauth_flows SET status = 'sync_pending', revision = revision + 1,
             updated_at = ? WHERE flow_id = ? AND revision = ? AND status = 'redeeming'",
        )
        .bind(&at)
        .bind(flow_id)
        .bind(expected_revision)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StoreError::InvalidCommand(
                "OAuth redemption is no longer claimable".into(),
            ));
        }
        Ok(())
    }

    pub async fn sdd_complete_oauth_redemption(
        &self,
        flow_id: &str,
        expected_revision: i64,
        connection: UpsertSddIntegrationConnection<'_>,
    ) -> Result<()> {
        if connection.credential_revision <= 0 {
            return Err(StoreError::InvalidCommand(
                "credential revision must be positive".into(),
            ));
        }
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query(
            "UPDATE sdd_oauth_flows SET status = 'redeemed', revision = revision + 1,
             connection_id = ?, updated_at = ?, redeemed_at = ?
             WHERE flow_id = ? AND revision = ? AND status = 'redeeming'",
        )
        .bind(connection.connection_id)
        .bind(&at)
        .bind(&at)
        .bind(flow_id)
        .bind(expected_revision)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StoreError::InvalidCommand(
                "OAuth redemption is no longer claimable".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO sdd_integration_connections
             (connection_id, provider, external_account_id, display_name, selected_site_id,
              metadata_json, credential_revision, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(connection_id) DO UPDATE SET
               external_account_id = excluded.external_account_id,
               display_name = excluded.display_name,
               selected_site_id = excluded.selected_site_id,
               metadata_json = excluded.metadata_json,
               credential_revision = excluded.credential_revision,
               updated_at = excluded.updated_at",
        )
        .bind(connection.connection_id)
        .bind(connection.provider)
        .bind(connection.external_account_id)
        .bind(connection.display_name)
        .bind(connection.selected_site_id)
        .bind(connection.metadata_json)
        .bind(connection.credential_revision)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn sdd_integration_connections(
        &self,
        provider: &str,
    ) -> Result<Vec<SddIntegrationConnectionRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_integration_connections
             WHERE provider = ? ORDER BY updated_at DESC, connection_id",
        )
        .bind(provider)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sdd_integration_connection(
        &self,
        provider: &str,
        connection_id: &str,
    ) -> Result<Option<SddIntegrationConnectionRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_integration_connections
             WHERE provider = ? AND connection_id = ?",
        )
        .bind(provider)
        .bind(connection_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn sdd_select_integration_site(
        &self,
        provider: &str,
        connection_id: &str,
        site_id: &str,
        expected_credential_revision: i64,
        metadata_json: &str,
    ) -> Result<()> {
        let at = now()?;
        let changed = sqlx::query(
            "UPDATE sdd_integration_connections SET selected_site_id = ?, metadata_json = ?,
             updated_at = ? WHERE provider = ? AND connection_id = ?
             AND credential_revision = ?",
        )
        .bind(site_id)
        .bind(metadata_json)
        .bind(&at)
        .bind(provider)
        .bind(connection_id)
        .bind(expected_credential_revision)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StoreError::InvalidCommand(
                "integration connection changed before site selection".into(),
            ));
        }
        Ok(())
    }

    pub async fn sdd_replace_integration_credential_revision(
        &self,
        provider: &str,
        connection_id: &str,
        expected_revision: i64,
        replacement_revision: i64,
    ) -> Result<()> {
        if replacement_revision != expected_revision + 1 {
            return Err(StoreError::InvalidCommand(
                "credential replacement revision must advance exactly once".into(),
            ));
        }
        let at = now()?;
        let changed = sqlx::query(
            "UPDATE sdd_integration_connections SET credential_revision = ?, updated_at = ?
             WHERE provider = ? AND connection_id = ? AND credential_revision = ?",
        )
        .bind(replacement_revision)
        .bind(&at)
        .bind(provider)
        .bind(connection_id)
        .bind(expected_revision)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StoreError::InvalidCommand(
                "integration credential changed before refresh replacement".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn oauth_redemption_is_one_time_and_connection_metadata_has_no_token() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("store.sqlite"))
            .await
            .unwrap();
        store
            .sdd_create_oauth_flow(NewSddOauthFlow {
                flow_id: "flow-1",
                provider: "jira",
                request_id: "request-1",
                state_hash: "state-hash",
                redemption_id: "redemption-1",
                authorization_url: "https://auth.atlassian.com/authorize",
                device_key_ref: "device-key-1",
                expires_at: "2999-01-01T00:00:00Z",
            })
            .await
            .unwrap();
        let claimed = store.sdd_claim_oauth_redemption("flow-1", 1).await.unwrap();
        assert_eq!(claimed.status, "redeeming");
        assert!(store.sdd_claim_oauth_redemption("flow-1", 1).await.is_err());
        store
            .sdd_complete_oauth_redemption(
                "flow-1",
                2,
                UpsertSddIntegrationConnection {
                    connection_id: "jira-account-1",
                    provider: "jira",
                    external_account_id: "account-1",
                    display_name: "Example",
                    selected_site_id: Some("site-1"),
                    metadata_json: r#"{"sites":[{"id":"site-1"}]}"#,
                    credential_revision: 1,
                },
            )
            .await
            .unwrap();
        let connection = store
            .sdd_integration_connection("jira", "jira-account-1")
            .await
            .unwrap()
            .unwrap();
        assert!(!connection.metadata_json.contains("token"));
        assert_eq!(connection.selected_site_id.as_deref(), Some("site-1"));
    }

    #[tokio::test]
    async fn non_brokered_connection_upsert_is_revision_cas_bound() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("store.sqlite"))
            .await
            .unwrap();
        let connection = |revision| UpsertSddIntegrationConnection {
            connection_id: "jira-local-account-1",
            provider: "jira",
            external_account_id: "account-local-1",
            display_name: "Example",
            selected_site_id: Some("example.atlassian.net"),
            metadata_json: r#"{"authKind":"api_token","sites":[]}"#,
            credential_revision: revision,
        };
        store
            .sdd_upsert_integration_connection(connection(1), 0)
            .await
            .unwrap();
        assert!(
            store
                .sdd_upsert_integration_connection(connection(2), 0)
                .await
                .is_err()
        );
        store
            .sdd_upsert_integration_connection(connection(2), 1)
            .await
            .unwrap();
        let saved = store
            .sdd_integration_connection("jira", "jira-local-account-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.credential_revision, 2);
        assert!(!saved.metadata_json.contains("token-value"));
    }
}
