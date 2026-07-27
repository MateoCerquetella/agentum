//! Durable, hash-bound delivery authorization and per-action execution state.
//!
//! Delivery is deliberately orthogonal to the lifecycle state machine: a run
//! remains `ready/succeeded` while selected external side effects are applied.
//! That makes partial failure retryable without turning a verified local result
//! into a false failure (or a false Completed state).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;

use crate::sdd::{EventInsert, append_event, now};
use crate::{Result, Store, StoreError};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddDeliveryPreviewRecord {
    pub preview_id: String,
    pub run_id: String,
    pub actor_id: String,
    #[serde(skip_serializing)]
    pub token_hash: String,
    pub digest: String,
    pub run_revision: i64,
    pub spec_revision: i64,
    pub actions_json: String,
    pub status: String,
    pub expires_at: String,
    pub created_at: String,
    pub confirmed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddDeliveryActionRecord {
    pub preview_id: String,
    pub action_id: String,
    pub action_type: String,
    pub intent_json: String,
    pub status: String,
    pub result_json: Option<String>,
    pub attempts: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NewDeliveryPreview<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub actor_id: &'a str,
    pub preview_id: &'a str,
    pub token_hash: &'a str,
    pub digest: &'a str,
    pub spec_revision: i64,
    pub actions_json: &'a str,
    pub expires_at: &'a str,
    pub event_json: &'a str,
    pub response_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct NewDeliveryAction<'a> {
    pub action_id: &'a str,
    pub action_type: &'a str,
    pub intent_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct ConfirmDelivery<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub actor_id: &'a str,
    pub token_hash: &'a str,
    pub digest: &'a str,
    pub selected: &'a [NewDeliveryAction<'a>],
    pub response_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct DeliveryActionResult<'a> {
    pub preview_id: &'a str,
    pub action_id: &'a str,
    pub status: &'a str,
    pub result_json: &'a str,
}

impl Store {
    /// Mark actions interrupted by process death as ambiguous. They are never
    /// replayed automatically because the external side effect may already
    /// have happened; an authenticated confirm retry performs reconciliation.
    pub async fn sdd_recover_interrupted_delivery(&self) -> Result<u64> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let actions: Vec<InterruptedDeliveryRow> = sqlx::query_as(
            "SELECT a.preview_id, a.action_id, a.action_type, r.repo_id, r.spec_id,
                    r.run_id, r.aggregate_revision
             FROM sdd_delivery_actions a
             JOIN sdd_delivery_previews p ON p.preview_id = a.preview_id
             JOIN sdd_runs r ON r.run_id = p.run_id
             WHERE a.status = 'running' ORDER BY r.run_id, a.action_id",
        )
        .fetch_all(&mut *tx)
        .await?;
        let mut recovered = 0_u64;
        for action in actions {
            let current: i64 =
                sqlx::query_scalar("SELECT aggregate_revision FROM sdd_runs WHERE run_id = ?")
                    .bind(&action.run_id)
                    .fetch_one(&mut *tx)
                    .await?;
            let result = serde_json::json!({
                "summary": "server restarted while delivery action was active"
            })
            .to_string();
            let changed = sqlx::query(
                "UPDATE sdd_delivery_actions SET status = 'sync_pending', result_json = ?,
                 updated_at = ? WHERE preview_id = ? AND action_id = ? AND status = 'running'",
            )
            .bind(&result)
            .bind(&at)
            .bind(&action.preview_id)
            .bind(&action.action_id)
            .execute(&mut *tx)
            .await?;
            if changed.rows_affected() != 1 {
                continue;
            }
            recovered += 1;
            let next = current + 1;
            sqlx::query(
                "UPDATE sdd_runs SET aggregate_revision = ?, updated_at = ?
                 WHERE run_id = ? AND aggregate_revision = ?",
            )
            .bind(next)
            .bind(&at)
            .bind(&action.run_id)
            .bind(current)
            .execute(&mut *tx)
            .await?;
            let payload = serde_json::json!({
                "runId": action.run_id,
                "previewId": action.preview_id,
                "actionId": action.action_id,
                "actionType": action.action_type,
                "status": "sync_pending",
                "revision": next,
                "reason": "server restarted while action was active"
            })
            .to_string();
            append_event(
                &mut tx,
                EventInsert {
                    repo_id: &action.repo_id,
                    spec_id: Some(&action.spec_id),
                    run_id: Some(&action.run_id),
                    revision: next,
                    kind: "sdd.delivery.action_sync_pending",
                    payload_json: &payload,
                    created_at: &at,
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(recovered)
    }

    /// Persist a preview, invalidate older pending previews, and CAS the run.
    /// The opaque confirmation token is never stored in this table, only its
    /// SHA-256. The idempotency response can reproduce it, but the token is not
    /// authorization by itself: confirmation is also bound to `actor_id`.
    pub async fn sdd_create_delivery_preview(&self, input: NewDeliveryPreview<'_>) -> Result<i64> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let run: Option<(String, String, String, String, i64, i64)> = sqlx::query_as(
            "SELECT r.repo_id, r.spec_id, r.phase, r.status, r.aggregate_revision, r.quarantined
             FROM sdd_runs r WHERE r.run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, phase, status, current, quarantined)) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        if phase != "ready" || status != "succeeded" || quarantined != 0 {
            return Err(StoreError::InvalidCommand(
                "delivery preview requires a non-quarantined Ready run".into(),
            ));
        }
        let current_spec_revision: i64 =
            sqlx::query_scalar("SELECT current_revision FROM sdd_specs WHERE spec_id = ?")
                .bind(&spec_id)
                .fetch_one(&mut *tx)
                .await?;
        if current_spec_revision != input.spec_revision {
            return Err(StoreError::InvalidCommand(
                "delivery preview does not target the current specification revision".into(),
            ));
        }
        if input.expires_at <= at.as_str() {
            return Err(StoreError::InvalidCommand(
                "delivery preview expiry must be in the future".into(),
            ));
        }
        sqlx::query(
            "UPDATE sdd_delivery_previews SET status = 'expired'
             WHERE run_id = ? AND status = 'pending' AND expires_at <= ?",
        )
        .bind(input.run_id)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_delivery_previews SET status = 'invalidated'
             WHERE run_id = ? AND status = 'pending'",
        )
        .bind(input.run_id)
        .execute(&mut *tx)
        .await?;
        let next = current + 1;
        sqlx::query(
            "INSERT INTO sdd_delivery_previews
             (preview_id, run_id, actor_id, token_hash, digest, run_revision, spec_revision,
              actions_json, status, expires_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
        )
        .bind(input.preview_id)
        .bind(input.run_id)
        .bind(input.actor_id)
        .bind(input.token_hash)
        .bind(input.digest)
        .bind(next)
        .bind(input.spec_revision)
        .bind(input.actions_json)
        .bind(input.expires_at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        let updated = sqlx::query(
            "UPDATE sdd_runs SET aggregate_revision = ?, updated_at = ?
             WHERE run_id = ? AND aggregate_revision = ?
               AND phase = 'ready' AND status = 'succeeded' AND quarantined = 0",
        )
        .bind(next)
        .bind(&at)
        .bind(input.run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::StaleRevision {
                expected: current,
                current: next,
            });
        }
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(input.run_id),
                revision: next,
                kind: "sdd.delivery.previewed",
                payload_json: input.event_json,
                created_at: &at,
            },
        )
        .await?;
        insert_idempotency(
            &mut tx,
            input.run_id,
            input.request_id,
            input.request_hash,
            current,
            input.response_json,
            &at,
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    pub async fn sdd_delivery_preview_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<SddDeliveryPreviewRecord>> {
        Ok(
            sqlx::query_as("SELECT * FROM sdd_delivery_previews WHERE token_hash = ? LIMIT 1")
                .bind(token_hash)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn sdd_delivery_preview(
        &self,
        preview_id: &str,
    ) -> Result<Option<SddDeliveryPreviewRecord>> {
        Ok(
            sqlx::query_as("SELECT * FROM sdd_delivery_previews WHERE preview_id = ?")
                .bind(preview_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn sdd_latest_delivery_preview_for_run(
        &self,
        run_id: &str,
    ) -> Result<Option<SddDeliveryPreviewRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_delivery_previews
             WHERE run_id = ? ORDER BY created_at DESC, preview_id DESC LIMIT 1",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn sdd_delivery_actions(
        &self,
        preview_id: &str,
    ) -> Result<Vec<SddDeliveryActionRecord>> {
        Ok(
            sqlx::query_as(
                "SELECT * FROM sdd_delivery_actions WHERE preview_id = ? ORDER BY rowid",
            )
            .bind(preview_id)
            .fetch_all(&self.pool)
            .await?,
        )
    }

    pub async fn sdd_external_link_for_spec(
        &self,
        spec_id: &str,
    ) -> Result<Option<SddExternalLinkRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_external_links WHERE spec_id = ? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(spec_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Confirm an exact, still-current preview. A previously confirmed preview
    /// may only be used to reset failed/ambiguous actions; it cannot authorize
    /// new side effects after the workspace or run revision has moved.
    pub async fn sdd_confirm_delivery(&self, input: ConfirmDelivery<'_>) -> Result<i64> {
        if input.selected.is_empty() {
            return Err(StoreError::InvalidCommand(
                "at least one delivery action is required".into(),
            ));
        }
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let preview: Option<SddDeliveryPreviewRecord> =
            sqlx::query_as("SELECT * FROM sdd_delivery_previews WHERE token_hash = ?")
                .bind(input.token_hash)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(preview) = preview else {
            return Err(StoreError::InvalidCommand(
                "delivery preview token is invalid".into(),
            ));
        };
        if preview.run_id != input.run_id
            || preview.actor_id != input.actor_id
            || preview.digest != input.digest
        {
            return Err(StoreError::InvalidCommand(
                "delivery preview does not match the actor, run, or digest".into(),
            ));
        }
        let run: Option<(String, String, String, String, i64, i64)> = sqlx::query_as(
            "SELECT repo_id, spec_id, phase, status, aggregate_revision, quarantined
             FROM sdd_runs WHERE run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, phase, status, current, quarantined)) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        if phase != "ready" || status != "succeeded" || quarantined != 0 {
            return Err(StoreError::InvalidCommand(
                "delivery confirmation requires a non-quarantined Ready run".into(),
            ));
        }
        let current_spec_revision: i64 =
            sqlx::query_scalar("SELECT current_revision FROM sdd_specs WHERE spec_id = ?")
                .bind(&spec_id)
                .fetch_one(&mut *tx)
                .await?;
        if current_spec_revision != preview.spec_revision {
            return Err(StoreError::InvalidCommand(
                "delivery preview was invalidated by a specification revision".into(),
            ));
        }
        validate_selected_against_preview(&preview.actions_json, input.selected)?;

        let first_confirmation = preview.status == "pending";
        let selected_ids = input
            .selected
            .iter()
            .map(|action| action.action_id)
            .collect::<std::collections::HashSet<_>>();
        for action in input.selected {
            let intent: Value = serde_json::from_str(action.intent_json)?;
            let dependencies = intent
                .get("dependsOn")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    StoreError::InvalidCommand("delivery dependencies are malformed".into())
                })?;
            for dependency in dependencies {
                let dependency = dependency.as_str().ok_or_else(|| {
                    StoreError::InvalidCommand("delivery dependency id is malformed".into())
                })?;
                if selected_ids.contains(dependency) {
                    continue;
                }
                if first_confirmation {
                    return Err(StoreError::InvalidCommand(format!(
                        "delivery action {} requires selected dependency {dependency}",
                        action.action_id
                    )));
                }
                let succeeded: i64 = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM sdd_delivery_actions
                     WHERE preview_id = ? AND action_id = ? AND status = 'succeeded')",
                )
                .bind(&preview.preview_id)
                .bind(dependency)
                .fetch_one(&mut *tx)
                .await?;
                if succeeded == 0 {
                    return Err(StoreError::InvalidCommand(format!(
                        "delivery dependency {dependency} has not succeeded"
                    )));
                }
            }
        }
        if first_confirmation {
            if preview.expires_at <= at {
                sqlx::query(
                    "UPDATE sdd_delivery_previews SET status = 'expired' WHERE preview_id = ?",
                )
                .bind(&preview.preview_id)
                .execute(&mut *tx)
                .await?;
                return Err(StoreError::InvalidCommand(
                    "delivery preview has expired".into(),
                ));
            }
            if current != preview.run_revision {
                return Err(StoreError::InvalidCommand(
                    "delivery preview is stale; create a new preview".into(),
                ));
            }
            sqlx::query(
                "UPDATE sdd_delivery_previews SET status = 'confirmed', confirmed_at = ?
                 WHERE preview_id = ? AND status = 'pending'",
            )
            .bind(&at)
            .bind(&preview.preview_id)
            .execute(&mut *tx)
            .await?;
            for action in input.selected {
                sqlx::query(
                    "INSERT INTO sdd_delivery_actions
                     (preview_id, action_id, action_type, intent_json, status, attempts, updated_at)
                     VALUES (?, ?, ?, ?, 'pending', 0, ?)",
                )
                .bind(&preview.preview_id)
                .bind(action.action_id)
                .bind(action.action_type)
                .bind(action.intent_json)
                .bind(&at)
                .execute(&mut *tx)
                .await?;
            }
        } else if preview.status == "confirmed" {
            let mut reset = 0_u64;
            for action in input.selected {
                reset += sqlx::query(
                    "UPDATE sdd_delivery_actions
                     SET status = 'pending', result_json = NULL, updated_at = ?
                     WHERE preview_id = ? AND action_id = ?
                       AND status IN ('failed', 'sync_pending')",
                )
                .bind(&at)
                .bind(&preview.preview_id)
                .bind(action.action_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            }
            if reset != input.selected.len() as u64 {
                return Err(StoreError::InvalidCommand(
                    "every selected delivery action must be failed or sync-pending".into(),
                ));
            }
        } else {
            return Err(StoreError::InvalidCommand(format!(
                "delivery preview is {}",
                preview.status
            )));
        }

        let next = current + 1;
        let updated = sqlx::query(
            "UPDATE sdd_runs SET aggregate_revision = ?, updated_at = ?
             WHERE run_id = ? AND aggregate_revision = ?
               AND phase = 'ready' AND status = 'succeeded' AND quarantined = 0",
        )
        .bind(next)
        .bind(&at)
        .bind(input.run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::StaleRevision {
                expected: current,
                current: next,
            });
        }
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(input.run_id),
                revision: next,
                kind: if first_confirmation {
                    "sdd.delivery.confirmed"
                } else {
                    "sdd.delivery.retry_requested"
                },
                payload_json: input.response_json,
                created_at: &at,
            },
        )
        .await?;
        insert_idempotency(
            &mut tx,
            input.run_id,
            input.request_id,
            input.request_hash,
            current,
            input.response_json,
            &at,
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    /// Claim one runnable action. Dependencies are action IDs stored in the
    /// immutable intent. Claims and events are committed together.
    pub async fn sdd_claim_delivery_action(
        &self,
        preview_id: &str,
        action_id: &str,
    ) -> Result<Option<SddDeliveryActionRecord>> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let row: Option<DeliveryClaimRow> = sqlx::query_as(
            "SELECT a.preview_id, a.action_id, a.action_type, a.intent_json, a.status,
                        a.result_json, a.attempts, a.updated_at,
                        r.repo_id, r.spec_id, r.run_id, p.status AS preview_status,
                        r.aggregate_revision,
                        r.quarantined
                 FROM sdd_delivery_actions a
                 JOIN sdd_delivery_previews p ON p.preview_id = a.preview_id
                 JOIN sdd_runs r ON r.run_id = p.run_id
                 WHERE a.preview_id = ? AND a.action_id = ?
                   AND r.phase = 'ready' AND r.status = 'succeeded'",
        )
        .bind(preview_id)
        .bind(action_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut action = SddDeliveryActionRecord {
            preview_id: row.preview_id,
            action_id: row.action_id,
            action_type: row.action_type,
            intent_json: row.intent_json,
            status: row.status,
            result_json: row.result_json,
            attempts: row.attempts,
            updated_at: row.updated_at,
        };
        if action.status != "pending" || row.preview_status != "confirmed" || row.quarantined != 0 {
            return Ok(None);
        }
        let intent: Value = serde_json::from_str(&action.intent_json)?;
        let dependencies = intent
            .get("dependsOn")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                StoreError::InvalidCommand("delivery dependencies are malformed".into())
            })?;
        for dependency in dependencies {
            let Some(dependency) = dependency.as_str() else {
                return Err(StoreError::InvalidCommand(
                    "delivery dependency id is malformed".into(),
                ));
            };
            let succeeded: i64 = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sdd_delivery_actions
                 WHERE preview_id = ? AND action_id = ? AND status = 'succeeded')",
            )
            .bind(preview_id)
            .bind(dependency)
            .fetch_one(&mut *tx)
            .await?;
            if succeeded == 0 {
                return Ok(None);
            }
        }
        let changed = sqlx::query(
            "UPDATE sdd_delivery_actions SET status = 'running', attempts = attempts + 1,
             updated_at = ? WHERE preview_id = ? AND action_id = ? AND status = 'pending'",
        )
        .bind(&at)
        .bind(preview_id)
        .bind(action_id)
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() != 1 {
            return Ok(None);
        }
        action.status = "running".into();
        action.attempts += 1;
        action.updated_at.clone_from(&at);
        let next = row.aggregate_revision + 1;
        sqlx::query(
            "UPDATE sdd_runs SET aggregate_revision = ?, updated_at = ?
             WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(next)
        .bind(&at)
        .bind(&row.run_id)
        .bind(row.aggregate_revision)
        .execute(&mut *tx)
        .await?;
        let payload = serde_json::json!({
            "runId": row.run_id,
            "previewId": preview_id,
            "actionId": action_id,
            "actionType": action.action_type,
            "status": "running",
            "attempt": action.attempts,
            "revision": next
        })
        .to_string();
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &row.repo_id,
                spec_id: Some(&row.spec_id),
                run_id: Some(&row.run_id),
                revision: next,
                kind: "sdd.delivery.action_started",
                payload_json: &payload,
                created_at: &at,
            },
        )
        .await?;
        tx.commit().await?;
        Ok(Some(action))
    }

    /// Finish one action while preserving `ready/succeeded`, even on failure.
    pub async fn sdd_record_delivery_action_result(
        &self,
        input: DeliveryActionResult<'_>,
    ) -> Result<i64> {
        if !matches!(input.status, "succeeded" | "failed" | "sync_pending") {
            return Err(StoreError::InvalidCommand(
                "invalid delivery action result".into(),
            ));
        }
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let row: Option<(String, String, String, i64, String)> = sqlx::query_as(
            "SELECT r.repo_id, r.spec_id, r.run_id, r.aggregate_revision, a.action_type
             FROM sdd_delivery_actions a
             JOIN sdd_delivery_previews p ON p.preview_id = a.preview_id
             JOIN sdd_runs r ON r.run_id = p.run_id
             WHERE a.preview_id = ? AND a.action_id = ? AND a.status = 'running'
               AND r.phase = 'ready' AND r.status = 'succeeded' AND r.quarantined = 0",
        )
        .bind(input.preview_id)
        .bind(input.action_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, run_id, current, action_type)) = row else {
            return Err(StoreError::InvalidCommand(
                "delivery action is not running".into(),
            ));
        };
        sqlx::query(
            "UPDATE sdd_delivery_actions SET status = ?, result_json = ?, updated_at = ?
             WHERE preview_id = ? AND action_id = ? AND status = 'running'",
        )
        .bind(input.status)
        .bind(input.result_json)
        .bind(&at)
        .bind(input.preview_id)
        .bind(input.action_id)
        .execute(&mut *tx)
        .await?;
        let next = current + 1;
        sqlx::query(
            "UPDATE sdd_runs SET aggregate_revision = ?, updated_at = ?
             WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(next)
        .bind(&at)
        .bind(&run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        let payload = serde_json::json!({
            "runId": run_id,
            "previewId": input.preview_id,
            "actionId": input.action_id,
            "actionType": action_type,
            "status": input.status,
            "result": serde_json::from_str::<Value>(input.result_json)
                .unwrap_or_else(|_| serde_json::json!({"summary": "result unavailable"})),
            "revision": next
        })
        .to_string();
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(&run_id),
                revision: next,
                kind: match input.status {
                    "succeeded" => "sdd.delivery.action_succeeded",
                    "sync_pending" => "sdd.delivery.action_sync_pending",
                    _ => "sdd.delivery.action_failed",
                },
                payload_json: &payload,
                created_at: &at,
            },
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }
}

#[derive(Debug, FromRow)]
struct InterruptedDeliveryRow {
    preview_id: String,
    action_id: String,
    action_type: String,
    repo_id: String,
    spec_id: String,
    run_id: String,
    #[allow(dead_code)]
    aggregate_revision: i64,
}

#[derive(Debug, FromRow)]
struct DeliveryClaimRow {
    preview_id: String,
    action_id: String,
    action_type: String,
    intent_json: String,
    status: String,
    result_json: Option<String>,
    attempts: i64,
    updated_at: String,
    repo_id: String,
    spec_id: String,
    run_id: String,
    preview_status: String,
    aggregate_revision: i64,
    quarantined: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddExternalLinkRecord {
    pub link_id: String,
    pub spec_id: String,
    pub provider: String,
    pub connection_id: String,
    pub site_id: Option<String>,
    pub external_id: String,
    pub key: Option<String>,
    pub url: String,
    pub source_revision: String,
    pub created_at: String,
}

fn validate_selected_against_preview(
    actions_json: &str,
    selected: &[NewDeliveryAction<'_>],
) -> Result<()> {
    let envelope: Value = serde_json::from_str(actions_json)?;
    let offered = envelope
        .get("actions")
        .and_then(Value::as_array)
        .ok_or_else(|| StoreError::InvalidCommand("delivery preview is malformed".into()))?;
    let mut ids = std::collections::HashSet::new();
    for action in selected {
        if !ids.insert(action.action_id) {
            return Err(StoreError::InvalidCommand(
                "delivery action ids must be unique".into(),
            ));
        }
        let intent: Value = serde_json::from_str(action.intent_json)?;
        let matches = offered.iter().any(|candidate| {
            candidate.get("id").and_then(Value::as_str) == Some(action.action_id)
                && candidate.get("type").and_then(Value::as_str) == Some(action.action_type)
                && candidate == &intent
        });
        if !matches {
            return Err(StoreError::InvalidCommand(format!(
                "delivery action {} was not offered by this preview",
                action.action_id
            )));
        }
    }
    Ok(())
}

async fn insert_idempotency(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    run_id: &str,
    request_id: &str,
    request_hash: &str,
    expected_revision: i64,
    response_json: &str,
    at: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO sdd_idempotency
         (scope, request_id, request_hash, expected_revision, response_json, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(format!("run:{run_id}"))
    .bind(request_id)
    .bind(request_hash)
    .bind(expected_revision)
    .bind(response_json)
    .bind(at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ready_store() -> (tempfile::TempDir, Store) {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("delivery.sqlite"))
            .await
            .unwrap();
        let at = now().unwrap();
        sqlx::query(
            "INSERT INTO sdd_specs
             (spec_id, spec_ulid, repo_id, title, slug, profile, control, provider,
              current_revision, aggregate_revision, created_at, updated_at)
             VALUES ('SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV',
                     '01ARZ3NDEKTSV4RRFFQ69G5FAV', 'repo-1', 'Ready spec', 'ready-spec',
                     'standard', 'guarded', 'codex', 4, 1, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(store.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sdd_runs
             (run_id, spec_id, repo_id, phase, status, aggregate_revision, base_ref,
              base_commit, branch_name, authoritative_path, workspace_fingerprint,
              policy_json, created_at, updated_at)
             VALUES ('run-1', 'SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV', 'repo-1', 'ready',
                     'succeeded', 7, 'HEAD', 'abc', 'agentum/test', '/tmp/test',
                     'fingerprint', '{}', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(store.pool())
        .await
        .unwrap();
        (temp, store)
    }

    fn action_json() -> String {
        serde_json::json!({
            "id": "action-1",
            "type": "commit",
            "dependsOn": [],
            "intent": {"type": "commit", "message": "Ship verified work"}
        })
        .to_string()
    }

    async fn preview(store: &Store) {
        let expires = (time::OffsetDateTime::now_utc() + time::Duration::hours(1))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let actions = serde_json::json!({
            "actions": [serde_json::from_str::<Value>(&action_json()).unwrap()]
        })
        .to_string();
        store
            .sdd_create_delivery_preview(NewDeliveryPreview {
                request_id: "preview-request",
                request_hash: "preview-hash",
                run_id: "run-1",
                expected_revision: 7,
                actor_id: "human-1",
                preview_id: "preview-1",
                token_hash: "token-hash",
                digest: "digest-1",
                spec_revision: 4,
                actions_json: &actions,
                expires_at: &expires,
                event_json: r#"{"previewId":"preview-1"}"#,
                response_json: r#"{"previewToken":"opaque"}"#,
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn preview_and_partial_failure_preserve_ready_and_are_retryable() {
        let (_temp, store) = ready_store().await;
        preview(&store).await;
        let after_preview = store.sdd_get_run("run-1").await.unwrap().unwrap();
        assert_eq!(after_preview.aggregate_revision, 8);
        assert_eq!(
            (after_preview.phase.as_str(), after_preview.status.as_str()),
            ("ready", "succeeded")
        );
        let replay = store
            .sdd_idempotent_response("run:run-1", "preview-request", "preview-hash")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replay["previewToken"], "opaque");

        let intent = action_json();
        let selected = [NewDeliveryAction {
            action_id: "action-1",
            action_type: "commit",
            intent_json: &intent,
        }];
        store
            .sdd_confirm_delivery(ConfirmDelivery {
                request_id: "confirm-request",
                request_hash: "confirm-hash",
                run_id: "run-1",
                expected_revision: 8,
                actor_id: "human-1",
                token_hash: "token-hash",
                digest: "digest-1",
                selected: &selected,
                response_json: r#"{"status":"succeeded"}"#,
            })
            .await
            .unwrap();
        let claimed = store
            .sdd_claim_delivery_action("preview-1", "action-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.status, "running");
        store
            .sdd_record_delivery_action_result(DeliveryActionResult {
                preview_id: "preview-1",
                action_id: "action-1",
                status: "sync_pending",
                result_json: r#"{"summary":"ambiguous"}"#,
            })
            .await
            .unwrap();
        let failed_run = store.sdd_get_run("run-1").await.unwrap().unwrap();
        assert_eq!(
            (failed_run.phase.as_str(), failed_run.status.as_str()),
            ("ready", "succeeded")
        );
        assert_eq!(failed_run.aggregate_revision, 11);

        store
            .sdd_confirm_delivery(ConfirmDelivery {
                request_id: "retry-request",
                request_hash: "retry-hash",
                run_id: "run-1",
                expected_revision: 11,
                actor_id: "human-1",
                token_hash: "token-hash",
                digest: "digest-1",
                selected: &selected,
                response_json: r#"{"retry":true}"#,
            })
            .await
            .unwrap();
        let actions = store.sdd_delivery_actions("preview-1").await.unwrap();
        assert_eq!(actions[0].status, "pending");
        assert_eq!(actions[0].attempts, 1);
    }

    #[tokio::test]
    async fn actor_mismatch_and_stale_revision_cannot_confirm() {
        let (_temp, store) = ready_store().await;
        preview(&store).await;
        let intent = action_json();
        let selected = [NewDeliveryAction {
            action_id: "action-1",
            action_type: "commit",
            intent_json: &intent,
        }];
        let actor_error = store
            .sdd_confirm_delivery(ConfirmDelivery {
                request_id: "actor-request",
                request_hash: "actor-hash",
                run_id: "run-1",
                expected_revision: 8,
                actor_id: "human-2",
                token_hash: "token-hash",
                digest: "digest-1",
                selected: &selected,
                response_json: "{}",
            })
            .await
            .unwrap_err();
        assert!(actor_error.to_string().contains("actor"));
        let stale = store
            .sdd_confirm_delivery(ConfirmDelivery {
                request_id: "stale-request",
                request_hash: "stale-hash",
                run_id: "run-1",
                expected_revision: 7,
                actor_id: "human-1",
                token_hash: "token-hash",
                digest: "digest-1",
                selected: &selected,
                response_json: "{}",
            })
            .await
            .unwrap_err();
        assert!(matches!(
            stale,
            StoreError::StaleRevision { current: 8, .. }
        ));
    }

    #[tokio::test]
    async fn restart_marks_running_action_ambiguous_without_losing_ready() {
        let (_temp, store) = ready_store().await;
        preview(&store).await;
        let intent = action_json();
        let selected = [NewDeliveryAction {
            action_id: "action-1",
            action_type: "commit",
            intent_json: &intent,
        }];
        store
            .sdd_confirm_delivery(ConfirmDelivery {
                request_id: "confirm-before-restart",
                request_hash: "confirm-before-restart-hash",
                run_id: "run-1",
                expected_revision: 8,
                actor_id: "human-1",
                token_hash: "token-hash",
                digest: "digest-1",
                selected: &selected,
                response_json: "{}",
            })
            .await
            .unwrap();
        store
            .sdd_claim_delivery_action("preview-1", "action-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(store.sdd_recover_interrupted_delivery().await.unwrap(), 1);
        let actions = store.sdd_delivery_actions("preview-1").await.unwrap();
        assert_eq!(actions[0].status, "sync_pending");
        let run = store.sdd_get_run("run-1").await.unwrap().unwrap();
        assert_eq!(
            (run.phase.as_str(), run.status.as_str()),
            ("ready", "succeeded")
        );
        assert_eq!(run.aggregate_revision, 11);
    }
}
