//! Desktop coordinator for the restart-safe, sequential remote lifecycle.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use agentum_core::Host;
use agentum_store::StoreError;
use agentum_store::sdd_remote_projection::{
    PreparedRemoteBrowserEvidence, PreparedRemoteEvidenceBlob, PublishRemoteDesktopPhase,
    RemoteArtifactPayloadInput, RemoteDesktopReservation, ReserveRemoteDesktopPhase,
};
use agentum_store::sdd_runtime::VerificationResultInput;
use base64::Engine as _;
use uuid::Uuid;

use super::evidence::{
    BROWSER_EVIDENCE_SCHEMA_VERSION, BrowserCaptureKind, BrowserCaptureRef, BrowserEvidence,
    persist_blob,
};
use super::remote::{
    OpenSshRemoteSddTransport, RemoteLifecycleCheckpoint, RemoteLifecycleError,
    RemoteLifecyclePhase, RemoteLifecyclePlan, RemotePhaseStatus, RemoteSddClient,
    RemoteSddTransport, RemoteVerificationEvidence, SequentialRemoteLifecycle, build_request,
};
use super::sha256;
use crate::AppState;

type Client = Arc<dyn RemoteSddClient>;

#[derive(Default)]
struct ActiveRun {
    client: Option<Client>,
}

fn active_runs() -> &'static Mutex<HashMap<String, ActiveRun>> {
    static ACTIVE: OnceLock<Mutex<HashMap<String, ActiveRun>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
fn test_clients() -> &'static Mutex<HashMap<Uuid, Client>> {
    static CLIENTS: OnceLock<Mutex<HashMap<Uuid, Client>>> = OnceLock::new();
    CLIENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
pub(crate) fn register_test_remote_client(host_id: Uuid, client: Client) {
    test_clients()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(host_id, client);
}

pub(crate) fn client_for_host(host: Host) -> Result<Client, RemoteLifecycleError> {
    #[cfg(test)]
    if let Some(client) = test_clients()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&host.id)
        .cloned()
    {
        return Ok(client);
    }
    Ok(Arc::new(OpenSshRemoteSddTransport::new(host)?))
}

pub fn spawn(state: AppState, run_id: String) {
    {
        let mut active = active_runs()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active.contains_key(&run_id) {
            return;
        }
        // Ownership is reserved before the task is spawned. Database/host
        // lookups inside `drive` may await, but no second coordinator can pass
        // this boundary and start an untracked SSH process meanwhile.
        active.insert(run_id.clone(), ActiveRun::default());
    }
    tokio::spawn(async move {
        if let Err(error) = drive(&state, &run_id).await {
            tracing::error!(run_id, error = %error, "remote SDD lifecycle stopped");
        }
        active_runs()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&run_id);
    });
}

pub async fn cancel_run(state: &AppState, run_id: &str) -> bool {
    let Some(projection) = state.store.sdd_remote_run(run_id).await.ok().flatten() else {
        return false;
    };
    let Some(request_id) = projection.active_request_id else {
        return false;
    };
    active_runs()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(run_id)
        .and_then(|active| active.client.as_ref())
        .is_some_and(|client| RemoteSddTransport::cancel(client.as_ref(), &request_id))
}

async fn drive(state: &AppState, run_id: &str) -> Result<(), DriveError> {
    let projection = state
        .store
        .sdd_remote_run(run_id)
        .await?
        .ok_or_else(|| StoreError::NotFound(run_id.into()))?;
    let host_id = Uuid::parse_str(&projection.host_id)
        .map_err(|_| DriveError::Contract("remote projection host id is invalid".into()))?;
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| StoreError::NotFound(projection.host_id.clone()))?;
    let client = client_for_host(host)?;
    {
        let mut active = active_runs()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let reservation = active
            .get_mut(run_id)
            .ok_or_else(|| DriveError::Contract("remote run ownership was not reserved".into()))?;
        if reservation.client.is_some() {
            return Err(DriveError::Contract(
                "remote run already has an active transport".into(),
            ));
        }
        reservation.client = Some(Arc::clone(&client));
    }
    let coordinator = SequentialRemoteLifecycle;

    loop {
        let run = state
            .store
            .sdd_get_run(run_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(run_id.into()))?;
        if run.status != "queued" {
            return Ok(());
        }
        let projection = state
            .store
            .sdd_remote_run(run_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(run_id.into()))?;
        if projection.status != "queued" || projection.active_request_id.is_some() {
            return Ok(());
        }
        let plan: RemoteLifecyclePlan = serde_json::from_str(&projection.plan_json)
            .map_err(|_| DriveError::Contract("remote lifecycle plan is malformed".into()))?;
        let checkpoint: RemoteLifecycleCheckpoint =
            serde_json::from_str(&projection.checkpoint_json)
                .map_err(|_| DriveError::Contract("remote checkpoint is malformed".into()))?;
        if checkpoint.is_ready() {
            return Ok(());
        }
        let request = build_request(&plan, &checkpoint);
        let request_json = serde_json::to_string(&request)
            .map_err(|error| DriveError::Contract(error.to_string()))?;
        let request_sha256 = sha256(request_json.as_bytes());
        let attempt_id = Uuid::new_v4().to_string();
        let phase = phase_name(request.phase);
        let isolated_path = format!(
            "agentum+ssh://{}/{}/attempts/{attempt_id}",
            projection.host_id, run_id
        );
        let session_identity = format!("remote:{phase}:{}:{attempt_id}", request.request_id);
        let reserve_event = serde_json::json!({
            "runId": run_id,
            "revision": run.aggregate_revision + 1,
            "phase": phase,
            "status": "running",
            "remoteRequestId": request.request_id,
        })
        .to_string();
        let reservation = state
            .store
            .sdd_remote_reserve_phase(ReserveRemoteDesktopPhase {
                request_id: &request.request_id,
                request_sha256: &request_sha256,
                run_id,
                expected_revision: run.aggregate_revision,
                phase,
                request_json: &request_json,
                attempt_id: &attempt_id,
                provider: &plan.provider,
                isolated_path: &isolated_path,
                session_identity: &session_identity,
                response_json: &reserve_event,
            })
            .await?;
        let running_revision = match reservation {
            RemoteDesktopReservation::Started { revision } => revision,
            RemoteDesktopReservation::Replay {
                revision, status, ..
            } if status == "running" => revision,
            RemoteDesktopReservation::Replay { .. } => continue,
        };

        let advance = coordinator
            .advance(client.as_ref(), &plan, &checkpoint)
            .await;
        let (mut next_checkpoint, mut result) = match advance {
            Ok(advance) => (advance.checkpoint, advance.result),
            Err(error) => {
                let code = lifecycle_error_code(&error);
                let failure = super::remote::RemotePhaseResult {
                    schema_version: super::remote::REMOTE_SDD_SCHEMA_VERSION,
                    request_id: request.request_id.clone(),
                    phase: request.phase,
                    status: if matches!(error, RemoteLifecycleError::Canceled) {
                        RemotePhaseStatus::Canceled
                    } else {
                        RemotePhaseStatus::Failed
                    },
                    workspace_state_sha256: checkpoint.workspace_state_sha256.clone(),
                    artifact_set_sha256: checkpoint.last_result_sha256.clone(),
                    evidence_sha256: sha256(code.as_bytes()),
                    evidence_summary: None,
                    artifacts: Vec::new(),
                    error_code: Some(code.into()),
                };
                (checkpoint.clone(), failure)
            }
        };
        let browser_evidence = if result.status == RemotePhaseStatus::Succeeded
            && request.phase == RemoteLifecyclePhase::Verification
        {
            match prepare_browser_evidence(
                state,
                run_id,
                &run.workspace_fingerprint,
                plan.spec_revision,
                &attempt_id,
                result.evidence_summary.as_deref(),
            )
            .await
            {
                Ok(evidence) => evidence,
                Err(error) => {
                    let code = "remote_browser_evidence_invalid";
                    next_checkpoint = checkpoint.clone();
                    result = super::remote::RemotePhaseResult {
                        schema_version: super::remote::REMOTE_SDD_SCHEMA_VERSION,
                        request_id: request.request_id.clone(),
                        phase: request.phase,
                        status: RemotePhaseStatus::Failed,
                        workspace_state_sha256: checkpoint.workspace_state_sha256.clone(),
                        artifact_set_sha256: checkpoint.last_result_sha256.clone(),
                        evidence_sha256: sha256(error.to_string()),
                        evidence_summary: None,
                        artifacts: Vec::new(),
                        error_code: Some(code.into()),
                    };
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let response_json = serde_json::to_string(&result)
            .map_err(|error| DriveError::Contract(error.to_string()))?;
        let checkpoint_json = serde_json::to_string(&next_checkpoint)
            .map_err(|error| DriveError::Contract(error.to_string()))?;
        let artifacts = result
            .artifacts
            .iter()
            .map(|artifact| RemoteArtifactPayloadInput {
                kind: &artifact.kind,
                relative_path: &artifact.relative_path,
                content_sha256: &artifact.content_sha256,
                content: &artifact.content,
            })
            .collect::<Vec<_>>();
        let status = match result.status {
            RemotePhaseStatus::Succeeded => "succeeded",
            RemotePhaseStatus::Failed => "failed",
            RemotePhaseStatus::Canceled => "canceled",
        };
        let published = state
            .store
            .sdd_remote_publish_phase(PublishRemoteDesktopPhase {
                request_id: &request.request_id,
                request_sha256: &request_sha256,
                run_id,
                expected_revision: running_revision,
                phase,
                status,
                checkpoint_json: &checkpoint_json,
                artifacts: &artifacts,
                evidence_sha256: &result.evidence_sha256,
                evidence_summary: result.evidence_summary.as_deref(),
                browser_evidence: &browser_evidence,
                error_code: result.error_code.as_deref(),
                response_json: &response_json,
            })
            .await;
        if let Err(error) = published {
            if matches!(error, StoreError::StaleRevision { .. }) {
                let terminal = if result.status == RemotePhaseStatus::Canceled {
                    "canceled"
                } else {
                    "interrupted"
                };
                state
                    .store
                    .sdd_remote_abandon_request(
                        run_id,
                        &request.request_id,
                        terminal,
                        "desktop_cas_changed",
                    )
                    .await?;
                return Ok(());
            }
            return Err(error.into());
        }
        if result.status != RemotePhaseStatus::Succeeded || next_checkpoint.is_ready() {
            return Ok(());
        }
    }
}

async fn prepare_browser_evidence(
    state: &AppState,
    run_id: &str,
    workspace_fingerprint: &str,
    spec_revision: i64,
    attempt_id: &str,
    summary: Option<&str>,
) -> Result<Vec<PreparedRemoteBrowserEvidence>, DriveError> {
    let summary = summary
        .ok_or_else(|| DriveError::Contract("remote verification returned no evidence".into()))?;
    let evidence: RemoteVerificationEvidence =
        serde_json::from_str(summary).map_err(|error| DriveError::Contract(error.to_string()))?;
    if evidence.schema_version != super::remote::REMOTE_SDD_SCHEMA_VERSION {
        return Err(DriveError::Contract(
            "remote verification evidence schema is invalid".into(),
        ));
    }
    let plan_metadata = state
        .store
        .sdd_artifacts(run_id)
        .await?
        .into_iter()
        .filter(|artifact| artifact.kind == "plan" && artifact.spec_revision == spec_revision)
        .max_by_key(|artifact| artifact.revision)
        .ok_or_else(|| DriveError::Contract("remote run has no projected plan artifact".into()))?;
    let plan_payload = state
        .store
        .sdd_remote_artifact_payloads(run_id)
        .await?
        .into_iter()
        .find(|payload| payload.artifact_revision_id == plan_metadata.artifact_revision_id)
        .ok_or_else(|| DriveError::Contract("remote plan payload is missing".into()))?;
    let plan_artifact: agentum_core::sdd::PlanArtifact =
        serde_json::from_str(&plan_payload.content)
            .map_err(|error| DriveError::Contract(error.to_string()))?;
    let browser_checks = plan_artifact
        .tasks
        .iter()
        .flat_map(|task| task.browser_checks.iter())
        .collect::<Vec<_>>();
    if browser_checks.len() != evidence.browser_results.len() {
        return Err(DriveError::Contract(
            "remote browser results do not cover the plan".into(),
        ));
    }
    let command_offset = 1 + plan_artifact
        .tasks
        .iter()
        .map(|task| task.verification.len())
        .sum::<usize>();
    let mut total_bytes = 0_usize;
    let mut prepared = Vec::with_capacity(browser_checks.len());
    for (index, (check, remote)) in browser_checks
        .iter()
        .zip(evidence.browser_results.iter())
        .enumerate()
    {
        if remote.check_id != check.id
            || remote.status != "passed"
            || remote.output_excerpt.len() > 64 * 1024
            || !(0..=3_600_000).contains(&remote.duration_ms)
        {
            return Err(DriveError::Contract(
                "remote browser result identity or status is invalid".into(),
            ));
        }
        let mut blobs = Vec::with_capacity(remote.blobs.len());
        let mut capture_refs = Vec::new();
        let mut console_hash = None;
        let mut network_hash = None;
        for blob in &remote.blobs {
            if !matches!(
                blob.role.as_str(),
                "capture" | "console_transcript" | "network_transcript"
            ) || blob.content_base64.len() > 12 * 1024 * 1024
            {
                return Err(DriveError::Contract(
                    "remote browser blob role or encoding is invalid".into(),
                ));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&blob.content_base64)
                .map_err(|_| DriveError::Contract("remote browser blob is not base64".into()))?;
            total_bytes = total_bytes.saturating_add(bytes.len());
            if total_bytes > 16 * 1024 * 1024
                || bytes.len() as u64 != blob.byte_length
                || sha256(&bytes) != blob.sha256
            {
                return Err(DriveError::Contract(
                    "remote browser blob digest or bound is invalid".into(),
                ));
            }
            let stored = persist_blob(&bytes, &blob.media_type)
                .map_err(|error| DriveError::Contract(error.to_string()))?;
            if stored.sha256 != blob.sha256 || stored.byte_length != blob.byte_length as i64 {
                return Err(DriveError::Contract(
                    "persisted browser blob does not match remote evidence".into(),
                ));
            }
            match blob.role.as_str() {
                "capture" => capture_refs.push(BrowserCaptureRef {
                    kind: BrowserCaptureKind::Screenshot,
                    sha256: stored.sha256.clone(),
                    byte_length: stored.byte_length as u64,
                    media_type: stored.media_type.clone(),
                }),
                "console_transcript" if console_hash.is_none() => {
                    console_hash = Some(stored.sha256.clone());
                }
                "network_transcript" if network_hash.is_none() => {
                    network_hash = Some(stored.sha256.clone());
                }
                _ => {
                    return Err(DriveError::Contract(
                        "remote browser evidence has duplicate diagnostic roles".into(),
                    ));
                }
            }
            blobs.push(PreparedRemoteEvidenceBlob {
                sha256: stored.sha256,
                byte_length: stored.byte_length,
                media_type: stored.media_type,
                storage_relative_path: stored.storage_relative_path,
                role: blob.role.clone(),
            });
        }
        if capture_refs.is_empty()
            || console_hash.as_deref() != Some(remote.console.transcript_sha256.as_str())
            || network_hash.as_deref() != Some(remote.network.transcript_sha256.as_str())
        {
            return Err(DriveError::Contract(
                "remote browser evidence is missing capture or diagnostic blobs".into(),
            ));
        }
        let evidence_id = Uuid::new_v4().to_string();
        let manifest = BrowserEvidence {
            schema_version: BROWSER_EVIDENCE_SCHEMA_VERSION,
            evidence_id: evidence_id.clone(),
            run_id: run_id.into(),
            attempt_id: attempt_id.into(),
            check_id: check.id.clone(),
            spec_revision,
            captured_at: remote.captured_at.clone(),
            workspace_fingerprint: workspace_fingerprint.into(),
            target: remote.target.clone(),
            browser: remote.browser.clone(),
            captures: capture_refs,
            assertions: remote.assertions.clone(),
            console: remote.console.clone(),
            network: remote.network.clone(),
        };
        manifest
            .validate()
            .map_err(|error| DriveError::Contract(error.to_string()))?;
        let manifest_sha256 = manifest
            .digest()
            .map_err(|error| DriveError::Contract(error.to_string()))?;
        let manifest_json = serde_json::to_string(&manifest)
            .map_err(|error| DriveError::Contract(error.to_string()))?;
        prepared.push(PreparedRemoteBrowserEvidence {
            evidence_id,
            check_id: check.id.clone(),
            manifest_sha256: manifest_sha256.clone(),
            manifest_json,
            status: remote.status.clone(),
            captured_at: remote.captured_at.clone(),
            verification_result: VerificationResultInput {
                command_index: (command_offset + index) as i64,
                command_json: serde_json::json!({ "type": "browserCheck", "check": check })
                    .to_string(),
                status: "succeeded".into(),
                exit_code: None,
                output_hash: manifest_sha256,
                output_excerpt: remote.output_excerpt.clone(),
                duration_ms: remote.duration_ms,
            },
            blobs,
        });
    }
    Ok(prepared)
}

fn phase_name(phase: RemoteLifecyclePhase) -> &'static str {
    match phase {
        RemoteLifecyclePhase::Design => "design",
        RemoteLifecyclePhase::Planning => "planning",
        RemoteLifecyclePhase::Implementation => "implementation",
        RemoteLifecyclePhase::Verification => "verification",
        RemoteLifecyclePhase::Review => "review",
        RemoteLifecyclePhase::Ready => "ready",
    }
}

fn lifecycle_error_code(error: &RemoteLifecycleError) -> &'static str {
    match error {
        RemoteLifecycleError::Timeout => "remote_timeout",
        RemoteLifecycleError::Canceled => "remote_canceled",
        RemoteLifecycleError::OutputLimit(_) => "remote_output_limit",
        RemoteLifecycleError::MalformedFrame | RemoteLifecycleError::InvalidResult => {
            "remote_invalid_result"
        }
        RemoteLifecycleError::InvalidPlan | RemoteLifecycleError::InvalidCheckpoint => {
            "remote_contract_invalid"
        }
        RemoteLifecycleError::AlreadyReady => "remote_already_ready",
        RemoteLifecycleError::Transport(_) | RemoteLifecycleError::InvalidHost => {
            "remote_transport_failed"
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum DriveError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Remote(#[from] RemoteLifecycleError),
    #[error("{0}")]
    Contract(String),
}
