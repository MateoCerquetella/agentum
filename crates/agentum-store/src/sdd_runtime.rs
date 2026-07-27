//! Durable runtime mutations for the Agentum-owned SDD lifecycle.
//!
//! Provider execution, Git, verification commands, and filesystem publication
//! happen outside SQLite transactions. These methods record the intent/result
//! with aggregate CAS, idempotency, audit events, and the realtime outbox in a
//! single commit.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::collections::HashSet;
use uuid::Uuid;

use crate::sdd::{EventInsert, append_event, now};
use crate::{Result, Store, StoreError};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddTaskRecord {
    pub run_id: String,
    pub task_id: String,
    pub spec_revision: i64,
    pub intent_json: String,
    pub runtime_status: String,
    pub aggregate_revision: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddAttemptRecord {
    pub attempt_id: String,
    pub run_id: String,
    pub task_id: Option<String>,
    pub spec_revision: i64,
    pub provider: String,
    pub isolated_path: String,
    pub status: String,
    pub session_identity: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddVerificationRecord {
    pub verification_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub task_id: Option<String>,
    pub spec_revision: i64,
    pub command_index: i64,
    pub command_json: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub output_hash: String,
    pub output_excerpt: String,
    pub duration_ms: i64,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct BeginAttemptMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub phase: &'a str,
    pub attempt_id: &'a str,
    pub task_id: Option<&'a str>,
    pub provider: &'a str,
    pub isolated_path: &'a str,
    pub session_identity: &'a str,
    pub response_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct ActivateAttemptMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub attempt_id: &'a str,
    pub response_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct FailAttemptMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub attempt_id: &'a str,
    pub status: &'a str,
    pub blocker: &'a str,
    pub event_kind: &'a str,
    pub response_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct ReservePatchMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub patch_id: &'a str,
    pub attempt_id: &'a str,
    pub relative_paths: &'a [String],
    pub preimage_hashes: &'a [String],
    pub operations_json: &'a str,
    pub preimages_json: &'a str,
    pub expires_at: &'a str,
    pub response_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct CompletePatchMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub patch_id: &'a str,
    pub attempt_id: &'a str,
    pub response_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct FailPatchMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub patch_id: &'a str,
    pub attempt_id: &'a str,
    pub error: &'a str,
    pub rollback_succeeded: bool,
    pub response_json: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationResultInput {
    pub command_index: i64,
    pub command_json: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub output_hash: String,
    pub output_excerpt: String,
    pub duration_ms: i64,
}

#[derive(Debug, Clone)]
pub struct RecordVerificationMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub attempt_id: &'a str,
    pub results: &'a [VerificationResultInput],
    pub success_status: &'a str,
    pub response_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct QuarantineRunMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub blocker: &'a str,
    pub response_json: &'a str,
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

impl Store {
    pub async fn sdd_tasks(&self, run_id: &str) -> Result<Vec<SddTaskRecord>> {
        Ok(sqlx::query_as(
            "SELECT t.* FROM sdd_tasks t
             JOIN sdd_runs r ON r.run_id = t.run_id
             JOIN sdd_specs s ON s.spec_id = r.spec_id
             WHERE t.run_id = ? AND t.spec_revision = s.current_revision
             ORDER BY t.task_id",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sdd_attempts(&self, run_id: &str) -> Result<Vec<SddAttemptRecord>> {
        Ok(sqlx::query_as(
            "SELECT a.* FROM sdd_attempts a
             JOIN sdd_runs r ON r.run_id = a.run_id
             JOIN sdd_specs s ON s.spec_id = r.spec_id
             WHERE a.run_id = ? AND a.spec_revision = s.current_revision
             ORDER BY a.started_at, a.attempt_id",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sdd_verification_results(
        &self,
        run_id: &str,
    ) -> Result<Vec<SddVerificationRecord>> {
        Ok(sqlx::query_as(
            "SELECT v.* FROM sdd_verification_results v
             JOIN sdd_runs r ON r.run_id = v.run_id
             JOIN sdd_specs s ON s.spec_id = r.spec_id
             WHERE v.run_id = ? AND v.spec_revision = s.current_revision
             ORDER BY v.created_at, v.command_index",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sdd_begin_attempt(&self, input: BeginAttemptMutation<'_>) -> Result<i64> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let run: Option<(String, String, String, String, i64, i64)> = sqlx::query_as(
            "SELECT r.repo_id, r.spec_id, r.phase, r.status, r.aggregate_revision,
                    s.current_revision
             FROM sdd_runs r JOIN sdd_specs s ON s.spec_id = r.spec_id
             WHERE r.run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, phase, status, current, spec_revision)) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        if phase != input.phase
            || !matches!(status.as_str(), "queued" | "running" | "retry_scheduled")
        {
            return Err(StoreError::InvalidCommand(format!(
                "{} attempt cannot start from {phase}/{status}",
                input.phase
            )));
        }
        if let Some(task_id) = input.task_id {
            let task_status: Option<String> = sqlx::query_scalar(
                "SELECT runtime_status FROM sdd_tasks WHERE run_id = ? AND task_id = ?",
            )
            .bind(input.run_id)
            .bind(task_id)
            .fetch_optional(&mut *tx)
            .await?;
            if !task_status.is_some_and(|status| {
                matches!(status.as_str(), "idle" | "queued" | "retry_scheduled")
            }) {
                return Err(StoreError::InvalidCommand(format!(
                    "task {task_id} is not startable"
                )));
            }
        }
        sqlx::query(
            "INSERT INTO sdd_attempts
             (attempt_id, run_id, task_id, spec_revision, provider, isolated_path, status,
              session_identity, started_at)
             VALUES (?, ?, ?, ?, ?, ?, 'queued', ?, ?)",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .bind(input.task_id)
        .bind(spec_revision)
        .bind(input.provider)
        .bind(input.isolated_path)
        .bind(input.session_identity)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        let next = current + 1;
        sqlx::query(
            "UPDATE sdd_runs SET status = 'running', blocker = NULL,
             aggregate_revision = ?, updated_at = ?
             WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(next)
        .bind(&at)
        .bind(input.run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(input.run_id),
                revision: next,
                kind: "sdd.attempt.reserved",
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
            input.expected_revision,
            input.response_json,
            &at,
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    pub async fn sdd_activate_attempt(&self, input: ActivateAttemptMutation<'_>) -> Result<i64> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let run: Option<(String, String, i64)> = sqlx::query_as(
            "SELECT repo_id, spec_id, aggregate_revision FROM sdd_runs WHERE run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, current)) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        let task_id: Option<String> = sqlx::query_scalar(
            "SELECT task_id FROM sdd_attempts
             WHERE attempt_id = ? AND run_id = ? AND status = 'queued'",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        let activated = sqlx::query(
            "UPDATE sdd_attempts SET status = 'running'
             WHERE attempt_id = ? AND run_id = ? AND status = 'queued'",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .execute(&mut *tx)
        .await?;
        if activated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "attempt is not reserved for activation".into(),
            ));
        }
        if let Some(task_id) = task_id {
            let task_activated = sqlx::query(
                "UPDATE sdd_tasks SET runtime_status = 'running',
                 aggregate_revision = aggregate_revision + 1, updated_at = ?
                 WHERE run_id = ? AND task_id = ?
                   AND runtime_status IN ('idle', 'queued', 'retry_scheduled')",
            )
            .bind(&at)
            .bind(input.run_id)
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
            if task_activated.rows_affected() != 1 {
                return Err(StoreError::InvalidCommand(
                    "task is not startable for attempt activation".into(),
                ));
            }
        }
        let next = current + 1;
        sqlx::query(
            "UPDATE sdd_runs SET status = 'running', blocker = NULL,
             aggregate_revision = ?, updated_at = ?
             WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(next)
        .bind(&at)
        .bind(input.run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(input.run_id),
                revision: next,
                kind: "sdd.attempt.started",
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
            input.expected_revision,
            input.response_json,
            &at,
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    pub async fn sdd_fail_attempt(&self, input: FailAttemptMutation<'_>) -> Result<i64> {
        if !matches!(input.status, "failed" | "blocked" | "paused" | "canceled") {
            return Err(StoreError::InvalidCommand(
                "attempt failure status is invalid".into(),
            ));
        }
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let run: Option<(String, String, String, Option<String>, i64)> = sqlx::query_as(
            "SELECT repo_id, spec_id, status, blocker, aggregate_revision
             FROM sdd_runs WHERE run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, run_status, run_blocker, current)) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        let task_id: Option<String> = sqlx::query_scalar(
            "SELECT task_id FROM sdd_attempts
             WHERE attempt_id = ? AND run_id = ? AND status IN ('queued', 'running')",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        let updated = sqlx::query(
            "UPDATE sdd_attempts SET status = ?, finished_at = ?, error_summary = ?
             WHERE attempt_id = ? AND run_id = ? AND status IN ('queued', 'running')",
        )
        .bind(if input.status == "blocked" {
            "failed"
        } else {
            input.status
        })
        .bind(&at)
        .bind(input.blocker)
        .bind(input.attempt_id)
        .bind(input.run_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "attempt is not active for this run".into(),
            ));
        }
        sqlx::query(
            "UPDATE sdd_capability_grants SET revoked_at = ?
             WHERE run_id = ? AND attempt_id = ? AND revoked_at IS NULL",
        )
        .bind(&at)
        .bind(input.run_id)
        .bind(input.attempt_id)
        .execute(&mut *tx)
        .await?;
        if let Some(task_id) = task_id {
            sqlx::query(
                "UPDATE sdd_tasks SET runtime_status = ?,
                 aggregate_revision = aggregate_revision + 1, updated_at = ?
                 WHERE run_id = ? AND task_id = ?",
            )
            .bind(if input.status == "blocked" {
                "blocked"
            } else {
                input.status
            })
            .bind(&at)
            .bind(input.run_id)
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
        }
        let preserve_run = matches!(
            run_status.as_str(),
            "waiting"
                | "pausing"
                | "paused"
                | "blocked"
                | "canceling"
                | "canceled"
                | "failed"
                | "succeeded"
        );
        let next = current + 1;
        sqlx::query(
            "UPDATE sdd_runs SET status = ?, blocker = ?, aggregate_revision = ?,
             updated_at = ? WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(if preserve_run {
            run_status.as_str()
        } else {
            input.status
        })
        .bind(if preserve_run {
            run_blocker.as_deref()
        } else {
            Some(input.blocker)
        })
        .bind(next)
        .bind(&at)
        .bind(input.run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(input.run_id),
                revision: next,
                kind: input.event_kind,
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
            input.expected_revision,
            input.response_json,
            &at,
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    pub async fn sdd_reserve_patch(&self, input: ReservePatchMutation<'_>) -> Result<i64> {
        if input.relative_paths.is_empty()
            || input.relative_paths.len() != input.preimage_hashes.len()
        {
            return Err(StoreError::InvalidCommand(
                "patch paths and preimages must be non-empty and aligned".into(),
            ));
        }
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let run: Option<(String, String, String, String, i64, i64)> = sqlx::query_as(
            "SELECT r.repo_id, r.spec_id, r.phase, r.status, r.aggregate_revision,
                    s.current_revision
             FROM sdd_runs r JOIN sdd_specs s ON s.spec_id = r.spec_id
             WHERE r.run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, phase, status, current, _spec_revision)) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        if phase != "implementation" || status != "running" {
            return Err(StoreError::InvalidCommand(
                "patches require a running implementation phase".into(),
            ));
        }
        let attempt_active: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sdd_attempts
             WHERE attempt_id = ? AND run_id = ? AND status = 'running')",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .fetch_one(&mut *tx)
        .await?;
        if attempt_active == 0 {
            return Err(StoreError::InvalidCommand(
                "patch attempt is not active".into(),
            ));
        }
        for (path, preimage) in input
            .relative_paths
            .iter()
            .zip(input.preimage_hashes.iter())
        {
            let conflict: i64 = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sdd_leases
                 WHERE run_id = ? AND relative_path = ?)",
            )
            .bind(input.run_id)
            .bind(path)
            .fetch_one(&mut *tx)
            .await?;
            if conflict != 0 {
                return Err(StoreError::InvalidCommand(format!(
                    "path lease conflict: {path}"
                )));
            }
            sqlx::query(
                "INSERT INTO sdd_leases
                 (run_id, relative_path, attempt_id, preimage_hash, expires_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(input.run_id)
            .bind(path)
            .bind(input.attempt_id)
            .bind(preimage)
            .bind(input.expires_at)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO sdd_patch_ledger
             (patch_id, run_id, attempt_id, operations_json, preimages_json,
              status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'pending', ?, ?)",
        )
        .bind(input.patch_id)
        .bind(input.run_id)
        .bind(input.attempt_id)
        .bind(input.operations_json)
        .bind(input.preimages_json)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        let next = current + 1;
        sqlx::query(
            "UPDATE sdd_runs SET aggregate_revision = ?, updated_at = ?
             WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(next)
        .bind(&at)
        .bind(input.run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(input.run_id),
                revision: next,
                kind: "sdd.patch.reserved",
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
            input.expected_revision,
            input.response_json,
            &at,
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    pub async fn sdd_complete_patch(&self, input: CompletePatchMutation<'_>) -> Result<i64> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let run: Option<(String, String, i64)> = sqlx::query_as(
            "SELECT repo_id, spec_id, aggregate_revision FROM sdd_runs WHERE run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, current)) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        let task_id: Option<String> = sqlx::query_scalar(
            "SELECT task_id FROM sdd_attempts
             WHERE attempt_id = ? AND run_id = ? AND status = 'running'",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        let task_id = task_id.ok_or_else(|| {
            StoreError::InvalidCommand("implementation attempt has no active task".into())
        })?;
        let patched = sqlx::query(
            "UPDATE sdd_patch_ledger SET status = 'applied', error = NULL, updated_at = ?
             WHERE patch_id = ? AND run_id = ? AND attempt_id = ? AND status = 'pending'",
        )
        .bind(&at)
        .bind(input.patch_id)
        .bind(input.run_id)
        .bind(input.attempt_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_capability_grants SET revoked_at = ?
             WHERE run_id = ? AND attempt_id = ? AND revoked_at IS NULL",
        )
        .bind(&at)
        .bind(input.run_id)
        .bind(input.attempt_id)
        .execute(&mut *tx)
        .await?;
        if patched.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "patch is not pending for this attempt".into(),
            ));
        }
        sqlx::query("DELETE FROM sdd_leases WHERE run_id = ? AND attempt_id = ?")
            .bind(input.run_id)
            .bind(input.attempt_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE sdd_attempts SET status = 'succeeded', finished_at = ?, error_summary = NULL
             WHERE attempt_id = ? AND status = 'running'",
        )
        .bind(&at)
        .bind(input.attempt_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_tasks SET runtime_status = 'succeeded',
             aggregate_revision = aggregate_revision + 1, updated_at = ?
             WHERE run_id = ? AND task_id = ? AND runtime_status = 'running'",
        )
        .bind(&at)
        .bind(input.run_id)
        .bind(&task_id)
        .execute(&mut *tx)
        .await?;
        let incomplete: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sdd_tasks
             WHERE run_id = ? AND runtime_status != 'succeeded'",
        )
        .bind(input.run_id)
        .fetch_one(&mut *tx)
        .await?;
        let (phase, status, event_kind) = if incomplete == 0 {
            ("verification", "queued", "sdd.implementation.completed")
        } else {
            ("implementation", "running", "sdd.task.completed")
        };
        let next = current + 1;
        sqlx::query(
            "UPDATE sdd_runs SET phase = ?, status = ?, blocker = NULL,
             aggregate_revision = ?, updated_at = ?
             WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(phase)
        .bind(status)
        .bind(next)
        .bind(&at)
        .bind(input.run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(input.run_id),
                revision: next,
                kind: event_kind,
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
            input.expected_revision,
            input.response_json,
            &at,
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    pub async fn sdd_fail_patch(&self, input: FailPatchMutation<'_>) -> Result<i64> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let run: Option<(String, String, i64)> = sqlx::query_as(
            "SELECT repo_id, spec_id, aggregate_revision FROM sdd_runs WHERE run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, current)) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        let ledger_status = if input.rollback_succeeded {
            "rolled_back"
        } else {
            "quarantined"
        };
        sqlx::query(
            "UPDATE sdd_patch_ledger SET status = ?, error = ?, updated_at = ?
             WHERE patch_id = ? AND run_id = ? AND attempt_id = ? AND status = 'pending'",
        )
        .bind(ledger_status)
        .bind(input.error)
        .bind(&at)
        .bind(input.patch_id)
        .bind(input.run_id)
        .bind(input.attempt_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM sdd_leases WHERE run_id = ? AND attempt_id = ?")
            .bind(input.run_id)
            .bind(input.attempt_id)
            .execute(&mut *tx)
            .await?;
        let task_id: Option<String> = sqlx::query_scalar(
            "SELECT task_id FROM sdd_attempts WHERE attempt_id = ? AND run_id = ?",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        sqlx::query(
            "UPDATE sdd_attempts SET status = 'failed', finished_at = ?, error_summary = ?
             WHERE attempt_id = ? AND status = 'running'",
        )
        .bind(&at)
        .bind(input.error)
        .bind(input.attempt_id)
        .execute(&mut *tx)
        .await?;
        if let Some(task_id) = task_id {
            sqlx::query(
                "UPDATE sdd_tasks SET runtime_status = 'failed',
                 aggregate_revision = aggregate_revision + 1, updated_at = ?
                 WHERE run_id = ? AND task_id = ?",
            )
            .bind(&at)
            .bind(input.run_id)
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
        }
        let blocker = if input.rollback_succeeded {
            input.error
        } else {
            "patch rollback failed; recovery evidence is quarantined"
        };
        let next = current + 1;
        sqlx::query(
            "UPDATE sdd_runs SET status = 'failed', blocker = ?, quarantined = ?,
             aggregate_revision = ?, updated_at = ?
             WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(blocker)
        .bind(i64::from(!input.rollback_succeeded))
        .bind(next)
        .bind(&at)
        .bind(input.run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(input.run_id),
                revision: next,
                kind: if input.rollback_succeeded {
                    "sdd.patch.rolled_back"
                } else {
                    "sdd.patch.quarantined"
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
            input.expected_revision,
            input.response_json,
            &at,
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    pub async fn sdd_record_verification(
        &self,
        input: RecordVerificationMutation<'_>,
    ) -> Result<i64> {
        if input.results.is_empty() {
            return Err(StoreError::InvalidCommand(
                "verification must record at least one result".into(),
            ));
        }
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let run: Option<(String, String, String, String, i64, i64)> = sqlx::query_as(
            "SELECT r.repo_id, r.spec_id, r.phase, r.status, r.aggregate_revision,
                    s.current_revision
             FROM sdd_runs r JOIN sdd_specs s ON s.spec_id = r.spec_id
             WHERE r.run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, phase, run_status, current, spec_revision)) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        if phase != "verification" || run_status != "running" {
            return Err(StoreError::InvalidCommand(
                "verification results require a running verification phase".into(),
            ));
        }
        let attempt: Option<(Option<String>, String)> = sqlx::query_as(
            "SELECT task_id, status FROM sdd_attempts
             WHERE attempt_id = ? AND run_id = ?",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((task_id, attempt_status)) = attempt else {
            return Err(StoreError::NotFound(input.attempt_id.into()));
        };
        if attempt_status != "running" {
            return Err(StoreError::InvalidCommand(
                "verification attempt is not active".into(),
            ));
        }
        let mut succeeded = true;
        let mut submitted_browser_hashes = HashSet::new();
        for result in input.results {
            if !matches!(
                result.status.as_str(),
                "succeeded" | "failed" | "timed_out" | "canceled"
            ) {
                return Err(StoreError::InvalidCommand(
                    "verification result status is invalid".into(),
                ));
            }
            succeeded &= result.status == "succeeded";
            let command: serde_json::Value = serde_json::from_str(&result.command_json)?;
            if command.get("type").and_then(serde_json::Value::as_str) == Some("browserCheck") {
                let check_id = command
                    .get("check")
                    .and_then(|check| check.get("id"))
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        StoreError::InvalidCommand(
                            "browser verification result has no typed check identity".into(),
                        )
                    })?;
                let expected_status = if result.status == "succeeded" {
                    "passed"
                } else {
                    "failed"
                };
                let backed: i64 = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM sdd_browser_evidence
                     WHERE run_id = ? AND attempt_id = ? AND spec_revision = ?
                       AND check_id = ? AND manifest_sha256 = ? AND status = ?)",
                )
                .bind(input.run_id)
                .bind(input.attempt_id)
                .bind(spec_revision)
                .bind(check_id)
                .bind(&result.output_hash)
                .bind(expected_status)
                .fetch_one(&mut *tx)
                .await?;
                if backed == 0 || !submitted_browser_hashes.insert(result.output_hash.as_str()) {
                    return Err(StoreError::InvalidCommand(
                        "browser verification result is not backed by unique attempt evidence"
                            .into(),
                    ));
                }
            }
            sqlx::query(
                "INSERT INTO sdd_verification_results
                 (verification_id, run_id, attempt_id, task_id, spec_revision, command_index,
                  command_json, status, exit_code, output_hash, output_excerpt,
                  duration_ms, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(input.run_id)
            .bind(input.attempt_id)
            .bind(task_id.as_deref())
            .bind(spec_revision)
            .bind(result.command_index)
            .bind(&result.command_json)
            .bind(&result.status)
            .bind(result.exit_code)
            .bind(&result.output_hash)
            .bind(&result.output_excerpt)
            .bind(result.duration_ms)
            .bind(&at)
            .execute(&mut *tx)
            .await?;
        }
        let durable_browser_hashes: Vec<String> = sqlx::query_scalar(
            "SELECT manifest_sha256 FROM sdd_browser_evidence
             WHERE run_id = ? AND attempt_id = ? AND spec_revision = ?",
        )
        .bind(input.run_id)
        .bind(input.attempt_id)
        .bind(spec_revision)
        .fetch_all(&mut *tx)
        .await?;
        if durable_browser_hashes.len() != submitted_browser_hashes.len()
            || durable_browser_hashes
                .iter()
                .any(|hash| !submitted_browser_hashes.contains(hash.as_str()))
        {
            return Err(StoreError::InvalidCommand(
                "verification results do not account for the exact browser evidence set".into(),
            ));
        }
        sqlx::query(
            "UPDATE sdd_attempts SET status = ?, finished_at = ?, error_summary = ?
             WHERE attempt_id = ? AND status = 'running'",
        )
        .bind(if succeeded { "succeeded" } else { "failed" })
        .bind(&at)
        .bind((!succeeded).then_some("one or more verification commands failed"))
        .bind(input.attempt_id)
        .execute(&mut *tx)
        .await?;
        if !matches!(input.success_status, "queued" | "paused") {
            return Err(StoreError::InvalidCommand(
                "verification success status must be queued or paused".into(),
            ));
        }
        let (next_phase, next_status, blocker, event_kind) = if succeeded {
            (
                "review",
                input.success_status,
                None,
                "sdd.verification.succeeded",
            )
        } else {
            (
                "verification",
                "failed",
                Some("verification failed"),
                "sdd.verification.failed",
            )
        };
        let next = current + 1;
        sqlx::query(
            "UPDATE sdd_runs SET phase = ?, status = ?, blocker = ?, aggregate_revision = ?,
             updated_at = ? WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(next_phase)
        .bind(next_status)
        .bind(blocker)
        .bind(next)
        .bind(&at)
        .bind(input.run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(input.run_id),
                revision: next,
                kind: event_kind,
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
            input.expected_revision,
            input.response_json,
            &at,
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    /// Fail closed when Agentum cannot prove that disposable-worktree cleanup
    /// or rollback completed. The evidence remains on disk and the run cannot
    /// be resumed through ordinary lifecycle commands.
    pub async fn sdd_quarantine_run(&self, input: QuarantineRunMutation<'_>) -> Result<i64> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let run: Option<(String, String, String, i64, i64)> = sqlx::query_as(
            "SELECT repo_id, spec_id, status, quarantined, aggregate_revision
             FROM sdd_runs WHERE run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, status, quarantined, current)) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        if quarantined != 0 {
            return Err(StoreError::InvalidCommand(
                "run is already quarantined".into(),
            ));
        }
        let _previous_status = status;
        let next = current + 1;
        let updated = sqlx::query(
            "UPDATE sdd_runs SET status = 'blocked', blocker = ?, quarantined = 1,
             aggregate_revision = ?, updated_at = ?
             WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(input.blocker)
        .bind(next)
        .bind(&at)
        .bind(input.run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        sqlx::query(
            "UPDATE sdd_attempts SET status = 'failed', finished_at = ?, error_summary = ?
             WHERE run_id = ? AND status IN ('queued', 'running', 'pausing', 'canceling')",
        )
        .bind(&at)
        .bind(input.blocker)
        .bind(input.run_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_tasks SET runtime_status = 'blocked', aggregate_revision = aggregate_revision + 1,
             updated_at = ? WHERE run_id = ? AND runtime_status IN ('queued', 'running', 'pausing')",
        )
        .bind(&at)
        .bind(input.run_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM sdd_leases WHERE run_id = ?")
            .bind(input.run_id)
            .execute(&mut *tx)
            .await?;
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(input.run_id),
                revision: next,
                kind: "sdd.run.quarantined",
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
            input.expected_revision,
            input.response_json,
            &at,
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }
}
