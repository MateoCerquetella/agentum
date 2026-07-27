//! Transactional persistence for Agentum-native SDD aggregates.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Sqlite, Transaction};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::sdd_remote_projection::{NewSddRemoteProjection, insert_initial_projection};
use crate::sdd_runtime::{SddAttemptRecord, SddTaskRecord, SddVerificationRecord};
use crate::{Result, Store, StoreError};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddSpecRecord {
    pub spec_id: String,
    #[serde(skip_serializing)]
    pub spec_ulid: String,
    pub repo_id: String,
    pub title: String,
    pub slug: String,
    pub profile: String,
    pub control: String,
    pub provider: String,
    pub source_ref_json: Option<String>,
    pub current_revision: i64,
    pub aggregate_revision: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddRunRecord {
    pub run_id: String,
    pub spec_id: String,
    pub repo_id: String,
    pub phase: String,
    pub status: String,
    pub aggregate_revision: i64,
    pub base_ref: String,
    pub base_commit: String,
    pub branch_name: String,
    pub authoritative_path: String,
    pub workspace_fingerprint: String,
    pub policy_json: String,
    pub blocker: Option<String>,
    pub quarantined: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddArtifactRecord {
    pub artifact_revision_id: String,
    pub run_id: String,
    pub spec_id: String,
    pub kind: String,
    pub revision: i64,
    pub spec_revision: i64,
    pub relative_path: String,
    pub content_hash: String,
    pub submitted_by: String,
    pub evidence_digest: Option<String>,
    pub evidence_manifest_hashes_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddApprovalRecord {
    pub approval_id: String,
    pub run_id: String,
    pub purpose: String,
    pub digest: String,
    pub requested_revision: i64,
    pub requested_by: String,
    pub status: String,
    pub invalidated_at: Option<String>,
    pub created_at: String,
    pub decided_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddEventRecord {
    pub cursor: i64,
    pub event_id: String,
    pub repo_id: String,
    pub spec_id: Option<String>,
    pub run_id: Option<String>,
    pub aggregate_revision: i64,
    pub kind: String,
    pub payload_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddCreateSagaRecord {
    pub repo_id: String,
    pub request_id: String,
    pub request_hash: String,
    pub spec_id: String,
    pub run_id: String,
    pub stage: String,
    pub repository_path: String,
    pub authoritative_path: String,
    pub branch_name: String,
    pub attempt_id: String,
    pub attempt_path: String,
    pub error_summary: Option<String>,
    pub response_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddRunCreateSagaRecord {
    pub spec_id: String,
    pub repo_id: String,
    pub request_id: String,
    pub request_hash: String,
    pub run_id: String,
    pub stage: String,
    pub expected_spec_revision: i64,
    pub expected_spec_hash: String,
    pub expected_aggregate_revision: i64,
    pub repository_path: String,
    pub authoritative_path: String,
    pub branch_name: String,
    pub attempt_id: String,
    pub attempt_path: String,
    pub error_summary: Option<String>,
    pub response_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SddImportJobRecord {
    pub import_id: String,
    pub repo_id: String,
    pub source_kind: String,
    pub source_hash: String,
    pub preview_json: String,
    pub disposition: String,
    pub created_at: String,
    pub committed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewSddCreateSaga<'a> {
    pub repo_id: &'a str,
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub spec_id: &'a str,
    pub run_id: &'a str,
    pub repository_path: &'a str,
    pub authoritative_path: &'a str,
    pub branch_name: &'a str,
    pub attempt_id: &'a str,
    pub attempt_path: &'a str,
    pub artifact_set_id: &'a str,
    pub artifact_set_required: bool,
}

/// Server-authoritative work-item provenance captured by the source adapter.
/// These values must come from the authenticated provider response, never from
/// a caller-supplied revision or external identifier.
#[derive(Debug, Clone)]
pub struct NewSddExternalLink<'a> {
    pub provider: &'a str,
    pub connection_id: &'a str,
    pub site_id: Option<&'a str>,
    pub external_id: &'a str,
    pub key: Option<&'a str>,
    pub url: &'a str,
    pub source_revision: &'a str,
}

/// Immutable import snapshot metadata committed with a newly authored spec.
#[derive(Debug, Clone)]
pub struct NewSddImportJob<'a> {
    pub source_kind: &'a str,
    pub source_hash: &'a str,
    pub preview_json: &'a str,
    pub disposition: &'a str,
}

#[derive(Debug, Clone)]
pub struct NewSddAggregate<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub spec_id: &'a str,
    pub spec_ulid: &'a str,
    pub repo_id: &'a str,
    pub title: &'a str,
    pub slug: &'a str,
    pub profile: &'a str,
    pub control: &'a str,
    pub provider: &'a str,
    pub source_ref_json: Option<&'a str>,
    pub external_link: Option<NewSddExternalLink<'a>>,
    pub import_job: Option<NewSddImportJob<'a>>,
    pub initial_spec_content: &'a str,
    pub initial_spec_hash: &'a str,
    pub spec_content: &'a str,
    pub spec_hash: &'a str,
    pub spec_revision: i64,
    pub submitted_by: &'a str,
    pub attempt_id: &'a str,
    pub attempt_path: &'a str,
    pub attempt_session_identity: &'a str,
    pub run_id: &'a str,
    pub base_ref: &'a str,
    pub base_commit: &'a str,
    pub branch_name: &'a str,
    pub authoritative_path: &'a str,
    pub workspace_fingerprint: &'a str,
    pub policy_json: &'a str,
    pub approval_id: &'a str,
    pub approval_digest: &'a str,
    pub response_json: &'a str,
    pub remote_projection: Option<NewSddRemoteProjection<'a>>,
}

#[derive(Debug, Clone)]
pub struct NewSddRunCreateSaga<'a> {
    pub spec_id: &'a str,
    pub repo_id: &'a str,
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_spec_revision: i64,
    pub expected_spec_hash: &'a str,
    pub expected_aggregate_revision: i64,
    pub repository_path: &'a str,
    pub authoritative_path: &'a str,
    pub branch_name: &'a str,
    pub attempt_id: &'a str,
    pub attempt_path: &'a str,
}

#[derive(Debug, Clone)]
pub struct NewSddRunArtifact<'a> {
    pub kind: &'a str,
    pub relative_path: &'a str,
    pub content_hash: &'a str,
}

#[derive(Debug, Clone)]
pub struct NewSddDiscoveredRun<'a> {
    pub spec_id: &'a str,
    pub repo_id: &'a str,
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub expected_aggregate_revision: i64,
    pub expected_spec_revision: i64,
    pub expected_spec_hash: &'a str,
    pub profile: &'a str,
    pub control: &'a str,
    pub provider: &'a str,
    pub run_id: &'a str,
    pub base_ref: &'a str,
    pub base_commit: &'a str,
    pub branch_name: &'a str,
    pub authoritative_path: &'a str,
    pub workspace_fingerprint: &'a str,
    pub policy_json: &'a str,
    pub attempt_id: &'a str,
    pub attempt_path: &'a str,
    pub attempt_session_identity: &'a str,
    pub submitted_by: &'a str,
    pub artifacts: &'a [NewSddRunArtifact<'a>],
    pub approval_id: &'a str,
    pub approval_digest: &'a str,
    pub response_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct ArtifactMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub kind: &'a str,
    pub relative_path: &'a str,
    pub content_hash: &'a str,
    pub content: Option<&'a str>,
    pub attempt_id: &'a str,
    pub submitted_by: &'a str,
    pub approval_id: Option<&'a str>,
    pub approval_digest: Option<&'a str>,
    pub approval_purpose: Option<&'a str>,
    pub evidence_digest: Option<&'a str>,
    pub evidence_manifest_hashes_json: Option<&'a str>,
    pub next_phase: &'a str,
    pub next_status: &'a str,
    pub response_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct ExternalSpecMutation<'a> {
    pub run_id: &'a str,
    pub expected_run_revision: i64,
    pub spec_revision: i64,
    pub title: &'a str,
    pub relative_path: &'a str,
    pub content_hash: &'a str,
    pub content: &'a str,
}

#[derive(Debug, Clone)]
pub struct DiscoveredSpecInput<'a> {
    pub spec_id: &'a str,
    pub spec_ulid: &'a str,
    pub title: &'a str,
    pub slug: &'a str,
    pub source_ref_json: Option<&'a str>,
    pub revision: i64,
    pub content_hash: &'a str,
    pub content: &'a str,
}

#[derive(Debug, Clone)]
pub struct ReconcileDiscoveredSpecs<'a> {
    pub repo_id: &'a str,
    pub artifact_set_id: &'a str,
    pub specs: &'a [DiscoveredSpecInput<'a>],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileDiscoveredResult {
    pub inserted: usize,
    pub revised: usize,
    pub unchanged: usize,
}

#[derive(Debug, Clone)]
pub struct TransitionMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub phase: &'a str,
    pub status: &'a str,
    pub blocker: Option<&'a str>,
    pub event_kind: &'a str,
    pub response_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct ApprovalDecisionMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub approval_id: &'a str,
    pub digest: &'a str,
    pub actor_id: &'a str,
    pub decision: &'a str,
    pub reason: Option<&'a str>,
    pub response_json: &'a str,
}

pub(crate) struct EventInsert<'a> {
    pub(crate) repo_id: &'a str,
    pub(crate) spec_id: Option<&'a str>,
    pub(crate) run_id: Option<&'a str>,
    pub(crate) revision: i64,
    pub(crate) kind: &'a str,
    pub(crate) payload_json: &'a str,
    pub(crate) created_at: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SddSnapshot {
    pub spec: SddSpecRecord,
    pub run: SddRunRecord,
    pub artifacts: Vec<SddArtifactRecord>,
    pub approval: Option<SddApprovalRecord>,
    pub tasks: Vec<SddTaskRecord>,
    pub attempts: Vec<SddAttemptRecord>,
    pub verification: Vec<SddVerificationRecord>,
}

pub(crate) fn now() -> Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

pub(crate) async fn append_event(
    tx: &mut Transaction<'_, Sqlite>,
    input: EventInsert<'_>,
) -> Result<i64> {
    let cursor: i64 = sqlx::query_scalar(
        "INSERT INTO sdd_events
         (event_id, repo_id, spec_id, run_id, aggregate_revision, kind, payload_json, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING cursor",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(input.repo_id)
    .bind(input.spec_id)
    .bind(input.run_id)
    .bind(input.revision)
    .bind(input.kind)
    .bind(input.payload_json)
    .bind(input.created_at)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO sdd_outbox
         (outbox_id, event_cursor, destination, payload_json, attempts, available_at)
         VALUES (?, ?, 'realtime', ?, 0, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(cursor)
    .bind(input.payload_json)
    .bind(input.created_at)
    .execute(&mut **tx)
    .await?;
    Ok(cursor)
}

impl Store {
    pub async fn sdd_create_saga(
        &self,
        repo_id: &str,
        request_id: &str,
    ) -> Result<Option<SddCreateSagaRecord>> {
        Ok(
            sqlx::query_as("SELECT * FROM sdd_create_sagas WHERE repo_id = ? AND request_id = ?")
                .bind(repo_id)
                .bind(request_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Commit the create intent before any worktree/provider side effect.
    pub async fn sdd_reserve_create(&self, input: NewSddCreateSaga<'_>) -> Result<String> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO sdd_repo_artifact_sets (repo_id, artifact_set_id, created_at)
             VALUES (?, ?, ?)
             ON CONFLICT(repo_id) DO NOTHING",
        )
        .bind(input.repo_id)
        .bind(input.artifact_set_id)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        let artifact_set_id: String = sqlx::query_scalar(
            "SELECT artifact_set_id FROM sdd_repo_artifact_sets WHERE repo_id = ?",
        )
        .bind(input.repo_id)
        .fetch_one(&mut *tx)
        .await?;
        if input.artifact_set_required && artifact_set_id != input.artifact_set_id {
            return Err(StoreError::ArtifactSetConflict(format!(
                "registered {}, repository contains {}",
                artifact_set_id, input.artifact_set_id
            )));
        }
        let result = sqlx::query(
            "INSERT INTO sdd_create_sagas
             (repo_id, request_id, request_hash, spec_id, run_id, stage,
              repository_path, authoritative_path, branch_name, attempt_id,
              attempt_path, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'reserved', ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(input.repo_id)
        .bind(input.request_id)
        .bind(input.request_hash)
        .bind(input.spec_id)
        .bind(input.run_id)
        .bind(input.repository_path)
        .bind(input.authoritative_path)
        .bind(input.branch_name)
        .bind(input.attempt_id)
        .bind(input.attempt_path)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await;
        match result {
            Ok(_) => {
                tx.commit().await?;
                Ok(artifact_set_id)
            }
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
                drop(tx);
                let existing = self
                    .sdd_create_saga(input.repo_id, input.request_id)
                    .await?;
                match existing {
                    Some(existing) if existing.request_hash != input.request_hash => {
                        Err(StoreError::IdempotencyConflict(format!(
                            "repo:{}:create_spec",
                            input.repo_id
                        )))
                    }
                    Some(existing) => Err(StoreError::AlreadyExists(existing.run_id)),
                    None => Err(StoreError::AlreadyExists(input.run_id.into())),
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn sdd_update_create_stage(
        &self,
        repo_id: &str,
        request_id: &str,
        request_hash: &str,
        from: &[&str],
        stage: &str,
        error_summary: Option<&str>,
    ) -> Result<()> {
        let allowed = [
            "reserved",
            "workspace_ready",
            "authoring",
            "publishing",
            "completed",
            "failed",
            "canceled",
            "recovery_required",
        ];
        if !allowed.contains(&stage) || from.is_empty() || from.iter().any(|v| !allowed.contains(v))
        {
            return Err(StoreError::InvalidCommand(
                "invalid create-saga stage transition".into(),
            ));
        }
        let at = now()?;
        let placeholders = std::iter::repeat_n("?", from.len())
            .collect::<Vec<_>>()
            .join(",");
        let query = format!(
            "UPDATE sdd_create_sagas SET stage = ?, error_summary = ?, updated_at = ?
             WHERE repo_id = ? AND request_id = ? AND request_hash = ?
               AND stage IN ({placeholders})"
        );
        let mut statement = sqlx::query(&query)
            .bind(stage)
            .bind(error_summary)
            .bind(&at)
            .bind(repo_id)
            .bind(request_id)
            .bind(request_hash);
        for value in from {
            statement = statement.bind(value);
        }
        if statement.execute(&self.pool).await?.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(format!(
                "create saga is not in an expected stage for {stage}"
            )));
        }
        Ok(())
    }

    /// Mark active sagas as interrupted at boot and return their exact cleanup
    /// targets. Completed/failed/canceled records are immutable history.
    pub async fn sdd_claim_interrupted_creates(&self) -> Result<Vec<SddCreateSagaRecord>> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE sdd_create_sagas SET stage = 'recovery_required',
             error_summary = 'server stopped before create publication completed', updated_at = ?
             WHERE stage IN ('reserved', 'workspace_ready', 'authoring', 'publishing')",
        )
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        let rows =
            sqlx::query_as("SELECT * FROM sdd_create_sagas WHERE stage = 'recovery_required'")
                .fetch_all(&mut *tx)
                .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn sdd_run_create_saga(
        &self,
        spec_id: &str,
        request_id: &str,
    ) -> Result<Option<SddRunCreateSagaRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_run_create_sagas WHERE spec_id = ? AND request_id = ?",
        )
        .bind(spec_id)
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Reserve the exact filesystem/Git targets before creating a first run
    /// for a discovered specification. The spec CAS and immutable revision
    /// hash are checked in the same transaction as the reservation.
    pub async fn sdd_reserve_discovered_run(&self, input: NewSddRunCreateSaga<'_>) -> Result<()> {
        if input.request_id.is_empty()
            || input.request_hash.is_empty()
            || input.expected_spec_revision < 1
            || input.expected_spec_hash.len() != 64
        {
            return Err(StoreError::InvalidCommand(
                "discovered-run reservation is malformed".into(),
            ));
        }
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let spec: Option<(String, i64, i64, String)> = sqlx::query_as(
            "SELECT repo_id, current_revision, aggregate_revision, provider
             FROM sdd_specs WHERE spec_id = ?",
        )
        .bind(input.spec_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_revision, aggregate_revision, provider)) = spec else {
            return Err(StoreError::NotFound(input.spec_id.into()));
        };
        if repo_id != input.repo_id {
            return Err(StoreError::InvalidCommand(
                "specification belongs to another repository".into(),
            ));
        }
        if aggregate_revision != input.expected_aggregate_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_aggregate_revision,
                current: aggregate_revision,
            });
        }
        if spec_revision != input.expected_spec_revision || provider != "unassigned" {
            return Err(StoreError::InvalidCommand(
                "only an unchanged filesystem-discovered specification can create its first run"
                    .into(),
            ));
        }
        let stored_hash: Option<String> = sqlx::query_scalar(
            "SELECT content_hash FROM sdd_spec_revisions WHERE spec_id = ? AND revision = ?",
        )
        .bind(input.spec_id)
        .bind(input.expected_spec_revision)
        .fetch_optional(&mut *tx)
        .await?;
        if stored_hash.as_deref() != Some(input.expected_spec_hash) {
            return Err(StoreError::InvalidCommand(
                "discovered specification revision hash changed before reservation".into(),
            ));
        }
        let run_exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sdd_runs WHERE spec_id = ?")
            .bind(input.spec_id)
            .fetch_one(&mut *tx)
            .await?;
        if run_exists != 0 {
            return Err(StoreError::AlreadyExists(input.spec_id.into()));
        }
        let inserted = sqlx::query(
            "INSERT INTO sdd_run_create_sagas
             (spec_id, repo_id, request_id, request_hash, run_id, stage,
              expected_spec_revision, expected_spec_hash, expected_aggregate_revision,
              repository_path, authoritative_path, branch_name, attempt_id, attempt_path,
              created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'reserved', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(input.spec_id)
        .bind(input.repo_id)
        .bind(input.request_id)
        .bind(input.request_hash)
        .bind(input.run_id)
        .bind(input.expected_spec_revision)
        .bind(input.expected_spec_hash)
        .bind(input.expected_aggregate_revision)
        .bind(input.repository_path)
        .bind(input.authoritative_path)
        .bind(input.branch_name)
        .bind(input.attempt_id)
        .bind(input.attempt_path)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await;
        match inserted {
            Ok(_) => {
                tx.commit().await?;
                Ok(())
            }
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
                drop(tx);
                match self
                    .sdd_run_create_saga(input.spec_id, input.request_id)
                    .await?
                {
                    Some(existing) if existing.request_hash != input.request_hash => {
                        Err(StoreError::IdempotencyConflict(format!(
                            "spec:{}:create_run",
                            input.spec_id
                        )))
                    }
                    Some(existing) => Err(StoreError::AlreadyExists(existing.run_id)),
                    None => Err(StoreError::AlreadyExists(input.spec_id.into())),
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn sdd_update_run_create_stage(
        &self,
        spec_id: &str,
        request_id: &str,
        request_hash: &str,
        from: &[&str],
        stage: &str,
        error_summary: Option<&str>,
    ) -> Result<()> {
        let allowed = [
            "reserved",
            "workspace_ready",
            "publishing",
            "completed",
            "failed",
            "canceled",
            "recovery_required",
        ];
        if !allowed.contains(&stage) || from.is_empty() || from.iter().any(|v| !allowed.contains(v))
        {
            return Err(StoreError::InvalidCommand(
                "invalid discovered-run saga stage transition".into(),
            ));
        }
        let at = now()?;
        let placeholders = std::iter::repeat_n("?", from.len())
            .collect::<Vec<_>>()
            .join(",");
        let query = format!(
            "UPDATE sdd_run_create_sagas SET stage = ?, error_summary = ?, updated_at = ?
             WHERE spec_id = ? AND request_id = ? AND request_hash = ?
               AND stage IN ({placeholders})"
        );
        let mut statement = sqlx::query(&query)
            .bind(stage)
            .bind(error_summary)
            .bind(&at)
            .bind(spec_id)
            .bind(request_id)
            .bind(request_hash);
        for value in from {
            statement = statement.bind(value);
        }
        if statement.execute(&self.pool).await?.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(format!(
                "discovered-run saga is not in an expected stage for {stage}"
            )));
        }
        Ok(())
    }

    pub async fn sdd_claim_interrupted_run_creates(&self) -> Result<Vec<SddRunCreateSagaRecord>> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE sdd_run_create_sagas SET stage = 'recovery_required',
             error_summary = 'server stopped before discovered-run publication completed',
             updated_at = ? WHERE stage IN ('reserved', 'workspace_ready', 'publishing')",
        )
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        let rows =
            sqlx::query_as("SELECT * FROM sdd_run_create_sagas WHERE stage = 'recovery_required'")
                .fetch_all(&mut *tx)
                .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn sdd_publish_discovered_run(&self, input: NewSddDiscoveredRun<'_>) -> Result<()> {
        let valid_artifacts = !input.artifacts.is_empty()
            && input
                .artifacts
                .iter()
                .any(|artifact| artifact.kind == "specification")
            && input.artifacts.iter().all(|artifact| {
                matches!(
                    artifact.kind,
                    "specification" | "design" | "plan" | "decisions" | "review"
                ) && !artifact.relative_path.is_empty()
                    && artifact.content_hash.len() == 64
            });
        let unique_kinds: std::collections::HashSet<_> = input
            .artifacts
            .iter()
            .map(|artifact| artifact.kind)
            .collect();
        if !valid_artifacts
            || unique_kinds.len() != input.artifacts.len()
            || !matches!(input.profile, "standard" | "high_risk")
            || !matches!(input.control, "guarded" | "interactive" | "autopilot")
            || input.provider.is_empty()
        {
            return Err(StoreError::InvalidCommand(
                "discovered-run publication is malformed".into(),
            ));
        }
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let saga: Option<(String, String, i64, String, i64, String)> = sqlx::query_as(
            "SELECT repo_id, run_id, expected_spec_revision, expected_spec_hash,
                    expected_aggregate_revision, stage
             FROM sdd_run_create_sagas
             WHERE spec_id = ? AND request_id = ? AND request_hash = ?",
        )
        .bind(input.spec_id)
        .bind(input.request_id)
        .bind(input.request_hash)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, run_id, spec_revision, spec_hash, aggregate_revision, stage)) = saga
        else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if repo_id != input.repo_id
            || run_id != input.run_id
            || stage != "publishing"
            || spec_revision != input.expected_spec_revision
            || spec_hash != input.expected_spec_hash
            || aggregate_revision != input.expected_aggregate_revision
        {
            return Err(StoreError::InvalidCommand(
                "discovered-run reservation binding changed".into(),
            ));
        }
        let current: Option<(i64, i64, String)> = sqlx::query_as(
            "SELECT s.aggregate_revision, s.current_revision, r.content_hash
             FROM sdd_specs s JOIN sdd_spec_revisions r
               ON r.spec_id = s.spec_id AND r.revision = s.current_revision
             WHERE s.spec_id = ? AND s.repo_id = ?",
        )
        .bind(input.spec_id)
        .bind(input.repo_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((current_aggregate, current_revision, current_hash)) = current else {
            return Err(StoreError::NotFound(input.spec_id.into()));
        };
        if current_aggregate != input.expected_aggregate_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_aggregate_revision,
                current: current_aggregate,
            });
        }
        if current_revision != input.expected_spec_revision
            || current_hash != input.expected_spec_hash
        {
            return Err(StoreError::InvalidCommand(
                "discovered specification changed before run publication".into(),
            ));
        }
        let existing_runs: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sdd_runs WHERE spec_id = ?")
                .bind(input.spec_id)
                .fetch_one(&mut *tx)
                .await?;
        if existing_runs != 0 {
            return Err(StoreError::AlreadyExists(input.spec_id.into()));
        }
        sqlx::query(
            "UPDATE sdd_specs SET profile = ?, control = ?, provider = ?,
             aggregate_revision = aggregate_revision + 1, updated_at = ?
             WHERE spec_id = ? AND aggregate_revision = ?",
        )
        .bind(input.profile)
        .bind(input.control)
        .bind(input.provider)
        .bind(&at)
        .bind(input.spec_id)
        .bind(input.expected_aggregate_revision)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO sdd_runs
             (run_id, spec_id, repo_id, phase, status, aggregate_revision, base_ref,
              base_commit, branch_name, authoritative_path, workspace_fingerprint,
              policy_json, created_at, updated_at)
             VALUES (?, ?, ?, 'specification', 'waiting', 1, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(input.run_id)
        .bind(input.spec_id)
        .bind(input.repo_id)
        .bind(input.base_ref)
        .bind(input.base_commit)
        .bind(input.branch_name)
        .bind(input.authoritative_path)
        .bind(input.workspace_fingerprint)
        .bind(input.policy_json)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO sdd_attempts
             (attempt_id, run_id, task_id, spec_revision, provider, isolated_path, status,
              session_identity, started_at, finished_at)
             VALUES (?, ?, NULL, ?, ?, ?, 'succeeded', ?, ?, ?)",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .bind(input.expected_spec_revision)
        .bind(input.provider)
        .bind(input.attempt_path)
        .bind(input.attempt_session_identity)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        for artifact in input.artifacts {
            sqlx::query(
                "INSERT INTO sdd_artifact_revisions
                 (artifact_revision_id, run_id, spec_id, kind, revision, spec_revision,
                  relative_path, content_hash, submitted_by, created_at)
                 VALUES (?, ?, ?, ?, 1, ?, ?, ?, ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(input.run_id)
            .bind(input.spec_id)
            .bind(artifact.kind)
            .bind(input.expected_spec_revision)
            .bind(artifact.relative_path)
            .bind(artifact.content_hash)
            .bind(input.submitted_by)
            .bind(&at)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO sdd_approval_requests
             (approval_id, run_id, purpose, digest, requested_revision, requested_by,
              status, created_at)
             VALUES (?, ?, 'specification', ?, ?, ?, 'pending', ?)",
        )
        .bind(input.approval_id)
        .bind(input.run_id)
        .bind(input.approval_digest)
        .bind(input.expected_spec_revision)
        .bind(input.submitted_by)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        append_event(
            &mut tx,
            EventInsert {
                repo_id: input.repo_id,
                spec_id: Some(input.spec_id),
                run_id: Some(input.run_id),
                revision: 1,
                kind: "sdd.run.created_from_discovered_spec",
                payload_json: input.response_json,
                created_at: &at,
            },
        )
        .await?;
        sqlx::query(
            "INSERT INTO sdd_idempotency
             (scope, request_id, request_hash, expected_revision, response_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(format!("spec:{}:create_run", input.spec_id))
        .bind(input.request_id)
        .bind(input.request_hash)
        .bind(input.expected_aggregate_revision)
        .bind(input.response_json)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        let completed = sqlx::query(
            "UPDATE sdd_run_create_sagas SET stage = 'completed', response_json = ?,
             error_summary = NULL, updated_at = ?
             WHERE spec_id = ? AND request_id = ? AND request_hash = ? AND stage = 'publishing'",
        )
        .bind(input.response_json)
        .bind(&at)
        .bind(input.spec_id)
        .bind(input.request_id)
        .bind(input.request_hash)
        .execute(&mut *tx)
        .await?;
        if completed.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "discovered-run reservation changed during publication".into(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Reconcile runtime-only ownership after a process restart. No provider or
    /// patch is assumed to still be safe: active runs pause, attempts fail,
    /// leases expire, and an in-flight patch quarantines its run.
    pub async fn sdd_recover_interrupted_runs(&self) -> Result<usize> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let runs: Vec<(String, String, String, i64, String, String)> = sqlx::query_as(
            "SELECT repo_id, spec_id, run_id, aggregate_revision, phase, status
             FROM sdd_runs WHERE status IN ('running', 'pausing', 'canceling')",
        )
        .fetch_all(&mut *tx)
        .await?;
        for (repo_id, spec_id, run_id, revision, phase, old_status) in &runs {
            let next = revision + 1;
            let (status, blocker, kind) = if old_status == "canceling" {
                ("canceled", None, "sdd.run.canceled_during_recovery")
            } else {
                (
                    "paused",
                    Some("server restarted while work was active"),
                    "sdd.run.paused_during_recovery",
                )
            };
            sqlx::query(
                "UPDATE sdd_runs SET status = ?, blocker = ?, aggregate_revision = ?,
                 updated_at = ? WHERE run_id = ? AND aggregate_revision = ?",
            )
            .bind(status)
            .bind(blocker)
            .bind(next)
            .bind(&at)
            .bind(run_id)
            .bind(revision)
            .execute(&mut *tx)
            .await?;
            let payload = serde_json::json!({
                "runId": run_id,
                "revision": next,
                "phase": phase,
                "status": status,
                "blocker": blocker,
            })
            .to_string();
            append_event(
                &mut tx,
                EventInsert {
                    repo_id,
                    spec_id: Some(spec_id),
                    run_id: Some(run_id),
                    revision: next,
                    kind,
                    payload_json: &payload,
                    created_at: &at,
                },
            )
            .await?;
        }
        sqlx::query(
            "UPDATE sdd_remote_requests SET status = 'interrupted',
             error_code = 'desktop_restart', updated_at = ?
             WHERE status IN ('running', 'cancel_requested')",
        )
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_remote_runs SET active_request_id = NULL,
             status = CASE
               WHEN run_id IN (SELECT run_id FROM sdd_runs WHERE status = 'canceled')
                 THEN 'canceled'
               ELSE 'paused'
             END,
             last_error_code = 'desktop_restart', updated_at = ?
             WHERE active_request_id IS NOT NULL",
        )
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_attempts SET status = 'failed', finished_at = ?,
             error_summary = 'server restarted while attempt was active'
             WHERE status IN ('running', 'pausing', 'canceling')",
        )
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE sdd_capability_grants SET revoked_at = ? WHERE revoked_at IS NULL")
            .bind(&at)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sdd_leases")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE sdd_patch_ledger SET status = 'quarantined',
             error = 'server restarted during patch publication', updated_at = ?
             WHERE status = 'pending'",
        )
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_runs SET quarantined = 1,
             blocker = 'patch recovery requires operator attention', updated_at = ?
             WHERE run_id IN (SELECT run_id FROM sdd_patch_ledger WHERE status = 'quarantined')",
        )
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(runs.len())
    }

    /// Realtime outbox delivery means making the durable cursor visible to the
    /// cursor/WS readers; those readers consume `sdd_events`, so observation is
    /// complete once this worker has acknowledged the committed row.
    pub async fn sdd_ack_realtime_outbox(&self, limit: i64) -> Result<u64> {
        let at = now()?;
        Ok(sqlx::query(
            "UPDATE sdd_outbox SET delivered_at = ?, attempts = attempts + 1,
             last_error = NULL
             WHERE outbox_id IN (
               SELECT outbox_id FROM sdd_outbox
               WHERE destination = 'realtime' AND delivered_at IS NULL
                 AND available_at <= ?
               ORDER BY available_at, outbox_id LIMIT ?
             )",
        )
        .bind(&at)
        .bind(&at)
        .bind(limit.clamp(1, 1000))
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    pub async fn sdd_idempotent_response(
        &self,
        scope: &str,
        request_id: &str,
        request_hash: &str,
    ) -> Result<Option<Value>> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT request_hash, response_json FROM sdd_idempotency
             WHERE scope = ? AND request_id = ?",
        )
        .bind(scope)
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((stored_hash, raw)) = row else {
            return Ok(None);
        };
        if stored_hash != request_hash {
            return Err(StoreError::IdempotencyConflict(scope.to_owned()));
        }
        Ok(Some(serde_json::from_str(&raw)?))
    }

    pub async fn sdd_create_aggregate(&self, input: NewSddAggregate<'_>) -> Result<()> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let saga: Option<(String, String, String, String)> = sqlx::query_as(
            "SELECT request_hash, spec_id, run_id, stage FROM sdd_create_sagas
             WHERE repo_id = ? AND request_id = ?",
        )
        .bind(input.repo_id)
        .bind(input.request_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((request_hash, saga_spec_id, saga_run_id, stage)) = saga else {
            return Err(StoreError::InvalidCommand(
                "create must have a durable reservation".into(),
            ));
        };
        if request_hash != input.request_hash
            || saga_spec_id != input.spec_id
            || saga_run_id != input.run_id
        {
            return Err(StoreError::IdempotencyConflict(format!(
                "repo:{}:create_spec",
                input.repo_id
            )));
        }
        if stage != "publishing" {
            return Err(StoreError::InvalidCommand(format!(
                "create cannot publish from saga stage {stage}"
            )));
        }
        sqlx::query(
            "INSERT INTO sdd_specs
             (spec_id, spec_ulid, repo_id, title, slug, profile, control, provider,
              source_ref_json, current_revision, aggregate_revision, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(input.spec_id)
        .bind(input.spec_ulid)
        .bind(input.repo_id)
        .bind(input.title)
        .bind(input.slug)
        .bind(input.profile)
        .bind(input.control)
        .bind(input.provider)
        .bind(input.source_ref_json)
        .bind(input.spec_revision)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        if let Some(link) = input.external_link.as_ref() {
            sqlx::query(
                "INSERT INTO sdd_external_links
                 (link_id, spec_id, provider, connection_id, site_id, external_id, key, url,
                  source_revision, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(input.spec_id)
            .bind(link.provider)
            .bind(link.connection_id)
            .bind(link.site_id)
            .bind(link.external_id)
            .bind(link.key)
            .bind(link.url)
            .bind(link.source_revision)
            .bind(&at)
            .execute(&mut *tx)
            .await?;
        }
        if let Some(import) = input.import_job.as_ref() {
            // A normalized source hash is deterministic. Reusing the same
            // immutable source snapshot for another spec is a no-op; each spec
            // still carries its own sanitized source_ref_json binding.
            sqlx::query(
                "INSERT INTO sdd_import_jobs
                 (import_id, repo_id, source_kind, source_hash, preview_json, disposition,
                  created_at, committed_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(repo_id, source_kind, source_hash) DO NOTHING",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(input.repo_id)
            .bind(import.source_kind)
            .bind(import.source_hash)
            .bind(import.preview_json)
            .bind(import.disposition)
            .bind(&at)
            .bind(&at)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO sdd_spec_revisions
             (spec_id, revision, content_hash, content, submitted_by, imported_external, created_at)
             VALUES (?, 1, ?, ?, 'agentum:initial-draft', 0, ?)",
        )
        .bind(input.spec_id)
        .bind(input.initial_spec_hash)
        .bind(input.initial_spec_content)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        if input.spec_revision > 1 {
            sqlx::query(
                "INSERT INTO sdd_spec_revisions
                 (spec_id, revision, content_hash, content, submitted_by, imported_external, created_at)
                 VALUES (?, ?, ?, ?, ?, 0, ?)",
            )
            .bind(input.spec_id)
            .bind(input.spec_revision)
            .bind(input.spec_hash)
            .bind(input.spec_content)
            .bind(input.submitted_by)
            .bind(&at)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO sdd_runs
             (run_id, spec_id, repo_id, phase, status, aggregate_revision, base_ref,
              base_commit, branch_name, authoritative_path, workspace_fingerprint,
              policy_json, created_at, updated_at)
             VALUES (?, ?, ?, 'specification', 'waiting', 1, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(input.run_id)
        .bind(input.spec_id)
        .bind(input.repo_id)
        .bind(input.base_ref)
        .bind(input.base_commit)
        .bind(input.branch_name)
        .bind(input.authoritative_path)
        .bind(input.workspace_fingerprint)
        .bind(input.policy_json)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO sdd_attempts
             (attempt_id, run_id, task_id, spec_revision, provider, isolated_path, status,
              session_identity, started_at, finished_at)
             VALUES (?, ?, NULL, ?, ?, ?, 'succeeded', ?, ?, ?)",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .bind(input.spec_revision)
        .bind(input.provider)
        .bind(input.attempt_path)
        .bind(input.attempt_session_identity)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        let specification_artifact_revision_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sdd_artifact_revisions
             (artifact_revision_id, run_id, spec_id, kind, revision, spec_revision, relative_path,
              content_hash, submitted_by, created_at)
             VALUES (?, ?, ?, 'specification', 1, ?, ?, ?, ?, ?)",
        )
        .bind(&specification_artifact_revision_id)
        .bind(input.run_id)
        .bind(input.spec_id)
        .bind(input.spec_revision)
        .bind(format!(".agentum/specs/{}/spec.md", input.slug))
        .bind(input.spec_hash)
        .bind(input.submitted_by)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        if let Some(projection) = input.remote_projection.as_ref() {
            insert_initial_projection(
                &mut tx,
                input.run_id,
                &specification_artifact_revision_id,
                projection,
                &at,
            )
            .await?;
        }
        sqlx::query(
            "INSERT INTO sdd_approval_requests
             (approval_id, run_id, purpose, digest, requested_revision, requested_by,
              status, created_at)
             VALUES (?, ?, 'specification', ?, ?, ?, 'pending', ?)",
        )
        .bind(input.approval_id)
        .bind(input.run_id)
        .bind(input.approval_digest)
        .bind(input.spec_revision)
        .bind(input.submitted_by)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        append_event(
            &mut tx,
            EventInsert {
                repo_id: input.repo_id,
                spec_id: Some(input.spec_id),
                run_id: Some(input.run_id),
                revision: 1,
                kind: "sdd.run.waiting_for_spec_approval",
                payload_json: input.response_json,
                created_at: &at,
            },
        )
        .await?;
        let completed = sqlx::query(
            "UPDATE sdd_create_sagas SET stage = 'completed', response_json = ?,
             error_summary = NULL, updated_at = ?
             WHERE repo_id = ? AND request_id = ? AND request_hash = ?
               AND stage = 'publishing'",
        )
        .bind(input.response_json)
        .bind(&at)
        .bind(input.repo_id)
        .bind(input.request_id)
        .bind(input.request_hash)
        .execute(&mut *tx)
        .await?;
        if completed.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "create reservation changed during publication".into(),
            ));
        }
        if input.remote_projection.is_some() {
            let intent_completed = sqlx::query(
                "UPDATE sdd_remote_create_intents SET status = 'completed', updated_at = ?
                 WHERE repo_id = ? AND request_id = ? AND status = 'authored'",
            )
            .bind(&at)
            .bind(input.repo_id)
            .bind(input.request_id)
            .execute(&mut *tx)
            .await?;
            if intent_completed.rows_affected() != 1 {
                return Err(StoreError::InvalidCommand(
                    "remote create intent changed during aggregate publication".into(),
                ));
            }
        }
        sqlx::query(
            "INSERT INTO sdd_idempotency
             (scope, request_id, request_hash, expected_revision, response_json, created_at)
             VALUES (?, ?, ?, 0, ?, ?)",
        )
        .bind(format!("repo:{}:create_spec", input.repo_id))
        .bind(input.request_id)
        .bind(input.request_hash)
        .bind(input.response_json)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Reconcile a fully validated filesystem scan in one transaction. The
    /// server performs no-follow artifact validation before calling this
    /// method; this layer enforces manifest ownership, immutable revision
    /// hashes, cross-repository identity, and all-or-nothing publication.
    pub async fn sdd_reconcile_discovered_specs(
        &self,
        input: ReconcileDiscoveredSpecs<'_>,
    ) -> Result<ReconcileDiscoveredResult> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let mut batch_ids = std::collections::HashSet::new();
        if input.repo_id.is_empty()
            || input.artifact_set_id.is_empty()
            || input.specs.iter().any(|spec| {
                spec.spec_id.is_empty()
                    || spec.spec_ulid.is_empty()
                    || spec.title.trim().is_empty()
                    || spec.slug.is_empty()
                    || spec.revision < 1
                    || spec.content_hash.len() != 64
                    || !batch_ids.insert(spec.spec_id)
            })
        {
            return Err(StoreError::InvalidCommand(
                "discovered artifact batch is malformed".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO sdd_repo_artifact_sets (repo_id, artifact_set_id, created_at)
             VALUES (?, ?, ?) ON CONFLICT(repo_id) DO NOTHING",
        )
        .bind(input.repo_id)
        .bind(input.artifact_set_id)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        let registered: String = sqlx::query_scalar(
            "SELECT artifact_set_id FROM sdd_repo_artifact_sets WHERE repo_id = ?",
        )
        .bind(input.repo_id)
        .fetch_one(&mut *tx)
        .await?;
        if registered != input.artifact_set_id {
            return Err(StoreError::ArtifactSetConflict(format!(
                "registered {registered}, repository contains {}",
                input.artifact_set_id
            )));
        }

        let mut result = ReconcileDiscoveredResult {
            inserted: 0,
            revised: 0,
            unchanged: 0,
        };
        for spec in input.specs {
            let existing: Option<(String, i64)> =
                sqlx::query_as("SELECT repo_id, current_revision FROM sdd_specs WHERE spec_id = ?")
                    .bind(spec.spec_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            let existing_revision: Option<String> = sqlx::query_scalar(
                "SELECT content_hash FROM sdd_spec_revisions
                 WHERE spec_id = ? AND revision = ?",
            )
            .bind(spec.spec_id)
            .bind(spec.revision)
            .fetch_optional(&mut *tx)
            .await?;

            let event_kind = match existing {
                None => {
                    if existing_revision.is_some() {
                        return Err(StoreError::InvalidCommand(
                            "orphan discovered specification revision exists".into(),
                        ));
                    }
                    sqlx::query(
                        "INSERT INTO sdd_specs
                         (spec_id, spec_ulid, repo_id, title, slug, profile, control, provider,
                          source_ref_json, current_revision, aggregate_revision, created_at, updated_at)
                         VALUES (?, ?, ?, ?, ?, 'standard', 'guarded', 'unassigned', ?, ?, 1, ?, ?)",
                    )
                    .bind(spec.spec_id)
                    .bind(spec.spec_ulid)
                    .bind(input.repo_id)
                    .bind(spec.title)
                    .bind(spec.slug)
                    .bind(spec.source_ref_json)
                    .bind(spec.revision)
                    .bind(&at)
                    .bind(&at)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        "INSERT INTO sdd_spec_revisions
                         (spec_id, revision, content_hash, content, submitted_by,
                          imported_external, created_at)
                         VALUES (?, ?, ?, ?, 'filesystem:discovery', 1, ?)",
                    )
                    .bind(spec.spec_id)
                    .bind(spec.revision)
                    .bind(spec.content_hash)
                    .bind(spec.content)
                    .bind(&at)
                    .execute(&mut *tx)
                    .await?;
                    result.inserted += 1;
                    Some("sdd.spec.discovered")
                }
                Some((repo_id, current_revision)) => {
                    if repo_id != input.repo_id {
                        return Err(StoreError::InvalidCommand(format!(
                            "specification {} belongs to another repository",
                            spec.spec_id
                        )));
                    }
                    if let Some(stored_hash) = existing_revision {
                        if stored_hash != spec.content_hash {
                            return Err(StoreError::InvalidCommand(format!(
                                "immutable specification revision changed: {} revision {}",
                                spec.spec_id, spec.revision
                            )));
                        }
                        result.unchanged += 1;
                        None
                    } else {
                        if spec.revision <= current_revision {
                            return Err(StoreError::InvalidCommand(format!(
                                "missing immutable historical revision for {}",
                                spec.spec_id
                            )));
                        }
                        if spec.revision != current_revision + 1 {
                            return Err(StoreError::InvalidCommand(format!(
                                "discovered revision for {} must be {}",
                                spec.spec_id,
                                current_revision + 1
                            )));
                        }
                        let run_count: i64 =
                            sqlx::query_scalar("SELECT COUNT(*) FROM sdd_runs WHERE spec_id = ?")
                                .bind(spec.spec_id)
                                .fetch_one(&mut *tx)
                                .await?;
                        if run_count != 0 {
                            return Err(StoreError::InvalidCommand(format!(
                                "filesystem revision for {} has a durable run; reconcile its authoritative worktree first",
                                spec.spec_id
                            )));
                        }
                        sqlx::query(
                            "INSERT INTO sdd_spec_revisions
                             (spec_id, revision, content_hash, content, submitted_by,
                              imported_external, created_at)
                             VALUES (?, ?, ?, ?, 'filesystem:discovery', 1, ?)",
                        )
                        .bind(spec.spec_id)
                        .bind(spec.revision)
                        .bind(spec.content_hash)
                        .bind(spec.content)
                        .bind(&at)
                        .execute(&mut *tx)
                        .await?;
                        sqlx::query(
                            "UPDATE sdd_specs SET title = ?, slug = ?, source_ref_json = ?,
                             current_revision = ?, aggregate_revision = aggregate_revision + 1,
                             updated_at = ? WHERE spec_id = ?",
                        )
                        .bind(spec.title)
                        .bind(spec.slug)
                        .bind(spec.source_ref_json)
                        .bind(spec.revision)
                        .bind(&at)
                        .bind(spec.spec_id)
                        .execute(&mut *tx)
                        .await?;
                        result.revised += 1;
                        Some("sdd.spec.discovered_revision")
                    }
                }
            };
            if let Some(event_kind) = event_kind {
                let payload = serde_json::json!({
                    "specId": spec.spec_id,
                    "specRevision": spec.revision,
                    "contentHash": spec.content_hash,
                    "source": "filesystem:discovery"
                })
                .to_string();
                append_event(
                    &mut tx,
                    EventInsert {
                        repo_id: input.repo_id,
                        spec_id: Some(spec.spec_id),
                        run_id: None,
                        revision: spec.revision,
                        kind: event_kind,
                        payload_json: &payload,
                        created_at: &at,
                    },
                )
                .await?;
            }
        }
        tx.commit().await?;
        Ok(result)
    }

    pub async fn sdd_list_specs(&self, repo_id: &str) -> Result<Vec<SddSpecRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_specs WHERE repo_id = ? ORDER BY updated_at DESC, spec_id",
        )
        .bind(repo_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sdd_get_spec(&self, spec_id: &str) -> Result<Option<SddSpecRecord>> {
        Ok(sqlx::query_as("SELECT * FROM sdd_specs WHERE spec_id = ?")
            .bind(spec_id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn sdd_spec_revision_content(
        &self,
        spec_id: &str,
        revision: i64,
    ) -> Result<Option<(String, String)>> {
        Ok(sqlx::query_as(
            "SELECT content_hash, content FROM sdd_spec_revisions
             WHERE spec_id = ? AND revision = ?",
        )
        .bind(spec_id)
        .bind(revision)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn sdd_external_links_for_spec(
        &self,
        spec_id: &str,
    ) -> Result<Vec<SddExternalLinkRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_external_links WHERE spec_id = ? ORDER BY created_at, link_id",
        )
        .bind(spec_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sdd_import_job(
        &self,
        repo_id: &str,
        source_kind: &str,
        source_hash: &str,
    ) -> Result<Option<SddImportJobRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_import_jobs
             WHERE repo_id = ? AND source_kind = ? AND source_hash = ?",
        )
        .bind(repo_id)
        .bind(source_kind)
        .bind(source_hash)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn sdd_get_run(&self, run_id: &str) -> Result<Option<SddRunRecord>> {
        Ok(sqlx::query_as("SELECT * FROM sdd_runs WHERE run_id = ?")
            .bind(run_id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn sdd_runs_for_repo(&self, repo_id: &str) -> Result<Vec<SddRunRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_runs WHERE repo_id = ? ORDER BY updated_at DESC, run_id",
        )
        .bind(repo_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sdd_attempt_for_run(
        &self,
        run_id: &str,
        attempt_id: &str,
    ) -> Result<Option<(String, String)>> {
        Ok(sqlx::query_as(
            "SELECT provider, status FROM sdd_attempts WHERE run_id = ? AND attempt_id = ?",
        )
        .bind(run_id)
        .bind(attempt_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn sdd_latest_run_for_spec(&self, spec_id: &str) -> Result<Option<SddRunRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_runs WHERE spec_id = ? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(spec_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn sdd_artifacts(&self, run_id: &str) -> Result<Vec<SddArtifactRecord>> {
        Ok(sqlx::query_as(
            "SELECT a.* FROM sdd_artifact_revisions a
             JOIN sdd_specs s ON s.spec_id = a.spec_id
             JOIN (SELECT kind, MAX(revision) revision FROM sdd_artifact_revisions
                   WHERE run_id = ? GROUP BY kind) latest
               ON latest.kind = a.kind AND latest.revision = a.revision
             WHERE a.run_id = ?
               AND (a.kind = 'specification' OR a.spec_revision = s.current_revision)
             ORDER BY a.kind",
        )
        .bind(run_id)
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sdd_pending_approval(&self, run_id: &str) -> Result<Option<SddApprovalRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_approval_requests
             WHERE run_id = ? AND status = 'pending' ORDER BY created_at DESC LIMIT 1",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// True only when the run's current immutable specification revision has a
    /// recorded human approval.  Older approvals never authorize a new edit.
    pub async fn sdd_current_spec_is_approved(&self, run_id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(
                SELECT 1
                FROM sdd_runs r
                JOIN sdd_specs s ON s.spec_id = r.spec_id
                JOIN sdd_approval_requests a ON a.run_id = r.run_id
                WHERE r.run_id = ?
                  AND a.purpose = 'specification'
                  AND a.status = 'approved'
                  AND a.requested_revision = s.current_revision
             )",
        )
        .bind(run_id)
        .fetch_one(&self.pool)
        .await?
            != 0)
    }

    pub async fn sdd_snapshot(&self, run_id: &str) -> Result<Option<SddSnapshot>> {
        let Some(run) = self.sdd_get_run(run_id).await? else {
            return Ok(None);
        };
        let spec = self
            .sdd_get_spec(&run.spec_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(run.spec_id.clone()))?;
        let artifacts = self.sdd_artifacts(run_id).await?;
        let approval = self.sdd_pending_approval(run_id).await?;
        let tasks = self.sdd_tasks(run_id).await?;
        let attempts = self.sdd_attempts(run_id).await?;
        let verification = self.sdd_verification_results(run_id).await?;
        Ok(Some(SddSnapshot {
            spec,
            run,
            artifacts,
            approval,
            tasks,
            attempts,
            verification,
        }))
    }

    pub async fn sdd_events_after(
        &self,
        run_id: &str,
        after: i64,
        limit: i64,
    ) -> Result<Vec<SddEventRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_events WHERE run_id = ? AND cursor > ? ORDER BY cursor LIMIT ?",
        )
        .bind(run_id)
        .bind(after)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sdd_repo_events_after(
        &self,
        repo_id: &str,
        after: i64,
        limit: i64,
    ) -> Result<Vec<SddEventRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_events WHERE repo_id = ? AND cursor > ? ORDER BY cursor LIMIT ?",
        )
        .bind(repo_id)
        .bind(after)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await?)
    }

    /// CAS a lifecycle command and its event/outbox/idempotency record.
    pub async fn sdd_transition(&self, input: TransitionMutation<'_>) -> Result<i64> {
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
        if input.status == "queued" {
            let active_remote: i64 = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sdd_remote_runs
                 WHERE run_id = ? AND active_request_id IS NOT NULL)",
            )
            .bind(input.run_id)
            .fetch_one(&mut *tx)
            .await?;
            if active_remote != 0 {
                return Err(StoreError::InvalidCommand(
                    "remote cancellation is still being acknowledged".into(),
                ));
            }
        }
        let next = current + 1;
        let updated = sqlx::query(
            "UPDATE sdd_runs SET phase = ?, status = ?, blocker = ?, aggregate_revision = ?,
             updated_at = ? WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(input.phase)
        .bind(input.status)
        .bind(input.blocker)
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
        if matches!(input.status, "paused" | "canceled") {
            sqlx::query(
                "UPDATE sdd_remote_requests SET status = 'cancel_requested', updated_at = ?
                 WHERE request_id = (SELECT active_request_id FROM sdd_remote_runs WHERE run_id = ?)
                   AND status = 'running'",
            )
            .bind(&at)
            .bind(input.run_id)
            .execute(&mut *tx)
            .await?;
        }
        if matches!(
            input.status,
            "waiting" | "queued" | "paused" | "blocked" | "canceled" | "failed" | "succeeded"
        ) {
            let remote_exists: i64 =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sdd_remote_runs WHERE run_id = ?)")
                    .bind(input.run_id)
                    .fetch_one(&mut *tx)
                    .await?;
            let projection_updated = sqlx::query(
                "UPDATE sdd_remote_runs SET status = ?, updated_at = ? WHERE run_id = ?",
            )
            .bind(input.status)
            .bind(&at)
            .bind(input.run_id)
            .execute(&mut *tx)
            .await?;
            if remote_exists != 0 && projection_updated.rows_affected() != 1 {
                return Err(StoreError::InvalidCommand(
                    "remote projection changed during lifecycle command".into(),
                ));
            }
        }
        match input.status {
            "paused" => {
                sqlx::query(
                    "UPDATE sdd_attempts SET status = 'paused', finished_at = ?,
                     error_summary = 'paused by lifecycle command'
                     WHERE run_id = ? AND status IN ('queued', 'running', 'pausing')",
                )
                .bind(&at)
                .bind(input.run_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE sdd_tasks SET runtime_status = 'paused',
                     aggregate_revision = aggregate_revision + 1, updated_at = ?
                     WHERE run_id = ? AND runtime_status IN ('queued', 'running', 'pausing')",
                )
                .bind(&at)
                .bind(input.run_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE sdd_capability_grants SET revoked_at = ?
                     WHERE run_id = ? AND revoked_at IS NULL",
                )
                .bind(&at)
                .bind(input.run_id)
                .execute(&mut *tx)
                .await?;
            }
            "canceled" => {
                sqlx::query(
                    "UPDATE sdd_attempts SET status = 'canceled', finished_at = ?,
                     error_summary = 'canceled by lifecycle command'
                     WHERE run_id = ? AND status IN
                       ('idle', 'queued', 'running', 'waiting', 'retry_scheduled',
                        'pausing', 'paused', 'blocked', 'canceling')",
                )
                .bind(&at)
                .bind(input.run_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE sdd_tasks SET runtime_status = 'canceled',
                     aggregate_revision = aggregate_revision + 1, updated_at = ?
                     WHERE run_id = ? AND runtime_status NOT IN ('canceled', 'succeeded')",
                )
                .bind(&at)
                .bind(input.run_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE sdd_capability_grants SET revoked_at = ?
                     WHERE run_id = ? AND revoked_at IS NULL",
                )
                .bind(&at)
                .bind(input.run_id)
                .execute(&mut *tx)
                .await?;
            }
            "queued" => {
                // A paused task is retried in a fresh disposable attempt; the
                // old attempt remains immutable recovery/audit evidence.
                sqlx::query(
                    "UPDATE sdd_tasks SET runtime_status = 'retry_scheduled',
                     aggregate_revision = aggregate_revision + 1, updated_at = ?
                     WHERE run_id = ? AND runtime_status IN ('paused', 'failed')",
                )
                .bind(&at)
                .bind(input.run_id)
                .execute(&mut *tx)
                .await?;
            }
            _ => {}
        }
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
        sqlx::query(
            "INSERT INTO sdd_idempotency
             (scope, request_id, request_hash, expected_revision, response_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(format!("run:{}", input.run_id))
        .bind(input.request_id)
        .bind(input.request_hash)
        .bind(input.expected_revision)
        .bind(input.response_json)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    /// Record a validated artifact after the filesystem CAS has published it.
    /// The caller compensates the file on transaction failure; all database
    /// state, invalidation, approval, event, outbox, and request replay are one
    /// SQLite commit.
    pub async fn sdd_submit_artifact(&self, input: ArtifactMutation<'_>) -> Result<i64> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let run: Option<(String, String, String, String, i64)> = sqlx::query_as(
            "SELECT repo_id, spec_id, phase, status, aggregate_revision
             FROM sdd_runs WHERE run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, run_phase, run_status, current)) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        let expected_phase = match input.kind {
            "specification" => "specification",
            "design" | "decisions" => "design",
            "plan" => "planning",
            "review" => "review",
            other => {
                return Err(StoreError::InvalidCommand(format!(
                    "unknown artifact kind: {other}"
                )));
            }
        };
        if run_phase != expected_phase || run_status != "running" {
            return Err(StoreError::InvalidCommand(format!(
                "{} cannot be submitted from {run_phase}/{run_status}",
                input.kind
            )));
        }
        let attempt: Option<(String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT provider, status, session_identity, task_id FROM sdd_attempts
             WHERE attempt_id = ? AND run_id = ?",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((attempt_provider, attempt_status, session_identity, task_id)) = attempt else {
            return Err(StoreError::NotFound(input.attempt_id.into()));
        };
        if attempt_status != "running" || task_id.is_some() {
            return Err(StoreError::InvalidCommand(
                "artifact attempt is not an active phase attempt".into(),
            ));
        }
        let expected_author = format!("agent:{attempt_provider}:{}", input.attempt_id);
        if input.submitted_by != expected_author {
            return Err(StoreError::InvalidCommand(
                "artifact submitter does not match the active attempt".into(),
            ));
        }
        let current_spec_revision: i64 =
            sqlx::query_scalar("SELECT current_revision FROM sdd_specs WHERE spec_id = ?")
                .bind(&spec_id)
                .fetch_one(&mut *tx)
                .await?;
        if input.kind != "specification" {
            let approved: i64 = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sdd_approval_requests
                 WHERE run_id = ? AND purpose = 'specification' AND status = 'approved'
                   AND requested_revision = ?)",
            )
            .bind(input.run_id)
            .bind(current_spec_revision)
            .fetch_one(&mut *tx)
            .await?;
            if approved == 0 {
                return Err(StoreError::ApprovalInvalid);
            }
        }
        let valid_next = match input.kind {
            "specification" => {
                input.next_phase == "specification" && input.next_status == "waiting"
            }
            "design" => {
                (input.next_phase == "planning" && matches!(input.next_status, "queued" | "paused"))
                    || (input.next_phase == "design" && input.next_status == "waiting")
            }
            "plan" => {
                (input.next_phase == "implementation"
                    && matches!(input.next_status, "queued" | "paused"))
                    || (input.next_phase == "planning" && input.next_status == "waiting")
            }
            "decisions" => input.next_phase == "design" && input.next_status == "running",
            "review" => input.next_phase == "ready" && input.next_status == "succeeded",
            _ => false,
        };
        if !valid_next {
            return Err(StoreError::InvalidCommand(
                "artifact requested an invalid lifecycle transition".into(),
            ));
        }
        if input.kind == "review" {
            let unfinished_tasks: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sdd_tasks
                 WHERE run_id = ? AND spec_revision = ? AND runtime_status != 'succeeded'",
            )
            .bind(input.run_id)
            .bind(current_spec_revision)
            .fetch_one(&mut *tx)
            .await?;
            let verification_failed: i64 = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sdd_verification_results
                 WHERE run_id = ? AND spec_revision = ? AND status != 'succeeded')",
            )
            .bind(input.run_id)
            .bind(current_spec_revision)
            .fetch_one(&mut *tx)
            .await?;
            let verification_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sdd_verification_results
                 WHERE run_id = ? AND spec_revision = ?",
            )
            .bind(input.run_id)
            .bind(current_spec_revision)
            .fetch_one(&mut *tx)
            .await?;
            let reused_implementation_session: i64 = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sdd_attempts
                 WHERE run_id = ? AND spec_revision = ? AND task_id IS NOT NULL
                   AND session_identity = ?)",
            )
            .bind(input.run_id)
            .bind(current_spec_revision)
            .bind(&session_identity)
            .fetch_one(&mut *tx)
            .await?;
            if unfinished_tasks != 0 || verification_failed != 0 || verification_count == 0 {
                return Err(StoreError::InvalidCommand(
                    "Ready requires completed tasks and successful verification".into(),
                ));
            }
            if reused_implementation_session != 0 {
                return Err(StoreError::InvalidCommand(
                    "review must use an independent provider session".into(),
                ));
            }
            let evidence_hashes: Vec<String> = sqlx::query_scalar(
                "SELECT e.manifest_sha256 FROM sdd_browser_evidence e
                 WHERE e.run_id = ? AND e.spec_revision = ? ORDER BY e.manifest_sha256",
            )
            .bind(input.run_id)
            .bind(current_spec_revision)
            .fetch_all(&mut *tx)
            .await?;
            let evidence_json = serde_json::to_string(&evidence_hashes)?;
            let evidence_digest = format!("{:x}", Sha256::digest(evidence_json.as_bytes()));
            if input.evidence_manifest_hashes_json != Some(evidence_json.as_str())
                || input.evidence_digest != Some(evidence_digest.as_str())
            {
                return Err(StoreError::InvalidCommand(
                    "review is not bound to the current immutable browser evidence set".into(),
                ));
            }
        } else if input.evidence_digest.is_some() || input.evidence_manifest_hashes_json.is_some() {
            return Err(StoreError::InvalidCommand(
                "only review artifacts may bind runtime browser evidence".into(),
            ));
        }
        let artifact_revision: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(revision), 0) + 1 FROM sdd_artifact_revisions
             WHERE run_id = ? AND kind = ?",
        )
        .bind(input.run_id)
        .bind(input.kind)
        .fetch_one(&mut *tx)
        .await?;
        let next = current + 1;
        sqlx::query(
            "INSERT INTO sdd_artifact_revisions
             (artifact_revision_id, run_id, spec_id, kind, revision, spec_revision, relative_path,
              content_hash, submitted_by, evidence_digest, evidence_manifest_hashes_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(input.run_id)
        .bind(&spec_id)
        .bind(input.kind)
        .bind(artifact_revision)
        .bind(if input.kind == "specification" {
            current_spec_revision + 1
        } else {
            current_spec_revision
        })
        .bind(input.relative_path)
        .bind(input.content_hash)
        .bind(input.submitted_by)
        .bind(input.evidence_digest)
        .bind(input.evidence_manifest_hashes_json)
        .bind(&at)
        .execute(&mut *tx)
        .await?;

        let event_kind = if input.kind == "specification" {
            let content = input.content.ok_or_else(|| {
                StoreError::InvalidCommand("specification content is required".into())
            })?;
            let spec_revision = current_spec_revision + 1;
            sqlx::query(
                "INSERT INTO sdd_spec_revisions
                 (spec_id, revision, content_hash, content, submitted_by, imported_external, created_at)
                 VALUES (?, ?, ?, ?, ?, 0, ?)",
            )
            .bind(&spec_id)
            .bind(spec_revision)
            .bind(input.content_hash)
            .bind(content)
            .bind(input.submitted_by)
            .bind(&at)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE sdd_specs SET current_revision = ?, aggregate_revision = aggregate_revision + 1,
                 updated_at = ? WHERE spec_id = ?",
            )
            .bind(spec_revision)
            .bind(&at)
            .bind(&spec_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE sdd_approval_requests SET status = 'invalidated', invalidated_at = ?
                 WHERE run_id = ? AND status IN ('pending', 'approved')",
            )
            .bind(&at)
            .bind(input.run_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE sdd_delivery_previews SET status = 'invalidated'
                 WHERE run_id = ? AND status = 'pending'",
            )
            .bind(input.run_id)
            .execute(&mut *tx)
            .await?;
            "sdd.artifact.specification_submitted"
        } else {
            if input.kind == "plan" {
                let content = input
                    .content
                    .ok_or_else(|| StoreError::InvalidCommand("plan content is required".into()))?;
                let plan: Value = serde_json::from_str(content)?;
                let tasks = plan
                    .get("tasks")
                    .and_then(Value::as_array)
                    .ok_or_else(|| StoreError::InvalidCommand("plan tasks are required".into()))?;
                if tasks.is_empty() {
                    return Err(StoreError::InvalidCommand(
                        "plan must contain at least one task".into(),
                    ));
                }
                for task in tasks {
                    let task_id = task.get("id").and_then(Value::as_str).ok_or_else(|| {
                        StoreError::InvalidCommand("plan task id is required".into())
                    })?;
                    sqlx::query(
                        "INSERT INTO sdd_tasks
                         (run_id, task_id, spec_revision, intent_json, runtime_status,
                          aggregate_revision, created_at, updated_at)
                         VALUES (?, ?, ?, ?, 'queued', 1, ?, ?)",
                    )
                    .bind(input.run_id)
                    .bind(task_id)
                    .bind(current_spec_revision)
                    .bind(task.to_string())
                    .bind(&at)
                    .bind(&at)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            match input.kind {
                "design" => "sdd.artifact.design_submitted",
                "plan" => "sdd.artifact.plan_submitted",
                "review" => "sdd.artifact.review_submitted",
                "decisions" => "sdd.artifact.decisions_submitted",
                _ => unreachable!("kind validated above"),
            }
        };
        match (
            input.approval_id,
            input.approval_digest,
            input.approval_purpose,
        ) {
            (Some(approval_id), Some(digest), Some(purpose)) => {
                if !matches!(purpose, "specification" | "design" | "planning")
                    || input.next_status != "waiting"
                {
                    return Err(StoreError::InvalidCommand(
                        "artifact approval purpose or state is invalid".into(),
                    ));
                }
                sqlx::query(
                    "INSERT INTO sdd_approval_requests
                     (approval_id, run_id, purpose, digest, requested_revision, requested_by,
                      status, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, 'pending', ?)",
                )
                .bind(approval_id)
                .bind(input.run_id)
                .bind(purpose)
                .bind(digest)
                .bind(if input.kind == "specification" {
                    current_spec_revision + 1
                } else {
                    current_spec_revision
                })
                .bind(input.submitted_by)
                .bind(&at)
                .execute(&mut *tx)
                .await?;
            }
            (None, None, None) if input.next_status != "waiting" => {}
            _ => {
                return Err(StoreError::InvalidCommand(
                    "approval id, digest, and purpose must be supplied together".into(),
                ));
            }
        }
        sqlx::query(
            "UPDATE sdd_attempts SET status = 'succeeded', finished_at = ?, error_summary = NULL
             WHERE attempt_id = ? AND run_id = ? AND status = 'running'",
        )
        .bind(&at)
        .bind(input.attempt_id)
        .bind(input.run_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_runs SET phase = ?, status = ?, blocker = NULL, aggregate_revision = ?,
             updated_at = ? WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(input.next_phase)
        .bind(input.next_status)
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
        sqlx::query(
            "INSERT INTO sdd_idempotency
             (scope, request_id, request_hash, expected_revision, response_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(format!("run:{}", input.run_id))
        .bind(input.request_id)
        .bind(input.request_hash)
        .bind(input.expected_revision)
        .bind(input.response_json)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    /// Import a valid user edit as a new immutable specification revision.
    /// No filesystem write occurs here: the user's bytes remain untouched.
    pub async fn sdd_import_external_spec(&self, input: ExternalSpecMutation<'_>) -> Result<i64> {
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
        if current != input.expected_run_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_run_revision,
                current,
            });
        }
        let current_spec_revision: i64 =
            sqlx::query_scalar("SELECT current_revision FROM sdd_specs WHERE spec_id = ?")
                .bind(&spec_id)
                .fetch_one(&mut *tx)
                .await?;
        if input.spec_revision != current_spec_revision + 1 {
            return Err(StoreError::InvalidCommand(format!(
                "external spec revision must be {}",
                current_spec_revision + 1
            )));
        }
        sqlx::query(
            "INSERT INTO sdd_spec_revisions
             (spec_id, revision, content_hash, content, submitted_by, imported_external, created_at)
             VALUES (?, ?, ?, ?, 'filesystem:user', 1, ?)",
        )
        .bind(&spec_id)
        .bind(input.spec_revision)
        .bind(input.content_hash)
        .bind(input.content)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_specs SET title = ?, current_revision = ?,
             aggregate_revision = aggregate_revision + 1, updated_at = ? WHERE spec_id = ?",
        )
        .bind(input.title)
        .bind(input.spec_revision)
        .bind(&at)
        .bind(&spec_id)
        .execute(&mut *tx)
        .await?;
        let artifact_revision: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(revision), 0) + 1 FROM sdd_artifact_revisions
             WHERE run_id = ? AND kind = 'specification'",
        )
        .bind(input.run_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO sdd_artifact_revisions
             (artifact_revision_id, run_id, spec_id, kind, revision, spec_revision,
              relative_path, content_hash, submitted_by, created_at)
             VALUES (?, ?, ?, 'specification', ?, ?, ?, ?, 'filesystem:user', ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(input.run_id)
        .bind(&spec_id)
        .bind(artifact_revision)
        .bind(input.spec_revision)
        .bind(input.relative_path)
        .bind(input.content_hash)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_approval_requests SET status = 'invalidated', invalidated_at = ?
             WHERE run_id = ? AND status IN ('pending', 'approved')",
        )
        .bind(&at)
        .bind(input.run_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_delivery_previews SET status = 'invalidated'
             WHERE run_id = ? AND status = 'pending'",
        )
        .bind(input.run_id)
        .execute(&mut *tx)
        .await?;
        let pending_patch: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sdd_patch_ledger
             WHERE run_id = ? AND status = 'pending')",
        )
        .bind(input.run_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_patch_ledger SET status = 'quarantined',
             error = 'specification changed while patch publication was active', updated_at = ?
             WHERE run_id = ? AND status = 'pending'",
        )
        .bind(&at)
        .bind(input.run_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM sdd_leases WHERE run_id = ?")
            .bind(input.run_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE sdd_attempts SET status = 'canceled', finished_at = ?,
             error_summary = 'specification revision changed'
             WHERE run_id = ? AND status IN ('queued', 'running', 'waiting', 'retry_scheduled')",
        )
        .bind(&at)
        .bind(input.run_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_tasks SET runtime_status = 'canceled',
             aggregate_revision = aggregate_revision + 1, updated_at = ?
             WHERE run_id = ? AND runtime_status IN
               ('idle', 'queued', 'running', 'waiting', 'retry_scheduled', 'paused', 'blocked')",
        )
        .bind(&at)
        .bind(input.run_id)
        .execute(&mut *tx)
        .await?;
        let next = current + 1;
        let blocker = "external specification edit imported; impact analysis required";
        sqlx::query(
            "UPDATE sdd_runs SET phase = 'specification', status = 'paused', blocker = ?,
             quarantined = ?, aggregate_revision = ?, updated_at = ?
             WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(blocker)
        .bind(pending_patch)
        .bind(next)
        .bind(&at)
        .bind(input.run_id)
        .bind(current)
        .execute(&mut *tx)
        .await?;
        let payload = serde_json::json!({
            "runId": input.run_id,
            "revision": next,
            "specRevision": input.spec_revision,
            "contentHash": input.content_hash,
            "status": "paused",
            "reason": blocker
        })
        .to_string();
        append_event(
            &mut tx,
            EventInsert {
                repo_id: &repo_id,
                spec_id: Some(&spec_id),
                run_id: Some(input.run_id),
                revision: next,
                kind: "sdd.spec.external_revision_imported",
                payload_json: &payload,
                created_at: &at,
            },
        )
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    pub async fn sdd_decide_approval(&self, input: ApprovalDecisionMutation<'_>) -> Result<i64> {
        let at = now()?;
        let mut tx = self.pool.begin().await?;
        let run: Option<(String, String, i64, String, String)> = sqlx::query_as(
            "SELECT repo_id, spec_id, aggregate_revision, phase, status
             FROM sdd_runs WHERE run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, current, phase, run_status)) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        if run_status != "waiting" {
            return Err(StoreError::InvalidCommand(
                "approval requires a waiting run".into(),
            ));
        }
        let approval: Option<(String, String, String, i64, String)> = sqlx::query_as(
            "SELECT digest, requested_by, status, requested_revision, purpose
             FROM sdd_approval_requests
             WHERE approval_id = ? AND run_id = ?",
        )
        .bind(input.approval_id)
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((stored_digest, requested_by, approval_status, requested_revision, purpose)) =
            approval
        else {
            return Err(StoreError::NotFound(input.approval_id.into()));
        };
        if approval_status != "pending" || stored_digest != input.digest {
            return Err(StoreError::ApprovalInvalid);
        }
        let current_spec_revision: i64 =
            sqlx::query_scalar("SELECT current_revision FROM sdd_specs WHERE spec_id = ?")
                .bind(&spec_id)
                .fetch_one(&mut *tx)
                .await?;
        if requested_revision != current_spec_revision {
            return Err(StoreError::ApprovalInvalid);
        }
        if requested_by == input.actor_id {
            return Err(StoreError::SelfApproval);
        }
        let approval_phase = match purpose.as_str() {
            "specification" => "specification",
            "design" => "design",
            "planning" => "planning",
            "implementation" => "implementation",
            "verification" => "verification",
            "review" => "review",
            _ => return Err(StoreError::ApprovalInvalid),
        };
        if phase != approval_phase {
            return Err(StoreError::ApprovalInvalid);
        }
        let next = current + 1;
        let (next_phase, status, approval_status, event_kind) = match input.decision {
            "approve" => (
                match purpose.as_str() {
                    "specification" => "design",
                    "design" => "planning",
                    "planning" => "implementation",
                    "implementation" => "verification",
                    "verification" => "review",
                    "review" => "ready",
                    _ => unreachable!("purpose validated above"),
                },
                if purpose == "review" {
                    "succeeded"
                } else {
                    "queued"
                },
                "approved",
                "sdd.approval.approved",
            ),
            "reject" => (
                approval_phase,
                "blocked",
                "rejected",
                "sdd.approval.rejected",
            ),
            _ => {
                return Err(StoreError::InvalidCommand(
                    "decision must be approve or reject".into(),
                ));
            }
        };
        let approval_updated = sqlx::query(
            "UPDATE sdd_approval_requests SET status = ?, decided_at = ?
             WHERE approval_id = ? AND status = 'pending'",
        )
        .bind(approval_status)
        .bind(&at)
        .bind(input.approval_id)
        .execute(&mut *tx)
        .await?;
        if approval_updated.rows_affected() != 1 {
            return Err(StoreError::ApprovalInvalid);
        }
        let decision_inserted = sqlx::query(
            "INSERT INTO sdd_approval_decisions
             (decision_id, approval_id, digest, actor_type, actor_id, decision, reason, created_at)
             VALUES (?, ?, ?, 'human', ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(input.approval_id)
        .bind(input.digest)
        .bind(input.actor_id)
        .bind(input.decision)
        .bind(input.reason)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        if decision_inserted.rows_affected() != 1 {
            return Err(StoreError::ApprovalInvalid);
        }
        let run_updated = sqlx::query(
            "UPDATE sdd_runs SET phase = ?, status = ?, blocker = ?, aggregate_revision = ?,
             updated_at = ? WHERE run_id = ? AND aggregate_revision = ?",
        )
        .bind(next_phase)
        .bind(status)
        .bind(if input.decision == "reject" {
            input.reason.or(Some("artifact approval rejected"))
        } else {
            None
        })
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
        let remote_exists: i64 =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sdd_remote_runs WHERE run_id = ?)")
                .bind(input.run_id)
                .fetch_one(&mut *tx)
                .await?;
        let projection_updated =
            sqlx::query("UPDATE sdd_remote_runs SET status = ?, updated_at = ? WHERE run_id = ?")
                .bind(status)
                .bind(&at)
                .bind(input.run_id)
                .execute(&mut *tx)
                .await?;
        if remote_exists != 0 && projection_updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote projection changed during approval".into(),
            ));
        }
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
        sqlx::query(
            "INSERT INTO sdd_idempotency
             (scope, request_id, request_hash, expected_revision, response_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(format!("run:{}", input.run_id))
        .bind(input.request_id)
        .bind(input.request_hash)
        .bind(input.expected_revision)
        .bind(input.response_json)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    /// Treat the authenticated user's explicit Start action as approval of
    /// exactly the pending digest, but only for a spec whose immutable workflow
    /// control is Autopilot. The approval mutation itself remains the same
    /// hash/revision-bound transaction used by the human approval endpoint.
    pub async fn sdd_authorize_autopilot_start(
        &self,
        input: ApprovalDecisionMutation<'_>,
    ) -> Result<i64> {
        if input.decision != "approve" {
            return Err(StoreError::InvalidCommand(
                "Autopilot Start can only authorize approval".into(),
            ));
        }
        let control: Option<String> = sqlx::query_scalar(
            "SELECT s.control FROM sdd_specs s
             JOIN sdd_runs r ON r.spec_id = s.spec_id
             WHERE r.run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(control) = control else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if control != "autopilot" {
            return Err(StoreError::InvalidCommand(
                "explicit Start can authorize a pending digest only in Autopilot control".into(),
            ));
        }
        self.sdd_decide_approval(input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> Store {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sdd.sqlite");
        std::mem::forget(dir);
        Store::open(&path).await.unwrap()
    }

    fn aggregate<'a>(response: &'a str) -> NewSddAggregate<'a> {
        NewSddAggregate {
            request_id: "req-create",
            request_hash: "hash-create",
            spec_id: "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            spec_ulid: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            repo_id: "repo-1",
            title: "Refresh tokens",
            slug: "spc-01arz3ndektsv4rrffq69g5fav-refresh-tokens",
            profile: "standard",
            control: "guarded",
            provider: "codex",
            source_ref_json: None,
            external_link: None,
            import_job: None,
            initial_spec_content: "# Initial",
            initial_spec_hash: "initial",
            spec_content: "# Spec",
            spec_hash: "abc",
            spec_revision: 2,
            submitted_by: "agent:codex:attempt-1",
            attempt_id: "attempt-1",
            attempt_path: "/tmp/attempt-1",
            attempt_session_identity: "session-1",
            run_id: "run-1",
            base_ref: "HEAD",
            base_commit: "deadbeef",
            branch_name: "agentum/spc-01arz3ndektsv4rrffq69g5fav-refresh-tokens",
            authoritative_path: "/tmp/run-1",
            workspace_fingerprint: "fp",
            policy_json: "{}",
            approval_id: "approval-1",
            approval_digest: "digest-1",
            response_json: response,
            remote_projection: None,
        }
    }

    async fn create(s: &Store, response: &str) {
        let input = aggregate(response);
        create_input(s, input).await;
    }

    async fn create_input(s: &Store, input: NewSddAggregate<'_>) {
        s.sdd_reserve_create(NewSddCreateSaga {
            repo_id: input.repo_id,
            request_id: input.request_id,
            request_hash: input.request_hash,
            spec_id: input.spec_id,
            run_id: input.run_id,
            repository_path: "/tmp/repo",
            authoritative_path: input.authoritative_path,
            branch_name: input.branch_name,
            attempt_id: input.attempt_id,
            attempt_path: input.attempt_path,
            artifact_set_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            artifact_set_required: false,
        })
        .await
        .unwrap();
        s.sdd_update_create_stage(
            input.repo_id,
            input.request_id,
            input.request_hash,
            &["reserved"],
            "publishing",
            None,
        )
        .await
        .unwrap();
        s.sdd_create_aggregate(input).await.unwrap();
    }

    #[tokio::test]
    async fn aggregate_is_durable_and_idempotency_response_is_replayable() {
        let s = store().await;
        create(&s, "{\"ok\":true}").await;
        let snapshot = s.sdd_snapshot("run-1").await.unwrap().unwrap();
        assert_eq!(snapshot.run.status, "waiting");
        assert_eq!(snapshot.run.authoritative_path, "/tmp/run-1");
        assert_eq!(snapshot.run.workspace_fingerprint, "fp");
        assert_eq!(snapshot.run.policy_json, "{}");
        assert_eq!(snapshot.artifacts.len(), 1);
        assert_eq!(snapshot.approval.unwrap().digest, "digest-1");
        assert_eq!(s.sdd_events_after("run-1", 0, 10).await.unwrap().len(), 1);
        assert_eq!(
            s.sdd_idempotent_response("repo:repo-1:create_spec", "req-create", "hash-create")
                .await
                .unwrap()
                .unwrap()["ok"],
            true
        );
    }

    #[tokio::test]
    async fn create_commits_normalized_source_provenance_atomically() {
        let s = store().await;
        let mut input = aggregate("{}");
        input.source_ref_json = Some(
            r#"{"kind":"github","sourceRevision":"revision-3","sourcePath":"https://github.com/o/r/issues/3"}"#,
        );
        input.external_link = Some(NewSddExternalLink {
            provider: "github",
            connection_id: "gh-cli:github.com",
            site_id: None,
            external_id: "3",
            key: Some("o/r#3"),
            url: "https://github.com/o/r/issues/3",
            source_revision: "revision-3",
        });
        input.import_job = Some(NewSddImportJob {
            source_kind: "markdown",
            source_hash: "sha256:source",
            preview_json: r#"{"kind":"markdown","sourceRevision":"sha256:source"}"#,
            disposition: "imported_revision",
        });
        create_input(&s, input).await;

        let spec = s
            .sdd_get_spec("SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .await
            .unwrap()
            .unwrap();
        assert!(spec.source_ref_json.unwrap().contains("revision-3"));
        let links = s
            .sdd_external_links_for_spec("SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .await
            .unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].external_id, "3");
        let import = s
            .sdd_import_job("repo-1", "markdown", "sha256:source")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(import.disposition, "imported_revision");
        assert!(import.committed_at.is_some());
    }

    #[tokio::test]
    async fn cas_rejects_stale_transition_without_an_event() {
        let s = store().await;
        create(&s, "{}").await;
        let error = s
            .sdd_transition(TransitionMutation {
                request_id: "req-pause",
                request_hash: "hash-pause",
                run_id: "run-1",
                expected_revision: 9,
                phase: "specification",
                status: "paused",
                blocker: None,
                event_kind: "paused",
                response_json: "{}",
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            StoreError::StaleRevision { current: 1, .. }
        ));
        assert_eq!(s.sdd_events_after("run-1", 0, 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn author_cannot_approve_their_own_artifact() {
        let s = store().await;
        create(&s, "{}").await;
        let error = s
            .sdd_decide_approval(ApprovalDecisionMutation {
                request_id: "req-approve",
                request_hash: "hash-approve",
                run_id: "run-1",
                expected_revision: 1,
                approval_id: "approval-1",
                digest: "digest-1",
                actor_id: "agent:codex:attempt-1",
                decision: "approve",
                reason: None,
                response_json: "{}",
            })
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::SelfApproval));
    }

    #[tokio::test]
    async fn autopilot_start_authorizes_only_the_pending_hash_bound_digest() {
        let s = store().await;
        let mut input = aggregate("{}");
        input.control = "autopilot";
        create_input(&s, input).await;
        let response = r#"{"runId":"run-1","revision":2,"phase":"design","status":"queued","authorization":{"digest":"digest-1","source":"explicit_start"}}"#;
        let next = s
            .sdd_authorize_autopilot_start(ApprovalDecisionMutation {
                request_id: "autopilot-start",
                request_hash: "autopilot-start-hash",
                run_id: "run-1",
                expected_revision: 1,
                approval_id: "approval-1",
                digest: "digest-1",
                actor_id: "human:user-1",
                decision: "approve",
                reason: Some("explicit Autopilot Start authorized the pending digest"),
                response_json: response,
            })
            .await
            .unwrap();
        assert_eq!(next, 2);
        let run = s.sdd_get_run("run-1").await.unwrap().unwrap();
        assert_eq!(
            (run.phase.as_str(), run.status.as_str()),
            ("design", "queued")
        );
        assert!(s.sdd_pending_approval("run-1").await.unwrap().is_none());
        let decision: (String, String, String, String) = sqlx::query_as(
            "SELECT digest, actor_type, actor_id, decision
             FROM sdd_approval_decisions WHERE approval_id = 'approval-1'",
        )
        .fetch_one(&s.pool)
        .await
        .unwrap();
        assert_eq!(
            decision,
            (
                "digest-1".into(),
                "human".into(),
                "human:user-1".into(),
                "approve".into()
            )
        );
        assert_eq!(
            s.sdd_idempotent_response("run:run-1", "autopilot-start", "autopilot-start-hash")
                .await
                .unwrap()
                .unwrap()["authorization"]["digest"],
            "digest-1"
        );
    }

    #[tokio::test]
    async fn explicit_start_cannot_authorize_a_guarded_approval() {
        let s = store().await;
        create(&s, "{}").await;
        let error = s
            .sdd_authorize_autopilot_start(ApprovalDecisionMutation {
                request_id: "guarded-start",
                request_hash: "guarded-start-hash",
                run_id: "run-1",
                expected_revision: 1,
                approval_id: "approval-1",
                digest: "digest-1",
                actor_id: "human:user-1",
                decision: "approve",
                reason: None,
                response_json: "{}",
            })
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::InvalidCommand(_)));
        let run = s.sdd_get_run("run-1").await.unwrap().unwrap();
        assert_eq!(
            (run.aggregate_revision, run.status.as_str()),
            (1, "waiting")
        );
        assert!(s.sdd_pending_approval("run-1").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn idempotency_is_scoped_and_payload_bound() {
        let s = store().await;
        create(&s, "{\"ok\":true}").await;
        assert!(
            s.sdd_idempotent_response("run:other", "req-create", "hash-create")
                .await
                .unwrap()
                .is_none()
        );
        let error = s
            .sdd_idempotent_response("repo:repo-1:create_spec", "req-create", "different-payload")
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::IdempotencyConflict(_)));
    }

    #[tokio::test]
    async fn approval_cannot_escape_waiting_state() {
        let s = store().await;
        create(&s, "{}").await;
        s.sdd_transition(TransitionMutation {
            request_id: "req-pause",
            request_hash: "hash-pause",
            run_id: "run-1",
            expected_revision: 1,
            phase: "specification",
            status: "paused",
            blocker: None,
            event_kind: "sdd.run.paused",
            response_json: "{}",
        })
        .await
        .unwrap();
        let error = s
            .sdd_decide_approval(ApprovalDecisionMutation {
                request_id: "req-approve",
                request_hash: "hash-approve",
                run_id: "run-1",
                expected_revision: 2,
                approval_id: "approval-1",
                digest: "digest-1",
                actor_id: "human:user-1",
                decision: "approve",
                reason: None,
                response_json: "{}",
            })
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::InvalidCommand(_)));
        assert!(!s.sdd_current_spec_is_approved("run-1").await.unwrap());
    }

    #[tokio::test]
    async fn interrupted_create_reservations_are_claimed_for_exact_recovery() {
        let s = store().await;
        let input = aggregate("{}");
        s.sdd_reserve_create(NewSddCreateSaga {
            repo_id: input.repo_id,
            request_id: input.request_id,
            request_hash: input.request_hash,
            spec_id: input.spec_id,
            run_id: input.run_id,
            repository_path: "/tmp/repo",
            authoritative_path: input.authoritative_path,
            branch_name: input.branch_name,
            attempt_id: input.attempt_id,
            attempt_path: input.attempt_path,
            artifact_set_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            artifact_set_required: false,
        })
        .await
        .unwrap();
        let claimed = s.sdd_claim_interrupted_creates().await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].stage, "recovery_required");
        assert_eq!(claimed[0].authoritative_path, "/tmp/run-1");
        assert!(s.sdd_claim_interrupted_creates().await.unwrap().len() == 1);
    }

    #[tokio::test]
    async fn concurrent_spec_reservations_share_one_repository_artifact_set() {
        let s = store().await;
        let first = s.sdd_reserve_create(NewSddCreateSaga {
            repo_id: "repo-concurrent",
            request_id: "request-a",
            request_hash: "hash-a",
            spec_id: "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            run_id: "run-a",
            repository_path: "/tmp/repo-concurrent",
            authoritative_path: "/tmp/run-a",
            branch_name: "agentum/spc-01arz3ndektsv4rrffq69g5fav-a",
            attempt_id: "attempt-a",
            attempt_path: "/tmp/attempt-a",
            artifact_set_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            artifact_set_required: false,
        });
        let second = s.sdd_reserve_create(NewSddCreateSaga {
            repo_id: "repo-concurrent",
            request_id: "request-b",
            request_hash: "hash-b",
            spec_id: "SPC-01BX5ZZKBKACTAV9WEVGEMMVRZ",
            run_id: "run-b",
            repository_path: "/tmp/repo-concurrent",
            authoritative_path: "/tmp/run-b",
            branch_name: "agentum/spc-01bx5zzkbkactav9wevgemmvrz-b",
            attempt_id: "attempt-b",
            attempt_path: "/tmp/attempt-b",
            artifact_set_id: "01BX5ZZKBKACTAV9WEVGEMMVRZ",
            artifact_set_required: false,
        });
        let (first, second) = tokio::join!(first, second);
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first, second);
        assert!(first == "01ARZ3NDEKTSV4RRFFQ69G5FAV" || first == "01BX5ZZKBKACTAV9WEVGEMMVRZ");
    }

    #[tokio::test]
    async fn repository_manifest_identity_cannot_replace_registered_identity() {
        let s = store().await;
        s.sdd_reserve_create(NewSddCreateSaga {
            repo_id: "repo-manifest-conflict",
            request_id: "request-a",
            request_hash: "hash-a",
            spec_id: "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            run_id: "run-a",
            repository_path: "/tmp/repo-manifest-conflict",
            authoritative_path: "/tmp/run-a",
            branch_name: "agentum/spc-01arz3ndektsv4rrffq69g5fav-a",
            attempt_id: "attempt-a",
            attempt_path: "/tmp/attempt-a",
            artifact_set_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            artifact_set_required: false,
        })
        .await
        .unwrap();
        let error = s
            .sdd_reserve_create(NewSddCreateSaga {
                repo_id: "repo-manifest-conflict",
                request_id: "request-b",
                request_hash: "hash-b",
                spec_id: "SPC-01BX5ZZKBKACTAV9WEVGEMMVRZ",
                run_id: "run-b",
                repository_path: "/tmp/repo-manifest-conflict",
                authoritative_path: "/tmp/run-b",
                branch_name: "agentum/spc-01bx5zzkbkactav9wevgemmvrz-b",
                attempt_id: "attempt-b",
                attempt_path: "/tmp/attempt-b",
                artifact_set_id: "01BX5ZZKBKACTAV9WEVGEMMVRZ",
                artifact_set_required: true,
            })
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::ArtifactSetConflict(_)));
        assert!(
            s.sdd_create_saga("repo-manifest-conflict", "request-b")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn external_revision_hides_all_downstream_state_and_invalidates_approval() {
        let s = store().await;
        create(&s, "{}").await;
        s.sdd_decide_approval(ApprovalDecisionMutation {
            request_id: "approve-before-edit",
            request_hash: "approve-before-edit-hash",
            run_id: "run-1",
            expected_revision: 1,
            approval_id: "approval-1",
            digest: "digest-1",
            actor_id: "human:reviewer",
            decision: "approve",
            reason: None,
            response_json: "{}",
        })
        .await
        .unwrap();
        let at = now().unwrap();
        for (kind, revision, path) in [
            ("design", 1_i64, ".agentum/specs/example/design.md"),
            ("plan", 1_i64, ".agentum/specs/example/plan.json"),
            ("review", 1_i64, ".agentum/specs/example/review.md"),
        ] {
            sqlx::query(
                "INSERT INTO sdd_artifact_revisions
                 (artifact_revision_id, run_id, spec_id, kind, revision, spec_revision,
                  relative_path, content_hash, submitted_by, created_at)
                 VALUES (?, 'run-1', 'SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV', ?, ?, 2, ?, ?,
                         'agent:codex:old', ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(kind)
            .bind(revision)
            .bind(path)
            .bind(format!("{kind}-hash"))
            .bind(&at)
            .execute(&s.pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO sdd_tasks
             (run_id, task_id, spec_revision, intent_json, runtime_status,
              aggregate_revision, created_at, updated_at)
             VALUES ('run-1', 'T-old', 2, '{}', 'succeeded', 1, ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&s.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sdd_attempts
             (attempt_id, run_id, task_id, spec_revision, provider, isolated_path, status,
              session_identity, started_at, finished_at)
             VALUES ('attempt-old', 'run-1', 'T-old', 2, 'codex', '/tmp/attempt-old',
                     'succeeded', 'old-session', ?, ?)",
        )
        .bind(&at)
        .bind(&at)
        .execute(&s.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sdd_verification_results
             (verification_id, run_id, attempt_id, task_id, spec_revision, command_index,
              command_json, status, exit_code, output_hash, output_excerpt, duration_ms, created_at)
             VALUES (?, 'run-1', 'attempt-old', 'T-old', 2, 0, '{}', 'succeeded', 0,
                     'old-output', '[redacted]', 1, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&at)
        .execute(&s.pool)
        .await
        .unwrap();

        s.sdd_import_external_spec(ExternalSpecMutation {
            run_id: "run-1",
            expected_run_revision: 2,
            spec_revision: 3,
            title: "Edited",
            relative_path: ".agentum/specs/example/spec.md",
            content_hash: "new-spec-hash",
            content: "# Edited",
        })
        .await
        .unwrap();

        let snapshot = s.sdd_snapshot("run-1").await.unwrap().unwrap();
        assert_eq!(snapshot.run.phase, "specification");
        assert_eq!(snapshot.run.status, "paused");
        assert_eq!(snapshot.spec.current_revision, 3);
        assert!(
            snapshot
                .artifacts
                .iter()
                .all(|artifact| artifact.kind == "specification")
        );
        assert!(snapshot.tasks.is_empty());
        assert!(snapshot.attempts.is_empty());
        assert!(snapshot.verification.is_empty());
        assert!(snapshot.approval.is_none());
        assert!(!s.sdd_current_spec_is_approved("run-1").await.unwrap());
        let approval_status: String = sqlx::query_scalar(
            "SELECT status FROM sdd_approval_requests WHERE approval_id = 'approval-1'",
        )
        .fetch_one(&s.pool)
        .await
        .unwrap();
        assert_eq!(approval_status, "invalidated");
    }

    fn discovered_spec_fixture() -> DiscoveredSpecInput<'static> {
        DiscoveredSpecInput {
            spec_id: "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            spec_ulid: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            title: "Discovered",
            slug: "spc-01arz3ndektsv4rrffq69g5fav-discovered",
            source_ref_json: None,
            revision: 1,
            content_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            content: "immutable discovered specification",
        }
    }

    async fn seed_discovered_spec(s: &Store) -> DiscoveredSpecInput<'static> {
        let discovered = discovered_spec_fixture();
        s.sdd_reconcile_discovered_specs(ReconcileDiscoveredSpecs {
            repo_id: "repo-discovered-run",
            artifact_set_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX",
            specs: std::slice::from_ref(&discovered),
        })
        .await
        .unwrap();
        discovered
    }

    fn discovered_run_reservation<'a>(
        discovered: &'a DiscoveredSpecInput<'a>,
    ) -> NewSddRunCreateSaga<'a> {
        NewSddRunCreateSaga {
            spec_id: discovered.spec_id,
            repo_id: "repo-discovered-run",
            request_id: "start-discovered",
            request_hash: "request-hash",
            run_id: "run-discovered",
            expected_spec_revision: discovered.revision,
            expected_spec_hash: discovered.content_hash,
            expected_aggregate_revision: 1,
            repository_path: "/tmp/repo-discovered",
            authoritative_path: "/tmp/agentum/run-discovered/authoritative",
            branch_name: "agentum/spc-01arz3ndektsv4rrffq69g5fav-discovered",
            attempt_id: "attempt-import",
            attempt_path: "/tmp/agentum/run-discovered/attempts/attempt-import",
        }
    }

    #[tokio::test]
    async fn discovered_run_publication_is_atomic_cas_bound_and_replayable() {
        let s = store().await;
        let discovered = seed_discovered_spec(&s).await;
        s.sdd_reserve_discovered_run(discovered_run_reservation(&discovered))
            .await
            .unwrap();
        s.sdd_update_run_create_stage(
            discovered.spec_id,
            "start-discovered",
            "request-hash",
            &["reserved"],
            "workspace_ready",
            None,
        )
        .await
        .unwrap();
        s.sdd_update_run_create_stage(
            discovered.spec_id,
            "start-discovered",
            "request-hash",
            &["workspace_ready"],
            "publishing",
            None,
        )
        .await
        .unwrap();
        let artifacts = [
            NewSddRunArtifact {
                kind: "specification",
                relative_path: ".agentum/specs/discovered/spec.md",
                content_hash: discovered.content_hash,
            },
            NewSddRunArtifact {
                kind: "design",
                relative_path: ".agentum/specs/discovered/design.md",
                content_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            },
        ];
        let input = NewSddDiscoveredRun {
            spec_id: discovered.spec_id,
            repo_id: "repo-discovered-run",
            request_id: "start-discovered",
            request_hash: "request-hash",
            expected_aggregate_revision: 1,
            expected_spec_revision: 1,
            expected_spec_hash: discovered.content_hash,
            profile: "standard",
            control: "guarded",
            provider: "codex",
            run_id: "run-discovered",
            base_ref: "HEAD",
            base_commit: "deadbeef",
            branch_name: "agentum/spc-01arz3ndektsv4rrffq69g5fav-discovered",
            authoritative_path: "/tmp/agentum/run-discovered/authoritative",
            workspace_fingerprint: "workspace-fingerprint",
            policy_json: r#"{"discoveredArtifactDisposition":{"status":"historical_unapproved_reopen_from_specification"}}"#,
            attempt_id: "attempt-import",
            attempt_path: "/tmp/agentum/run-discovered/attempts/attempt-import",
            attempt_session_identity: "filesystem-import:attempt-import",
            submitted_by: "agentum:filesystem-discovery:attempt-import",
            artifacts: &artifacts,
            approval_id: "approval-discovered",
            approval_digest: "spec-only-digest",
            response_json: r#"{"runId":"run-discovered","status":"waiting"}"#,
        };

        let stale = NewSddDiscoveredRun {
            expected_aggregate_revision: 2,
            ..input.clone()
        };
        assert!(matches!(
            s.sdd_publish_discovered_run(stale).await.unwrap_err(),
            StoreError::InvalidCommand(_)
        ));
        assert!(s.sdd_snapshot("run-discovered").await.unwrap().is_none());
        assert_eq!(
            s.sdd_get_spec(discovered.spec_id)
                .await
                .unwrap()
                .unwrap()
                .provider,
            "unassigned"
        );

        s.sdd_publish_discovered_run(input).await.unwrap();
        let snapshot = s.sdd_snapshot("run-discovered").await.unwrap().unwrap();
        assert_eq!(snapshot.spec.aggregate_revision, 2);
        assert_eq!(snapshot.spec.profile, "standard");
        assert_eq!(snapshot.spec.control, "guarded");
        assert_eq!(snapshot.spec.provider, "codex");
        assert_eq!(snapshot.run.phase, "specification");
        assert_eq!(snapshot.run.status, "waiting");
        assert_eq!(snapshot.artifacts.len(), 2);
        assert!(snapshot.tasks.is_empty());
        assert_eq!(snapshot.attempts.len(), 1);
        assert_eq!(snapshot.attempts[0].status, "succeeded");
        assert_eq!(snapshot.approval.unwrap().digest, "spec-only-digest");
        assert_eq!(
            s.sdd_events_after("run-discovered", 0, 10).await.unwrap()[0].kind,
            "sdd.run.created_from_discovered_spec"
        );
        let outbox_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sdd_outbox o
             JOIN sdd_events e ON e.cursor = o.event_cursor
             WHERE o.delivered_at IS NULL
               AND e.kind = 'sdd.run.created_from_discovered_spec'",
        )
        .fetch_one(&s.pool)
        .await
        .unwrap();
        assert_eq!(outbox_count, 1);
        assert_eq!(
            s.sdd_idempotent_response(
                &format!("spec:{}:create_run", discovered.spec_id),
                "start-discovered",
                "request-hash"
            )
            .await
            .unwrap()
            .unwrap()["runId"],
            "run-discovered"
        );
        assert_eq!(
            s.sdd_run_create_saga(discovered.spec_id, "start-discovered")
                .await
                .unwrap()
                .unwrap()
                .stage,
            "completed"
        );

        let mut duplicate = discovered_run_reservation(&discovered);
        duplicate.request_id = "start-after-complete";
        duplicate.request_hash = "request-hash-after-complete";
        duplicate.run_id = "run-duplicate";
        assert!(s.sdd_reserve_discovered_run(duplicate).await.is_err());
        assert!(s.sdd_get_run("run-duplicate").await.unwrap().is_none());
        let run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sdd_runs WHERE spec_id = ?")
            .bind(discovered.spec_id)
            .fetch_one(&s.pool)
            .await
            .unwrap();
        assert_eq!(run_count, 1);
    }

    #[tokio::test]
    async fn interrupted_discovered_run_reservation_is_recovered_and_retryable_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("sdd.sqlite");
        let s = Store::open(&database).await.unwrap();
        let discovered = seed_discovered_spec(&s).await;
        let mut stale = discovered_run_reservation(&discovered);
        stale.expected_aggregate_revision = 2;
        assert!(matches!(
            s.sdd_reserve_discovered_run(stale).await.unwrap_err(),
            StoreError::StaleRevision {
                expected: 2,
                current: 1
            }
        ));
        assert!(
            s.sdd_run_create_saga(discovered.spec_id, "start-discovered")
                .await
                .unwrap()
                .is_none()
        );
        s.sdd_reserve_discovered_run(discovered_run_reservation(&discovered))
            .await
            .unwrap();
        s.pool.close().await;

        let restarted = Store::open(&database).await.unwrap();
        let claimed = restarted.sdd_claim_interrupted_run_creates().await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].stage, "recovery_required");
        assert_eq!(claimed[0].run_id, "run-discovered");
        assert_eq!(
            claimed[0].authoritative_path,
            "/tmp/agentum/run-discovered/authoritative"
        );
        assert_eq!(
            claimed[0].attempt_path,
            "/tmp/agentum/run-discovered/attempts/attempt-import"
        );
        restarted
            .sdd_update_run_create_stage(
                discovered.spec_id,
                "start-discovered",
                "request-hash",
                &["recovery_required"],
                "failed",
                Some("recovered exact filesystem targets"),
            )
            .await
            .unwrap();
        assert!(
            restarted
                .sdd_latest_run_for_spec(discovered.spec_id)
                .await
                .unwrap()
                .is_none()
        );
        let spec = restarted
            .sdd_get_spec(discovered.spec_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(spec.aggregate_revision, 1);
        assert_eq!(spec.provider, "unassigned");

        let mut retry = discovered_run_reservation(&discovered);
        retry.request_id = "retry-discovered";
        retry.request_hash = "retry-request-hash";
        retry.run_id = "run-discovered-retry";
        retry.attempt_id = "attempt-import-retry";
        retry.attempt_path = "/tmp/agentum/run-discovered-retry/attempts/attempt-import-retry";
        retry.authoritative_path = "/tmp/agentum/run-discovered-retry/authoritative";
        restarted.sdd_reserve_discovered_run(retry).await.unwrap();
        let retry_saga = restarted
            .sdd_run_create_saga(discovered.spec_id, "retry-discovered")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retry_saga.stage, "reserved");
        assert_eq!(retry_saga.run_id, "run-discovered-retry");
        let failed_saga = restarted
            .sdd_run_create_saga(discovered.spec_id, "start-discovered")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed_saga.stage, "failed");
        assert_eq!(
            failed_saga.error_summary.as_deref(),
            Some("recovered exact filesystem targets")
        );
    }

    #[tokio::test]
    async fn filesystem_discovery_is_atomic_idempotent_and_revision_immutable() {
        let s = store().await;
        let first = DiscoveredSpecInput {
            spec_id: "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            spec_ulid: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            title: "First",
            slug: "spc-01arz3ndektsv4rrffq69g5fav-first",
            source_ref_json: None,
            revision: 1,
            content_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            content: "first revision",
        };
        let second = DiscoveredSpecInput {
            spec_id: "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAW",
            spec_ulid: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
            title: "Second",
            slug: "spc-01arz3ndektsv4rrffq69g5faw-second",
            source_ref_json: None,
            revision: 1,
            content_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            content: "second revision",
        };
        let initial = s
            .sdd_reconcile_discovered_specs(ReconcileDiscoveredSpecs {
                repo_id: "repo-discovery",
                artifact_set_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX",
                specs: &[first.clone(), second.clone()],
            })
            .await
            .unwrap();
        assert_eq!(
            initial,
            ReconcileDiscoveredResult {
                inserted: 2,
                revised: 0,
                unchanged: 0
            }
        );
        let replay = s
            .sdd_reconcile_discovered_specs(ReconcileDiscoveredSpecs {
                repo_id: "repo-discovery",
                artifact_set_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX",
                specs: &[first.clone(), second.clone()],
            })
            .await
            .unwrap();
        assert_eq!(replay.unchanged, 2);
        assert_eq!(s.sdd_list_specs("repo-discovery").await.unwrap().len(), 2);

        let revised = DiscoveredSpecInput {
            revision: 2,
            title: "First revised",
            content_hash: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            content: "second immutable revision",
            ..first.clone()
        };
        let outcome = s
            .sdd_reconcile_discovered_specs(ReconcileDiscoveredSpecs {
                repo_id: "repo-discovery",
                artifact_set_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX",
                specs: &[revised],
            })
            .await
            .unwrap();
        assert_eq!(outcome.revised, 1);
        assert_eq!(
            s.sdd_get_spec(first.spec_id)
                .await
                .unwrap()
                .unwrap()
                .current_revision,
            2
        );

        let changed_same_revision = DiscoveredSpecInput {
            content_hash: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            content: "tampered",
            ..second.clone()
        };
        let error = s
            .sdd_reconcile_discovered_specs(ReconcileDiscoveredSpecs {
                repo_id: "repo-discovery",
                artifact_set_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX",
                specs: &[changed_same_revision],
            })
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::InvalidCommand(_)));
        let hash: String = sqlx::query_scalar(
            "SELECT content_hash FROM sdd_spec_revisions
             WHERE spec_id = ? AND revision = 1",
        )
        .bind(second.spec_id)
        .fetch_one(&s.pool)
        .await
        .unwrap();
        assert_eq!(hash, second.content_hash);
    }

    #[tokio::test]
    async fn filesystem_discovery_rolls_back_the_whole_batch_on_collision() {
        let s = store().await;
        let existing = DiscoveredSpecInput {
            spec_id: "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            spec_ulid: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            title: "Existing",
            slug: "spc-01arz3ndektsv4rrffq69g5fav-existing",
            source_ref_json: None,
            revision: 1,
            content_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            content: "existing",
        };
        s.sdd_reconcile_discovered_specs(ReconcileDiscoveredSpecs {
            repo_id: "repo-discovery",
            artifact_set_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX",
            specs: std::slice::from_ref(&existing),
        })
        .await
        .unwrap();
        let new_spec = DiscoveredSpecInput {
            spec_id: "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAW",
            spec_ulid: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
            title: "New",
            slug: "spc-01arz3ndektsv4rrffq69g5faw-new",
            source_ref_json: None,
            revision: 1,
            content_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            content: "new",
        };
        let tampered = DiscoveredSpecInput {
            content_hash: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            ..existing
        };
        assert!(
            s.sdd_reconcile_discovered_specs(ReconcileDiscoveredSpecs {
                repo_id: "repo-discovery",
                artifact_set_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX",
                specs: &[new_spec.clone(), tampered],
            })
            .await
            .is_err()
        );
        assert!(s.sdd_get_spec(new_spec.spec_id).await.unwrap().is_none());
    }
}
