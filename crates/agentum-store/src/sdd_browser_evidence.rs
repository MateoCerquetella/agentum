//! Durable, attempt-attributed browser verification evidence.
//!
//! Capture bytes are published outside SQLite into Agentum-owned immutable
//! content-addressed storage. These mutations atomically bind those bytes to a
//! live attempt capability, the current specification revision, durable events,
//! the realtime outbox, and idempotent command replay.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::sdd::{EventInsert, append_event, now};
use crate::{Result, Store, StoreError};

const CAPABILITY: &str = "browser_evidence.submit";
const MAX_EVIDENCE_PER_SUBMISSION: usize = 32;
const MAX_BLOBS_PER_SUBMISSION: usize = 96;
const MAX_BLOB_BYTES: i64 = 8 * 1024 * 1024;
const MAX_TOTAL_BYTES: i64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SddEvidenceBlobRecord {
    pub sha256: String,
    pub byte_length: i64,
    pub media_type: String,
    pub storage_relative_path: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SddBrowserEvidenceRecord {
    pub evidence_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub grant_id: String,
    pub spec_revision: i64,
    pub check_id: String,
    pub manifest_sha256: String,
    pub evidence: Value,
    pub status: String,
    pub submitted_by: String,
    pub captured_at: String,
    pub created_at: String,
    pub blobs: Vec<SddEvidenceBlobRecord>,
}

#[derive(Debug, Clone, FromRow)]
struct BrowserEvidenceRow {
    evidence_id: String,
    run_id: String,
    attempt_id: String,
    grant_id: String,
    spec_revision: i64,
    check_id: String,
    manifest_sha256: String,
    manifest_json: String,
    status: String,
    submitted_by: String,
    captured_at: String,
    created_at: String,
}

#[derive(Debug, Clone, FromRow)]
struct EvidenceBlobRow {
    evidence_id: String,
    sha256: String,
    byte_length: i64,
    media_type: String,
    storage_relative_path: String,
    role: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrowserGrantScope {
    schema_version: u32,
    run_id: String,
    attempt_id: String,
    spec_revision: i64,
    workspace_fingerprint: String,
    check_ids: Vec<String>,
    max_total_bytes: i64,
}

#[derive(Debug, Clone)]
pub struct IssueBrowserEvidenceGrantMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub attempt_id: &'a str,
    pub grant_id: &'a str,
    pub token_hash: &'a str,
    pub scope_json: &'a str,
    pub expires_at: &'a str,
    pub response_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct NewEvidenceBlob<'a> {
    pub sha256: &'a str,
    pub byte_length: i64,
    pub media_type: &'a str,
    pub storage_relative_path: &'a str,
}

#[derive(Debug, Clone)]
pub struct NewBrowserEvidence<'a> {
    pub evidence_id: &'a str,
    pub check_id: &'a str,
    pub manifest_sha256: &'a str,
    pub manifest_json: &'a str,
    pub status: &'a str,
    pub captured_at: &'a str,
}

#[derive(Debug, Clone)]
pub struct NewBrowserEvidenceBlobRef<'a> {
    pub evidence_id: &'a str,
    pub sha256: &'a str,
    pub role: &'a str,
}

#[derive(Debug, Clone)]
pub struct SubmitBrowserEvidenceMutation<'a> {
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub run_id: &'a str,
    pub expected_revision: i64,
    pub attempt_id: &'a str,
    pub grant_token_hash: &'a str,
    pub submitted_by: &'a str,
    pub evidence: &'a [NewBrowserEvidence<'a>],
    pub blobs: &'a [NewEvidenceBlob<'a>],
    pub blob_refs: &'a [NewBrowserEvidenceBlobRef<'a>],
    pub response_json: &'a str,
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_storage_path(value: &str, sha256: &str) -> bool {
    value == format!("evidence/blobs/sha256/{}/{sha256}", &sha256[..2])
}

fn sha256(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

fn replay_revision(value: Value) -> Result<i64> {
    value
        .get("revision")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            StoreError::InvalidCommand("idempotent evidence response has no revision".into())
        })
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
    pub async fn sdd_issue_browser_evidence_grant(
        &self,
        input: IssueBrowserEvidenceGrantMutation<'_>,
    ) -> Result<i64> {
        if let Some(response) = self
            .sdd_idempotent_response(
                &format!("run:{}", input.run_id),
                input.request_id,
                input.request_hash,
            )
            .await?
        {
            return replay_revision(response);
        }
        if !valid_sha256(input.token_hash)
            || Uuid::parse_str(input.run_id).is_err()
            || Uuid::parse_str(input.attempt_id).is_err()
            || Uuid::parse_str(input.grant_id).is_err()
        {
            return Err(StoreError::InvalidCommand(
                "browser evidence grant token hash is invalid".into(),
            ));
        }
        let expires = OffsetDateTime::parse(input.expires_at, &Rfc3339)?;
        if expires <= OffsetDateTime::now_utc() {
            return Err(StoreError::InvalidCommand(
                "browser evidence grant already expired".into(),
            ));
        }
        let scope: BrowserGrantScope = serde_json::from_str(input.scope_json)?;
        if scope.schema_version != 1
            || scope.run_id != input.run_id
            || scope.attempt_id != input.attempt_id
            || scope.spec_revision < 1
            || !valid_sha256(&scope.workspace_fingerprint)
            || scope.check_ids.is_empty()
            || scope.check_ids.len() > MAX_EVIDENCE_PER_SUBMISSION
            || scope.check_ids.iter().any(|id| id.trim().is_empty())
            || scope.check_ids.iter().collect::<HashSet<_>>().len() != scope.check_ids.len()
            || scope.max_total_bytes != MAX_TOTAL_BYTES
        {
            return Err(StoreError::InvalidCommand(
                "browser evidence grant scope is invalid".into(),
            ));
        }

        let at = now()?;
        let mut tx = self.begin_write().await?;
        let run: Option<(String, String, String, String, String, i64, i64)> = sqlx::query_as(
            "SELECT r.repo_id, r.spec_id, r.phase, r.status, r.workspace_fingerprint,
                    r.aggregate_revision, s.current_revision
             FROM sdd_runs r JOIN sdd_specs s ON s.spec_id = r.spec_id
             WHERE r.run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, phase, status, fingerprint, current, spec_revision)) = run
        else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        if phase != "verification"
            || status != "running"
            || scope.spec_revision != spec_revision
            || scope.workspace_fingerprint != fingerprint
        {
            return Err(StoreError::InvalidCommand(
                "browser evidence grant requires the current running verification phase".into(),
            ));
        }
        let active_attempt: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sdd_attempts
             WHERE attempt_id = ? AND run_id = ? AND spec_revision = ?
               AND task_id IS NULL AND status = 'running')",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .bind(spec_revision)
        .fetch_one(&mut *tx)
        .await?;
        if active_attempt == 0 {
            return Err(StoreError::InvalidCommand(
                "browser evidence grant requires an active verification attempt".into(),
            ));
        }
        let live_grant: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sdd_capability_grants
             WHERE run_id = ? AND attempt_id = ? AND capability = ? AND revoked_at IS NULL)",
        )
        .bind(input.run_id)
        .bind(input.attempt_id)
        .bind(CAPABILITY)
        .fetch_one(&mut *tx)
        .await?;
        if live_grant != 0 {
            return Err(StoreError::InvalidCommand(
                "attempt already owns a live browser evidence grant".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO sdd_capability_grants
             (grant_id, run_id, attempt_id, capability, scope_json, token_hash, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(input.grant_id)
        .bind(input.run_id)
        .bind(input.attempt_id)
        .bind(CAPABILITY)
        .bind(input.scope_json)
        .bind(input.token_hash)
        .bind(input.expires_at)
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
                kind: "sdd.browser_evidence.granted",
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

    pub async fn sdd_submit_browser_evidence(
        &self,
        input: SubmitBrowserEvidenceMutation<'_>,
    ) -> Result<i64> {
        // Replay must precede checking the one-use grant: the first successful
        // submission consumes it by design.
        if let Some(response) = self
            .sdd_idempotent_response(
                &format!("run:{}", input.run_id),
                input.request_id,
                input.request_hash,
            )
            .await?
        {
            return replay_revision(response);
        }
        if input.evidence.is_empty()
            || input.evidence.len() > MAX_EVIDENCE_PER_SUBMISSION
            || input.blobs.is_empty()
            || input.blobs.len() > MAX_BLOBS_PER_SUBMISSION
            || !valid_sha256(input.grant_token_hash)
        {
            return Err(StoreError::InvalidCommand(
                "browser evidence submission is empty or exceeds its bound".into(),
            ));
        }
        let evidence_ids: HashSet<_> = input.evidence.iter().map(|item| item.evidence_id).collect();
        let check_ids: HashSet<_> = input.evidence.iter().map(|item| item.check_id).collect();
        if evidence_ids.len() != input.evidence.len()
            || check_ids.len() != input.evidence.len()
            || Uuid::parse_str(input.run_id).is_err()
            || Uuid::parse_str(input.attempt_id).is_err()
            || input
                .evidence
                .iter()
                .any(|item| Uuid::parse_str(item.evidence_id).is_err())
        {
            return Err(StoreError::InvalidCommand(
                "browser evidence identities and check ids must be unique".into(),
            ));
        }
        let mut total_bytes = 0_i64;
        let mut blob_by_hash = HashMap::new();
        for blob in input.blobs {
            if !valid_sha256(blob.sha256)
                || blob.byte_length <= 0
                || blob.byte_length > MAX_BLOB_BYTES
                || blob.media_type.is_empty()
                || !valid_storage_path(blob.storage_relative_path, blob.sha256)
                || blob_by_hash.insert(blob.sha256, blob).is_some()
            {
                return Err(StoreError::InvalidCommand(
                    "browser evidence blob metadata is invalid".into(),
                ));
            }
            total_bytes = total_bytes.saturating_add(blob.byte_length);
        }
        if total_bytes > MAX_TOTAL_BYTES {
            return Err(StoreError::InvalidCommand(
                "browser evidence blobs exceed the total byte bound".into(),
            ));
        }
        let mut refs_per_evidence: HashMap<&str, HashMap<&str, Vec<&str>>> = HashMap::new();
        let mut unique_refs = HashSet::new();
        for reference in input.blob_refs {
            if !evidence_ids.contains(reference.evidence_id)
                || !blob_by_hash.contains_key(reference.sha256)
                || !matches!(
                    reference.role,
                    "capture" | "console_transcript" | "network_transcript"
                )
                || !unique_refs.insert((reference.evidence_id, reference.sha256, reference.role))
            {
                return Err(StoreError::InvalidCommand(
                    "browser evidence blob reference is invalid".into(),
                ));
            }
            refs_per_evidence
                .entry(reference.evidence_id)
                .or_default()
                .entry(reference.role)
                .or_default()
                .push(reference.sha256);
        }
        if evidence_ids.iter().any(|id| {
            let roles = refs_per_evidence.get(id);
            roles
                .and_then(|roles| roles.get("capture"))
                .map(Vec::len)
                .unwrap_or_default()
                < 1
                || roles
                    .and_then(|roles| roles.get("console_transcript"))
                    .map(Vec::len)
                    .unwrap_or_default()
                    != 1
                || roles
                    .and_then(|roles| roles.get("network_transcript"))
                    .map(Vec::len)
                    .unwrap_or_default()
                    != 1
        }) {
            return Err(StoreError::InvalidCommand(
                "each browser evidence record requires capture and diagnostic references".into(),
            ));
        }

        let at = now()?;
        let mut tx = self.begin_write().await?;
        let run: Option<(String, String, String, String, String, i64, i64)> = sqlx::query_as(
            "SELECT r.repo_id, r.spec_id, r.phase, r.status, r.workspace_fingerprint,
                    r.aggregate_revision, s.current_revision
             FROM sdd_runs r JOIN sdd_specs s ON s.spec_id = r.spec_id
             WHERE r.run_id = ?",
        )
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((repo_id, spec_id, phase, status, fingerprint, current, spec_revision)) = run
        else {
            return Err(StoreError::NotFound(input.run_id.into()));
        };
        if current != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                current,
            });
        }
        if phase != "verification" || status != "running" {
            return Err(StoreError::InvalidCommand(
                "browser evidence can be submitted only during running verification".into(),
            ));
        }
        let grant: Option<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT grant_id, scope_json, expires_at, capability, attempt_id
             FROM sdd_capability_grants
             WHERE token_hash = ? AND run_id = ? AND revoked_at IS NULL",
        )
        .bind(input.grant_token_hash)
        .bind(input.run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((grant_id, scope_json, expires_at, capability, grant_attempt_id)) = grant else {
            return Err(StoreError::InvalidCommand(
                "browser evidence capability is missing, expired, or revoked".into(),
            ));
        };
        if capability != CAPABILITY
            || grant_attempt_id != input.attempt_id
            || OffsetDateTime::parse(&expires_at, &Rfc3339)? <= OffsetDateTime::now_utc()
        {
            return Err(StoreError::InvalidCommand(
                "browser evidence capability does not authorize this attempt".into(),
            ));
        }
        let scope: BrowserGrantScope = serde_json::from_str(&scope_json)?;
        let expected_checks: HashSet<_> = scope.check_ids.iter().map(String::as_str).collect();
        if scope.run_id != input.run_id
            || scope.attempt_id != input.attempt_id
            || scope.spec_revision != spec_revision
            || scope.workspace_fingerprint != fingerprint
            || scope.max_total_bytes != MAX_TOTAL_BYTES
            || expected_checks != check_ids
        {
            return Err(StoreError::InvalidCommand(
                "browser evidence does not match its immutable capability scope".into(),
            ));
        }
        let active_attempt: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sdd_attempts
             WHERE attempt_id = ? AND run_id = ? AND spec_revision = ?
               AND task_id IS NULL AND status = 'running')",
        )
        .bind(input.attempt_id)
        .bind(input.run_id)
        .bind(spec_revision)
        .fetch_one(&mut *tx)
        .await?;
        if active_attempt == 0 {
            return Err(StoreError::InvalidCommand(
                "browser evidence attempt is no longer active".into(),
            ));
        }
        let expected_submitter = format!("agentum:browser-driver:{}", input.attempt_id);
        if input.submitted_by != expected_submitter {
            return Err(StoreError::InvalidCommand(
                "browser evidence submitter is not the granted attempt driver".into(),
            ));
        }

        for evidence in input.evidence {
            if !valid_sha256(evidence.manifest_sha256)
                || sha256(evidence.manifest_json.as_bytes()) != evidence.manifest_sha256
                || !matches!(evidence.status, "passed" | "failed")
                || OffsetDateTime::parse(evidence.captured_at, &Rfc3339).is_err()
            {
                return Err(StoreError::InvalidCommand(
                    "browser evidence manifest metadata is invalid".into(),
                ));
            }
            let manifest: Value = serde_json::from_str(evidence.manifest_json)?;
            let roles = refs_per_evidence
                .get(evidence.evidence_id)
                .expect("role completeness validated above");
            let capture_refs = roles["capture"].iter().copied().collect::<HashSet<_>>();
            let manifest_captures = manifest
                .get("captures")
                .and_then(Value::as_array)
                .map(|captures| {
                    captures
                        .iter()
                        .filter_map(|capture| capture.get("sha256").and_then(Value::as_str))
                        .collect::<HashSet<_>>()
                })
                .unwrap_or_default();
            let console_hash = manifest
                .get("console")
                .and_then(|console| console.get("transcriptSha256"))
                .and_then(Value::as_str);
            let network_hash = manifest
                .get("network")
                .and_then(|network| network.get("transcriptSha256"))
                .and_then(Value::as_str);
            if manifest.get("schemaVersion").and_then(Value::as_u64) != Some(1)
                || manifest.get("evidenceId").and_then(Value::as_str) != Some(evidence.evidence_id)
                || manifest.get("runId").and_then(Value::as_str) != Some(input.run_id)
                || manifest.get("attemptId").and_then(Value::as_str) != Some(input.attempt_id)
                || manifest.get("checkId").and_then(Value::as_str) != Some(evidence.check_id)
                || manifest.get("specRevision").and_then(Value::as_i64) != Some(spec_revision)
                || manifest.get("workspaceFingerprint").and_then(Value::as_str)
                    != Some(fingerprint.as_str())
                || manifest_captures != capture_refs
                || console_hash != roles["console_transcript"].first().copied()
                || network_hash != roles["network_transcript"].first().copied()
            {
                return Err(StoreError::InvalidCommand(
                    "browser evidence manifest identity is not bound to the run attempt".into(),
                ));
            }
        }

        for blob in input.blobs {
            sqlx::query(
                "INSERT INTO sdd_evidence_blobs
                 (sha256, byte_length, media_type, storage_relative_path, created_at)
                 VALUES (?, ?, ?, ?, ?) ON CONFLICT(sha256) DO NOTHING",
            )
            .bind(blob.sha256)
            .bind(blob.byte_length)
            .bind(blob.media_type)
            .bind(blob.storage_relative_path)
            .bind(&at)
            .execute(&mut *tx)
            .await?;
            let stored: (i64, String, String) = sqlx::query_as(
                "SELECT byte_length, media_type, storage_relative_path
                 FROM sdd_evidence_blobs WHERE sha256 = ?",
            )
            .bind(blob.sha256)
            .fetch_one(&mut *tx)
            .await?;
            if stored
                != (
                    blob.byte_length,
                    blob.media_type.to_owned(),
                    blob.storage_relative_path.to_owned(),
                )
            {
                return Err(StoreError::InvalidCommand(
                    "content-addressed evidence blob metadata collided".into(),
                ));
            }
        }
        for evidence in input.evidence {
            sqlx::query(
                "INSERT INTO sdd_browser_evidence
                 (evidence_id, run_id, attempt_id, grant_id, spec_revision, check_id,
                  manifest_sha256, manifest_json, status, submitted_by, captured_at, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(evidence.evidence_id)
            .bind(input.run_id)
            .bind(input.attempt_id)
            .bind(&grant_id)
            .bind(spec_revision)
            .bind(evidence.check_id)
            .bind(evidence.manifest_sha256)
            .bind(evidence.manifest_json)
            .bind(evidence.status)
            .bind(input.submitted_by)
            .bind(evidence.captured_at)
            .bind(&at)
            .execute(&mut *tx)
            .await?;
        }
        for reference in input.blob_refs {
            sqlx::query(
                "INSERT INTO sdd_browser_evidence_blobs (evidence_id, sha256, role)
                 VALUES (?, ?, ?)",
            )
            .bind(reference.evidence_id)
            .bind(reference.sha256)
            .bind(reference.role)
            .execute(&mut *tx)
            .await?;
        }
        let revoked = sqlx::query(
            "UPDATE sdd_capability_grants SET revoked_at = ?
             WHERE grant_id = ? AND revoked_at IS NULL",
        )
        .bind(&at)
        .bind(&grant_id)
        .execute(&mut *tx)
        .await?;
        if revoked.rows_affected() != 1 {
            return Err(StoreError::InvalidCommand(
                "browser evidence capability was already consumed".into(),
            ));
        }
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
                kind: "sdd.browser_evidence.submitted",
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

    pub async fn sdd_browser_evidence(
        &self,
        run_id: &str,
    ) -> Result<Vec<SddBrowserEvidenceRecord>> {
        let rows: Vec<BrowserEvidenceRow> = sqlx::query_as(
            "SELECT e.* FROM sdd_browser_evidence e
             JOIN sdd_runs r ON r.run_id = e.run_id
             JOIN sdd_specs s ON s.spec_id = r.spec_id
             WHERE e.run_id = ? AND e.spec_revision = s.current_revision
             ORDER BY e.captured_at, e.evidence_id",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?;
        let blob_rows: Vec<EvidenceBlobRow> = sqlx::query_as(
            "SELECT x.evidence_id, b.sha256, b.byte_length, b.media_type,
                    b.storage_relative_path, x.role
             FROM sdd_browser_evidence_blobs x
             JOIN sdd_evidence_blobs b ON b.sha256 = x.sha256
             JOIN sdd_browser_evidence e ON e.evidence_id = x.evidence_id
             WHERE e.run_id = ? ORDER BY x.evidence_id, x.role, b.sha256",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?;
        let mut blobs: HashMap<String, Vec<SddEvidenceBlobRecord>> = HashMap::new();
        for row in blob_rows {
            blobs
                .entry(row.evidence_id)
                .or_default()
                .push(SddEvidenceBlobRecord {
                    sha256: row.sha256,
                    byte_length: row.byte_length,
                    media_type: row.media_type,
                    storage_relative_path: row.storage_relative_path,
                    role: row.role,
                });
        }
        rows.into_iter()
            .map(|row| {
                Ok(SddBrowserEvidenceRecord {
                    evidence_id: row.evidence_id.clone(),
                    run_id: row.run_id,
                    attempt_id: row.attempt_id,
                    grant_id: row.grant_id,
                    spec_revision: row.spec_revision,
                    check_id: row.check_id,
                    manifest_sha256: row.manifest_sha256,
                    evidence: serde_json::from_str(&row.manifest_json)?,
                    status: row.status,
                    submitted_by: row.submitted_by,
                    captured_at: row.captured_at,
                    created_at: row.created_at,
                    blobs: blobs.remove(&row.evidence_id).unwrap_or_default(),
                })
            })
            .collect()
    }

    pub async fn sdd_browser_evidence_manifest_hashes(&self, run_id: &str) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT e.manifest_sha256 FROM sdd_browser_evidence e
             JOIN sdd_runs r ON r.run_id = e.run_id
             JOIN sdd_specs s ON s.spec_id = r.spec_id
             WHERE e.run_id = ? AND e.spec_revision = s.current_revision
             ORDER BY e.manifest_sha256",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn sdd_browser_evidence_blob(
        &self,
        run_id: &str,
        evidence_id: &str,
        sha256: &str,
    ) -> Result<Option<SddEvidenceBlobRecord>> {
        let row: Option<EvidenceBlobRow> = sqlx::query_as(
            "SELECT x.evidence_id, b.sha256, b.byte_length, b.media_type,
                    b.storage_relative_path, x.role
             FROM sdd_browser_evidence_blobs x
             JOIN sdd_evidence_blobs b ON b.sha256 = x.sha256
             JOIN sdd_browser_evidence e ON e.evidence_id = x.evidence_id
             WHERE e.run_id = ? AND e.evidence_id = ? AND b.sha256 = ?",
        )
        .bind(run_id)
        .bind(evidence_id)
        .bind(sha256)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| SddEvidenceBlobRecord {
            sha256: row.sha256,
            byte_length: row.byte_length,
            media_type: row.media_type,
            storage_relative_path: row.storage_relative_path,
            role: row.role,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdd::TransitionMutation;
    use crate::sdd_runtime::FailAttemptMutation;

    const RUN_ID: &str = "10000000-0000-4000-8000-000000000001";
    const ATTEMPT_ID: &str = "20000000-0000-4000-8000-000000000002";
    const GRANT_ID: &str = "30000000-0000-4000-8000-000000000003";
    const EVIDENCE_ID: &str = "40000000-0000-4000-8000-000000000004";

    async fn fixture() -> Store {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("evidence.sqlite");
        std::mem::forget(directory);
        let store = Store::open(&path).await.unwrap();
        let now = "2026-07-27T00:00:00Z";
        sqlx::query(
            "INSERT INTO sdd_specs
             (spec_id, spec_ulid, repo_id, title, slug, profile, control, provider,
              current_revision, aggregate_revision, created_at, updated_at)
             VALUES ('SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV', '01ARZ3NDEKTSV4RRFFQ69G5FAV',
              'repo-1', 'Evidence', 'evidence', 'standard', 'guarded', 'codex', 1, 1, ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(store.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sdd_runs
             (run_id, spec_id, repo_id, phase, status, aggregate_revision, base_ref,
              base_commit, branch_name, authoritative_path, workspace_fingerprint,
              policy_json, quarantined, created_at, updated_at)
             VALUES (?, 'SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV', 'repo-1', 'verification',
              'running', 1, 'HEAD', 'deadbeef', 'agentum/evidence', '/tmp/evidence', ?,
              '{}', 0, ?, ?)",
        )
        .bind(RUN_ID)
        .bind("f".repeat(64))
        .bind(now)
        .bind(now)
        .execute(store.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sdd_attempts
             (attempt_id, run_id, spec_revision, provider, isolated_path, status,
              session_identity, started_at)
             VALUES (?, ?, 1, 'codex', '/tmp/attempt', 'running', 'session:evidence', ?)",
        )
        .bind(ATTEMPT_ID)
        .bind(RUN_ID)
        .bind(now)
        .execute(store.pool())
        .await
        .unwrap();
        store
    }

    async fn grant(store: &Store) -> i64 {
        let scope = serde_json::json!({
            "schemaVersion": 1,
            "runId": RUN_ID,
            "attemptId": ATTEMPT_ID,
            "specRevision": 1,
            "workspaceFingerprint": "f".repeat(64),
            "checkIds": ["browser-check"],
            "maxTotalBytes": 16 * 1024 * 1024
        })
        .to_string();
        store
            .sdd_issue_browser_evidence_grant(IssueBrowserEvidenceGrantMutation {
                request_id: "grant-request",
                request_hash: "grant-request-hash",
                run_id: RUN_ID,
                expected_revision: 1,
                attempt_id: ATTEMPT_ID,
                grant_id: GRANT_ID,
                token_hash: &"1".repeat(64),
                scope_json: &scope,
                expires_at: "2099-07-27T00:00:00Z",
                response_json: r#"{"revision":2}"#,
            })
            .await
            .unwrap()
    }

    async fn grant_is_revoked(store: &Store) -> bool {
        sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM sdd_capability_grants
             WHERE grant_id = ? AND revoked_at IS NOT NULL)",
        )
        .bind(GRANT_ID)
        .fetch_one(store.pool())
        .await
        .unwrap()
            == 1
    }

    #[tokio::test]
    async fn submission_hash_roles_and_consumed_grant_replay_are_enforced() {
        let store = fixture().await;
        assert_eq!(grant(&store).await, 2);
        let hashes = ["a".repeat(64), "b".repeat(64), "c".repeat(64)];
        let manifest = serde_json::json!({
            "schemaVersion": 1,
            "evidenceId": EVIDENCE_ID,
            "runId": RUN_ID,
            "attemptId": ATTEMPT_ID,
            "checkId": "browser-check",
            "specRevision": 1,
            "workspaceFingerprint": "f".repeat(64),
            "captures": [{ "sha256": hashes[0] }],
            "console": { "transcriptSha256": hashes[1] },
            "network": { "transcriptSha256": hashes[2] }
        })
        .to_string();
        let manifest_hash = sha256(manifest.as_bytes());
        let storage_paths = hashes
            .iter()
            .map(|hash| format!("evidence/blobs/sha256/{}/{hash}", &hash[..2]))
            .collect::<Vec<_>>();
        let blobs = hashes
            .iter()
            .zip(&storage_paths)
            .map(|(hash, storage_relative_path)| NewEvidenceBlob {
                sha256: hash,
                byte_length: 10,
                media_type: "application/json",
                storage_relative_path,
            })
            .collect::<Vec<_>>();
        let evidence = [NewBrowserEvidence {
            evidence_id: EVIDENCE_ID,
            check_id: "browser-check",
            manifest_sha256: &manifest_hash,
            manifest_json: &manifest,
            status: "passed",
            captured_at: "2026-07-27T00:00:01Z",
        }];
        let refs = [
            NewBrowserEvidenceBlobRef {
                evidence_id: EVIDENCE_ID,
                sha256: &hashes[0],
                role: "capture",
            },
            NewBrowserEvidenceBlobRef {
                evidence_id: EVIDENCE_ID,
                sha256: &hashes[1],
                role: "console_transcript",
            },
            NewBrowserEvidenceBlobRef {
                evidence_id: EVIDENCE_ID,
                sha256: &hashes[2],
                role: "network_transcript",
            },
        ];
        let token_hash = "1".repeat(64);
        let wrong_manifest_hash = "d".repeat(64);
        let bad_evidence = [NewBrowserEvidence {
            evidence_id: EVIDENCE_ID,
            check_id: "browser-check",
            manifest_sha256: &wrong_manifest_hash,
            manifest_json: &manifest,
            status: "passed",
            captured_at: "2026-07-27T00:00:01Z",
        }];
        let bad_hash = store
            .sdd_submit_browser_evidence(SubmitBrowserEvidenceMutation {
                request_id: "bad-manifest-request",
                request_hash: "bad-manifest-request-hash",
                run_id: RUN_ID,
                expected_revision: 2,
                attempt_id: ATTEMPT_ID,
                grant_token_hash: &token_hash,
                submitted_by: "agentum:browser-driver:20000000-0000-4000-8000-000000000002",
                evidence: &bad_evidence,
                blobs: &blobs,
                blob_refs: &refs,
                response_json: r#"{"revision":3}"#,
            })
            .await
            .unwrap_err();
        assert!(matches!(bad_hash, StoreError::InvalidCommand(_)));

        let bad_roles = [
            NewBrowserEvidenceBlobRef {
                evidence_id: EVIDENCE_ID,
                sha256: &hashes[0],
                role: "capture",
            },
            NewBrowserEvidenceBlobRef {
                evidence_id: EVIDENCE_ID,
                sha256: &hashes[1],
                role: "capture",
            },
            NewBrowserEvidenceBlobRef {
                evidence_id: EVIDENCE_ID,
                sha256: &hashes[2],
                role: "network_transcript",
            },
        ];
        let bad_roles = store
            .sdd_submit_browser_evidence(SubmitBrowserEvidenceMutation {
                request_id: "bad-role-request",
                request_hash: "bad-role-request-hash",
                run_id: RUN_ID,
                expected_revision: 2,
                attempt_id: ATTEMPT_ID,
                grant_token_hash: &token_hash,
                submitted_by: "agentum:browser-driver:20000000-0000-4000-8000-000000000002",
                evidence: &evidence,
                blobs: &blobs,
                blob_refs: &bad_roles,
                response_json: r#"{"revision":3}"#,
            })
            .await
            .unwrap_err();
        assert!(matches!(bad_roles, StoreError::InvalidCommand(_)));

        let input = || SubmitBrowserEvidenceMutation {
            request_id: "submit-request",
            request_hash: "submit-request-hash",
            run_id: RUN_ID,
            expected_revision: 2,
            attempt_id: ATTEMPT_ID,
            grant_token_hash: &token_hash,
            submitted_by: "agentum:browser-driver:20000000-0000-4000-8000-000000000002",
            evidence: &evidence,
            blobs: &blobs,
            blob_refs: &refs,
            response_json: r#"{"revision":3}"#,
        };
        assert_eq!(store.sdd_submit_browser_evidence(input()).await.unwrap(), 3);
        assert!(grant_is_revoked(&store).await);
        // Replay succeeds even though the first call consumed the one-use grant.
        assert_eq!(store.sdd_submit_browser_evidence(input()).await.unwrap(), 3);
        let records = store.sdd_browser_evidence(RUN_ID).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].blobs.len(), 3);
    }

    #[tokio::test]
    async fn restart_revokes_live_grants_without_inventing_evidence() {
        let store = fixture().await;
        grant(&store).await;
        assert_eq!(store.sdd_recover_interrupted_runs().await.unwrap(), 1);
        let revoked: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sdd_capability_grants
             WHERE grant_id = ? AND revoked_at IS NOT NULL)",
        )
        .bind(GRANT_ID)
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(revoked, 1);
        assert!(store.sdd_browser_evidence(RUN_ID).await.unwrap().is_empty());
        assert_eq!(
            store.sdd_get_run(RUN_ID).await.unwrap().unwrap().status,
            "paused"
        );
    }

    #[tokio::test]
    async fn lifecycle_pause_cancel_and_attempt_failure_revoke_live_grants() {
        for status in ["paused", "canceled"] {
            let store = fixture().await;
            assert_eq!(grant(&store).await, 2);
            store
                .sdd_transition(TransitionMutation {
                    request_id: status,
                    request_hash: status,
                    run_id: RUN_ID,
                    expected_revision: 2,
                    phase: "verification",
                    status,
                    blocker: None,
                    event_kind: "sdd.run.lifecycle_test",
                    response_json: "{}",
                })
                .await
                .unwrap();
            assert!(
                grant_is_revoked(&store).await,
                "{status} must revoke grants"
            );
        }

        let store = fixture().await;
        assert_eq!(grant(&store).await, 2);
        store
            .sdd_fail_attempt(FailAttemptMutation {
                request_id: "attempt-failed",
                request_hash: "attempt-failed",
                run_id: RUN_ID,
                expected_revision: 2,
                attempt_id: ATTEMPT_ID,
                status: "failed",
                blocker: "verification failed",
                event_kind: "sdd.attempt.lifecycle_test",
                response_json: "{}",
            })
            .await
            .unwrap();
        assert!(grant_is_revoked(&store).await);
    }
}
