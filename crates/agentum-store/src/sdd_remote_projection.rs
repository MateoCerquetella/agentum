//! Restart-safe desktop projection of the fixed remote SDD subsystem.
//!
//! SSH, provider and filesystem work happens outside SQLite. These methods
//! reserve and publish one typed remote phase with the ordinary run CAS so a
//! restart can never manufacture a green phase or lose its artifact bodies.

use agentum_core::sdd::{CommandSpec, PlanArtifact};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Sqlite, Transaction};
use uuid::Uuid;

use crate::sdd::{EventInsert, append_event, now};
use crate::sdd_runtime::VerificationResultInput;
use crate::{Result, Store, StoreError};

#[derive(Debug, Clone)]
pub struct NewSddRemoteProjection<'a> {
    pub host_id: &'a str,
    pub repository_identity_sha256: &'a str,
    pub artifact_set_id: &'a str,
    pub worker_version: &'a str,
    pub plan_json: &'a str,
    pub checkpoint_json: &'a str,
    pub specification_content: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddRemoteRunRecord {
    pub run_id: String,
    pub host_id: String,
    pub repository_identity_sha256: String,
    pub artifact_set_id: String,
    pub worker_version: String,
    pub plan_json: String,
    pub checkpoint_json: String,
    pub checkpoint_revision: i64,
    pub active_request_id: Option<String>,
    pub status: String,
    pub last_error_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddRemoteRequestRecord {
    pub request_id: String,
    pub run_id: String,
    pub phase: String,
    pub request_json: String,
    pub request_sha256: String,
    pub expected_run_revision: i64,
    pub attempt_id: String,
    pub status: String,
    pub response_json: Option<String>,
    pub error_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddRemoteArtifactPayloadRecord {
    pub artifact_revision_id: String,
    pub run_id: String,
    pub request_id: Option<String>,
    pub content: String,
    pub content_sha256: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddRemoteCreateIntentRecord {
    pub repo_id: String,
    pub request_id: String,
    pub host_id: String,
    pub author_request_json: String,
    pub publication_intent_json: String,
    pub author_result_json: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, FromRow)]
struct RemoteReservationStateRow {
    repo_id: String,
    spec_id: String,
    run_phase: String,
    run_status: String,
    aggregate_revision: i64,
    spec_revision: i64,
    projection_status: String,
    active_request_id: Option<String>,
}

#[derive(Debug, FromRow)]
struct RemotePublicationStateRow {
    repo_id: String,
    spec_id: String,
    run_phase: String,
    run_status: String,
    aggregate_revision: i64,
    spec_revision: i64,
    projection_status: String,
    active_request_id: Option<String>,
    request_status: String,
    request_sha256: String,
}

#[derive(Debug, Clone)]
pub struct ReserveRemoteDesktopPhase<'a> {
    pub request_id: &'a str,
    pub request_sha256: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub phase: &'a str,
    pub request_json: &'a str,
    pub attempt_id: &'a str,
    pub provider: &'a str,
    pub isolated_path: &'a str,
    pub session_identity: &'a str,
    pub response_json: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteDesktopReservation {
    Started {
        revision: i64,
    },
    Replay {
        revision: i64,
        status: String,
        response_json: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRemotePlan {
    schema_version: u32,
    host_id: String,
    run_id: String,
    spec_id: String,
    spec_revision: i64,
    repository_identity_sha256: String,
    artifact_set_id: String,
    base_commit: String,
    provider: String,
    approval_digest: String,
    timeout_ms: u64,
    output_limit: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRemoteCheckpoint {
    schema_version: u32,
    host_id: String,
    run_id: String,
    spec_revision: i64,
    approval_digest: String,
    next_phase: String,
    completed_phases: u8,
    workspace_state_sha256: String,
    last_result_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRemotePhaseRequest {
    schema_version: u32,
    request_id: String,
    host_id: String,
    run_id: String,
    spec_id: String,
    spec_revision: i64,
    phase: String,
    repository_identity_sha256: String,
    artifact_set_id: String,
    base_commit: String,
    provider: String,
    expected_workspace_state_sha256: String,
    previous_result_sha256: String,
    approval_digest: String,
    timeout_ms: u64,
    output_limit: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredImplementationEvidence {
    schema_version: u32,
    request_id: String,
    spec_id: String,
    spec_revision: i64,
    tasks: Vec<StoredTaskCompletionEvidence>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredTaskCompletionEvidence {
    task_id: String,
    patch_sha256: String,
    write_set_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRemoteVerificationEvidence {
    schema_version: u32,
    command_results: Vec<VerificationResultInput>,
    browser_results: Vec<StoredRemoteBrowserCheckResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRemoteBrowserCheckResult {
    check_id: String,
    captured_at: String,
    status: String,
    duration_ms: i64,
    output_excerpt: String,
    target: serde_json::Value,
    browser: serde_json::Value,
    assertions: serde_json::Value,
    console: serde_json::Value,
    network: serde_json::Value,
    blobs: Vec<StoredRemoteBrowserBlob>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRemoteBrowserBlob {
    sha256: String,
    byte_length: u64,
    media_type: String,
    role: String,
    content_base64: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRemotePhaseResult {
    schema_version: u32,
    request_id: String,
    phase: String,
    status: String,
    workspace_state_sha256: String,
    artifact_set_sha256: String,
    evidence_sha256: String,
    evidence_summary: Option<String>,
    artifacts: Vec<StoredRemoteArtifactPayload>,
    error_code: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRemoteArtifactPayload {
    kind: String,
    relative_path: String,
    content_sha256: String,
    content: String,
}

#[derive(Debug, Clone)]
pub struct RemoteArtifactPayloadInput<'a> {
    pub kind: &'a str,
    pub relative_path: &'a str,
    pub content_sha256: &'a str,
    pub content: &'a str,
}

#[derive(Debug, Clone)]
pub struct PreparedRemoteEvidenceBlob {
    pub sha256: String,
    pub byte_length: i64,
    pub media_type: String,
    pub storage_relative_path: String,
    pub role: String,
}

#[derive(Debug, Clone)]
pub struct PreparedRemoteBrowserEvidence {
    pub evidence_id: String,
    pub check_id: String,
    pub manifest_sha256: String,
    pub manifest_json: String,
    pub status: String,
    pub captured_at: String,
    pub verification_result: VerificationResultInput,
    pub blobs: Vec<PreparedRemoteEvidenceBlob>,
}

#[derive(Debug, Clone)]
pub struct PublishRemoteDesktopPhase<'a> {
    pub request_id: &'a str,
    pub request_sha256: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub phase: &'a str,
    pub status: &'a str,
    pub checkpoint_json: &'a str,
    pub artifacts: &'a [RemoteArtifactPayloadInput<'a>],
    pub evidence_sha256: &'a str,
    pub evidence_summary: Option<&'a str>,
    pub browser_evidence: &'a [PreparedRemoteBrowserEvidence],
    pub error_code: Option<&'a str>,
    pub response_json: &'a str,
}

pub(crate) async fn insert_initial_projection(
    tx: &mut Transaction<'_, Sqlite>,
    run_id: &str,
    artifact_revision_id: &str,
    projection: &NewSddRemoteProjection<'_>,
    at: &str,
) -> Result<()> {
    if !valid_sha256(projection.repository_identity_sha256)
        || projection.artifact_set_id.is_empty()
        || projection.worker_version.is_empty()
        || sha256(projection.specification_content.as_bytes()).len() != 64
    {
        return Err(StoreError::InvalidCommand(
            "remote projection identity is malformed".into(),
        ));
    }
    let plan: StoredRemotePlan = serde_json::from_str(projection.plan_json)?;
    let checkpoint: StoredRemoteCheckpoint = serde_json::from_str(projection.checkpoint_json)?;
    validate_plan_checkpoint(&plan, &checkpoint, run_id)?;
    if plan.host_id != projection.host_id
        || plan.repository_identity_sha256 != projection.repository_identity_sha256
        || plan.artifact_set_id != projection.artifact_set_id
    {
        return Err(StoreError::InvalidCommand(
            "remote plan does not match its projection identity".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO sdd_remote_runs
         (run_id, host_id, repository_identity_sha256, artifact_set_id, worker_version,
          plan_json, checkpoint_json, checkpoint_revision, status, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 1, 'waiting', ?, ?)",
    )
    .bind(run_id)
    .bind(projection.host_id)
    .bind(projection.repository_identity_sha256)
    .bind(projection.artifact_set_id)
    .bind(projection.worker_version)
    .bind(projection.plan_json)
    .bind(projection.checkpoint_json)
    .bind(at)
    .bind(at)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO sdd_remote_artifact_payloads
         (artifact_revision_id, run_id, request_id, content, content_sha256, created_at)
         VALUES (?, ?, NULL, ?, ?, ?)",
    )
    .bind(artifact_revision_id)
    .bind(run_id)
    .bind(projection.specification_content)
    .bind(sha256(projection.specification_content.as_bytes()))
    .bind(at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

impl Store {
    pub async fn sdd_prepare_remote_create(
        &self,
        repo_id: &str,
        request_id: &str,
        request_hash: &str,
        host_id: &str,
        author_request_json: &str,
        publication_intent_json: &str,
    ) -> Result<()> {
        serde_json::from_str::<serde_json::Value>(author_request_json)?;
        serde_json::from_str::<serde_json::Value>(publication_intent_json)?;
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let inserted = sqlx::query(
            "INSERT INTO sdd_remote_create_intents
             (repo_id, request_id, host_id, author_request_json, publication_intent_json,
              status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'prepared', ?, ?)",
        )
        .bind(repo_id)
        .bind(request_id)
        .bind(host_id)
        .bind(author_request_json)
        .bind(publication_intent_json)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote create intent was not reserved".into(),
            ));
        }
        let saga_updated = sqlx::query(
            "UPDATE sdd_create_sagas SET stage = 'authoring', updated_at = ?
             WHERE repo_id = ? AND request_id = ? AND request_hash = ? AND stage = 'reserved'",
        )
        .bind(&at)
        .bind(repo_id)
        .bind(request_id)
        .bind(request_hash)
        .execute(&mut *tx)
        .await?;
        if saga_updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote create saga changed before authoring".into(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn sdd_remote_create_intent(
        &self,
        repo_id: &str,
        request_id: &str,
    ) -> Result<Option<SddRemoteCreateIntentRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_remote_create_intents WHERE repo_id = ? AND request_id = ?",
        )
        .bind(repo_id)
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn sdd_record_remote_authoring_result(
        &self,
        repo_id: &str,
        request_id: &str,
        request_hash: &str,
        author_result_json: &str,
    ) -> Result<()> {
        serde_json::from_str::<serde_json::Value>(author_result_json)?;
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let existing: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT status, author_result_json FROM sdd_remote_create_intents
             WHERE repo_id = ? AND request_id = ?",
        )
        .bind(repo_id)
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((status, stored_result)) = existing else {
            return Err(StoreError::NotFound(request_id.into()));
        };
        if status == "authored" && stored_result.as_deref() == Some(author_result_json) {
            let saga_stage: Option<String> = sqlx::query_scalar(
                "SELECT stage FROM sdd_create_sagas
                 WHERE repo_id = ? AND request_id = ? AND request_hash = ?",
            )
            .bind(repo_id)
            .bind(request_id)
            .bind(request_hash)
            .fetch_optional(&mut *tx)
            .await?;
            match saga_stage.as_deref() {
                Some("recovery_required") => {
                    let recovered = sqlx::query(
                        "UPDATE sdd_create_sagas SET stage = 'publishing', updated_at = ?
                         WHERE repo_id = ? AND request_id = ? AND request_hash = ?
                           AND stage = 'recovery_required'",
                    )
                    .bind(&at)
                    .bind(repo_id)
                    .bind(request_id)
                    .bind(request_hash)
                    .execute(&mut *tx)
                    .await?;
                    if recovered.rows_affected() != 1 {
                        return Err(StoreError::InvalidCommand(
                            "remote create saga changed during recovery".into(),
                        ));
                    }
                }
                Some("publishing") => {}
                _ => {
                    return Err(StoreError::InvalidCommand(
                        "remote create saga cannot resume publication".into(),
                    ));
                }
            }
            tx.commit().await?;
            return Ok(());
        }
        if status != "prepared" {
            return Err(StoreError::InvalidCommand(
                "remote create intent cannot accept an author result".into(),
            ));
        }
        let intent_updated = sqlx::query(
            "UPDATE sdd_remote_create_intents SET author_result_json = ?, status = 'authored',
             updated_at = ? WHERE repo_id = ? AND request_id = ? AND status = 'prepared'",
        )
        .bind(author_result_json)
        .bind(&at)
        .bind(repo_id)
        .bind(request_id)
        .execute(&mut *tx)
        .await?;
        let saga_updated = sqlx::query(
            "UPDATE sdd_create_sagas SET stage = 'publishing', updated_at = ?
             WHERE repo_id = ? AND request_id = ? AND request_hash = ?
               AND stage IN ('authoring', 'recovery_required')",
        )
        .bind(&at)
        .bind(repo_id)
        .bind(request_id)
        .bind(request_hash)
        .execute(&mut *tx)
        .await?;
        if intent_updated.rows_affected() != 1 || saga_updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote authoring result changed during publication reservation".into(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn sdd_fail_remote_create_intent(
        &self,
        repo_id: &str,
        request_id: &str,
    ) -> Result<()> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT status FROM sdd_remote_create_intents
             WHERE repo_id = ? AND request_id = ?",
        )
        .bind(repo_id)
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(status) = existing else {
            tx.commit().await?;
            return Ok(());
        };
        if matches!(status.as_str(), "failed" | "completed") {
            tx.commit().await?;
            return Ok(());
        }
        let failed = sqlx::query(
            "UPDATE sdd_remote_create_intents SET status = 'failed', updated_at = ?
             WHERE repo_id = ? AND request_id = ? AND status IN ('prepared', 'authored')",
        )
        .bind(&at)
        .bind(repo_id)
        .bind(request_id)
        .execute(&mut *tx)
        .await?;
        if failed.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote create intent changed while failing".into(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn sdd_remote_run(&self, run_id: &str) -> Result<Option<SddRemoteRunRecord>> {
        Ok(
            sqlx::query_as("SELECT * FROM sdd_remote_runs WHERE run_id = ?")
                .bind(run_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn sdd_remote_request(
        &self,
        request_id: &str,
    ) -> Result<Option<SddRemoteRequestRecord>> {
        Ok(
            sqlx::query_as("SELECT * FROM sdd_remote_requests WHERE request_id = ?")
                .bind(request_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn sdd_remote_artifact_payloads(
        &self,
        run_id: &str,
    ) -> Result<Vec<SddRemoteArtifactPayloadRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_remote_artifact_payloads WHERE run_id = ? ORDER BY created_at, artifact_revision_id",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sdd_remote_reserve_phase(
        &self,
        input: ReserveRemoteDesktopPhase<'_>,
    ) -> Result<RemoteDesktopReservation> {
        validate_phase(input.phase)?;
        if !valid_sha256(input.request_sha256)
            || sha256(input.request_json.as_bytes()) != input.request_sha256
        {
            return Err(StoreError::InvalidCommand(
                "remote request digest is malformed".into(),
            ));
        }
        let typed_request: StoredRemotePhaseRequest = serde_json::from_str(input.request_json)?;
        validate_phase_request(&typed_request, &input)?;
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let replay: Option<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT request_sha256, status, response_json FROM sdd_remote_requests
             WHERE request_id = ?",
        )
        .bind(input.request_id)
        .fetch_optional(&mut *tx)
        .await?;
        let retry_from = if let Some((stored_hash, status, response_json)) = replay {
            if stored_hash != input.request_sha256 {
                return Err(StoreError::IdempotencyConflict(format!(
                    "remote-run:{}",
                    input.run_id
                )));
            }
            if matches!(status.as_str(), "failed" | "canceled" | "interrupted") {
                Some(status)
            } else {
                let revision: i64 =
                    sqlx::query_scalar("SELECT aggregate_revision FROM sdd_runs WHERE run_id = ?")
                        .bind(input.run_id)
                        .fetch_one(&mut *tx)
                        .await?;
                tx.commit().await?;
                return Ok(RemoteDesktopReservation::Replay {
                    revision,
                    status,
                    response_json,
                });
            }
        } else {
            None
        };
        let run: Option<RemoteReservationStateRow> = sqlx::query_as(
            "SELECT r.repo_id, r.spec_id, r.phase AS run_phase, r.status AS run_status,
                        r.aggregate_revision, s.current_revision AS spec_revision,
                        p.status AS projection_status, p.active_request_id
                 FROM sdd_runs r JOIN sdd_specs s ON s.spec_id = r.spec_id
                 JOIN sdd_remote_runs p ON p.run_id = r.run_id WHERE r.run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(RemoteReservationStateRow {
            repo_id,
            spec_id,
            run_phase,
            run_status,
            aggregate_revision: current,
            spec_revision,
            projection_status,
            active_request_id: active,
        }) = run
        else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        if run_phase != input.phase
            || run_status != "queued"
            || projection_status != "queued"
            || active.is_some()
        {
            return Err(StoreError::InvalidCommand(format!(
                "remote phase cannot start from {run_phase}/{run_status}/{projection_status}"
            )));
        }
        let (plan_json, checkpoint_json): (String, String) = sqlx::query_as(
            "SELECT plan_json, checkpoint_json FROM sdd_remote_runs WHERE run_id = ?",
        )
        .bind(input.run_id)
        .fetch_one(&mut *tx)
        .await?;
        let plan: StoredRemotePlan = serde_json::from_str(&plan_json)?;
        let checkpoint: StoredRemoteCheckpoint = serde_json::from_str(&checkpoint_json)?;
        validate_current_plan_checkpoint(&plan, &checkpoint, input.run_id)?;
        if typed_request.host_id != plan.host_id
            || typed_request.spec_id != plan.spec_id
            || typed_request.spec_revision != plan.spec_revision
            || typed_request.repository_identity_sha256 != plan.repository_identity_sha256
            || typed_request.artifact_set_id != plan.artifact_set_id
            || typed_request.base_commit != plan.base_commit
            || typed_request.provider != plan.provider
            || typed_request.approval_digest != plan.approval_digest
            || typed_request.timeout_ms != plan.timeout_ms
            || typed_request.output_limit != plan.output_limit
            || typed_request.phase != checkpoint.next_phase
            || typed_request.expected_workspace_state_sha256 != checkpoint.workspace_state_sha256
            || typed_request.previous_result_sha256 != checkpoint.last_result_sha256
        {
            return Err(StoreError::InvalidCommand(
                "remote request does not match the durable plan/checkpoint".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO sdd_attempts
             (attempt_id, run_id, task_id, spec_revision, provider, isolated_path, status,
              session_identity, started_at)
             VALUES (?, ?, NULL, ?, ?, ?, 'running', ?, ?)",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .bind(spec_revision)
        .bind(input.provider)
        .bind(input.isolated_path)
        .bind(input.session_identity)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        if let Some(previous_status) = retry_from.as_deref() {
            let request_updated = sqlx::query(
                "UPDATE sdd_remote_requests SET expected_run_revision = ?, attempt_id = ?,
                 status = 'running', response_json = NULL, error_code = NULL, updated_at = ?
                 WHERE request_id = ? AND request_sha256 = ? AND status = ?",
            )
            .bind(current)
            .bind(input.attempt_id)
            .bind(&at)
            .bind(input.request_id)
            .bind(input.request_sha256)
            .bind(previous_status)
            .execute(&mut *tx)
            .await?;
            if request_updated.rows_affected() != 1 {
                return Err(StoreError::InvalidCommand(
                    "remote retry request changed during reservation".into(),
                ));
            }
        } else {
            sqlx::query(
                "INSERT INTO sdd_remote_requests
                 (request_id, run_id, phase, request_json, request_sha256, expected_run_revision,
                  attempt_id, status, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 'running', ?, ?)",
            )
            .bind(input.request_id)
            .bind(input.run_id)
            .bind(input.phase)
            .bind(input.request_json)
            .bind(input.request_sha256)
            .bind(current)
            .bind(input.attempt_id)
            .bind(&at)
            .bind(&at)
            .execute(&mut *tx)
            .await?;
        }
        let next = current + 1;
        let projection_updated = sqlx::query(
            "UPDATE sdd_remote_runs SET active_request_id = ?, status = 'running',
             updated_at = ? WHERE run_id = ? AND active_request_id IS NULL AND status = 'queued'",
        )
        .bind(input.request_id)
        .bind(&at)
        .bind(input.run_id)
        .execute(&mut *tx)
        .await?;
        if projection_updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote projection changed during reservation".into(),
            ));
        }
        let updated = sqlx::query(
            "UPDATE sdd_runs SET status = 'running', blocker = NULL, aggregate_revision = ?,
             updated_at = ? WHERE run_id = ? AND aggregate_revision = ? AND status = 'queued'",
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
                kind: "sdd.remote.phase_started",
                payload_json: input.response_json,
                created_at: &at,
            },
        )
        .await?;
        tx.commit().await?;
        Ok(RemoteDesktopReservation::Started { revision: next })
    }

    pub async fn sdd_remote_publish_phase(
        &self,
        input: PublishRemoteDesktopPhase<'_>,
    ) -> Result<i64> {
        validate_phase(input.phase)?;
        if !valid_sha256(input.request_sha256)
            || !valid_sha256(input.evidence_sha256)
            || input
                .evidence_summary
                .is_some_and(|summary| summary.len() > 2 * 1024 * 1024)
        {
            return Err(StoreError::InvalidCommand(
                "remote phase result is malformed".into(),
            ));
        }
        let next_checkpoint: StoredRemoteCheckpoint = serde_json::from_str(input.checkpoint_json)?;
        let typed_result: StoredRemotePhaseResult = serde_json::from_str(input.response_json)?;
        if typed_result.schema_version != 1
            || typed_result.request_id != input.request_id
            || typed_result.phase != input.phase
            || typed_result.status != input.status
            || typed_result.evidence_sha256 != input.evidence_sha256
            || typed_result.evidence_summary.as_deref() != input.evidence_summary
            || typed_result.error_code.as_deref() != input.error_code
            || !valid_sha256(&typed_result.workspace_state_sha256)
            || !valid_sha256(&typed_result.artifact_set_sha256)
            || typed_result.artifacts.len() != input.artifacts.len()
            || typed_result
                .artifacts
                .iter()
                .zip(input.artifacts)
                .any(|(stored, offered)| {
                    stored.kind != offered.kind
                        || stored.relative_path != offered.relative_path
                        || stored.content_sha256 != offered.content_sha256
                        || stored.content != offered.content
                })
        {
            return Err(StoreError::InvalidCommand(
                "remote phase result does not match its typed publication".into(),
            ));
        }
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let row: Option<RemotePublicationStateRow> = sqlx::query_as(
            "SELECT r.repo_id, r.spec_id, r.phase AS run_phase, r.status AS run_status,
                        r.aggregate_revision, s.current_revision AS spec_revision,
                        p.status AS projection_status, p.active_request_id,
                        q.status AS request_status, q.request_sha256
                 FROM sdd_runs r JOIN sdd_specs s ON s.spec_id = r.spec_id
                 JOIN sdd_remote_runs p ON p.run_id = r.run_id
                 JOIN sdd_remote_requests q ON q.run_id = r.run_id AND q.request_id = ?
                 WHERE r.run_id = ?",
        )
        .bind(input.request_id)
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(RemotePublicationStateRow {
            repo_id,
            spec_id,
            run_phase,
            run_status,
            aggregate_revision: current,
            spec_revision,
            projection_status,
            active_request_id: active,
            request_status,
            request_sha256: stored_hash,
        }) = row
        else {
            return Err(StoreError::NotFound(input.request_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        if run_phase != input.phase
            || run_status != "running"
            || projection_status != "running"
            || active.as_deref() != Some(input.request_id)
            || request_status != "running"
            || stored_hash != input.request_sha256
        {
            return Err(StoreError::InvalidCommand(
                "remote phase reservation is stale".into(),
            ));
        }
        let (plan_json, checkpoint_json): (String, String) = sqlx::query_as(
            "SELECT plan_json, checkpoint_json FROM sdd_remote_runs WHERE run_id = ?",
        )
        .bind(input.run_id)
        .fetch_one(&mut *tx)
        .await?;
        let plan: StoredRemotePlan = serde_json::from_str(&plan_json)?;
        let previous_checkpoint: StoredRemoteCheckpoint = serde_json::from_str(&checkpoint_json)?;
        validate_current_plan_checkpoint(&plan, &previous_checkpoint, input.run_id)?;
        validate_checkpoint_advance(
            &plan,
            &previous_checkpoint,
            &next_checkpoint,
            input.phase,
            input.status,
            &typed_result.workspace_state_sha256,
            &sha256(input.response_json.as_bytes()),
        )?;
        let attempt_id: String =
            sqlx::query_scalar("SELECT attempt_id FROM sdd_remote_requests WHERE request_id = ?")
                .bind(input.request_id)
                .fetch_one(&mut *tx)
                .await?;
        let spec_slug: String = sqlx::query_scalar("SELECT slug FROM sdd_specs WHERE spec_id = ?")
            .bind(&spec_id)
            .fetch_one(&mut *tx)
            .await?;
        let (profile, control, policy_json, workspace_fingerprint): (
            String,
            String,
            String,
            String,
        ) = sqlx::query_as(
            "SELECT s.profile, s.control, r.policy_json, r.workspace_fingerprint
             FROM sdd_specs s JOIN sdd_runs r ON r.spec_id = s.spec_id
             WHERE r.run_id = ?",
        )
        .bind(input.run_id)
        .fetch_one(&mut *tx)
        .await?;
        let approval_purpose = (input.status == "succeeded"
            && ((profile == "high_risk" && matches!(input.phase, "design" | "planning"))
                || control == "interactive"))
            .then_some(input.phase);

        let (next_phase, next_status, request_status, event_kind, blocker): (
            &str,
            &str,
            &str,
            &str,
            Option<String>,
        ) = match input.status {
            "succeeded" => {
                validate_success_shape(
                    input.phase,
                    &spec_slug,
                    input.artifacts,
                    input.evidence_summary,
                )?;
                let next_phase = match input.phase {
                    "design" => "planning",
                    "planning" => "implementation",
                    "implementation" => "verification",
                    "verification" => "review",
                    "review" => "ready",
                    _ => unreachable!(),
                };
                if let Some(purpose) = approval_purpose {
                    (
                        input.phase,
                        "waiting",
                        "succeeded",
                        "sdd.remote.approval_required",
                        Some(format!("{purpose} approval required")),
                    )
                } else {
                    let status = if input.phase == "review" {
                        "succeeded"
                    } else {
                        "queued"
                    };
                    (
                        next_phase,
                        status,
                        "succeeded",
                        "sdd.remote.phase_succeeded",
                        None,
                    )
                }
            }
            "failed" => (
                input.phase,
                "failed",
                "failed",
                "sdd.remote.phase_failed",
                Some(input.error_code.unwrap_or("remote_phase_failed").to_owned()),
            ),
            "canceled" => (
                input.phase,
                "canceled",
                "canceled",
                "sdd.remote.phase_canceled",
                None,
            ),
            _ => {
                return Err(StoreError::InvalidCommand(
                    "remote result status is invalid".into(),
                ));
            }
        };

        if input.status == "succeeded" {
            let review_binding = if input.phase == "review" {
                let artifact = input.artifacts.first().expect("review shape was validated");
                Some(
                    validate_review_binding(&mut tx, &input, &attempt_id, spec_revision, artifact)
                        .await?,
                )
            } else {
                None
            };
            for artifact in input.artifacts {
                if !valid_sha256(artifact.content_sha256)
                    || sha256(artifact.content.as_bytes()) != artifact.content_sha256
                    || artifact.content.len() > 8 * 1024 * 1024
                {
                    return Err(StoreError::InvalidCommand(
                        "remote artifact digest mismatch".into(),
                    ));
                }
                agentum_core::sdd::validate_relative_path(artifact.relative_path)
                    .map_err(|error| StoreError::InvalidCommand(error.to_string()))?;
                let revision: i64 = sqlx::query_scalar(
                    "SELECT COALESCE(MAX(revision), 0) + 1 FROM sdd_artifact_revisions
                     WHERE run_id = ? AND kind = ?",
                )
                .bind(input.run_id)
                .bind(artifact.kind)
                .fetch_one(&mut *tx)
                .await?;
                let artifact_revision_id = Uuid::new_v4().to_string();
                sqlx::query(
                    "INSERT INTO sdd_artifact_revisions
                     (artifact_revision_id, run_id, spec_id, kind, revision, spec_revision,
                      relative_path, content_hash, submitted_by, evidence_digest,
                      evidence_manifest_hashes_json, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&artifact_revision_id)
                .bind(input.run_id)
                .bind(&spec_id)
                .bind(artifact.kind)
                .bind(revision)
                .bind(spec_revision)
                .bind(artifact.relative_path)
                .bind(artifact.content_sha256)
                .bind(format!("remote:{}:{}", input.phase, input.request_id))
                .bind(review_binding.as_ref().map(|binding| binding.0.as_str()))
                .bind(review_binding.as_ref().map(|binding| binding.1.as_str()))
                .bind(&at)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO sdd_remote_artifact_payloads
                     (artifact_revision_id, run_id, request_id, content, content_sha256, created_at)
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(&artifact_revision_id)
                .bind(input.run_id)
                .bind(input.request_id)
                .bind(artifact.content)
                .bind(artifact.content_sha256)
                .bind(&at)
                .execute(&mut *tx)
                .await?;
                if input.phase == "planning" {
                    insert_plan_tasks(
                        &mut tx,
                        input.run_id,
                        &spec_id,
                        spec_revision,
                        artifact.content,
                        &at,
                    )
                    .await?;
                }
            }
            if input.phase == "implementation" {
                record_implementation_evidence(&mut tx, &input, &spec_id, spec_revision, &at)
                    .await?;
            }
            if input.phase == "verification" {
                record_verification_evidence(&mut tx, &input, &attempt_id, spec_revision, &at)
                    .await?;
            }
        }

        let approval = if let Some(purpose) = approval_purpose {
            let mut artifact_hashes: Vec<(String, String)> = sqlx::query_as(
                "SELECT relative_path, content_hash FROM sdd_artifact_revisions
                 WHERE run_id = ? ORDER BY relative_path, revision",
            )
            .bind(input.run_id)
            .fetch_all(&mut *tx)
            .await?;
            artifact_hashes.sort_unstable();
            let policy: serde_json::Value = serde_json::from_str(&policy_json)?;
            let digest = sha256(serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "specId": spec_id,
                "specRevision": spec_revision,
                "purpose": purpose,
                "artifacts": artifact_hashes,
                "evidenceSha256": input.evidence_sha256,
                "checkpointSha256": sha256(input.checkpoint_json.as_bytes()),
                "policy": policy,
                "workspaceFingerprint": workspace_fingerprint,
            }))?);
            let approval_id = Uuid::new_v4().to_string();
            let inserted = sqlx::query(
                "INSERT INTO sdd_approval_requests
                 (approval_id, run_id, purpose, digest, requested_revision, requested_by,
                  status, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, 'pending', ?)",
            )
            .bind(&approval_id)
            .bind(input.run_id)
            .bind(purpose)
            .bind(&digest)
            .bind(spec_revision)
            .bind(format!("remote-agent:{}:{}", input.phase, input.request_id))
            .bind(&at)
            .execute(&mut *tx)
            .await?;
            if inserted.rows_affected() != 1 {
                return Err(StoreError::InvalidCommand(
                    "remote phase approval was not created".into(),
                ));
            }
            Some((approval_id, digest, purpose))
        } else {
            None
        };

        let attempt_updated = sqlx::query(
            "UPDATE sdd_attempts SET status = ?, finished_at = ?, error_summary = ?
             WHERE attempt_id = ? AND status = 'running'",
        )
        .bind(if input.status == "succeeded" {
            "succeeded"
        } else {
            input.status
        })
        .bind(&at)
        .bind(blocker.as_deref())
        .bind(&attempt_id)
        .execute(&mut *tx)
        .await?;
        if attempt_updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote attempt changed during publication".into(),
            ));
        }
        let request_updated = sqlx::query(
            "UPDATE sdd_remote_requests SET status = ?, response_json = ?, error_code = ?,
             updated_at = ? WHERE request_id = ? AND status = 'running'",
        )
        .bind(request_status)
        .bind(input.response_json)
        .bind(input.error_code)
        .bind(&at)
        .bind(input.request_id)
        .execute(&mut *tx)
        .await?;
        if request_updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote request changed during publication".into(),
            ));
        }
        let projection_updated = sqlx::query(
            "UPDATE sdd_remote_runs SET checkpoint_json = ?,
             checkpoint_revision = checkpoint_revision + 1, active_request_id = NULL,
             status = ?, last_error_code = ?, updated_at = ?
             WHERE run_id = ? AND active_request_id = ?",
        )
        .bind(input.checkpoint_json)
        .bind(next_status)
        .bind(input.error_code)
        .bind(&at)
        .bind(input.run_id)
        .bind(input.request_id)
        .execute(&mut *tx)
        .await?;
        if projection_updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote checkpoint changed during publication".into(),
            ));
        }
        let next = current + 1;
        let run_updated = sqlx::query(
            "UPDATE sdd_runs SET phase = ?, status = ?, blocker = ?, aggregate_revision = ?,
             updated_at = ? WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(next_phase)
        .bind(next_status)
        .bind(blocker.as_deref())
        .bind(next)
        .bind(&at)
        .bind(input.run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        if run_updated.rows_affected() != 1 {
            return Err(StoreError::StaleRevision {
                expected: current,
                current: next,
            });
        }
        let event_payload = approval
            .as_ref()
            .map(|(approval_id, digest, purpose)| {
                serde_json::json!({
                    "runId": input.run_id,
                    "revision": next,
                    "phase": next_phase,
                    "status": next_status,
                    "remoteRequestId": input.request_id,
                    "approval": {
                        "approvalId": approval_id,
                        "purpose": purpose,
                        "digest": digest,
                        "status": "pending"
                    }
                })
                .to_string()
            })
            .unwrap_or_else(|| input.response_json.to_owned());
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(input.run_id),
                revision: next,
                kind: event_kind,
                payload_json: &event_payload,
                created_at: &at,
            },
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    pub async fn sdd_remote_abandon_request(
        &self,
        run_id: &str,
        request_id: &str,
        status: &str,
        error_code: &str,
    ) -> Result<()> {
        if !matches!(status, "canceled" | "interrupted" | "failed") {
            return Err(StoreError::InvalidCommand(
                "remote request terminal status is invalid".into(),
            ));
        }
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let row: Option<(String, String, String, String, i64, String, String)> = sqlx::query_as(
            "SELECT q.attempt_id, q.status, r.repo_id, r.spec_id, r.aggregate_revision,
                    r.status, a.status
             FROM sdd_remote_requests q JOIN sdd_runs r ON r.run_id = q.run_id
             JOIN sdd_attempts a ON a.attempt_id = q.attempt_id
             WHERE q.run_id = ? AND q.request_id = ?",
        )
        .bind(run_id)
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((
            attempt,
            request_status,
            repo_id,
            spec_id,
            revision,
            run_status,
            attempt_status,
        )) = row
        {
            if matches!(
                request_status.as_str(),
                "succeeded" | "failed" | "canceled" | "interrupted"
            ) {
                tx.commit().await?;
                return Ok(());
            }
            if !matches!(request_status.as_str(), "running" | "cancel_requested") {
                return Err(StoreError::InvalidCommand(
                    "remote request cannot be abandoned from its current state".into(),
                ));
            }
            let request_updated = sqlx::query(
                "UPDATE sdd_remote_requests SET status = ?, error_code = ?, updated_at = ?
                 WHERE request_id = ? AND status = ?",
            )
            .bind(status)
            .bind(error_code)
            .bind(&at)
            .bind(request_id)
            .bind(&request_status)
            .execute(&mut *tx)
            .await?;
            if request_updated.rows_affected() != 1 {
                return Err(StoreError::InvalidCommand(
                    "remote request changed during abandon".into(),
                ));
            }
            let attempt_target = match run_status.as_str() {
                "paused" => "paused",
                "canceled" => "canceled",
                _ if status == "canceled" => "canceled",
                _ => "failed",
            };
            let attempt_updated = sqlx::query(
                "UPDATE sdd_attempts SET status = ?, finished_at = ?, error_summary = ?
                 WHERE attempt_id = ? AND status = ?",
            )
            .bind(attempt_target)
            .bind(&at)
            .bind(error_code)
            .bind(&attempt)
            .bind(&attempt_status)
            .execute(&mut *tx)
            .await?;
            if attempt_updated.rows_affected() != 1 {
                return Err(StoreError::InvalidCommand(
                    "remote attempt changed during abandon".into(),
                ));
            }
            let projection_updated = sqlx::query(
                "UPDATE sdd_remote_runs SET active_request_id = NULL, updated_at = ?
                 WHERE run_id = ? AND active_request_id = ?",
            )
            .bind(&at)
            .bind(run_id)
            .bind(request_id)
            .execute(&mut *tx)
            .await?;
            if projection_updated.rows_affected() != 1 {
                return Err(StoreError::InvalidCommand(
                    "remote projection changed during abandon".into(),
                ));
            }
            let payload = serde_json::json!({
                "runId": run_id,
                "revision": revision,
                "requestId": request_id,
                "requestStatus": status,
                "runStatus": run_status,
                "errorCode": error_code,
            })
            .to_string();
            append_event(
                &mut tx,
                EventInsert {
                    repo_id: &repo_id,
                    spec_id: Some(&spec_id),
                    run_id: Some(run_id),
                    revision,
                    kind: "sdd.remote.request_abandoned",
                    payload_json: &payload,
                    created_at: &at,
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

async fn validate_review_binding(
    tx: &mut Transaction<'_, Sqlite>,
    input: &PublishRemoteDesktopPhase<'_>,
    review_attempt_id: &str,
    spec_revision: i64,
    review: &RemoteArtifactPayloadInput<'_>,
) -> Result<(String, String)> {
    let passes = review
        .content
        .lines()
        .filter(|line| line.trim() == "Verdict: PASS")
        .count();
    if passes != 1
        || review
            .content
            .lines()
            .any(|line| line.trim() == "Verdict: FAIL")
        || !review.content.contains("AC-")
    {
        return Err(StoreError::InvalidCommand(
            "review artifact has no single acceptance-criteria-bound PASS verdict".into(),
        ));
    }
    let (planned, completed, unfinished): (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM sdd_tasks WHERE run_id = ? AND spec_revision = ?),
           (SELECT COUNT(*) FROM sdd_remote_task_completions WHERE run_id = ?),
           (SELECT COUNT(*) FROM sdd_tasks WHERE run_id = ? AND spec_revision = ?
             AND runtime_status != 'succeeded')",
    )
    .bind(input.run_id)
    .bind(spec_revision)
    .bind(input.run_id)
    .bind(input.run_id)
    .bind(spec_revision)
    .fetch_one(&mut **tx)
    .await?;
    let verified: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sdd_verification_results WHERE run_id = ?
         AND spec_revision = ? AND status = 'succeeded'",
    )
    .bind(input.run_id)
    .bind(spec_revision)
    .fetch_one(&mut **tx)
    .await?;
    let plan_content: String = sqlx::query_scalar(
        "SELECT p.content FROM sdd_remote_artifact_payloads p
         JOIN sdd_artifact_revisions a ON a.artifact_revision_id = p.artifact_revision_id
         WHERE p.run_id = ? AND a.kind = 'plan' AND a.spec_revision = ?
         ORDER BY a.revision DESC LIMIT 1",
    )
    .bind(input.run_id)
    .bind(spec_revision)
    .fetch_one(&mut **tx)
    .await?;
    let plan: PlanArtifact = serde_json::from_str(&plan_content)?;
    let expected_browser_checks = plan
        .tasks
        .iter()
        .map(|task| task.browser_checks.len())
        .sum::<usize>();
    let browser_evidence: Vec<(String, String)> = sqlx::query_as(
        "SELECT manifest_sha256, status FROM sdd_browser_evidence
         WHERE run_id = ? AND spec_revision = ? ORDER BY check_id",
    )
    .bind(input.run_id)
    .bind(spec_revision)
    .fetch_all(&mut **tx)
    .await?;
    let implementation: Option<(String, String, String)> = sqlx::query_as(
        "SELECT q.request_id, e.evidence_sha256, a.session_identity
         FROM sdd_remote_requests q JOIN sdd_remote_evidence e ON e.request_id = q.request_id
         JOIN sdd_attempts a ON a.attempt_id = q.attempt_id
         WHERE q.run_id = ? AND q.phase = 'implementation' AND q.status = 'succeeded'
         ORDER BY q.created_at DESC LIMIT 1",
    )
    .bind(input.run_id)
    .fetch_optional(&mut **tx)
    .await?;
    let verification: Option<(String, String)> = sqlx::query_as(
        "SELECT q.request_id, e.evidence_sha256
         FROM sdd_remote_requests q JOIN sdd_remote_evidence e ON e.request_id = q.request_id
         WHERE q.run_id = ? AND q.phase = 'verification' AND q.status = 'succeeded'
         ORDER BY q.created_at DESC LIMIT 1",
    )
    .bind(input.run_id)
    .fetch_optional(&mut **tx)
    .await?;
    let review_identity: String =
        sqlx::query_scalar("SELECT session_identity FROM sdd_attempts WHERE attempt_id = ?")
            .bind(review_attempt_id)
            .fetch_one(&mut **tx)
            .await?;
    let (implementation_request, implementation_sha, implementation_identity) = implementation
        .ok_or_else(|| {
            StoreError::InvalidCommand("implementation completion evidence is missing".into())
        })?;
    let (verification_request, verification_sha) = verification
        .ok_or_else(|| StoreError::InvalidCommand("verification evidence is missing".into()))?;
    if planned == 0
        || planned != completed
        || unfinished != 0
        || verified == 0
        || browser_evidence.len() != expected_browser_checks
        || browser_evidence
            .iter()
            .any(|(digest, status)| !valid_sha256(digest) || status != "passed")
        || implementation_identity == review_identity
    {
        return Err(StoreError::InvalidCommand(
            "Ready requires evidence-complete tasks, verification, and an independent review session"
                .into(),
        ));
    }
    let mut evidence_hashes = vec![implementation_sha.clone(), verification_sha.clone()];
    evidence_hashes.extend(browser_evidence.iter().map(|(digest, _)| digest.clone()));
    evidence_hashes.sort_unstable();
    let manifest_hashes = serde_json::to_string(&evidence_hashes)?;
    let binding = serde_json::json!({
        "schemaVersion": 1,
        "runId": input.run_id,
        "specRevision": spec_revision,
        "implementationRequestId": implementation_request,
        "implementationEvidenceSha256": implementation_sha,
        "verificationRequestId": verification_request,
        "verificationEvidenceSha256": verification_sha,
        "browserEvidenceManifestSha256": browser_evidence
            .iter()
            .map(|(digest, _)| digest)
            .collect::<Vec<_>>(),
        "reviewRequestId": input.request_id,
        "reviewSessionIdentity": review_identity,
        "reviewContentSha256": review.content_sha256,
    });
    Ok((sha256(serde_json::to_vec(&binding)?), manifest_hashes))
}

async fn record_implementation_evidence(
    tx: &mut Transaction<'_, Sqlite>,
    input: &PublishRemoteDesktopPhase<'_>,
    spec_id: &str,
    spec_revision: i64,
    at: &str,
) -> Result<()> {
    let summary = input.evidence_summary.ok_or_else(|| {
        StoreError::InvalidCommand("implementation completion evidence is required".into())
    })?;
    if sha256(summary.as_bytes()) != input.evidence_sha256 {
        return Err(StoreError::InvalidCommand(
            "implementation evidence digest mismatch".into(),
        ));
    }
    let evidence: StoredImplementationEvidence = serde_json::from_str(summary)?;
    if evidence.schema_version != 1
        || evidence.request_id != input.request_id
        || evidence.spec_id != spec_id
        || evidence.spec_revision != spec_revision
        || evidence.tasks.is_empty()
    {
        return Err(StoreError::InvalidCommand(
            "implementation evidence identity is invalid".into(),
        ));
    }
    let planned: Vec<String> = sqlx::query_scalar(
        "SELECT task_id FROM sdd_tasks WHERE run_id = ? AND spec_revision = ? ORDER BY task_id",
    )
    .bind(input.run_id)
    .bind(spec_revision)
    .fetch_all(&mut **tx)
    .await?;
    let mut offered = evidence
        .tasks
        .iter()
        .map(|task| task.task_id.clone())
        .collect::<Vec<_>>();
    offered.sort();
    offered.dedup();
    if offered != planned || evidence.tasks.len() != planned.len() {
        return Err(StoreError::InvalidCommand(
            "implementation evidence does not cover the exact planned task set".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO sdd_remote_evidence
         (request_id, run_id, phase, evidence_sha256, summary, created_at)
         VALUES (?, ?, 'implementation', ?, ?, ?)",
    )
    .bind(input.request_id)
    .bind(input.run_id)
    .bind(input.evidence_sha256)
    .bind(summary)
    .bind(at)
    .execute(&mut **tx)
    .await?;
    for task in &evidence.tasks {
        if !valid_sha256(&task.patch_sha256) || !valid_sha256(&task.write_set_sha256) {
            return Err(StoreError::InvalidCommand(
                "implementation task evidence digest is invalid".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO sdd_remote_task_completions
             (run_id, task_id, request_id, patch_sha256, write_set_sha256, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(input.run_id)
        .bind(&task.task_id)
        .bind(input.request_id)
        .bind(&task.patch_sha256)
        .bind(&task.write_set_sha256)
        .bind(at)
        .execute(&mut **tx)
        .await?;
        let updated = sqlx::query(
            "UPDATE sdd_tasks SET runtime_status = 'succeeded',
             aggregate_revision = aggregate_revision + 1, updated_at = ?
             WHERE run_id = ? AND task_id = ? AND spec_revision = ?
               AND runtime_status IN ('queued', 'retry_scheduled')",
        )
        .bind(at)
        .bind(input.run_id)
        .bind(&task.task_id)
        .bind(spec_revision)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "planned task changed before implementation evidence publication".into(),
            ));
        }
    }
    Ok(())
}

async fn record_verification_evidence(
    tx: &mut Transaction<'_, Sqlite>,
    input: &PublishRemoteDesktopPhase<'_>,
    attempt_id: &str,
    spec_revision: i64,
    at: &str,
) -> Result<()> {
    let summary = input.evidence_summary.ok_or_else(|| {
        StoreError::InvalidCommand("verification evidence summary is required".into())
    })?;
    if sha256(summary.as_bytes()) != input.evidence_sha256 {
        return Err(StoreError::InvalidCommand(
            "verification evidence digest mismatch".into(),
        ));
    }
    let evidence: StoredRemoteVerificationEvidence = serde_json::from_str(summary)?;
    if evidence.schema_version != 1 {
        return Err(StoreError::InvalidCommand(
            "remote verification evidence schema is invalid".into(),
        ));
    }
    let plan_content: String = sqlx::query_scalar(
        "SELECT p.content FROM sdd_remote_artifact_payloads p
         JOIN sdd_artifact_revisions a ON a.artifact_revision_id = p.artifact_revision_id
         WHERE p.run_id = ? AND a.kind = 'plan' AND a.spec_revision = ?
         ORDER BY a.revision DESC LIMIT 1",
    )
    .bind(input.run_id)
    .bind(spec_revision)
    .fetch_one(&mut **tx)
    .await?;
    let plan: PlanArtifact = serde_json::from_str(&plan_content)?;
    let mut commands = vec![CommandSpec {
        program: "git".into(),
        args: vec!["diff".into(), "--check".into()],
        cwd: ".".into(),
        env_allowlist: vec!["PATH".into()],
        timeout_ms: 60_000,
        output_limit: 256 * 1024,
    }];
    commands.extend(
        plan.tasks
            .iter()
            .flat_map(|task| task.verification.iter().cloned()),
    );
    if evidence.command_results.len() != commands.len() || evidence.command_results.is_empty() {
        return Err(StoreError::InvalidCommand(
            "verification evidence does not cover the typed plan commands".into(),
        ));
    }
    for (index, (result, expected)) in evidence
        .command_results
        .iter()
        .zip(commands.iter())
        .enumerate()
    {
        let command: CommandSpec = serde_json::from_str(&result.command_json)?;
        if result.command_index != index as i64
            || command != *expected
            || !valid_command(&command)
            || result.status != "succeeded"
            || result.exit_code != Some(0)
            || !valid_sha256(&result.output_hash)
            || result.output_excerpt.len() > 64 * 1024
            || !(0..=3_600_000).contains(&result.duration_ms)
        {
            return Err(StoreError::InvalidCommand(
                "verification evidence is malformed or not source-bound".into(),
            ));
        }
    }
    let browser_checks = plan
        .tasks
        .iter()
        .flat_map(|task| task.browser_checks.iter())
        .collect::<Vec<_>>();
    if evidence.browser_results.len() != browser_checks.len()
        || input.browser_evidence.len() != browser_checks.len()
    {
        return Err(StoreError::InvalidCommand(
            "remote browser evidence does not cover the typed plan checks".into(),
        ));
    }
    for (index, ((remote, prepared), check)) in evidence
        .browser_results
        .iter()
        .zip(input.browser_evidence.iter())
        .zip(browser_checks.iter())
        .enumerate()
    {
        let manifest: serde_json::Value = serde_json::from_str(&prepared.manifest_json)?;
        let mut remote_blobs = remote
            .blobs
            .iter()
            .map(|blob| {
                (
                    blob.sha256.as_str(),
                    blob.byte_length as i64,
                    blob.media_type.as_str(),
                    blob.role.as_str(),
                )
            })
            .collect::<Vec<_>>();
        remote_blobs.sort_unstable();
        let mut prepared_blobs = prepared
            .blobs
            .iter()
            .map(|blob| {
                (
                    blob.sha256.as_str(),
                    blob.byte_length,
                    blob.media_type.as_str(),
                    blob.role.as_str(),
                )
            })
            .collect::<Vec<_>>();
        prepared_blobs.sort_unstable();
        let expected_command = serde_json::json!({ "type": "browserCheck", "check": check });
        let command: serde_json::Value =
            serde_json::from_str(&prepared.verification_result.command_json)?;
        if remote.check_id != check.id
            || remote.check_id != prepared.check_id
            || remote.status != "passed"
            || prepared.status != "passed"
            || remote.captured_at != prepared.captured_at
            || remote.duration_ms != prepared.verification_result.duration_ms
            || remote.output_excerpt != prepared.verification_result.output_excerpt
            || remote.output_excerpt.len() > 64 * 1024
            || !matches!(remote.target, serde_json::Value::Object(_))
            || !matches!(remote.browser, serde_json::Value::Object(_))
            || !matches!(remote.assertions, serde_json::Value::Array(_))
            || !matches!(remote.console, serde_json::Value::Object(_))
            || !matches!(remote.network, serde_json::Value::Object(_))
            || remote.blobs.iter().any(|blob| {
                !valid_sha256(&blob.sha256)
                    || blob.byte_length == 0
                    || blob.byte_length > 8 * 1024 * 1024
                    || blob.media_type.is_empty()
                    || !matches!(
                        blob.role.as_str(),
                        "capture" | "console_transcript" | "network_transcript"
                    )
                    || blob.content_base64.is_empty()
            })
            || remote_blobs != prepared_blobs
            || sha256(prepared.manifest_json.as_bytes()) != prepared.manifest_sha256
            || manifest
                .get("schemaVersion")
                .and_then(serde_json::Value::as_u64)
                != Some(1)
            || manifest
                .get("evidenceId")
                .and_then(serde_json::Value::as_str)
                != Some(prepared.evidence_id.as_str())
            || manifest.get("runId").and_then(serde_json::Value::as_str) != Some(input.run_id)
            || manifest
                .get("attemptId")
                .and_then(serde_json::Value::as_str)
                != Some(attempt_id)
            || manifest.get("checkId").and_then(serde_json::Value::as_str)
                != Some(check.id.as_str())
            || manifest
                .get("specRevision")
                .and_then(serde_json::Value::as_i64)
                != Some(spec_revision)
            || command != expected_command
            || prepared.verification_result.command_index != commands.len() as i64 + index as i64
            || prepared.verification_result.status != "succeeded"
            || prepared.verification_result.exit_code.is_some()
            || prepared.verification_result.output_hash != prepared.manifest_sha256
            || !(0..=3_600_000).contains(&prepared.verification_result.duration_ms)
        {
            return Err(StoreError::InvalidCommand(
                "remote browser evidence is malformed or not source-bound".into(),
            ));
        }
    }
    sqlx::query(
        "INSERT INTO sdd_remote_evidence
         (request_id, run_id, phase, evidence_sha256, summary, created_at)
         VALUES (?, ?, 'verification', ?, ?, ?)",
    )
    .bind(input.request_id)
    .bind(input.run_id)
    .bind(input.evidence_sha256)
    .bind(summary)
    .bind(at)
    .execute(&mut **tx)
    .await?;
    let grant_id = (!input.browser_evidence.is_empty()).then(|| Uuid::new_v4().to_string());
    if let Some(grant_id) = grant_id.as_deref() {
        let scope_json = serde_json::json!({
            "schemaVersion": 1,
            "source": "trusted_remote_sdd_worker",
            "runId": input.run_id,
            "attemptId": attempt_id,
            "specRevision": spec_revision,
            "checkIds": input.browser_evidence.iter().map(|item| item.check_id.as_str()).collect::<Vec<_>>()
        })
        .to_string();
        let grant_inserted = sqlx::query(
            "INSERT INTO sdd_capability_grants
             (grant_id, run_id, attempt_id, capability, scope_json, token_hash, expires_at,
              revoked_at)
             VALUES (?, ?, ?, 'browser_evidence.submit', ?, ?, ?, ?)",
        )
        .bind(grant_id)
        .bind(input.run_id)
        .bind(attempt_id)
        .bind(&scope_json)
        .bind(sha256(format!(
            "remote-browser:{}:{grant_id}",
            input.request_id
        )))
        .bind(at)
        .bind(at)
        .execute(&mut **tx)
        .await?;
        if grant_inserted.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote browser evidence grant was not recorded".into(),
            ));
        }
        for prepared in input.browser_evidence {
            for blob in &prepared.blobs {
                let inserted = sqlx::query(
                    "INSERT INTO sdd_evidence_blobs
                     (sha256, byte_length, media_type, storage_relative_path, created_at)
                     VALUES (?, ?, ?, ?, ?) ON CONFLICT(sha256) DO NOTHING",
                )
                .bind(&blob.sha256)
                .bind(blob.byte_length)
                .bind(&blob.media_type)
                .bind(&blob.storage_relative_path)
                .bind(at)
                .execute(&mut **tx)
                .await?;
                let _ = inserted;
                let stored: (i64, String, String) = sqlx::query_as(
                    "SELECT byte_length, media_type, storage_relative_path
                     FROM sdd_evidence_blobs WHERE sha256 = ?",
                )
                .bind(&blob.sha256)
                .fetch_one(&mut **tx)
                .await?;
                if stored
                    != (
                        blob.byte_length,
                        blob.media_type.clone(),
                        blob.storage_relative_path.clone(),
                    )
                {
                    return Err(StoreError::InvalidCommand(
                        "remote evidence blob metadata collided".into(),
                    ));
                }
            }
            let evidence_inserted = sqlx::query(
                "INSERT INTO sdd_browser_evidence
                 (evidence_id, run_id, attempt_id, grant_id, spec_revision, check_id,
                  manifest_sha256, manifest_json, status, submitted_by, captured_at, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&prepared.evidence_id)
            .bind(input.run_id)
            .bind(attempt_id)
            .bind(grant_id)
            .bind(spec_revision)
            .bind(&prepared.check_id)
            .bind(&prepared.manifest_sha256)
            .bind(&prepared.manifest_json)
            .bind(&prepared.status)
            .bind(format!("remote-browser:{}", input.request_id))
            .bind(&prepared.captured_at)
            .bind(at)
            .execute(&mut **tx)
            .await?;
            if evidence_inserted.rows_affected() != 1 {
                return Err(StoreError::InvalidCommand(
                    "remote browser evidence was not recorded".into(),
                ));
            }
            for blob in &prepared.blobs {
                let reference_inserted = sqlx::query(
                    "INSERT INTO sdd_browser_evidence_blobs (evidence_id, sha256, role)
                     VALUES (?, ?, ?)",
                )
                .bind(&prepared.evidence_id)
                .bind(&blob.sha256)
                .bind(&blob.role)
                .execute(&mut **tx)
                .await?;
                if reference_inserted.rows_affected() != 1 {
                    return Err(StoreError::InvalidCommand(
                        "remote browser evidence reference was not recorded".into(),
                    ));
                }
            }
        }
    }
    let results = evidence
        .command_results
        .iter()
        .chain(
            input
                .browser_evidence
                .iter()
                .map(|item| &item.verification_result),
        )
        .collect::<Vec<_>>();
    for result in results {
        sqlx::query(
            "INSERT INTO sdd_verification_results
             (verification_id, run_id, attempt_id, task_id, spec_revision, command_index,
              command_json, status, exit_code, output_hash, output_excerpt, duration_ms, created_at)
             VALUES (?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(input.run_id)
        .bind(attempt_id)
        .bind(spec_revision)
        .bind(result.command_index)
        .bind(&result.command_json)
        .bind(&result.status)
        .bind(result.exit_code)
        .bind(&result.output_hash)
        .bind(&result.output_excerpt)
        .bind(result.duration_ms)
        .bind(at)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn insert_plan_tasks(
    tx: &mut Transaction<'_, Sqlite>,
    run_id: &str,
    spec_id: &str,
    spec_revision: i64,
    content: &str,
    at: &str,
) -> Result<()> {
    let plan: PlanArtifact = serde_json::from_str(content)?;
    if plan.schema_version != 1
        || plan.spec_id.to_string() != spec_id
        || plan.spec_revision != spec_revision
        || plan.tasks.is_empty()
    {
        return Err(StoreError::InvalidCommand(
            "remote plan identity or task set is invalid".into(),
        ));
    }
    let mut ids = std::collections::HashSet::new();
    for task in &plan.tasks {
        if task.id.trim().is_empty()
            || task.id.len() > 128
            || !ids.insert(task.id.as_str())
            || task.objective.trim().is_empty()
            || task.risk.trim().is_empty()
            || task.acceptance_criteria.is_empty()
            || task
                .acceptance_criteria
                .iter()
                .any(|criterion| !criterion.starts_with("AC-") || criterion.len() > 128)
            || task
                .read_scopes
                .iter()
                .chain(task.write_scopes.iter())
                .any(|path| agentum_core::sdd::validate_relative_path(path).is_err())
            || task
                .verification
                .iter()
                .any(|command| !valid_command(command))
        {
            return Err(StoreError::InvalidCommand(
                "remote plan task intent is invalid".into(),
            ));
        }
    }
    if plan.tasks.iter().any(|task| {
        task.dependencies
            .iter()
            .any(|dependency| dependency == &task.id || !ids.contains(dependency.as_str()))
    }) || !acyclic_plan(&plan)
    {
        return Err(StoreError::InvalidCommand(
            "remote plan dependency graph is invalid".into(),
        ));
    }
    for task in &plan.tasks {
        sqlx::query(
            "INSERT INTO sdd_tasks
             (run_id, task_id, spec_revision, intent_json, runtime_status,
              aggregate_revision, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'queued', 1, ?, ?)",
        )
        .bind(run_id)
        .bind(&task.id)
        .bind(spec_revision)
        .bind(serde_json::to_string(task)?)
        .bind(at)
        .bind(at)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn validate_success_shape(
    phase: &str,
    spec_slug: &str,
    artifacts: &[RemoteArtifactPayloadInput<'_>],
    evidence_summary: Option<&str>,
) -> Result<()> {
    let valid_artifact = |kind: &str, file_name: &str| {
        artifacts.len() == 1
            && artifacts[0].kind == kind
            && artifacts[0].relative_path == format!(".agentum/specs/{spec_slug}/{file_name}")
    };
    let valid = match phase {
        "design" => valid_artifact("design", "design.md") && evidence_summary.is_none(),
        "planning" => valid_artifact("plan", "plan.json") && evidence_summary.is_none(),
        "implementation" => artifacts.is_empty() && evidence_summary.is_some(),
        "verification" => artifacts.is_empty() && evidence_summary.is_some(),
        "review" => valid_artifact("review", "review.md") && evidence_summary.is_none(),
        _ => false,
    };
    if !valid {
        return Err(StoreError::InvalidCommand(format!(
            "remote {phase} result has an invalid artifact/evidence shape"
        )));
    }
    Ok(())
}

fn validate_plan_checkpoint(
    plan: &StoredRemotePlan,
    checkpoint: &StoredRemoteCheckpoint,
    run_id: &str,
) -> Result<()> {
    validate_current_plan_checkpoint(plan, checkpoint, run_id)?;
    if checkpoint.next_phase == "design" && checkpoint.completed_phases == 0 {
        Ok(())
    } else {
        Err(StoreError::InvalidCommand(
            "initial remote checkpoint must begin at design".into(),
        ))
    }
}

fn validate_current_plan_checkpoint(
    plan: &StoredRemotePlan,
    checkpoint: &StoredRemoteCheckpoint,
    run_id: &str,
) -> Result<()> {
    let expected_completed = match checkpoint.next_phase.as_str() {
        "design" => Some(0),
        "planning" => Some(1),
        "implementation" => Some(2),
        "verification" => Some(3),
        "review" => Some(4),
        "ready" => Some(5),
        _ => None,
    };
    let valid = plan.schema_version == 1
        && checkpoint.schema_version == 1
        && plan.run_id == run_id
        && checkpoint.run_id == run_id
        && plan.host_id == checkpoint.host_id
        && plan.spec_revision > 0
        && plan.spec_revision == checkpoint.spec_revision
        && plan.approval_digest == checkpoint.approval_digest
        && valid_sha256(&plan.repository_identity_sha256)
        && valid_sha256(&plan.approval_digest)
        && valid_sha256(&checkpoint.workspace_state_sha256)
        && valid_sha256(&checkpoint.last_result_sha256)
        && plan.spec_id.parse::<agentum_core::sdd::SpecId>().is_ok()
        && matches!(plan.base_commit.len(), 40 | 64)
        && plan
            .base_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        && !plan.provider.trim().is_empty()
        && plan.provider.len() <= 160
        && valid_ulid(&plan.artifact_set_id)
        && (1_000..=3_600_000).contains(&plan.timeout_ms)
        && (1_024..=8 * 1024 * 1024).contains(&plan.output_limit)
        && expected_completed == Some(checkpoint.completed_phases);
    if valid {
        Ok(())
    } else {
        Err(StoreError::InvalidCommand(
            "remote plan/checkpoint contract is invalid".into(),
        ))
    }
}

fn validate_phase_request(
    request: &StoredRemotePhaseRequest,
    input: &ReserveRemoteDesktopPhase<'_>,
) -> Result<()> {
    let valid = request.schema_version == 1
        && request.request_id == input.request_id
        && request.run_id == input.run_id
        && request.phase == input.phase
        && request.spec_revision > 0
        && request.spec_id.parse::<agentum_core::sdd::SpecId>().is_ok()
        && uuid::Uuid::parse_str(&request.host_id).is_ok()
        && valid_sha256(&request.repository_identity_sha256)
        && valid_sha256(&request.expected_workspace_state_sha256)
        && valid_sha256(&request.previous_result_sha256)
        && valid_sha256(&request.approval_digest)
        && valid_ulid(&request.artifact_set_id)
        && matches!(request.base_commit.len(), 40 | 64)
        && request
            .base_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        && !request.provider.trim().is_empty()
        && request.provider.len() <= 160
        && (1_000..=3_600_000).contains(&request.timeout_ms)
        && (1_024..=8 * 1024 * 1024).contains(&request.output_limit);
    if valid {
        Ok(())
    } else {
        Err(StoreError::InvalidCommand(
            "remote phase request contract is invalid".into(),
        ))
    }
}

fn validate_checkpoint_advance(
    plan: &StoredRemotePlan,
    previous: &StoredRemoteCheckpoint,
    next: &StoredRemoteCheckpoint,
    phase: &str,
    status: &str,
    result_workspace_sha256: &str,
    result_sha256: &str,
) -> Result<()> {
    validate_current_plan_checkpoint(plan, next, &plan.run_id)?;
    if previous.next_phase != phase
        || next.host_id != previous.host_id
        || next.run_id != previous.run_id
        || next.spec_revision != previous.spec_revision
        || next.approval_digest != previous.approval_digest
    {
        return Err(StoreError::InvalidCommand(
            "remote checkpoint identity or current phase changed".into(),
        ));
    }
    if status == "succeeded" {
        let expected_next = match phase {
            "design" => "planning",
            "planning" => "implementation",
            "implementation" => "verification",
            "verification" => "review",
            "review" => "ready",
            _ => {
                return Err(StoreError::InvalidCommand(
                    "remote checkpoint phase is invalid".into(),
                ));
            }
        };
        if next.next_phase != expected_next
            || next.completed_phases != previous.completed_phases.saturating_add(1)
            || next.workspace_state_sha256 != result_workspace_sha256
            || next.last_result_sha256 != result_sha256
        {
            return Err(StoreError::InvalidCommand(
                "remote checkpoint did not advance exactly one successful phase".into(),
            ));
        }
    } else if next.next_phase != previous.next_phase
        || next.completed_phases != previous.completed_phases
        || next.workspace_state_sha256 != previous.workspace_state_sha256
        || next.last_result_sha256 != previous.last_result_sha256
    {
        return Err(StoreError::InvalidCommand(
            "failed or canceled remote result changed the checkpoint".into(),
        ));
    }
    Ok(())
}

fn validate_phase(phase: &str) -> Result<()> {
    if matches!(
        phase,
        "design" | "planning" | "implementation" | "verification" | "review"
    ) {
        Ok(())
    } else {
        Err(StoreError::InvalidCommand(
            "remote lifecycle phase is invalid".into(),
        ))
    }
}

fn valid_command(command: &CommandSpec) -> bool {
    let forbidden = [
        "bash",
        "sh",
        "dash",
        "zsh",
        "fish",
        "cmd",
        "cmd.exe",
        "powershell",
        "powershell.exe",
        "pwsh",
        "osascript",
        "wscript",
        "cscript",
    ];
    let program = command.program.trim().to_ascii_lowercase();
    !program.is_empty()
        && !command.program.contains(['/', '\\'])
        && !forbidden.contains(&program.as_str())
        && command.args.len() <= 256
        && command
            .args
            .iter()
            .all(|argument| !argument.contains('\0') && argument.len() <= 64 * 1024)
        && (command.cwd == "." || agentum_core::sdd::validate_relative_path(&command.cwd).is_ok())
        && command.env_allowlist.len() <= 32
        && command.env_allowlist.iter().all(|key| {
            matches!(
                key.as_str(),
                "PATH"
                    | "CI"
                    | "NO_COLOR"
                    | "FORCE_COLOR"
                    | "TERM"
                    | "RUST_BACKTRACE"
                    | "RUSTFLAGS"
                    | "CARGO_TERM_COLOR"
                    | "CARGO_INCREMENTAL"
                    | "NODE_ENV"
                    | "PYTHONDONTWRITEBYTECODE"
            )
        })
        && (1..=3_600_000).contains(&command.timeout_ms)
        && (1..=16 * 1024 * 1024).contains(&command.output_limit)
}

fn acyclic_plan(plan: &PlanArtifact) -> bool {
    fn visit<'a>(
        id: &'a str,
        tasks: &std::collections::HashMap<&'a str, &'a agentum_core::sdd::PlanTask>,
        visiting: &mut std::collections::HashSet<&'a str>,
        visited: &mut std::collections::HashSet<&'a str>,
    ) -> bool {
        if visited.contains(id) {
            return true;
        }
        if !visiting.insert(id) {
            return false;
        }
        let valid = tasks[id]
            .dependencies
            .iter()
            .all(|dependency| visit(dependency, tasks, visiting, visited));
        visiting.remove(id);
        visited.insert(id);
        valid
    }
    let tasks = plan
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect::<std::collections::HashMap<_, _>>();
    let mut visiting = std::collections::HashSet::new();
    let mut visited = std::collections::HashSet::new();
    plan.tasks
        .iter()
        .all(|task| visit(&task.id, &tasks, &mut visiting, &mut visited))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_ulid(value: &str) -> bool {
    value.len() == 26
        && value.bytes().all(|byte| {
            matches!(byte, b'0'..=b'9' | b'A'..=b'H' | b'J'..=b'N' | b'P'..=b'T' | b'V'..=b'Z')
        })
}

fn sha256(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
