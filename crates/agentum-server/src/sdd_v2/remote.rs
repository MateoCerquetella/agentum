//! Sequential remote SDD lifecycle protocol.
//!
//! OpenSSH's ordinary remote-command mode ultimately sends one shell string.
//! That is not an acceptable authority boundary for typed SDD execution. This
//! module therefore defines the restartable coordinator and the fixed SSH
//! subsystem contract (`agentum-sdd-v1`). A production transport must speak
//! length-bounded JSON frames to that subsystem; it must not translate these
//! requests into generated remote shell commands.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agentum_core::{Host, HostKind};
use agentum_store::sdd_runtime::VerificationResultInput;
use agentum_tmux::ssh::{SshMux, is_mux_transport_error, ssh_subsystem_command};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use super::delivery::{DeliveryPreviewEnvelope, PreparedDeliveryAction};
use super::evidence::{
    BrowserAssertion, BrowserConsoleSummary, BrowserNetworkSummary, BrowserRuntime, BrowserTarget,
};

pub const REMOTE_SDD_SCHEMA_VERSION: u32 = 1;
pub const REMOTE_SDD_SSH_SUBSYSTEM: &str = "agentum-sdd-v1";
const MAX_REMOTE_TIMEOUT_MS: u64 = 60 * 60 * 1000;
const MAX_REMOTE_OUTPUT: usize = 8 * 1024 * 1024;
// Delivery previews can contain a bounded OpenSpec export. Keep the request
// bound aligned with the negotiated response bound while retaining explicit
// length framing (and never translating the payload into a shell command).
const MAX_REMOTE_REQUEST: usize = 8 * 1024 * 1024;
const MAX_REMOTE_STDERR: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteLifecyclePhase {
    Design,
    Planning,
    Implementation,
    Verification,
    Review,
    Ready,
}

impl RemoteLifecyclePhase {
    fn next(self) -> Option<Self> {
        match self {
            Self::Design => Some(Self::Planning),
            Self::Planning => Some(Self::Implementation),
            Self::Implementation => Some(Self::Verification),
            Self::Verification => Some(Self::Review),
            Self::Review => Some(Self::Ready),
            Self::Ready => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteLifecyclePlan {
    pub schema_version: u32,
    pub host_id: String,
    pub run_id: String,
    pub spec_id: String,
    pub spec_revision: i64,
    pub repository_identity_sha256: String,
    pub artifact_set_id: String,
    pub base_commit: String,
    pub provider: String,
    pub approval_digest: String,
    pub timeout_ms: u64,
    pub output_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteLifecycleCheckpoint {
    pub schema_version: u32,
    pub host_id: String,
    pub run_id: String,
    pub spec_revision: i64,
    pub approval_digest: String,
    pub next_phase: RemoteLifecyclePhase,
    pub completed_phases: u8,
    pub workspace_state_sha256: String,
    pub last_result_sha256: String,
}

impl RemoteLifecycleCheckpoint {
    pub fn initial(
        plan: &RemoteLifecyclePlan,
        workspace_state_sha256: String,
    ) -> Result<Self, RemoteLifecycleError> {
        validate_plan(plan)?;
        if !valid_sha256(&workspace_state_sha256) {
            return Err(RemoteLifecycleError::InvalidCheckpoint);
        }
        Ok(Self {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            host_id: plan.host_id.clone(),
            run_id: plan.run_id.clone(),
            spec_revision: plan.spec_revision,
            approval_digest: plan.approval_digest.clone(),
            next_phase: RemoteLifecyclePhase::Design,
            completed_phases: 0,
            workspace_state_sha256,
            last_result_sha256: plan.approval_digest.clone(),
        })
    }

    pub fn is_ready(&self) -> bool {
        self.next_phase == RemoteLifecyclePhase::Ready
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemotePhaseRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub host_id: String,
    pub run_id: String,
    pub spec_id: String,
    pub spec_revision: i64,
    pub phase: RemoteLifecyclePhase,
    pub repository_identity_sha256: String,
    pub artifact_set_id: String,
    pub base_commit: String,
    pub provider: String,
    pub expected_workspace_state_sha256: String,
    pub previous_result_sha256: String,
    pub approval_digest: String,
    pub timeout_ms: u64,
    pub output_limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemotePhaseStatus {
    Succeeded,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemotePhaseResult {
    pub schema_version: u32,
    pub request_id: String,
    pub phase: RemoteLifecyclePhase,
    pub status: RemotePhaseStatus,
    pub workspace_state_sha256: String,
    pub artifact_set_sha256: String,
    pub evidence_sha256: String,
    pub evidence_summary: Option<String>,
    pub artifacts: Vec<RemoteArtifactPayload>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteArtifactPayload {
    pub kind: String,
    pub relative_path: String,
    pub content_sha256: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteImplementationEvidence {
    pub schema_version: u32,
    pub request_id: String,
    pub spec_id: String,
    pub spec_revision: i64,
    pub tasks: Vec<RemoteTaskCompletionEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteTaskCompletionEvidence {
    pub task_id: String,
    pub patch_sha256: String,
    pub write_set_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteVerificationEvidence {
    pub schema_version: u32,
    pub command_results: Vec<VerificationResultInput>,
    pub browser_results: Vec<RemoteBrowserCheckResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteBrowserCheckResult {
    pub check_id: String,
    pub captured_at: String,
    pub status: String,
    pub duration_ms: i64,
    pub output_excerpt: String,
    pub target: BrowserTarget,
    pub browser: BrowserRuntime,
    pub assertions: Vec<BrowserAssertion>,
    pub console: BrowserConsoleSummary,
    pub network: BrowserNetworkSummary,
    pub blobs: Vec<RemoteBrowserBlob>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteBrowserBlob {
    pub sha256: String,
    pub byte_length: u64,
    pub media_type: String,
    pub role: String,
    pub content_base64: String,
}

/// Authoring is a separate operation because Standard + Guarded must return
/// the authored specification and stop for approval before design begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteAuthoringRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub host_id: String,
    pub run_id: String,
    pub spec_id: String,
    pub repository_identity_sha256: String,
    pub artifact_set_id: String,
    pub base_commit: String,
    pub provider: String,
    pub source_checkout: String,
    pub title: String,
    pub goal: String,
    pub timeout_ms: u64,
    pub output_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteAuthoringResult {
    pub schema_version: u32,
    pub request_id: String,
    pub run_id: String,
    pub spec_id: String,
    pub spec_revision: i64,
    pub status: RemotePhaseStatus,
    pub workspace_state_sha256: String,
    pub artifact_set_sha256: String,
    pub spec: Option<RemoteArtifactPayload>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteProbeRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub host_id: String,
    pub repository_identity_sha256: String,
    pub provider: String,
    pub base_ref: String,
    pub expected_worker_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteProbeResult {
    pub schema_version: u32,
    pub request_id: String,
    pub host_id: String,
    pub worker_version: String,
    pub repository_registered: bool,
    pub artifact_set_id: Option<String>,
    pub base_commit: Option<String>,
    pub provider_ready: bool,
    pub reason: Option<String>,
}

/// Side-effect-free inspection of a Ready host-owned authoritative worktree.
/// The path itself never crosses the SSH boundary; only its identity hash is
/// returned and bound into the delivery preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteDeliverySnapshotRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub host_id: String,
    pub run_id: String,
    pub spec_id: String,
    pub spec_revision: i64,
    pub repository_identity_sha256: String,
    pub artifact_set_id: String,
    pub base_commit: String,
    pub approval_digest: String,
    pub expected_workspace_state_sha256: String,
    pub openspec_destination: Option<String>,
    pub timeout_ms: u64,
    pub output_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteDeliverySnapshotResult {
    pub schema_version: u32,
    pub request_id: String,
    pub run_id: String,
    pub workspace_state_sha256: String,
    pub artifact_set_sha256: String,
    pub worktree_identity_sha256: String,
    pub branch_name: String,
    pub openspec_destination_exists: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteDeliveryActionStatus {
    Succeeded,
    Failed,
    SyncPending,
}

/// One hash-bound repository delivery action. `attempt` changes only after a
/// caller has acknowledged a failed or ambiguous durable result; retransmits
/// of the same attempt retain the same idempotency key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteDeliveryActionRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub host_id: String,
    pub run_id: String,
    pub spec_id: String,
    pub spec_revision: i64,
    pub repository_identity_sha256: String,
    pub artifact_set_id: String,
    pub base_commit: String,
    pub approval_digest: String,
    pub preview_digest: String,
    pub envelope: DeliveryPreviewEnvelope,
    pub action: PreparedDeliveryAction,
    pub attempt: i64,
    pub timeout_ms: u64,
    pub output_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteDeliveryActionResult {
    pub schema_version: u32,
    pub request_id: String,
    pub run_id: String,
    pub action_id: String,
    pub status: RemoteDeliveryActionStatus,
    pub result: serde_json::Value,
    pub workspace_state_sha256: String,
    pub artifact_set_sha256: String,
    pub error_code: Option<String>,
}

/// Closed client-to-worker frame contract for the `agentum-sdd-v1` OpenSSH
/// subsystem. The four-byte big-endian frame length is outside this JSON
/// value; no newline or shell framing is involved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RemoteClientFrame {
    Probe(RemoteProbeRequest),
    AuthorSpec(RemoteAuthoringRequest),
    ExecutePhase(RemotePhaseRequest),
    InspectDelivery(RemoteDeliverySnapshotRequest),
    ExecuteDeliveryAction(Box<RemoteDeliveryActionRequest>),
}

/// Closed worker-to-client frame contract for the `agentum-sdd-v1` OpenSSH
/// subsystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RemoteServerFrame {
    ProbeResult(RemoteProbeResult),
    AuthoringResult(RemoteAuthoringResult),
    PhaseResult(RemotePhaseResult),
    DeliverySnapshotResult(RemoteDeliverySnapshotResult),
    DeliveryActionResult(RemoteDeliveryActionResult),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteAdvance {
    /// Persist this value with aggregate CAS before issuing another request.
    /// On failed/canceled results it remains byte-for-byte unchanged.
    pub checkpoint: RemoteLifecycleCheckpoint,
    pub result: RemotePhaseResult,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RemoteLifecycleError {
    #[error("remote lifecycle plan is invalid")]
    InvalidPlan,
    #[error("remote lifecycle checkpoint is invalid or stale")]
    InvalidCheckpoint,
    #[error("remote lifecycle is already Ready")]
    AlreadyReady,
    #[error("remote subsystem transport failed: {0}")]
    Transport(String),
    #[error("remote subsystem request timed out")]
    Timeout,
    #[error("remote subsystem request was canceled")]
    Canceled,
    #[error("remote subsystem transport requires the matching SSH host")]
    InvalidHost,
    #[error("remote subsystem frame is malformed or truncated")]
    MalformedFrame,
    #[error("remote subsystem output exceeded its {0}-byte limit")]
    OutputLimit(usize),
    #[error("remote subsystem returned a malformed or mismatched result")]
    InvalidResult,
}

/// Transport implemented by the fixed SSH subsystem client. Taking an owned
/// request keeps cancellation/drop semantics explicit and makes idempotent
/// retry of the same `request_id` straightforward.
pub trait RemoteSddTransport: Send + Sync {
    fn execute(
        &self,
        request: RemotePhaseRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RemotePhaseResult, RemoteLifecycleError>> + Send + '_>>;

    /// Cancel a currently executing request. Implementations that cannot
    /// supervise a process tree remain fail-closed and return `false`.
    fn cancel(&self, _request_id: &str) -> bool {
        false
    }

    fn inspect_delivery(
        &self,
        _request: RemoteDeliverySnapshotRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<RemoteDeliverySnapshotResult, RemoteLifecycleError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async {
            Err(RemoteLifecycleError::Transport(
                "remote delivery transport is unavailable".into(),
            ))
        })
    }

    fn execute_delivery_action(
        &self,
        _request: RemoteDeliveryActionRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<RemoteDeliveryActionResult, RemoteLifecycleError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async {
            Err(RemoteLifecycleError::Transport(
                "remote delivery transport is unavailable".into(),
            ))
        })
    }
}

pub trait RemoteSddAuthoringTransport: Send + Sync {
    fn author(
        &self,
        request: RemoteAuthoringRequest,
    ) -> Pin<
        Box<dyn Future<Output = Result<RemoteAuthoringResult, RemoteLifecycleError>> + Send + '_>,
    >;

    fn cancel(&self, _request_id: &str) -> bool {
        false
    }
}

pub trait RemoteSddProbeTransport: Send + Sync {
    fn probe(
        &self,
        repository_identity_sha256: &str,
        provider: &str,
        base_ref: &str,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteProbeResult, RemoteLifecycleError>> + Send + '_>>;
}

pub trait RemoteSddClient:
    RemoteSddTransport + RemoteSddAuthoringTransport + RemoteSddProbeTransport
{
}

impl<T> RemoteSddClient for T where
    T: RemoteSddTransport + RemoteSddAuthoringTransport + RemoteSddProbeTransport
{
}

/// Production OpenSSH client for the fixed `agentum-sdd-v1` subsystem.
///
/// It deliberately has no repository path or remote command field. OpenSSH
/// receives `-s`, the persisted host destination, and the literal subsystem
/// name as separate argv entries. Request data is sent only over stdin as a
/// bounded, typed frame.
#[derive(Clone)]
pub struct OpenSshRemoteSddTransport {
    host: Host,
    ssh_program: PathBuf,
    extra_environment: Arc<Vec<(OsString, OsString)>>,
    active: Arc<Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>>>,
}

impl std::fmt::Debug for OpenSshRemoteSddTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenSshRemoteSddTransport")
            .field("host_id", &self.host.id)
            .field("host_name", &self.host.name)
            .field("ssh_program", &self.ssh_program)
            .finish_non_exhaustive()
    }
}

impl OpenSshRemoteSddTransport {
    pub fn new(host: Host) -> Result<Self, RemoteLifecycleError> {
        if !valid_ssh_host(&host) {
            return Err(RemoteLifecycleError::InvalidHost);
        }
        Ok(Self {
            host,
            ssh_program: PathBuf::from("ssh"),
            extra_environment: Arc::new(Vec::new()),
            active: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    #[cfg(test)]
    fn with_test_program(
        host: Host,
        program: PathBuf,
        environment: Vec<(OsString, OsString)>,
    ) -> Result<Self, RemoteLifecycleError> {
        let mut transport = Self::new(host)?;
        transport.ssh_program = program;
        transport.extra_environment = Arc::new(environment);
        Ok(transport)
    }

    async fn execute_request(
        &self,
        request: RemotePhaseRequest,
    ) -> Result<RemotePhaseResult, RemoteLifecycleError> {
        validate_transport_request(&self.host, &request)?;
        let frame = serde_json::to_vec(&RemoteClientFrame::ExecutePhase(request.clone()))
            .map_err(|_| RemoteLifecycleError::InvalidPlan)?;
        let response = self
            .exchange(
                &request.request_id,
                request.timeout_ms,
                request.output_limit,
                &frame,
            )
            .await?;
        let RemoteServerFrame::PhaseResult(result) = response else {
            return Err(RemoteLifecycleError::InvalidResult);
        };
        validate_result(&request, &result)?;
        Ok(result)
    }

    async fn author_request(
        &self,
        request: RemoteAuthoringRequest,
    ) -> Result<RemoteAuthoringResult, RemoteLifecycleError> {
        validate_authoring_request(&self.host, &request)?;
        let frame = serde_json::to_vec(&RemoteClientFrame::AuthorSpec(request.clone()))
            .map_err(|_| RemoteLifecycleError::InvalidPlan)?;
        let response = self
            .exchange(
                &request.request_id,
                request.timeout_ms,
                request.output_limit,
                &frame,
            )
            .await?;
        let RemoteServerFrame::AuthoringResult(result) = response else {
            return Err(RemoteLifecycleError::InvalidResult);
        };
        validate_authoring_result(&request, &result)?;
        Ok(result)
    }

    async fn inspect_delivery_request(
        &self,
        request: RemoteDeliverySnapshotRequest,
    ) -> Result<RemoteDeliverySnapshotResult, RemoteLifecycleError> {
        validate_delivery_snapshot_request(&self.host, &request)?;
        let frame = serde_json::to_vec(&RemoteClientFrame::InspectDelivery(request.clone()))
            .map_err(|_| RemoteLifecycleError::InvalidPlan)?;
        let response = self
            .exchange(
                &request.request_id,
                request.timeout_ms,
                request.output_limit,
                &frame,
            )
            .await?;
        let RemoteServerFrame::DeliverySnapshotResult(result) = response else {
            return Err(RemoteLifecycleError::InvalidResult);
        };
        validate_delivery_snapshot_result(&request, &result)?;
        Ok(result)
    }

    async fn execute_delivery_action_request(
        &self,
        request: RemoteDeliveryActionRequest,
    ) -> Result<RemoteDeliveryActionResult, RemoteLifecycleError> {
        validate_delivery_action_request(&self.host, &request)?;
        let frame = serde_json::to_vec(&RemoteClientFrame::ExecuteDeliveryAction(Box::new(
            request.clone(),
        )))
        .map_err(|_| RemoteLifecycleError::InvalidPlan)?;
        let response = self
            .exchange(
                &request.request_id,
                request.timeout_ms,
                request.output_limit,
                &frame,
            )
            .await?;
        let RemoteServerFrame::DeliveryActionResult(result) = response else {
            return Err(RemoteLifecycleError::InvalidResult);
        };
        validate_delivery_action_result(&request, &result)?;
        Ok(result)
    }

    pub async fn probe(
        &self,
        repository_identity_sha256: &str,
        provider: &str,
        base_ref: &str,
    ) -> Result<RemoteProbeResult, RemoteLifecycleError> {
        if !valid_sha256(repository_identity_sha256)
            || !valid_provider_reference(provider)
            || !valid_base_ref(base_ref)
        {
            return Err(RemoteLifecycleError::InvalidPlan);
        }
        let material = format!(
            "{}\n{}\n{}\n{}\n{}",
            self.host.id,
            repository_identity_sha256,
            provider,
            base_ref,
            env!("CARGO_PKG_VERSION")
        );
        let request = RemoteProbeRequest {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: format!("probe-{}", &super::sha256(material)[..32]),
            host_id: self.host.id.to_string(),
            repository_identity_sha256: repository_identity_sha256.into(),
            provider: provider.into(),
            base_ref: base_ref.into(),
            expected_worker_version: env!("CARGO_PKG_VERSION").into(),
        };
        let frame = serde_json::to_vec(&RemoteClientFrame::Probe(request.clone()))
            .map_err(|_| RemoteLifecycleError::InvalidPlan)?;
        let response = self
            .exchange(&request.request_id, 10_000, 256 * 1024, &frame)
            .await?;
        let RemoteServerFrame::ProbeResult(result) = response else {
            return Err(RemoteLifecycleError::InvalidResult);
        };
        if result.schema_version != REMOTE_SDD_SCHEMA_VERSION
            || result.request_id != request.request_id
            || result.host_id != request.host_id
            || result.worker_version != request.expected_worker_version
            || result
                .reason
                .as_ref()
                .is_some_and(|reason| reason.len() > 512)
            || match (&result.artifact_set_id, result.repository_registered) {
                (Some(value), true) => value.parse::<ulid::Ulid>().is_err(),
                (None, false) => false,
                _ => true,
            }
            || match (&result.base_commit, result.repository_registered) {
                (Some(value), true) => !valid_git_object(value),
                (None, false) => false,
                (None, true) if result.reason.is_some() => false,
                _ => true,
            }
            || (result.repository_registered && result.provider_ready && result.reason.is_some())
        {
            return Err(RemoteLifecycleError::InvalidResult);
        }
        Ok(result)
    }

    async fn exchange(
        &self,
        request_id: &str,
        timeout_ms: u64,
        output_limit: usize,
        frame: &[u8],
    ) -> Result<RemoteServerFrame, RemoteLifecycleError> {
        if frame.is_empty() || frame.len() > MAX_REMOTE_REQUEST {
            return Err(RemoteLifecycleError::OutputLimit(MAX_REMOTE_REQUEST));
        }

        let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
        {
            let mut active = self
                .active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if active.contains_key(request_id) {
                return Err(RemoteLifecycleError::Transport(
                    "duplicate remote request is already executing".into(),
                ));
            }
            active.insert(request_id.to_owned(), cancel_tx);
        }
        let _active_guard = ActiveRemoteRequest {
            request_id: request_id.to_owned(),
            active: Arc::clone(&self.active),
        };
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);

        match self
            .execute_once(
                frame,
                output_limit,
                SshMux::Interactive,
                deadline,
                &mut cancel_rx,
            )
            .await
        {
            Err(SshAttemptError::RetryableMux) => {
                if *cancel_rx.borrow() {
                    return Err(RemoteLifecycleError::Canceled);
                }
                self.execute_once(frame, output_limit, SshMux::Off, deadline, &mut cancel_rx)
                    .await
                    .map_err(SshAttemptError::into_public)
            }
            result => result.map_err(SshAttemptError::into_public),
        }
    }

    fn command(&self, mux: SshMux) -> tokio::process::Command {
        let command = ssh_subsystem_command(&self.host, REMOTE_SDD_SSH_SUBSYSTEM, mux);
        let mut command = if self.ssh_program == Path::new("ssh") {
            command
        } else {
            replace_command_program(command, &self.ssh_program)
        };
        for (key, value) in self.extra_environment.iter() {
            command.env(key, value);
        }
        command
    }

    pub fn cancel(&self, request_id: &str) -> bool {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(request_id)
            .is_some_and(|sender| sender.send(true).is_ok())
    }

    async fn execute_once(
        &self,
        frame: &[u8],
        output_limit: usize,
        mux: SshMux,
        deadline: tokio::time::Instant,
        cancel_rx: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<RemoteServerFrame, SshAttemptError> {
        if tokio::time::Instant::now() >= deadline {
            return Err(SshAttemptError::Public(RemoteLifecycleError::Timeout));
        }
        if *cancel_rx.borrow() {
            return Err(SshAttemptError::Public(RemoteLifecycleError::Canceled));
        }

        let mut command = self.command(mux);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x0000_0200); // CREATE_NEW_PROCESS_GROUP
        }
        let mut child = command.spawn().map_err(|_| {
            SshAttemptError::Public(RemoteLifecycleError::Transport(
                "could not start the OpenSSH client".into(),
            ))
        })?;
        let pid = child.id();
        let mut process_guard = LocalProcessTreeGuard::new(pid);
        let mut stdin = child.stdin.take().ok_or_else(|| {
            SshAttemptError::Public(RemoteLifecycleError::Transport(
                "OpenSSH stdin was not available".into(),
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            SshAttemptError::Public(RemoteLifecycleError::Transport(
                "OpenSSH stdout was not available".into(),
            ))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            SshAttemptError::Public(RemoteLifecycleError::Transport(
                "OpenSSH stderr was not available".into(),
            ))
        })?;

        let input_result = {
            let write = write_frame(&mut stdin, frame);
            tokio::pin!(write);
            tokio::select! {
                result = &mut write => Some(result),
                _ = tokio::time::sleep_until(deadline) => None,
                changed = cancel_rx.changed() => {
                    if changed.is_err() || *cancel_rx.borrow() {
                        terminate_remote_process(&mut child, pid, &mut process_guard).await;
                        return Err(SshAttemptError::Public(RemoteLifecycleError::Canceled));
                    }
                    None
                }
            }
        };
        let Some(input_result) = input_result else {
            terminate_remote_process(&mut child, pid, &mut process_guard).await;
            return Err(SshAttemptError::Public(RemoteLifecycleError::Timeout));
        };
        let input_failed = input_result.is_err();

        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let stdout_tx = event_tx.clone();
        let stdout_limit = output_limit;
        let stdout_task = tokio::spawn(async move {
            let result = read_json_frame(stdout, stdout_limit).await;
            let _ = stdout_tx.send(SshStreamEvent::Stdout(result));
        });
        let stderr_limit = output_limit.min(MAX_REMOTE_STDERR);
        let stderr_task = tokio::spawn(async move {
            let result = read_bounded_stream(stderr, stderr_limit).await;
            let _ = event_tx.send(SshStreamEvent::Stderr(result));
        });

        let mut status = None;
        let mut stdout = None;
        let mut stderr = None;
        while status.is_none() || stdout.is_none() || stderr.is_none() {
            tokio::select! {
                result = child.wait(), if status.is_none() => {
                    status = Some(result.map_err(|_| {
                        SshAttemptError::Public(RemoteLifecycleError::Transport(
                            "could not wait for the OpenSSH client".into(),
                        ))
                    })?);
                }
                event = event_rx.recv(), if stdout.is_none() || stderr.is_none() => {
                    match event {
                        Some(SshStreamEvent::Stdout(Err(FrameReadError::Limit))) => {
                            terminate_remote_process(&mut child, pid, &mut process_guard).await;
                            stdout_task.abort();
                            stderr_task.abort();
                            return Err(SshAttemptError::Public(
                                RemoteLifecycleError::OutputLimit(output_limit),
                            ));
                        }
                        Some(SshStreamEvent::Stdout(Err(FrameReadError::Trailing))) => {
                            terminate_remote_process(&mut child, pid, &mut process_guard).await;
                            stdout_task.abort();
                            stderr_task.abort();
                            return Err(SshAttemptError::Public(
                                RemoteLifecycleError::MalformedFrame,
                            ));
                        }
                        Some(SshStreamEvent::Stdout(result)) => stdout = Some(result),
                        Some(SshStreamEvent::Stderr(Err(StreamReadError::Limit))) => {
                            terminate_remote_process(&mut child, pid, &mut process_guard).await;
                            stdout_task.abort();
                            stderr_task.abort();
                            return Err(SshAttemptError::Public(
                                RemoteLifecycleError::OutputLimit(stderr_limit),
                            ));
                        }
                        Some(SshStreamEvent::Stderr(result)) => stderr = Some(result),
                        None => {
                            terminate_remote_process(&mut child, pid, &mut process_guard).await;
                            stdout_task.abort();
                            stderr_task.abort();
                            return Err(SshAttemptError::Public(
                                RemoteLifecycleError::Transport(
                                    "OpenSSH output supervision stopped unexpectedly".into(),
                                ),
                            ));
                        }
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    terminate_remote_process(&mut child, pid, &mut process_guard).await;
                    stdout_task.abort();
                    stderr_task.abort();
                    return Err(SshAttemptError::Public(RemoteLifecycleError::Timeout));
                }
                changed = cancel_rx.changed() => {
                    if changed.is_err() || *cancel_rx.borrow() {
                        terminate_remote_process(&mut child, pid, &mut process_guard).await;
                        stdout_task.abort();
                        stderr_task.abort();
                        return Err(SshAttemptError::Public(RemoteLifecycleError::Canceled));
                    }
                }
            }
        }
        process_guard.disarm();
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        let status = status.expect("loop requires status");
        let stderr = stderr.expect("loop requires stderr").map_err(|_| {
            SshAttemptError::Public(RemoteLifecycleError::Transport(
                "could not read bounded OpenSSH diagnostics".into(),
            ))
        })?;

        if !status.success() {
            if mux != SshMux::Off
                && status.code() == Some(255)
                && is_mux_transport_error(&String::from_utf8_lossy(&stderr))
            {
                return Err(SshAttemptError::RetryableMux);
            }
            return Err(SshAttemptError::Public(RemoteLifecycleError::Transport(
                format!(
                    "OpenSSH subsystem exited with status {}",
                    status
                        .code()
                        .map_or_else(|| "signal".into(), |code| code.to_string())
                ),
            )));
        }
        if input_failed {
            return Err(SshAttemptError::Public(RemoteLifecycleError::Transport(
                "could not write the bounded OpenSSH request frame".into(),
            )));
        }
        let stdout = stdout
            .expect("loop requires stdout")
            .map_err(|_| SshAttemptError::Public(RemoteLifecycleError::MalformedFrame))?;
        if stdout.len().saturating_add(stderr.len()) > output_limit {
            return Err(SshAttemptError::Public(RemoteLifecycleError::OutputLimit(
                output_limit,
            )));
        }
        let response: RemoteServerFrame = serde_json::from_slice(&stdout)
            .map_err(|_| SshAttemptError::Public(RemoteLifecycleError::InvalidResult))?;
        Ok(response)
    }
}

impl RemoteSddTransport for OpenSshRemoteSddTransport {
    fn execute(
        &self,
        request: RemotePhaseRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RemotePhaseResult, RemoteLifecycleError>> + Send + '_>>
    {
        Box::pin(self.execute_request(request))
    }

    fn cancel(&self, request_id: &str) -> bool {
        OpenSshRemoteSddTransport::cancel(self, request_id)
    }

    fn inspect_delivery(
        &self,
        request: RemoteDeliverySnapshotRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<RemoteDeliverySnapshotResult, RemoteLifecycleError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(self.inspect_delivery_request(request))
    }

    fn execute_delivery_action(
        &self,
        request: RemoteDeliveryActionRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<RemoteDeliveryActionResult, RemoteLifecycleError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(self.execute_delivery_action_request(request))
    }
}

impl RemoteSddAuthoringTransport for OpenSshRemoteSddTransport {
    fn author(
        &self,
        request: RemoteAuthoringRequest,
    ) -> Pin<
        Box<dyn Future<Output = Result<RemoteAuthoringResult, RemoteLifecycleError>> + Send + '_>,
    > {
        Box::pin(self.author_request(request))
    }

    fn cancel(&self, request_id: &str) -> bool {
        OpenSshRemoteSddTransport::cancel(self, request_id)
    }
}

impl RemoteSddProbeTransport for OpenSshRemoteSddTransport {
    fn probe(
        &self,
        repository_identity_sha256: &str,
        provider: &str,
        base_ref: &str,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteProbeResult, RemoteLifecycleError>> + Send + '_>>
    {
        let repository_identity_sha256 = repository_identity_sha256.to_owned();
        let provider = provider.to_owned();
        let base_ref = base_ref.to_owned();
        Box::pin(async move {
            OpenSshRemoteSddTransport::probe(
                self,
                &repository_identity_sha256,
                &provider,
                &base_ref,
            )
            .await
        })
    }
}

struct ActiveRemoteRequest {
    request_id: String,
    active: Arc<Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>>>,
}

impl Drop for ActiveRemoteRequest {
    fn drop(&mut self) {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.request_id);
    }
}

enum SshAttemptError {
    Public(RemoteLifecycleError),
    RetryableMux,
}

impl SshAttemptError {
    fn into_public(self) -> RemoteLifecycleError {
        match self {
            Self::Public(error) => error,
            Self::RetryableMux => RemoteLifecycleError::Transport(
                "the OpenSSH ControlMaster retry also failed".into(),
            ),
        }
    }
}

enum SshStreamEvent {
    Stdout(Result<Vec<u8>, FrameReadError>),
    Stderr(Result<Vec<u8>, StreamReadError>),
}

enum FrameReadError {
    Truncated,
    Trailing,
    Limit,
}

enum StreamReadError {
    Io,
    Limit,
}

struct LocalProcessTreeGuard {
    pid: Option<u32>,
}

impl LocalProcessTreeGuard {
    fn new(pid: Option<u32>) -> Self {
        Self { pid }
    }

    fn disarm(&mut self) {
        self.pid = None;
    }
}

impl Drop for LocalProcessTreeGuard {
    fn drop(&mut self) {
        let Some(pid) = self.pid else {
            return;
        };
        #[cfg(unix)]
        // SAFETY: the OpenSSH child is placed in a fresh process group whose
        // id equals its pid. A negative target cannot address Agentum's group.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }
}

async fn terminate_remote_process(
    child: &mut tokio::process::Child,
    pid: Option<u32>,
    guard: &mut LocalProcessTreeGuard,
) {
    super::providers::terminate_process_tree(child, pid).await;
    guard.disarm();
}

async fn write_frame(
    mut writer: impl tokio::io::AsyncWrite + Unpin,
    payload: &[u8],
) -> std::io::Result<()> {
    let length = u32::try_from(payload.len())
        .map_err(|_| std::io::Error::other("remote request frame is too large"))?;
    writer.write_all(&length.to_be_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await
}

async fn read_json_frame(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, FrameReadError> {
    let mut header = [0_u8; 4];
    reader
        .read_exact(&mut header)
        .await
        .map_err(|_| FrameReadError::Truncated)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 {
        return Err(FrameReadError::Truncated);
    }
    if length > limit {
        return Err(FrameReadError::Limit);
    }
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|_| FrameReadError::Truncated)?;
    let mut trailing = [0_u8; 1];
    if reader
        .read(&mut trailing)
        .await
        .map_err(|_| FrameReadError::Truncated)?
        != 0
    {
        return Err(FrameReadError::Trailing);
    }
    Ok(payload)
}

async fn read_bounded_stream(
    reader: impl AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, StreamReadError> {
    let mut output = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut output)
        .await
        .map_err(|_| StreamReadError::Io)?;
    if output.len() > limit {
        return Err(StreamReadError::Limit);
    }
    Ok(output)
}

fn replace_command_program(
    command: tokio::process::Command,
    program: &Path,
) -> tokio::process::Command {
    let source = command.as_std();
    let arguments = source.get_args().map(OsStr::to_owned).collect::<Vec<_>>();
    let environment = source
        .get_envs()
        .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
        .collect::<Vec<_>>();
    let current_directory = source.get_current_dir().map(Path::to_owned);
    let mut replacement = tokio::process::Command::new(program);
    replacement.args(arguments);
    if let Some(directory) = current_directory {
        replacement.current_dir(directory);
    }
    for (key, value) in environment {
        match value {
            Some(value) => {
                replacement.env(key, value);
            }
            None => {
                replacement.env_remove(key);
            }
        }
    }
    replacement
}

fn valid_ssh_host(host: &Host) -> bool {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        ..
    } = &host.kind
    else {
        return false;
    };
    *port != 0 && valid_ssh_user(user) && valid_ssh_hostname(hostname)
}

fn valid_ssh_user(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| !byte.is_ascii_control() && !byte.is_ascii_whitespace() && byte != b'@')
}

fn valid_ssh_hostname(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| !byte.is_ascii_control() && !byte.is_ascii_whitespace() && byte != b'@')
}

fn validate_transport_request(
    host: &Host,
    request: &RemotePhaseRequest,
) -> Result<(), RemoteLifecycleError> {
    if !valid_ssh_host(host)
        || request.host_id != host.id.to_string()
        || request.schema_version != REMOTE_SDD_SCHEMA_VERSION
        || !valid_remote_request_id(&request.request_id)
        || Uuid::parse_str(&request.run_id).is_err()
        || !valid_spec_id(&request.spec_id)
        || request.spec_revision < 1
        || !valid_sha256(&request.repository_identity_sha256)
        || request.artifact_set_id.parse::<ulid::Ulid>().is_err()
        || !valid_git_object(&request.base_commit)
        || !valid_provider_reference(&request.provider)
        || !valid_sha256(&request.expected_workspace_state_sha256)
        || !valid_sha256(&request.previous_result_sha256)
        || !valid_sha256(&request.approval_digest)
        || !(1_000..=MAX_REMOTE_TIMEOUT_MS).contains(&request.timeout_ms)
        || !(1_024..=MAX_REMOTE_OUTPUT).contains(&request.output_limit)
    {
        return Err(RemoteLifecycleError::InvalidPlan);
    }
    Ok(())
}

fn validate_authoring_request(
    host: &Host,
    request: &RemoteAuthoringRequest,
) -> Result<(), RemoteLifecycleError> {
    if !valid_ssh_host(host)
        || request.host_id != host.id.to_string()
        || request.schema_version != REMOTE_SDD_SCHEMA_VERSION
        || !valid_authoring_request_id(&request.request_id)
        || Uuid::parse_str(&request.run_id).is_err()
        || !valid_spec_id(&request.spec_id)
        || !valid_sha256(&request.repository_identity_sha256)
        || request.artifact_set_id.parse::<ulid::Ulid>().is_err()
        || !valid_git_object(&request.base_commit)
        || !valid_provider_reference(&request.provider)
        || !matches!(
            request.source_checkout.as_str(),
            "require_clean" | "committed_base"
        )
        || request.title.trim().is_empty()
        || request.title.len() > 256
        || request.title.contains(['\n', '\r'])
        || request.goal.trim().is_empty()
        || request.goal.len() > 32 * 1024
        || !(1_000..=MAX_REMOTE_TIMEOUT_MS).contains(&request.timeout_ms)
        || !(1_024..=MAX_REMOTE_OUTPUT).contains(&request.output_limit)
    {
        return Err(RemoteLifecycleError::InvalidPlan);
    }
    Ok(())
}

fn validate_authoring_result(
    request: &RemoteAuthoringRequest,
    result: &RemoteAuthoringResult,
) -> Result<(), RemoteLifecycleError> {
    let payload_valid = match result.status {
        RemotePhaseStatus::Succeeded => {
            result
                .spec
                .as_ref()
                .is_some_and(valid_remote_artifact_payload)
                && result.spec_revision == 2
                && result.error_code.is_none()
        }
        RemotePhaseStatus::Failed | RemotePhaseStatus::Canceled => {
            result.spec.is_none() && result.error_code.as_deref().is_some_and(valid_error_code)
        }
    };
    if result.schema_version != REMOTE_SDD_SCHEMA_VERSION
        || result.request_id != request.request_id
        || result.run_id != request.run_id
        || result.spec_id != request.spec_id
        || !valid_sha256(&result.workspace_state_sha256)
        || !valid_sha256(&result.artifact_set_sha256)
        || !payload_valid
    {
        return Err(RemoteLifecycleError::InvalidResult);
    }
    Ok(())
}

pub(crate) fn validate_delivery_snapshot_request(
    host: &Host,
    request: &RemoteDeliverySnapshotRequest,
) -> Result<(), RemoteLifecycleError> {
    if !valid_ssh_host(host)
        || request.host_id != host.id.to_string()
        || request.schema_version != REMOTE_SDD_SCHEMA_VERSION
        || !valid_prefixed_request_id(&request.request_id, "delivery-inspect-")
        || Uuid::parse_str(&request.run_id).is_err()
        || !valid_spec_id(&request.spec_id)
        || request.spec_revision < 1
        || !valid_sha256(&request.repository_identity_sha256)
        || request.artifact_set_id.parse::<ulid::Ulid>().is_err()
        || !valid_git_object(&request.base_commit)
        || !valid_sha256(&request.approval_digest)
        || !valid_sha256(&request.expected_workspace_state_sha256)
        || request.openspec_destination.as_ref().is_some_and(|path| {
            path.len() > 1_024 || agentum_core::sdd::validate_relative_path(path).is_err()
        })
        || !(1_000..=MAX_REMOTE_TIMEOUT_MS).contains(&request.timeout_ms)
        || !(1_024..=MAX_REMOTE_OUTPUT).contains(&request.output_limit)
    {
        return Err(RemoteLifecycleError::InvalidPlan);
    }
    Ok(())
}

pub(crate) fn validate_delivery_snapshot_result(
    request: &RemoteDeliverySnapshotRequest,
    result: &RemoteDeliverySnapshotResult,
) -> Result<(), RemoteLifecycleError> {
    if result.schema_version != REMOTE_SDD_SCHEMA_VERSION
        || result.request_id != request.request_id
        || result.run_id != request.run_id
        || !valid_sha256(&result.workspace_state_sha256)
        || result.workspace_state_sha256 != request.expected_workspace_state_sha256
        || !valid_sha256(&result.artifact_set_sha256)
        || !valid_sha256(&result.worktree_identity_sha256)
        || !valid_base_ref(&result.branch_name)
        || result.openspec_destination_exists.is_some() != request.openspec_destination.is_some()
    {
        return Err(RemoteLifecycleError::InvalidResult);
    }
    Ok(())
}

pub(crate) fn validate_delivery_action_request(
    host: &Host,
    request: &RemoteDeliveryActionRequest,
) -> Result<(), RemoteLifecycleError> {
    let envelope_digest = super::delivery::preview_digest(&request.envelope)
        .map_err(|_| RemoteLifecycleError::InvalidPlan)?;
    let action_matches = request
        .envelope
        .actions
        .iter()
        .any(|action| action == &request.action);
    if !valid_ssh_host(host)
        || request.host_id != host.id.to_string()
        || request.schema_version != REMOTE_SDD_SCHEMA_VERSION
        || !valid_prefixed_request_id(&request.request_id, "delivery-action-")
        || Uuid::parse_str(&request.run_id).is_err()
        || !valid_spec_id(&request.spec_id)
        || request.spec_revision < 1
        || !valid_sha256(&request.repository_identity_sha256)
        || request.artifact_set_id.parse::<ulid::Ulid>().is_err()
        || !valid_git_object(&request.base_commit)
        || !valid_sha256(&request.approval_digest)
        || !valid_sha256(&request.preview_digest)
        || request.preview_digest != envelope_digest
        || request.envelope.schema_version != 1
        || request.envelope.run_id != request.run_id
        || request.envelope.spec_id != request.spec_id
        || request.envelope.spec_revision != request.spec_revision
        || request.envelope.base_commit != request.base_commit
        || request.envelope.actor_id.trim().is_empty()
        || request.envelope.actor_id.len() > 256
        || !valid_sha256(&request.envelope.workspace_fingerprint)
        || !valid_sha256(&request.envelope.workspace_state_hash)
        || !valid_sha256(&request.envelope.worktree_identity)
        || !valid_base_ref(&request.envelope.branch_name)
        || request.envelope.actions.is_empty()
        || request.envelope.actions.len() > 12
        || request.envelope.artifact_hashes.iter().any(|artifact| {
            artifact.kind.is_empty()
                || artifact.kind.len() > 128
                || artifact.relative_path.is_empty()
                || artifact.relative_path.len() > 1_024
                || !valid_sha256(&artifact.content_hash)
        })
        || !action_matches
        || !super::delivery::is_repository_delivery_action(&request.action)
        || !(1..=1_000).contains(&request.attempt)
        || !(1_000..=MAX_REMOTE_TIMEOUT_MS).contains(&request.timeout_ms)
        || !(1_024..=MAX_REMOTE_OUTPUT).contains(&request.output_limit)
    {
        return Err(RemoteLifecycleError::InvalidPlan);
    }
    Ok(())
}

pub(crate) fn validate_delivery_action_result(
    request: &RemoteDeliveryActionRequest,
    result: &RemoteDeliveryActionResult,
) -> Result<(), RemoteLifecycleError> {
    let remote_artifact_hashes = request
        .envelope
        .artifact_hashes
        .iter()
        .filter(|artifact| {
            artifact.kind == "remote_artifact_set"
                && artifact.relative_path == "agentum+ssh://artifact-set"
        })
        .collect::<Vec<_>>();
    let error_shape_valid = match result.status {
        RemoteDeliveryActionStatus::Succeeded => result.error_code.is_none(),
        RemoteDeliveryActionStatus::Failed | RemoteDeliveryActionStatus::SyncPending => {
            result.error_code.as_deref().is_some_and(valid_error_code)
        }
    };
    if result.schema_version != REMOTE_SDD_SCHEMA_VERSION
        || result.request_id != request.request_id
        || result.run_id != request.run_id
        || result.action_id != request.action.id
        || !valid_sha256(&result.workspace_state_sha256)
        || !valid_sha256(&result.artifact_set_sha256)
        || remote_artifact_hashes.len() != 1
        || result.artifact_set_sha256 != remote_artifact_hashes[0].content_hash
        || serde_json::to_vec(&result.result)
            .map_or(true, |bytes| bytes.len() > request.output_limit)
        || !error_shape_valid
    {
        return Err(RemoteLifecycleError::InvalidResult);
    }
    Ok(())
}

fn valid_prefixed_request_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn valid_remote_artifact_payload(artifact: &RemoteArtifactPayload) -> bool {
    artifact.content.len() <= MAX_REMOTE_OUTPUT
        && valid_sha256(&artifact.content_sha256)
        && super::sha256(artifact.content.as_bytes()) == artifact.content_sha256
        && agentum_core::sdd::validate_relative_path(&artifact.relative_path).is_ok()
}

fn valid_remote_request_id(value: &str) -> bool {
    value.strip_prefix("remote-").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn valid_authoring_request_id(value: &str) -> bool {
    value.strip_prefix("author-").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[derive(Debug, Default, Clone)]
pub struct SequentialRemoteLifecycle;

impl SequentialRemoteLifecycle {
    /// Execute at most one remote phase globally. The initial remote release
    /// intentionally has concurrency one, including across different hosts.
    /// Callers persist the returned checkpoint with CAS, then invoke again.
    pub async fn advance<T: RemoteSddTransport + ?Sized>(
        &self,
        transport: &T,
        plan: &RemoteLifecyclePlan,
        checkpoint: &RemoteLifecycleCheckpoint,
    ) -> Result<RemoteAdvance, RemoteLifecycleError> {
        validate_plan(plan)?;
        validate_checkpoint(plan, checkpoint)?;
        if checkpoint.is_ready() {
            return Err(RemoteLifecycleError::AlreadyReady);
        }
        let _lease = remote_lifecycle_lease().lock().await;
        let request = build_request(plan, checkpoint);
        let result = tokio::time::timeout(
            Duration::from_millis(plan.timeout_ms),
            transport.execute(request.clone()),
        )
        .await
        .map_err(|_| RemoteLifecycleError::Timeout)??;
        validate_result(&request, &result)?;

        let mut next = checkpoint.clone();
        if result.status == RemotePhaseStatus::Succeeded {
            next.completed_phases = next
                .completed_phases
                .checked_add(1)
                .ok_or(RemoteLifecycleError::InvalidCheckpoint)?;
            next.workspace_state_sha256 = result.workspace_state_sha256.clone();
            next.last_result_sha256 = result_digest(&result)?;
            next.next_phase = checkpoint
                .next_phase
                .next()
                .ok_or(RemoteLifecycleError::AlreadyReady)?;
        }
        Ok(RemoteAdvance {
            checkpoint: next,
            result,
        })
    }
}

fn remote_lifecycle_lease() -> &'static tokio::sync::Mutex<()> {
    static LEASE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LEASE.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub(crate) fn build_request(
    plan: &RemoteLifecyclePlan,
    checkpoint: &RemoteLifecycleCheckpoint,
) -> RemotePhaseRequest {
    let request_material = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        plan.run_id,
        plan.artifact_set_id,
        plan.spec_revision,
        checkpoint.completed_phases,
        serde_json::to_string(&checkpoint.next_phase).expect("phase serializes"),
        checkpoint.workspace_state_sha256,
        checkpoint.last_result_sha256
    );
    RemotePhaseRequest {
        schema_version: REMOTE_SDD_SCHEMA_VERSION,
        request_id: format!("remote-{}", &super::sha256(request_material)[..32]),
        host_id: plan.host_id.clone(),
        run_id: plan.run_id.clone(),
        spec_id: plan.spec_id.clone(),
        spec_revision: plan.spec_revision,
        phase: checkpoint.next_phase,
        repository_identity_sha256: plan.repository_identity_sha256.clone(),
        artifact_set_id: plan.artifact_set_id.clone(),
        base_commit: plan.base_commit.clone(),
        provider: plan.provider.clone(),
        expected_workspace_state_sha256: checkpoint.workspace_state_sha256.clone(),
        previous_result_sha256: checkpoint.last_result_sha256.clone(),
        approval_digest: plan.approval_digest.clone(),
        timeout_ms: plan.timeout_ms,
        output_limit: plan.output_limit,
    }
}

fn validate_plan(plan: &RemoteLifecyclePlan) -> Result<(), RemoteLifecycleError> {
    if plan.schema_version != REMOTE_SDD_SCHEMA_VERSION
        || Uuid::parse_str(&plan.host_id).is_err()
        || Uuid::parse_str(&plan.run_id).is_err()
        || !valid_spec_id(&plan.spec_id)
        || plan.spec_revision < 1
        || !valid_sha256(&plan.repository_identity_sha256)
        || plan.artifact_set_id.parse::<ulid::Ulid>().is_err()
        || !valid_git_object(&plan.base_commit)
        || !valid_provider_reference(&plan.provider)
        || !valid_sha256(&plan.approval_digest)
        || !(1_000..=MAX_REMOTE_TIMEOUT_MS).contains(&plan.timeout_ms)
        || !(1_024..=MAX_REMOTE_OUTPUT).contains(&plan.output_limit)
    {
        return Err(RemoteLifecycleError::InvalidPlan);
    }
    Ok(())
}

fn validate_checkpoint(
    plan: &RemoteLifecyclePlan,
    checkpoint: &RemoteLifecycleCheckpoint,
) -> Result<(), RemoteLifecycleError> {
    let expected_completed = match checkpoint.next_phase {
        RemoteLifecyclePhase::Design => 0,
        RemoteLifecyclePhase::Planning => 1,
        RemoteLifecyclePhase::Implementation => 2,
        RemoteLifecyclePhase::Verification => 3,
        RemoteLifecyclePhase::Review => 4,
        RemoteLifecyclePhase::Ready => 5,
    };
    if checkpoint.schema_version != REMOTE_SDD_SCHEMA_VERSION
        || checkpoint.host_id != plan.host_id
        || checkpoint.run_id != plan.run_id
        || checkpoint.spec_revision != plan.spec_revision
        || checkpoint.approval_digest != plan.approval_digest
        || checkpoint.completed_phases != expected_completed
        || !valid_sha256(&checkpoint.workspace_state_sha256)
        || !valid_sha256(&checkpoint.last_result_sha256)
    {
        return Err(RemoteLifecycleError::InvalidCheckpoint);
    }
    Ok(())
}

fn validate_result(
    request: &RemotePhaseRequest,
    result: &RemotePhaseResult,
) -> Result<(), RemoteLifecycleError> {
    let error_shape_valid = match result.status {
        RemotePhaseStatus::Succeeded => result.error_code.is_none(),
        RemotePhaseStatus::Failed | RemotePhaseStatus::Canceled => {
            result.error_code.as_deref().is_some_and(valid_error_code)
        }
    };
    if result.schema_version != REMOTE_SDD_SCHEMA_VERSION
        || result.request_id != request.request_id
        || !valid_sha256(&result.workspace_state_sha256)
        || !valid_sha256(&result.artifact_set_sha256)
        || !valid_sha256(&result.evidence_sha256)
        || result
            .evidence_summary
            .as_ref()
            .is_some_and(|summary| summary.len() > 2 * 1024 * 1024)
        || result.artifacts.iter().any(|artifact| {
            artifact.content.len() > MAX_REMOTE_OUTPUT
                || !valid_sha256(&artifact.content_sha256)
                || super::sha256(artifact.content.as_bytes()) != artifact.content_sha256
                || agentum_core::sdd::validate_relative_path(&artifact.relative_path).is_err()
        })
        || !error_shape_valid
    {
        return Err(RemoteLifecycleError::InvalidResult);
    }
    Ok(())
}

fn result_digest(result: &RemotePhaseResult) -> Result<String, RemoteLifecycleError> {
    serde_json::to_vec(result)
        .map(super::sha256)
        .map_err(|_| RemoteLifecycleError::InvalidResult)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_git_object(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_base_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('-')
        && !value.ends_with('.')
        && !value.contains("..")
        && !value.contains("@{")
        && !value.bytes().any(|byte| {
            byte.is_ascii_control()
                || matches!(byte, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
}

fn valid_spec_id(value: &str) -> bool {
    value.parse::<agentum_core::sdd::SpecId>().is_ok()
}

fn valid_error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_provider_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| !byte.is_ascii_control() && !byte.is_ascii_whitespace())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use agentum_core::sdd::SpecId;
    #[cfg(unix)]
    use agentum_core::{Host, HostKind, SshAuth};

    use super::*;

    fn hash(value: impl AsRef<[u8]>) -> String {
        super::super::sha256(value)
    }

    fn plan() -> RemoteLifecyclePlan {
        RemoteLifecyclePlan {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            host_id: Uuid::new_v4().to_string(),
            run_id: Uuid::new_v4().to_string(),
            spec_id: SpecId::new().to_string(),
            spec_revision: 2,
            repository_identity_sha256: hash(b"remote repository"),
            artifact_set_id: ulid::Ulid::new().to_string(),
            base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            provider: "codex".into(),
            approval_digest: hash(b"approved spec and policy"),
            timeout_ms: 5_000,
            output_limit: 64 * 1024,
        }
    }

    fn delivery_envelope(plan: &RemoteLifecyclePlan) -> DeliveryPreviewEnvelope {
        let action = PreparedDeliveryAction {
            id: Uuid::new_v4().to_string(),
            kind: "commit".into(),
            depends_on: Vec::new(),
            intent: super::super::delivery::DeliveryActionRequest::Commit {
                message: "Deliver fixture".into(),
            },
            openspec_export: None,
            tracker_mutation: None,
        };
        DeliveryPreviewEnvelope {
            schema_version: 1,
            actor_id: "human:test".into(),
            repo_id: "repo-test".into(),
            spec_id: plan.spec_id.clone(),
            spec_revision: plan.spec_revision,
            run_id: plan.run_id.clone(),
            run_revision: 7,
            base_commit: plan.base_commit.clone(),
            branch_name: "agentum/test-delivery".into(),
            worktree_identity: hash(b"remote worktree identity"),
            workspace_fingerprint: hash(b"remote workspace fingerprint"),
            workspace_state_hash: hash(b"ready workspace"),
            artifact_hashes: vec![super::super::delivery::DeliveryArtifactHash {
                kind: "remote_artifact_set".into(),
                relative_path: "agentum+ssh://artifact-set".into(),
                content_hash: hash(b"ready artifact set"),
            }],
            actions: vec![action],
        }
    }

    #[test]
    fn remote_delivery_results_reject_workspace_and_artifact_tampering() {
        let plan = plan();
        let envelope = delivery_envelope(&plan);
        let snapshot_request = RemoteDeliverySnapshotRequest {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: format!("delivery-inspect-{}", "1".repeat(32)),
            host_id: plan.host_id.clone(),
            run_id: plan.run_id.clone(),
            spec_id: plan.spec_id.clone(),
            spec_revision: plan.spec_revision,
            repository_identity_sha256: plan.repository_identity_sha256.clone(),
            artifact_set_id: plan.artifact_set_id.clone(),
            base_commit: plan.base_commit.clone(),
            approval_digest: plan.approval_digest.clone(),
            expected_workspace_state_sha256: envelope.workspace_state_hash.clone(),
            openspec_destination: None,
            timeout_ms: 5_000,
            output_limit: 64 * 1024,
        };
        let mut snapshot_result = RemoteDeliverySnapshotResult {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: snapshot_request.request_id.clone(),
            run_id: plan.run_id.clone(),
            workspace_state_sha256: envelope.workspace_state_hash.clone(),
            artifact_set_sha256: envelope.artifact_hashes[0].content_hash.clone(),
            worktree_identity_sha256: envelope.worktree_identity.clone(),
            branch_name: envelope.branch_name.clone(),
            openspec_destination_exists: None,
        };
        assert!(validate_delivery_snapshot_result(&snapshot_request, &snapshot_result).is_ok());
        snapshot_result.workspace_state_sha256 = hash(b"tampered workspace");
        assert_eq!(
            validate_delivery_snapshot_result(&snapshot_request, &snapshot_result),
            Err(RemoteLifecycleError::InvalidResult)
        );

        let preview_digest = super::super::delivery::preview_digest(&envelope).unwrap();
        let action_request = RemoteDeliveryActionRequest {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: format!("delivery-action-{}", "2".repeat(32)),
            host_id: plan.host_id,
            run_id: plan.run_id.clone(),
            spec_id: plan.spec_id,
            spec_revision: plan.spec_revision,
            repository_identity_sha256: plan.repository_identity_sha256,
            artifact_set_id: plan.artifact_set_id,
            base_commit: plan.base_commit,
            approval_digest: plan.approval_digest,
            preview_digest,
            action: envelope.actions[0].clone(),
            envelope: envelope.clone(),
            attempt: 1,
            timeout_ms: 5_000,
            output_limit: 64 * 1024,
        };
        let mut action_result = RemoteDeliveryActionResult {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: action_request.request_id.clone(),
            run_id: plan.run_id,
            action_id: action_request.action.id.clone(),
            status: RemoteDeliveryActionStatus::Succeeded,
            result: serde_json::json!({"summary": "delivered"}),
            workspace_state_sha256: hash(b"post-delivery workspace"),
            artifact_set_sha256: envelope.artifact_hashes[0].content_hash.clone(),
            error_code: None,
        };
        assert!(validate_delivery_action_result(&action_request, &action_result).is_ok());
        action_result.artifact_set_sha256 = hash(b"tampered artifact set");
        assert_eq!(
            validate_delivery_action_result(&action_request, &action_result),
            Err(RemoteLifecycleError::InvalidResult)
        );
    }

    struct FixtureTransport {
        active: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
        calls: Arc<AtomicUsize>,
        malformed: bool,
        status: RemotePhaseStatus,
    }

    impl RemoteSddTransport for FixtureTransport {
        fn execute(
            &self,
            request: RemotePhaseRequest,
        ) -> Pin<
            Box<dyn Future<Output = Result<RemotePhaseResult, RemoteLifecycleError>> + Send + '_>,
        > {
            Box::pin(async move {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum.fetch_max(active, Ordering::SeqCst);
                self.calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(15)).await;
                self.active.fetch_sub(1, Ordering::SeqCst);
                Ok(RemotePhaseResult {
                    schema_version: REMOTE_SDD_SCHEMA_VERSION,
                    request_id: if self.malformed {
                        "wrong-request".into()
                    } else {
                        request.request_id
                    },
                    phase: request.phase,
                    status: self.status,
                    workspace_state_sha256: hash(format!("workspace:{:?}", request.phase)),
                    artifact_set_sha256: hash(format!("artifacts:{:?}", request.phase)),
                    evidence_sha256: hash(format!("evidence:{:?}", request.phase)),
                    evidence_summary: None,
                    artifacts: Vec::new(),
                    error_code: (self.status != RemotePhaseStatus::Succeeded)
                        .then(|| "fixture_failure".into()),
                })
            })
        }
    }

    fn transport(status: RemotePhaseStatus) -> FixtureTransport {
        FixtureTransport {
            active: Arc::new(AtomicUsize::new(0)),
            maximum: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(AtomicUsize::new(0)),
            malformed: false,
            status,
        }
    }

    #[cfg(unix)]
    const FAKE_SSH: &str = r#"#!/usr/bin/env python3
import hashlib
import json
import os
import struct
import subprocess
import sys
import time

mode = os.environ["AGENTUM_FAKE_SSH_MODE"]
record = os.environ["AGENTUM_FAKE_SSH_RECORD"]
pids = os.environ["AGENTUM_FAKE_SSH_PIDS"]
counter = os.environ["AGENTUM_FAKE_SSH_COUNTER"]

with open(record, "a", encoding="utf-8") as stream:
    stream.write(json.dumps(sys.argv[1:]) + "\n")

header = sys.stdin.buffer.read(4)
if len(header) != 4:
    sys.exit(90)
length = struct.unpack(">I", header)[0]
body = sys.stdin.buffer.read(length)
if len(body) != length:
    sys.exit(91)
request_frame = json.loads(body.decode("utf-8"))
request = request_frame["payload"]

if mode in ("timeout", "cancel", "drop"):
    descendant = subprocess.Popen([
        sys.executable,
        "-c",
        "import time; time.sleep(30)",
    ])
    with open(pids, "w", encoding="utf-8") as stream:
        stream.write(f"{os.getpid()} {descendant.pid}")
        stream.flush()
        os.fsync(stream.fileno())
    time.sleep(30)

if mode == "retry":
    try:
        with open(counter, "r", encoding="utf-8") as stream:
            count = int(stream.read() or "0")
    except FileNotFoundError:
        count = 0
    count += 1
    with open(counter, "w", encoding="utf-8") as stream:
        stream.write(str(count))
    if count == 1:
        sys.stderr.write("mux_client_request_session: read from master failed: Broken pipe\n")
        sys.exit(255)

if mode == "oversized":
    sys.stdout.buffer.write(struct.pack(">I", request["outputLimit"] + 1))
    sys.stdout.buffer.flush()
    time.sleep(30)

if mode == "truncated":
    sys.stdout.buffer.write(b"\x00\x01")
    sys.stdout.buffer.flush()
    sys.exit(0)

if mode == "malformed":
    payload = b"{not-json"
    sys.stdout.buffer.write(struct.pack(">I", len(payload)) + payload)
    sys.stdout.buffer.flush()
    sys.exit(0)

def digest(value):
    return hashlib.sha256(value.encode("utf-8")).hexdigest()

result = {
    "type": "phase_result",
    "payload": {
        "schemaVersion": 1,
        "requestId": request["requestId"],
        "phase": request["phase"],
        "status": "succeeded",
        "workspaceStateSha256": digest("workspace:" + request["phase"]),
        "artifactSetSha256": digest("artifacts:" + request["phase"]),
        "evidenceSha256": digest("evidence:" + request["phase"]),
        "evidenceSummary": None,
        "artifacts": [],
        "errorCode": None,
    },
}
payload = json.dumps(result, separators=(",", ":")).encode("utf-8")
sys.stdout.buffer.write(struct.pack(">I", len(payload)) + payload)
sys.stdout.buffer.flush()
"#;

    #[cfg(unix)]
    struct FakeSshFixture {
        _directory: tempfile::TempDir,
        transport: OpenSshRemoteSddTransport,
        record: PathBuf,
        pids: PathBuf,
        counter: PathBuf,
    }

    #[cfg(unix)]
    fn fake_ssh(mode: &str, plan: &RemoteLifecyclePlan) -> FakeSshFixture {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("fake-ssh");
        std::fs::write(&program, FAKE_SSH).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let record = directory.path().join("argv.jsonl");
        let pids = directory.path().join("pids");
        let counter = directory.path().join("counter");
        let host = Host {
            id: Uuid::parse_str(&plan.host_id).unwrap(),
            name: "fixture SSH".into(),
            kind: HostKind::Ssh {
                user: "fixture".into(),
                hostname: "example.invalid".into(),
                port: 2222,
                auth: SshAuth::Agent,
            },
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        };
        let environment = vec![
            ("AGENTUM_FAKE_SSH_MODE".into(), mode.into()),
            (
                "AGENTUM_FAKE_SSH_RECORD".into(),
                record.as_os_str().to_owned(),
            ),
            ("AGENTUM_FAKE_SSH_PIDS".into(), pids.as_os_str().to_owned()),
            (
                "AGENTUM_FAKE_SSH_COUNTER".into(),
                counter.as_os_str().to_owned(),
            ),
        ];
        let transport =
            OpenSshRemoteSddTransport::with_test_program(host, program, environment).unwrap();
        FakeSshFixture {
            _directory: directory,
            transport,
            record,
            pids,
            counter,
        }
    }

    #[cfg(unix)]
    fn phase_request(plan: &RemoteLifecyclePlan) -> RemotePhaseRequest {
        let checkpoint =
            RemoteLifecycleCheckpoint::initial(plan, hash(b"initial workspace")).unwrap();
        build_request(plan, &checkpoint)
    }

    #[cfg(unix)]
    fn recorded_invocations(path: &Path) -> Vec<Vec<String>> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[cfg(unix)]
    async fn wait_for_pids(path: &Path) -> Vec<i32> {
        for _ in 0..200 {
            if let Ok(raw) = std::fs::read_to_string(path) {
                return raw
                    .split_whitespace()
                    .map(|value| value.parse().unwrap())
                    .collect();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("fake SSH process ids were not published");
    }

    #[cfg(unix)]
    async fn assert_processes_terminated(pids: &[i32]) {
        for _ in 0..200 {
            let alive = pids.iter().any(|pid| {
                // SAFETY: signal zero only checks for existence/permission.
                unsafe { libc::kill(*pid, 0) == 0 }
            });
            if !alive {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("SSH process tree still exists after cancellation: {pids:?}");
    }

    #[tokio::test]
    async fn full_remote_lifecycle_restarts_between_every_phase_and_stops_at_ready() {
        let plan = plan();
        let coordinator = SequentialRemoteLifecycle;
        let transport = transport(RemotePhaseStatus::Succeeded);
        let mut checkpoint =
            RemoteLifecycleCheckpoint::initial(&plan, hash(b"initial workspace")).unwrap();
        let mut phases = Vec::new();
        while !checkpoint.is_ready() {
            let advance = coordinator
                .advance(&transport, &plan, &checkpoint)
                .await
                .unwrap();
            phases.push(advance.result.phase);
            // JSON round-trip models a process restart from durable storage.
            checkpoint =
                serde_json::from_slice(&serde_json::to_vec(&advance.checkpoint).unwrap()).unwrap();
        }
        assert_eq!(
            phases,
            [
                RemoteLifecyclePhase::Design,
                RemoteLifecyclePhase::Planning,
                RemoteLifecyclePhase::Implementation,
                RemoteLifecyclePhase::Verification,
                RemoteLifecyclePhase::Review,
            ]
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 5);
        assert_eq!(
            coordinator.advance(&transport, &plan, &checkpoint).await,
            Err(RemoteLifecycleError::AlreadyReady)
        );
    }

    #[tokio::test]
    async fn remote_execution_is_globally_sequential_across_hosts() {
        let first_plan = plan();
        let second_plan = plan();
        let first_checkpoint =
            RemoteLifecycleCheckpoint::initial(&first_plan, hash(b"first")).unwrap();
        let second_checkpoint =
            RemoteLifecycleCheckpoint::initial(&second_plan, hash(b"second")).unwrap();
        let transport = Arc::new(transport(RemotePhaseStatus::Succeeded));
        let first_transport = Arc::clone(&transport);
        let second_transport = Arc::clone(&transport);
        let first = tokio::spawn(async move {
            SequentialRemoteLifecycle
                .advance(first_transport.as_ref(), &first_plan, &first_checkpoint)
                .await
        });
        let second = tokio::spawn(async move {
            SequentialRemoteLifecycle
                .advance(second_transport.as_ref(), &second_plan, &second_checkpoint)
                .await
        });
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        assert_eq!(transport.maximum.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn malformed_failed_and_canceled_results_never_advance_checkpoint() {
        let plan = plan();
        let checkpoint =
            RemoteLifecycleCheckpoint::initial(&plan, hash(b"initial workspace")).unwrap();
        let malformed = FixtureTransport {
            malformed: true,
            ..transport(RemotePhaseStatus::Succeeded)
        };
        assert_eq!(
            SequentialRemoteLifecycle
                .advance(&malformed, &plan, &checkpoint)
                .await,
            Err(RemoteLifecycleError::InvalidResult)
        );
        for status in [RemotePhaseStatus::Failed, RemotePhaseStatus::Canceled] {
            let result = SequentialRemoteLifecycle
                .advance(&transport(status), &plan, &checkpoint)
                .await
                .unwrap();
            assert_eq!(result.result.status, status);
            assert_eq!(result.checkpoint, checkpoint);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn openssh_transport_uses_fixed_subsystem_argv_and_typed_frames() {
        let plan = plan();
        let fixture = fake_ssh("success", &plan);
        let request = phase_request(&plan);
        let result = fixture.transport.execute(request.clone()).await.unwrap();

        assert_eq!(result.request_id, request.request_id);
        assert_eq!(result.phase, request.phase);
        let invocations = recorded_invocations(&fixture.record);
        assert_eq!(invocations.len(), 1);
        assert_eq!(
            &invocations[0][invocations[0].len() - 3..],
            ["-s", "fixture@example.invalid", REMOTE_SDD_SSH_SUBSYSTEM]
        );
        assert!(
            !invocations[0]
                .iter()
                .any(|argument| argument == "sh" || argument == "bash")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn openssh_transport_rejects_malformed_truncated_and_oversized_frames() {
        for (mode, expected) in [
            ("malformed", RemoteLifecycleError::InvalidResult),
            ("truncated", RemoteLifecycleError::MalformedFrame),
            ("oversized", RemoteLifecycleError::OutputLimit(64 * 1024)),
        ] {
            let plan = plan();
            let fixture = fake_ssh(mode, &plan);
            let result = fixture.transport.execute(phase_request(&plan)).await;
            assert_eq!(result, Err(expected), "mode {mode}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn openssh_transport_timeout_terminates_the_complete_process_group() {
        let mut plan = plan();
        plan.timeout_ms = 1_000;
        let fixture = fake_ssh("timeout", &plan);
        let request = phase_request(&plan);
        let pids_path = fixture.pids.clone();
        let execution = fixture.transport.execute(request);
        let pids_future = wait_for_pids(&pids_path);
        let (result, pids) = tokio::join!(execution, pids_future);

        assert_eq!(result, Err(RemoteLifecycleError::Timeout));
        assert_processes_terminated(&pids).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn openssh_transport_cancel_terminates_the_complete_process_group() {
        let plan = plan();
        let fixture = fake_ssh("cancel", &plan);
        let request = phase_request(&plan);
        let request_id = request.request_id.clone();
        let executing_transport = fixture.transport.clone();
        let execution = tokio::spawn(async move { executing_transport.execute(request).await });
        let pids = wait_for_pids(&fixture.pids).await;

        assert!(fixture.transport.cancel(&request_id));
        assert_eq!(
            execution.await.unwrap(),
            Err(RemoteLifecycleError::Canceled)
        );
        assert!(!fixture.transport.cancel(&request_id));
        assert_processes_terminated(&pids).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_transport_future_terminates_the_complete_process_group() {
        let plan = plan();
        let fixture = fake_ssh("drop", &plan);
        let request = phase_request(&plan);
        let executing_transport = fixture.transport.clone();
        let execution = tokio::spawn(async move { executing_transport.execute(request).await });
        let pids = wait_for_pids(&fixture.pids).await;

        execution.abort();
        let _ = execution.await;
        assert_processes_terminated(&pids).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_control_master_failure_retries_once_without_mux() {
        let plan = plan();
        let fixture = fake_ssh("retry", &plan);
        let request = phase_request(&plan);
        let result = fixture.transport.execute(request.clone()).await.unwrap();

        assert_eq!(result.request_id, request.request_id);
        assert_eq!(std::fs::read_to_string(&fixture.counter).unwrap(), "2");
        let invocations = recorded_invocations(&fixture.record);
        assert_eq!(invocations.len(), 2);
        assert!(
            invocations[0]
                .iter()
                .any(|argument| argument == "ControlMaster=auto")
        );
        assert!(
            !invocations[1]
                .iter()
                .any(|argument| argument == "ControlMaster=auto")
        );
    }

    #[test]
    fn checkpoint_and_plan_reject_identity_or_digest_tampering() {
        let mut plan = plan();
        let checkpoint =
            RemoteLifecycleCheckpoint::initial(&plan, hash(b"initial workspace")).unwrap();
        plan.approval_digest = hash(b"different approval");
        assert_eq!(
            validate_checkpoint(&plan, &checkpoint),
            Err(RemoteLifecycleError::InvalidCheckpoint)
        );
        plan.approval_digest = checkpoint.approval_digest.clone();
        plan.base_commit = "HEAD".into();
        assert_eq!(validate_plan(&plan), Err(RemoteLifecycleError::InvalidPlan));
    }
}
