//! Durable state transitions for the host-side `agentum-sdd-v1` subsystem.
//! Model/provider execution and filesystem work happen outside transactions;
//! these methods reserve and publish their intent/results atomically.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::sdd::now;
use crate::{Result, Store, StoreError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWorkerRunRecord {
    pub run_id: String,
    pub host_id: String,
    pub repository_identity_sha256: String,
    pub artifact_set_id: String,
    pub spec_id: String,
    pub spec_revision: i64,
    pub base_commit: String,
    pub provider: String,
    pub authoritative_path: String,
    pub branch_name: String,
    pub approval_digest: Option<String>,
    pub next_phase: String,
    pub completed_phases: i64,
    pub workspace_state_sha256: String,
    pub last_result_sha256: String,
    pub status: String,
    pub blocker: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWorkerRequestRecord {
    pub request_id: String,
    pub request_sha256: String,
    pub run_id: String,
    pub operation: String,
    pub phase: Option<String>,
    pub stage: String,
    pub attempt_path: Option<String>,
    pub response_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWorkerPatchRecord {
    pub patch_id: String,
    pub request_id: String,
    pub run_id: String,
    pub operations_json: String,
    pub preimages_json: String,
    pub status: String,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteWorkerReservation {
    Started,
    Replay(String),
    RecoveryRequired(Box<RemoteWorkerRequestRecord>),
}

#[derive(Debug, Clone)]
pub struct ReserveRemoteAuthoring<'a> {
    pub request_id: &'a str,
    pub request_sha256: &'a str,
    pub run_id: &'a str,
    pub host_id: &'a str,
    pub repository_identity_sha256: &'a str,
    pub artifact_set_id: &'a str,
    pub spec_id: &'a str,
    pub base_commit: &'a str,
    pub provider: &'a str,
    pub authoritative_path: &'a str,
    pub branch_name: &'a str,
    pub initial_workspace_state_sha256: &'a str,
}

#[derive(Debug, Clone)]
pub struct ReserveRemotePhase<'a> {
    pub request_id: &'a str,
    pub request_sha256: &'a str,
    pub run_id: &'a str,
    pub host_id: &'a str,
    pub repository_identity_sha256: &'a str,
    pub artifact_set_id: &'a str,
    pub spec_id: &'a str,
    pub spec_revision: i64,
    pub base_commit: &'a str,
    pub provider: &'a str,
    pub approval_digest: &'a str,
    pub phase: &'a str,
    pub completed_phases: i64,
    pub expected_workspace_state_sha256: &'a str,
    pub previous_result_sha256: &'a str,
}

#[derive(Debug, Clone)]
pub struct ReserveRemoteDelivery<'a> {
    pub request_id: &'a str,
    pub request_sha256: &'a str,
    pub run_id: &'a str,
    pub host_id: &'a str,
    pub repository_identity_sha256: &'a str,
    pub artifact_set_id: &'a str,
    pub spec_id: &'a str,
    pub spec_revision: i64,
    pub base_commit: &'a str,
    pub approval_digest: &'a str,
    pub preview_digest: &'a str,
    pub action_id: &'a str,
    pub dependencies: &'a [String],
    pub initial_workspace_state_sha256: &'a str,
}

impl Store {
    pub async fn sdd_remote_worker_run(
        &self,
        run_id: &str,
    ) -> Result<Option<RemoteWorkerRunRecord>> {
        Ok(
            sqlx::query_as("SELECT * FROM sdd_remote_worker_runs WHERE run_id = ?")
                .bind(run_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn sdd_remote_worker_phase_response(
        &self,
        run_id: &str,
        phase: &str,
    ) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT response_json FROM sdd_remote_worker_requests
             WHERE run_id = ? AND operation = 'phase' AND phase = ?
               AND stage = 'completed' AND response_json IS NOT NULL
             ORDER BY created_at DESC, request_id DESC LIMIT 1",
        )
        .bind(run_id)
        .bind(phase)
        .fetch_optional(&self.pool)
        .await?
        .flatten())
    }

    pub async fn sdd_remote_worker_request(
        &self,
        request_id: &str,
    ) -> Result<Option<RemoteWorkerRequestRecord>> {
        Ok(
            sqlx::query_as("SELECT * FROM sdd_remote_worker_requests WHERE request_id = ?")
                .bind(request_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn sdd_remote_worker_acquire_lease(
        &self,
        owner_id: &str,
        request_id: &str,
        expires_at: &str,
    ) -> Result<()> {
        let at = now()?;
        let current = OffsetDateTime::parse(&at, &Rfc3339)?;
        let requested = OffsetDateTime::parse(expires_at, &Rfc3339).map_err(|_| {
            StoreError::InvalidCommand("remote worker lease expiration is invalid".into())
        })?;
        if requested <= current || requested > current + time::Duration::hours(2) {
            return Err(StoreError::InvalidCommand(
                "remote worker lease expiration is outside its allowed window".into(),
            ));
        }
        let expires_at = requested.format(&Rfc3339)?;
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "DELETE FROM sdd_remote_worker_lease
             WHERE singleton = 1 AND julianday(expires_at) <= julianday(?)",
        )
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO sdd_remote_worker_lease
             (singleton, owner_id, request_id, expires_at, acquired_at)
             VALUES (1, ?, ?, ?, ?)",
        )
        .bind(owner_id)
        .bind(request_id)
        .bind(&expires_at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "the remote SDD worker is busy".into(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn sdd_remote_worker_release_lease(&self, owner_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sdd_remote_worker_lease WHERE singleton = 1 AND owner_id = ?")
            .bind(owner_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Reserve the exact authoritative-file publication before touching the
    /// filesystem. A surviving `reserved` row is recovery evidence and must
    /// be reconciled from its recorded preimages before another phase starts.
    pub async fn sdd_remote_worker_reserve_patch(
        &self,
        patch_id: &str,
        request_id: &str,
        run_id: &str,
        operations_json: &str,
        preimages_json: &str,
    ) -> Result<()> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let request: Option<(String, String)> = sqlx::query_as(
            "SELECT run_id, stage FROM sdd_remote_worker_requests WHERE request_id = ?",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await?;
        if !matches!(request.as_ref(), Some((bound_run, stage)) if bound_run == run_id && stage != "completed")
        {
            return Err(StoreError::InvalidCommand(
                "remote patch is not bound to an active request".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO sdd_remote_worker_patch_journal
             (patch_id, request_id, run_id, operations_json, preimages_json,
              status, error, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'reserved', NULL, ?, ?)",
        )
        .bind(patch_id)
        .bind(request_id)
        .bind(run_id)
        .bind(operations_json)
        .bind(preimages_json)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sdd_remote_worker_requests SET stage = 'publishing', updated_at = ?
             WHERE request_id = ? AND response_json IS NULL",
        )
        .bind(&at)
        .bind(request_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn sdd_remote_worker_complete_patch(&self, patch_id: &str) -> Result<()> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        let request_id: Option<String> = sqlx::query_scalar(
            "SELECT request_id FROM sdd_remote_worker_patch_journal
             WHERE patch_id = ? AND status = 'reserved'",
        )
        .bind(patch_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(request_id) = request_id else {
            return Err(StoreError::InvalidCommand(
                "remote patch journal is not completable".into(),
            ));
        };
        let updated = sqlx::query(
            "UPDATE sdd_remote_worker_patch_journal SET status = 'completed', error = NULL,
             updated_at = ? WHERE patch_id = ? AND status = 'reserved'",
        )
        .bind(&at)
        .bind(patch_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote patch journal is not completable".into(),
            ));
        }
        sqlx::query(
            "UPDATE sdd_remote_worker_requests SET stage = 'running', updated_at = ?
             WHERE request_id = ? AND stage = 'publishing' AND response_json IS NULL",
        )
        .bind(&at)
        .bind(request_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn sdd_remote_worker_fail_patch(&self, patch_id: &str, error: &str) -> Result<()> {
        let at = now()?;
        let updated = sqlx::query(
            "UPDATE sdd_remote_worker_patch_journal SET status = 'failed', error = ?,
             updated_at = ? WHERE patch_id = ? AND status = 'reserved'",
        )
        .bind(error)
        .bind(&at)
        .bind(patch_id)
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote patch journal is not fail-able".into(),
            ));
        }
        Ok(())
    }

    pub async fn sdd_remote_worker_unfinished_patches(
        &self,
        run_id: &str,
    ) -> Result<Vec<RemoteWorkerPatchRecord>> {
        Ok(sqlx::query_as(
            "SELECT * FROM sdd_remote_worker_patch_journal
             WHERE run_id = ? AND status = 'reserved' ORDER BY created_at, patch_id",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sdd_remote_worker_reserve_authoring(
        &self,
        input: ReserveRemoteAuthoring<'_>,
    ) -> Result<RemoteWorkerReservation> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        if let Some(existing) = remote_request(&mut tx, input.request_id).await? {
            let reservation = replay_or_recover(existing, input.request_sha256, input.run_id)?;
            tx.commit().await?;
            return Ok(reservation);
        }
        let existing_run: Option<RemoteWorkerRunRecord> =
            sqlx::query_as("SELECT * FROM sdd_remote_worker_runs WHERE run_id = ?")
                .bind(input.run_id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(run) = existing_run {
            let retryable = run.status == "blocked"
                && run.host_id == input.host_id
                && run.repository_identity_sha256 == input.repository_identity_sha256
                && run.artifact_set_id == input.artifact_set_id
                && run.spec_id == input.spec_id
                && run.spec_revision == 1
                && run.base_commit == input.base_commit
                && run.provider == input.provider
                && run.authoritative_path == input.authoritative_path
                && run.branch_name == input.branch_name;
            if !retryable {
                return Err(StoreError::AlreadyExists(input.run_id.into()));
            }
            let unfinished: i64 = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sdd_remote_worker_requests
                 WHERE run_id = ? AND response_json IS NULL)",
            )
            .bind(input.run_id)
            .fetch_one(&mut *tx)
            .await?;
            if unfinished != 0 {
                return Err(StoreError::InvalidCommand(
                    "remote authoring recovery is still unfinished".into(),
                ));
            }
            sqlx::query(
                "UPDATE sdd_remote_worker_runs SET status = 'authoring', blocker = NULL,
                 workspace_state_sha256 = ?, last_result_sha256 = ?, updated_at = ?
                 WHERE run_id = ? AND status = 'blocked'",
            )
            .bind(input.initial_workspace_state_sha256)
            .bind(input.initial_workspace_state_sha256)
            .bind(&at)
            .bind(input.run_id)
            .execute(&mut *tx)
            .await?;
            insert_remote_request(
                &mut tx,
                input.request_id,
                input.request_sha256,
                input.run_id,
                "authoring",
                None,
                &at,
            )
            .await?;
            tx.commit().await?;
            return Ok(RemoteWorkerReservation::Started);
        }
        sqlx::query(
            "INSERT INTO sdd_remote_worker_runs
             (run_id, host_id, repository_identity_sha256, artifact_set_id, spec_id, spec_revision,
              base_commit, provider, authoritative_path, branch_name, approval_digest,
              next_phase, completed_phases, workspace_state_sha256, last_result_sha256,
              status, blocker, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?, ?, NULL, 'design', 0, ?, ?,
                     'authoring', NULL, ?, ?)",
        )
        .bind(input.run_id)
        .bind(input.host_id)
        .bind(input.repository_identity_sha256)
        .bind(input.artifact_set_id)
        .bind(input.spec_id)
        .bind(input.base_commit)
        .bind(input.provider)
        .bind(input.authoritative_path)
        .bind(input.branch_name)
        .bind(input.initial_workspace_state_sha256)
        .bind(input.initial_workspace_state_sha256)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        insert_remote_request(
            &mut tx,
            input.request_id,
            input.request_sha256,
            input.run_id,
            "authoring",
            None,
            &at,
        )
        .await?;
        tx.commit().await?;
        Ok(RemoteWorkerReservation::Started)
    }

    pub async fn sdd_remote_worker_reserve_phase(
        &self,
        input: ReserveRemotePhase<'_>,
    ) -> Result<RemoteWorkerReservation> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        if let Some(existing) = remote_request(&mut tx, input.request_id).await? {
            let reservation = replay_or_recover(existing, input.request_sha256, input.run_id)?;
            tx.commit().await?;
            return Ok(reservation);
        }
        let run: Option<RemoteWorkerRunRecord> =
            sqlx::query_as("SELECT * FROM sdd_remote_worker_runs WHERE run_id = ?")
                .bind(input.run_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(run) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        let first_approved_phase = run.status == "waiting_approval"
            && run.approval_digest.is_none()
            && input.phase == "design";
        if run.host_id != input.host_id
            || run.repository_identity_sha256 != input.repository_identity_sha256
            || run.artifact_set_id != input.artifact_set_id
            || run.spec_id != input.spec_id
            || run.spec_revision != input.spec_revision
            || run.base_commit != input.base_commit
            || run.provider != input.provider
            || run.next_phase != input.phase
            || run.completed_phases != input.completed_phases
            || run.workspace_state_sha256 != input.expected_workspace_state_sha256
            || (!first_approved_phase
                && (run.approval_digest.as_deref() != Some(input.approval_digest)
                    || run.last_result_sha256 != input.previous_result_sha256))
            || (first_approved_phase && input.previous_result_sha256 != input.approval_digest)
            || !matches!(
                run.status.as_str(),
                "waiting_approval" | "ready_for_phase" | "blocked"
            )
        {
            return Err(StoreError::InvalidCommand(
                "remote phase request is stale or does not match durable state".into(),
            ));
        }
        if first_approved_phase {
            sqlx::query(
                "UPDATE sdd_remote_worker_runs SET approval_digest = ?,
                 last_result_sha256 = ?, status = 'running', blocker = NULL, updated_at = ?
                 WHERE run_id = ?",
            )
            .bind(input.approval_digest)
            .bind(input.approval_digest)
            .bind(&at)
            .bind(input.run_id)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                "UPDATE sdd_remote_worker_runs SET status = 'running', blocker = NULL,
                 updated_at = ? WHERE run_id = ?",
            )
            .bind(&at)
            .bind(input.run_id)
            .execute(&mut *tx)
            .await?;
        }
        insert_remote_request(
            &mut tx,
            input.request_id,
            input.request_sha256,
            input.run_id,
            "phase",
            Some(input.phase),
            &at,
        )
        .await?;
        tx.commit().await?;
        Ok(RemoteWorkerReservation::Started)
    }

    /// Reserve one Ready-state delivery action. Phase execution is over at
    /// this point: delivery updates only its own request ledger and the
    /// authoritative workspace hash, never the run's Ready status.
    pub async fn sdd_remote_worker_reserve_delivery(
        &self,
        input: ReserveRemoteDelivery<'_>,
    ) -> Result<RemoteWorkerReservation> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        if let Some(existing) = remote_request(&mut tx, input.request_id).await? {
            let reservation = replay_or_recover(existing, input.request_sha256, input.run_id)?;
            tx.commit().await?;
            return Ok(reservation);
        }
        let run: Option<RemoteWorkerRunRecord> =
            sqlx::query_as("SELECT * FROM sdd_remote_worker_runs WHERE run_id = ?")
                .bind(input.run_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(run) = run else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if run.host_id != input.host_id
            || run.repository_identity_sha256 != input.repository_identity_sha256
            || run.artifact_set_id != input.artifact_set_id
            || run.spec_id != input.spec_id
            || run.spec_revision != input.spec_revision
            || run.base_commit != input.base_commit
            || run.approval_digest.as_deref() != Some(input.approval_digest)
            || run.status != "ready"
        {
            return Err(StoreError::InvalidCommand(
                "remote delivery request does not match a durable Ready run".into(),
            ));
        }
        let previous_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sdd_remote_worker_requests
             WHERE run_id = ? AND operation = 'delivery' AND phase = ?",
        )
        .bind(input.run_id)
        .bind(input.preview_digest)
        .fetch_one(&mut *tx)
        .await?;
        if previous_count == 0 && run.workspace_state_sha256 != input.initial_workspace_state_sha256
        {
            return Err(StoreError::InvalidCommand(
                "remote delivery preview workspace is stale".into(),
            ));
        }
        for dependency in input.dependencies {
            let completed: i64 = sqlx::query_scalar(
                "SELECT EXISTS(
                   SELECT 1 FROM sdd_remote_worker_requests
                   WHERE run_id = ? AND operation = 'delivery' AND phase = ?
                     AND attempt_path = ? AND stage = 'completed'
                     AND json_extract(response_json, '$.status') = 'succeeded'
                 )",
            )
            .bind(input.run_id)
            .bind(input.preview_digest)
            .bind(dependency)
            .fetch_one(&mut *tx)
            .await?;
            if completed == 0 {
                return Err(StoreError::InvalidCommand(format!(
                    "remote delivery dependency {dependency} has not succeeded"
                )));
            }
        }
        sqlx::query(
            "INSERT INTO sdd_remote_worker_requests
             (request_id, request_sha256, run_id, operation, phase, stage, attempt_path,
              response_json, created_at, updated_at)
             VALUES (?, ?, ?, 'delivery', ?, 'running', ?, NULL, ?, ?)",
        )
        .bind(input.request_id)
        .bind(input.request_sha256)
        .bind(input.run_id)
        .bind(input.preview_digest)
        .bind(input.action_id)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(RemoteWorkerReservation::Started)
    }

    pub async fn sdd_remote_worker_mark_stage(
        &self,
        request_id: &str,
        request_sha256: &str,
        expected_stages: &[&str],
        stage: &str,
        attempt_path: Option<&str>,
    ) -> Result<()> {
        if expected_stages.is_empty() {
            return Err(StoreError::InvalidCommand(
                "expected remote worker stages are required".into(),
            ));
        }
        let at = now()?;
        let placeholders = std::iter::repeat_n("?", expected_stages.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE sdd_remote_worker_requests SET stage = ?, attempt_path = ?, updated_at = ?
             WHERE request_id = ? AND request_sha256 = ? AND response_json IS NULL
               AND stage IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql)
            .bind(stage)
            .bind(attempt_path)
            .bind(&at)
            .bind(request_id)
            .bind(request_sha256);
        for expected in expected_stages {
            query = query.bind(expected);
        }
        if query.execute(&self.pool).await?.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote worker request stage changed".into(),
            ));
        }
        Ok(())
    }

    pub async fn sdd_remote_worker_complete_authoring(
        &self,
        request_id: &str,
        request_sha256: &str,
        run_id: &str,
        workspace_state_sha256: &str,
        response_json: &str,
    ) -> Result<()> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        complete_request(&mut tx, request_id, request_sha256, response_json, &at).await?;
        let updated = sqlx::query(
            "UPDATE sdd_remote_worker_runs SET spec_revision = 2,
             workspace_state_sha256 = ?, status = 'waiting_approval', blocker = NULL,
             updated_at = ? WHERE run_id = ? AND status = 'authoring'",
        )
        .bind(workspace_state_sha256)
        .bind(&at)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote authoring run is not publishable".into(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn sdd_remote_worker_complete_phase(
        &self,
        request_id: &str,
        request_sha256: &str,
        run_id: &str,
        next_phase: &str,
        completed_phases: i64,
        workspace_state_sha256: &str,
        last_result_sha256: &str,
        ready: bool,
        response_json: &str,
    ) -> Result<()> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        complete_request(&mut tx, request_id, request_sha256, response_json, &at).await?;
        let updated = sqlx::query(
            "UPDATE sdd_remote_worker_runs SET next_phase = ?, completed_phases = ?,
             workspace_state_sha256 = ?, last_result_sha256 = ?, status = ?, blocker = NULL,
             updated_at = ? WHERE run_id = ? AND status = 'running'",
        )
        .bind(next_phase)
        .bind(completed_phases)
        .bind(workspace_state_sha256)
        .bind(last_result_sha256)
        .bind(if ready { "ready" } else { "ready_for_phase" })
        .bind(&at)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote phase run is not publishable".into(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn sdd_remote_worker_complete_failure(
        &self,
        request_id: &str,
        request_sha256: &str,
        run_id: &str,
        blocker: &str,
        response_json: &str,
    ) -> Result<()> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        complete_request(&mut tx, request_id, request_sha256, response_json, &at).await?;
        sqlx::query(
            "UPDATE sdd_remote_worker_runs SET status = 'blocked', blocker = ?, updated_at = ?
             WHERE run_id = ?",
        )
        .bind(blocker)
        .bind(&at)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn sdd_remote_worker_complete_delivery(
        &self,
        request_id: &str,
        request_sha256: &str,
        run_id: &str,
        workspace_state_sha256: &str,
        response_json: &str,
    ) -> Result<()> {
        let at = now()?;
        let mut tx = self.begin_write().await?;
        complete_request(&mut tx, request_id, request_sha256, response_json, &at).await?;
        let updated = sqlx::query(
            "UPDATE sdd_remote_worker_runs SET workspace_state_sha256 = ?, updated_at = ?
             WHERE run_id = ? AND status = 'ready'",
        )
        .bind(workspace_state_sha256)
        .bind(&at)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "remote delivery cannot alter a non-Ready run".into(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }
}

async fn remote_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    request_id: &str,
) -> Result<Option<RemoteWorkerRequestRecord>> {
    Ok(
        sqlx::query_as("SELECT * FROM sdd_remote_worker_requests WHERE request_id = ?")
            .bind(request_id)
            .fetch_optional(&mut **tx)
            .await?,
    )
}

fn replay_or_recover(
    existing: RemoteWorkerRequestRecord,
    request_sha256: &str,
    run_id: &str,
) -> Result<RemoteWorkerReservation> {
    if existing.request_sha256 != request_sha256 || existing.run_id != run_id {
        return Err(StoreError::IdempotencyConflict(format!(
            "remote-worker:{}",
            existing.request_id
        )));
    }
    Ok(match existing.response_json.clone() {
        Some(response) => RemoteWorkerReservation::Replay(response),
        None => RemoteWorkerReservation::RecoveryRequired(Box::new(existing)),
    })
}

async fn insert_remote_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    request_id: &str,
    request_sha256: &str,
    run_id: &str,
    operation: &str,
    phase: Option<&str>,
    at: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO sdd_remote_worker_requests
         (request_id, request_sha256, run_id, operation, phase, stage, attempt_path,
          response_json, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 'reserved', NULL, NULL, ?, ?)",
    )
    .bind(request_id)
    .bind(request_sha256)
    .bind(run_id)
    .bind(operation)
    .bind(phase)
    .bind(at)
    .bind(at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn complete_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    request_id: &str,
    request_sha256: &str,
    response_json: &str,
    at: &str,
) -> Result<()> {
    let updated = sqlx::query(
        "UPDATE sdd_remote_worker_requests SET stage = 'completed', response_json = ?,
         updated_at = ? WHERE request_id = ? AND request_sha256 = ? AND response_json IS NULL",
    )
    .bind(response_json)
    .bind(at)
    .bind(request_id)
    .bind(request_sha256)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::InvalidCommand(
            "remote worker request is not publishable".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("worker.sqlite"))
            .await
            .unwrap();
        (directory, store)
    }

    fn authoring<'a>(
        request_id: &'a str,
        request_sha256: &'a str,
        run_id: &'a str,
        artifact_set_id: &'a str,
    ) -> ReserveRemoteAuthoring<'a> {
        ReserveRemoteAuthoring {
            request_id,
            request_sha256,
            run_id,
            host_id: "11111111-1111-4111-8111-111111111111",
            repository_identity_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            artifact_set_id,
            spec_id: "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            base_commit: "0123456789abcdef0123456789abcdef01234567",
            provider: "codex",
            authoritative_path: "/agentum/worktrees/repo/run/authoritative",
            branch_name: "agentum/spc-01arz3ndektsv4rrffq69g5fav-fixture",
            initial_workspace_state_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        }
    }

    fn delivery<'a>(
        request_id: &'a str,
        request_hash: &'a str,
        run_id: &'a str,
        artifact_set_id: &'a str,
        workspace: &'a str,
        action_id: &'a str,
        dependencies: &'a [String],
    ) -> ReserveRemoteDelivery<'a> {
        ReserveRemoteDelivery {
            request_id,
            request_sha256: request_hash,
            run_id,
            host_id: "11111111-1111-4111-8111-111111111111",
            repository_identity_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            artifact_set_id,
            spec_id: "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            spec_revision: 2,
            base_commit: "0123456789abcdef0123456789abcdef01234567",
            approval_digest: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            preview_digest: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            action_id,
            dependencies,
            initial_workspace_state_sha256: workspace,
        }
    }

    #[tokio::test]
    async fn lease_is_bounded_exclusive_and_releasable() {
        let (_directory, store) = store().await;
        assert!(
            store
                .sdd_remote_worker_acquire_lease("owner-a", "request-a", "not-a-time")
                .await
                .is_err()
        );
        let expires = (OffsetDateTime::now_utc() + time::Duration::minutes(1))
            .format(&Rfc3339)
            .unwrap();
        store
            .sdd_remote_worker_acquire_lease("owner-a", "request-a", &expires)
            .await
            .unwrap();
        assert!(
            store
                .sdd_remote_worker_acquire_lease("owner-b", "request-b", &expires)
                .await
                .is_err()
        );
        store
            .sdd_remote_worker_release_lease("owner-a")
            .await
            .unwrap();
        store
            .sdd_remote_worker_acquire_lease("owner-b", "request-b", &expires)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn authoring_is_idempotent_and_artifact_set_identity_is_immutable() {
        let (_directory, store) = store().await;
        let artifact_set = "01ARZ3NDEKTSV4RRFFQ69G5FAA";
        assert_eq!(
            store
                .sdd_remote_worker_reserve_authoring(authoring(
                    "author-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "request-hash-a",
                    "22222222-2222-4222-8222-222222222222",
                    artifact_set,
                ))
                .await
                .unwrap(),
            RemoteWorkerReservation::Started
        );
        assert!(matches!(
            store
                .sdd_remote_worker_reserve_authoring(authoring(
                    "author-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "request-hash-a",
                    "22222222-2222-4222-8222-222222222222",
                    artifact_set,
                ))
                .await
                .unwrap(),
            RemoteWorkerReservation::RecoveryRequired(_)
        ));
        assert!(
            store
                .sdd_remote_worker_reserve_authoring(authoring(
                    "author-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "different-hash",
                    "22222222-2222-4222-8222-222222222222",
                    artifact_set,
                ))
                .await
                .is_err()
        );
        store
            .sdd_remote_worker_complete_authoring(
                "author-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "request-hash-a",
                "22222222-2222-4222-8222-222222222222",
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "{\"status\":\"succeeded\"}",
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .sdd_remote_worker_reserve_authoring(authoring(
                    "author-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "request-hash-a",
                    "22222222-2222-4222-8222-222222222222",
                    artifact_set,
                ))
                .await
                .unwrap(),
            RemoteWorkerReservation::Replay(_)
        ));
        let run = store
            .sdd_remote_worker_run("22222222-2222-4222-8222-222222222222")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.artifact_set_id, artifact_set);
        assert_eq!(run.status, "waiting_approval");
    }

    #[tokio::test]
    async fn blocked_phase_has_an_explicit_new_request_retry_path() {
        let (_directory, store) = store().await;
        let run_id = "33333333-3333-4333-8333-333333333333";
        let artifact_set = "01ARZ3NDEKTSV4RRFFQ69G5FAB";
        store
            .sdd_remote_worker_reserve_authoring(authoring(
                "author-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "author-hash",
                run_id,
                artifact_set,
            ))
            .await
            .unwrap();
        store
            .sdd_remote_worker_complete_authoring(
                "author-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "author-hash",
                run_id,
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "{}",
            )
            .await
            .unwrap();
        let phase = |request_id| ReserveRemotePhase {
            request_id,
            request_sha256: request_id,
            run_id,
            host_id: "11111111-1111-4111-8111-111111111111",
            repository_identity_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            artifact_set_id: artifact_set,
            spec_id: "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            spec_revision: 2,
            base_commit: "0123456789abcdef0123456789abcdef01234567",
            provider: "codex",
            approval_digest: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            phase: "design",
            completed_phases: 0,
            expected_workspace_state_sha256: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            previous_result_sha256: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        };
        store
            .sdd_remote_worker_reserve_phase(phase("remote-first"))
            .await
            .unwrap();
        store
            .sdd_remote_worker_complete_failure(
                "remote-first",
                "remote-first",
                run_id,
                "provider failed",
                "{}",
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .sdd_remote_worker_reserve_phase(phase("remote-retry"))
                .await
                .unwrap(),
            RemoteWorkerReservation::Started
        );
    }

    #[tokio::test]
    async fn delivery_is_workspace_bound_recoverable_and_preserves_ready() {
        let (_directory, store) = store().await;
        let run_id = "44444444-4444-4444-8444-444444444444";
        let artifact_set = "01ARZ3NDEKTSV4RRFFQ69G5FAC";
        store
            .sdd_remote_worker_reserve_authoring(authoring(
                "author-cccccccccccccccccccccccccccccccc",
                "author-delivery-hash",
                run_id,
                artifact_set,
            ))
            .await
            .unwrap();
        store
            .sdd_remote_worker_complete_authoring(
                "author-cccccccccccccccccccccccccccccccc",
                "author-delivery-hash",
                run_id,
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "{}",
            )
            .await
            .unwrap();
        sqlx::query(
            "UPDATE sdd_remote_worker_runs SET status = 'ready', next_phase = 'ready',
             completed_phases = 5, approval_digest = ? WHERE run_id = ?",
        )
        .bind("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd")
        .bind(run_id)
        .execute(&store.pool)
        .await
        .unwrap();

        let no_dependencies = Vec::new();
        assert!(
            store
                .sdd_remote_worker_reserve_delivery(delivery(
                    "delivery-action-stale",
                    "delivery-hash-stale",
                    run_id,
                    artifact_set,
                    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                    "commit-action",
                    &no_dependencies,
                ))
                .await
                .is_err(),
            "the first delivery action must match the exact previewed Ready workspace"
        );
        assert_eq!(
            store
                .sdd_remote_worker_reserve_delivery(delivery(
                    "delivery-action-commit",
                    "delivery-hash-commit",
                    run_id,
                    artifact_set,
                    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                    "commit-action",
                    &no_dependencies,
                ))
                .await
                .unwrap(),
            RemoteWorkerReservation::Started
        );
        assert!(matches!(
            store
                .sdd_remote_worker_reserve_delivery(delivery(
                    "delivery-action-commit",
                    "delivery-hash-commit",
                    run_id,
                    artifact_set,
                    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                    "commit-action",
                    &no_dependencies,
                ))
                .await
                .unwrap(),
            RemoteWorkerReservation::RecoveryRequired(_)
        ));
        store
            .sdd_remote_worker_complete_delivery(
                "delivery-action-commit",
                "delivery-hash-commit",
                run_id,
                "abababababababababababababababababababababababababababababababab",
                r#"{"status":"succeeded"}"#,
            )
            .await
            .unwrap();
        let dependencies = vec!["commit-action".to_owned()];
        assert_eq!(
            store
                .sdd_remote_worker_reserve_delivery(delivery(
                    "delivery-action-push",
                    "delivery-hash-push",
                    run_id,
                    artifact_set,
                    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                    "push-action",
                    &dependencies,
                ))
                .await
                .unwrap(),
            RemoteWorkerReservation::Started
        );
        let ready = store.sdd_remote_worker_run(run_id).await.unwrap().unwrap();
        assert_eq!(ready.status, "ready");
        assert_eq!(
            ready.workspace_state_sha256,
            "abababababababababababababababababababababababababababababababab"
        );
    }
}
