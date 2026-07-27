//! Host-side implementation of the fixed `agentum-sdd-v1` SSH subsystem.
//!
//! The SSH daemon starts one process per channel. Repository paths are never
//! accepted from a client: an owner-only administrator configuration maps a
//! stable repository identity to one canonical local checkout. SQLite owns
//! sequencing, request idempotency, the concurrency-one lease, and recovery
//! evidence across worker process restarts.

use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use agentum_core::sdd::{
    ArtifactKind, BrowserCheck, BrowserCheckAssertion, BrowserWaitUntil, CommandSpec, PlanArtifact,
    PlanTask, SpecId,
};
use agentum_store::sdd_remote_worker::{
    RemoteWorkerReservation, ReserveRemoteAuthoring, ReserveRemoteDelivery, ReserveRemotePhase,
};
use agentum_store::{Store, StoreError};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use ulid::Ulid;
use uuid::Uuid;

use super::artifacts::{
    MISSING_HASH, atomic_remove, atomic_write, content_hash, discover_specs, initialize,
    read_bytes, read_text, render_spec,
};
use super::evidence::{
    BrowserAssertion, BrowserAssertionStatus, BrowserConsoleSummary, BrowserDiagnosticCoverage,
    BrowserNetworkSummary, BrowserRuntime, BrowserTarget,
};
use super::lifecycle::{
    LifecycleError, create_owned_directories, run_verification_command, sync_authoritative_overlay,
    validate_and_apply_provider_diff, validate_plan,
};
use super::providers::{
    BundledProvider, DESIGN_BEGIN, DESIGN_END, DIFF_BEGIN, DIFF_END, PLAN_BEGIN, PLAN_END,
    ProviderAdapter, ProviderOperation, REVIEW_BEGIN, REVIEW_END, authoring_prompt,
    cancel_run as cancel_provider_run, probe_custom_provider, probe_provider, resolve_provider,
    run_artifact, run_authoring,
};
use super::remote::{
    REMOTE_SDD_SCHEMA_VERSION, RemoteArtifactPayload, RemoteAuthoringRequest,
    RemoteAuthoringResult, RemoteBrowserBlob, RemoteBrowserCheckResult, RemoteClientFrame,
    RemoteDeliveryActionRequest, RemoteDeliveryActionResult, RemoteDeliveryActionStatus,
    RemoteDeliverySnapshotRequest, RemoteDeliverySnapshotResult, RemoteImplementationEvidence,
    RemoteLifecyclePhase, RemotePhaseRequest, RemotePhaseResult, RemotePhaseStatus,
    RemoteProbeRequest, RemoteProbeResult, RemoteServerFrame, RemoteTaskCompletionEvidence,
    RemoteVerificationEvidence,
};
use super::workspace::{self, AttemptWorkspace, AuthoritativeWorkspace, SourceCheckoutMode};
use super::{lifecycle, sha256};

#[cfg(unix)]
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_STATE_BYTES: usize = 64 * 1024 * 1024;
const WORKER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum RemoteWorkerError {
    #[error("worker configuration: {0}")]
    Config(String),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("workspace: {0}")]
    Workspace(#[from] workspace::WorkspaceError),
    #[error("artifact: {0}")]
    Artifact(#[from] super::artifacts::ArtifactError),
    #[error("provider: {0}")]
    Provider(#[from] super::providers::ProviderError),
    #[error("lifecycle: {0}")]
    Lifecycle(#[from] LifecycleError),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("remote browser verification is unavailable: {0}")]
    BrowserVerificationUnavailable(String),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("time: {0}")]
    Time(#[from] time::error::Format),
    #[error("paths: {0}")]
    Path(#[from] agentum_store::paths::PathError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteWorkerConfig {
    pub schema_version: u32,
    pub host_id: String,
    pub repositories: Vec<RegisteredRepository>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisteredRepository {
    pub identity_sha256: String,
    pub artifact_set_id: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
struct ResolvedRepository {
    identity_sha256: String,
    artifact_set_id: String,
    path: PathBuf,
}

#[derive(Clone)]
pub struct RemoteSubsystemWorker {
    config: RemoteWorkerConfig,
    repositories: HashMap<String, ResolvedRepository>,
    store: Store,
}

impl std::fmt::Debug for RemoteSubsystemWorker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteSubsystemWorker")
            .field("host_id", &self.config.host_id)
            .field("repository_count", &self.repositories.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JournalOperation {
    relative_path: String,
    preimage_sha256: String,
    postimage_sha256: String,
    postimage_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JournalPreimage {
    relative_path: String,
    sha256: String,
    content_base64: Option<String>,
}

struct AttemptGuard {
    repository: PathBuf,
    workspace: Option<AttemptWorkspace>,
}

impl AttemptGuard {
    fn new(repository: &Path, workspace: AttemptWorkspace) -> Self {
        Self {
            repository: repository.to_path_buf(),
            workspace: Some(workspace),
        }
    }

    fn path(&self) -> &Path {
        &self.workspace.as_ref().expect("attempt is live").path
    }

    async fn cleanup(mut self) -> Result<(), RemoteWorkerError> {
        if let Some(workspace) = self.workspace.take() {
            workspace::remove_attempt(&self.repository, &workspace.path).await?;
        }
        Ok(())
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        // Every subprocess uses kill-on-drop. An interrupted async cleanup is
        // reconciled from the exact durable attempt path on the next request.
    }
}

impl RemoteWorkerConfig {
    pub fn load(path: &Path) -> Result<Self, RemoteWorkerError> {
        let bytes = read_owner_only_config(path)?;
        let config: Self = serde_json::from_slice(&bytes)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), RemoteWorkerError> {
        if self.schema_version != REMOTE_SDD_SCHEMA_VERSION
            || Uuid::parse_str(&self.host_id).is_err()
            || self.repositories.is_empty()
            || self.repositories.len() > 1_024
        {
            return Err(RemoteWorkerError::Config(
                "schema, host identity, or repository registration is invalid".into(),
            ));
        }
        let mut identities = HashSet::new();
        let mut canonical_paths = HashSet::new();
        for repository in &self.repositories {
            if !valid_sha256(&repository.identity_sha256)
                || repository.artifact_set_id.parse::<Ulid>().is_err()
                || !repository.path.is_absolute()
                || !identities.insert(repository.identity_sha256.clone())
            {
                return Err(RemoteWorkerError::Config(
                    "repository identities must be unique SHA-256 values".into(),
                ));
            }
            let metadata = std::fs::symlink_metadata(&repository.path).map_err(|error| {
                RemoteWorkerError::Config(format!(
                    "could not inspect registered repository {}: {error}",
                    repository.path.display()
                ))
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(RemoteWorkerError::Config(format!(
                    "registered repository is not a real directory: {}",
                    repository.path.display()
                )));
            }
            let canonical = repository.path.canonicalize()?;
            if !canonical_paths.insert(canonical) {
                return Err(RemoteWorkerError::Config(
                    "the same repository path is registered more than once".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn read_owner_only_config(path: &Path) -> Result<Vec<u8>, RemoteWorkerError> {
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            RemoteWorkerError::Config(format!(
                "could not securely open {}: {error}",
                path.display()
            ))
        })?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_CONFIG_BYTES
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(RemoteWorkerError::Config(
            "configuration must be an owner-only bounded regular file (mode 0600)".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(RemoteWorkerError::Config(
            "configuration changed while it was being read".into(),
        ));
    }
    Ok(bytes)
}

#[cfg(not(unix))]
fn read_owner_only_config(_path: &Path) -> Result<Vec<u8>, RemoteWorkerError> {
    Err(RemoteWorkerError::Config(
        "the SSH subsystem worker requires no-follow owner checks unavailable on this platform"
            .into(),
    ))
}

impl RemoteSubsystemWorker {
    pub async fn open(config_path: &Path) -> Result<Self, RemoteWorkerError> {
        let config = RemoteWorkerConfig::load(config_path)?;
        let repositories = config
            .repositories
            .iter()
            .map(|repository| {
                Ok((
                    repository.identity_sha256.clone(),
                    ResolvedRepository {
                        identity_sha256: repository.identity_sha256.clone(),
                        artifact_set_id: repository.artifact_set_id.clone(),
                        path: repository.path.canonicalize()?,
                    },
                ))
            })
            .collect::<Result<HashMap<_, _>, std::io::Error>>()?;
        let database = agentum_store::paths::data_dir()?.join("sdd-worker.sqlite");
        let store = Store::open(&database).await?;
        Ok(Self {
            config,
            repositories,
            store,
        })
    }

    #[cfg(test)]
    async fn with_store(
        config: RemoteWorkerConfig,
        store: Store,
    ) -> Result<Self, RemoteWorkerError> {
        config.validate()?;
        let repositories = config
            .repositories
            .iter()
            .map(|repository| {
                Ok((
                    repository.identity_sha256.clone(),
                    ResolvedRepository {
                        identity_sha256: repository.identity_sha256.clone(),
                        artifact_set_id: repository.artifact_set_id.clone(),
                        path: repository.path.canonicalize()?,
                    },
                ))
            })
            .collect::<Result<HashMap<_, _>, std::io::Error>>()?;
        Ok(Self {
            config,
            repositories,
            store,
        })
    }

    pub fn cancel(&self, run_id: &str) -> bool {
        cancel_provider_run(run_id) | lifecycle::cancel_run(run_id)
    }

    pub async fn handle(
        &self,
        frame: RemoteClientFrame,
    ) -> Result<RemoteServerFrame, RemoteWorkerError> {
        match frame {
            RemoteClientFrame::Probe(request) => {
                Ok(RemoteServerFrame::ProbeResult(self.probe(request).await))
            }
            RemoteClientFrame::AuthorSpec(request) => Ok(RemoteServerFrame::AuthoringResult(
                self.with_lease(&request.request_id, request.timeout_ms, || {
                    self.author(request.clone())
                })
                .await?,
            )),
            RemoteClientFrame::ExecutePhase(request) => Ok(RemoteServerFrame::PhaseResult(
                self.with_lease(&request.request_id, request.timeout_ms, || {
                    self.execute_phase(request.clone())
                })
                .await?,
            )),
            RemoteClientFrame::InspectDelivery(request) => {
                Ok(RemoteServerFrame::DeliverySnapshotResult(
                    self.with_lease(&request.request_id, request.timeout_ms, || {
                        self.inspect_delivery(request.clone())
                    })
                    .await?,
                ))
            }
            RemoteClientFrame::ExecuteDeliveryAction(request) => {
                Ok(RemoteServerFrame::DeliveryActionResult(
                    self.with_lease(&request.request_id, request.timeout_ms, || {
                        self.execute_delivery_action(request.as_ref().clone())
                    })
                    .await?,
                ))
            }
        }
    }

    async fn with_lease<T, F, Fut>(
        &self,
        request_id: &str,
        timeout_ms: u64,
        operation: F,
    ) -> Result<T, RemoteWorkerError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, RemoteWorkerError>>,
    {
        let owner_id = Uuid::new_v4().to_string();
        let milliseconds = i64::try_from(timeout_ms.min(3_600_000)).unwrap_or(3_600_000) + 30_000;
        let expires_at = (OffsetDateTime::now_utc() + time::Duration::milliseconds(milliseconds))
            .format(&Rfc3339)?;
        self.store
            .sdd_remote_worker_acquire_lease(&owner_id, request_id, &expires_at)
            .await?;
        let result = operation().await;
        let release = self.store.sdd_remote_worker_release_lease(&owner_id).await;
        match (result, release) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }

    async fn probe(&self, request: RemoteProbeRequest) -> RemoteProbeResult {
        let version_matches = request.expected_worker_version == WORKER_VERSION;
        let shape_valid = request.schema_version == REMOTE_SDD_SCHEMA_VERSION
            && request.request_id.starts_with("probe-")
            && request.request_id.len() == 38
            && request.host_id == self.config.host_id
            && valid_sha256(&request.repository_identity_sha256)
            && valid_provider_reference(&request.provider)
            && valid_base_ref(&request.base_ref)
            && !request.expected_worker_version.is_empty()
            && request.expected_worker_version.len() <= 128;
        let repository = shape_valid
            .then(|| self.repositories.get(&request.repository_identity_sha256))
            .flatten();
        let registered = repository.is_some();
        let repository_ready = match repository {
            Some(repository) => registered_repository_ready(&repository.path).await,
            None => false,
        };
        let base_commit = if repository_ready {
            match repository {
                Some(repository) => resolve_base_commit(&repository.path, &request.base_ref).await,
                None => None,
            }
        } else {
            None
        };
        let provider_capability = if base_commit.is_some() {
            if let Some(provider) = BundledProvider::get(&request.provider) {
                Some(probe_provider(provider).await)
            } else {
                probe_custom_provider(&request.provider).await.ok()
            }
        } else {
            None
        };
        let provider_ready = provider_capability
            .as_ref()
            .is_some_and(|capability| capability.available);
        let reason = if !shape_valid {
            Some("request_contract_mismatch".into())
        } else if !version_matches {
            Some("worker_version_mismatch".into())
        } else if !registered {
            Some("repository_not_registered".into())
        } else if !repository_ready {
            Some("repository_unavailable".into())
        } else if base_commit.is_none() {
            Some("base_ref_unavailable".into())
        } else if !provider_ready {
            Some(
                provider_capability
                    .and_then(|capability| capability.reason)
                    .map(|reason| reason.chars().take(512).collect())
                    .unwrap_or_else(|| "provider_not_ready".into()),
            )
        } else {
            None
        };
        RemoteProbeResult {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: request.request_id,
            host_id: self.config.host_id.clone(),
            worker_version: WORKER_VERSION.into(),
            repository_registered: registered,
            artifact_set_id: repository.map(|repository| repository.artifact_set_id.clone()),
            base_commit,
            provider_ready,
            reason,
        }
    }

    fn repository(&self, identity: &str) -> Result<&ResolvedRepository, RemoteWorkerError> {
        self.repositories.get(identity).ok_or_else(|| {
            RemoteWorkerError::Invalid("repository identity is not registered on this host".into())
        })
    }

    async fn author(
        &self,
        request: RemoteAuthoringRequest,
    ) -> Result<RemoteAuthoringResult, RemoteWorkerError> {
        self.validate_authoring_request(&request)?;
        let repository = self.repository(&request.repository_identity_sha256)?;
        if request.artifact_set_id != repository.artifact_set_id {
            return Err(RemoteWorkerError::Invalid(
                "artifact-set identity does not match repository registration".into(),
            ));
        }
        let spec_id: SpecId = request
            .spec_id
            .parse()
            .map_err(|_| RemoteWorkerError::Invalid("spec identity is invalid".into()))?;
        let provider = resolve_provider(&request.provider)
            .map_err(|error| RemoteWorkerError::Invalid(error.to_string()))?;
        let workspace = workspace::plan_authoritative(
            &repository.identity_sha256,
            &repository.path,
            &request.run_id,
            &spec_id,
            &request.title,
            &request.base_commit,
            SourceCheckoutMode::CommittedBase,
        )
        .await?;
        if workspace.base_commit != request.base_commit {
            return Err(RemoteWorkerError::Invalid(
                "base commit did not resolve to the approved object".into(),
            ));
        }
        let request_json = serde_json::to_vec(&request)?;
        let request_sha256 = sha256(&request_json);
        let initial_state = sha256(format!(
            "{}\n{}\n{}",
            workspace.fingerprint, request.spec_id, request.run_id
        ));
        let reservation = self
            .store
            .sdd_remote_worker_reserve_authoring(ReserveRemoteAuthoring {
                request_id: &request.request_id,
                request_sha256: &request_sha256,
                run_id: &request.run_id,
                host_id: &request.host_id,
                repository_identity_sha256: &request.repository_identity_sha256,
                artifact_set_id: &request.artifact_set_id,
                spec_id: &request.spec_id,
                base_commit: &request.base_commit,
                provider: &request.provider,
                authoritative_path: &workspace.path.to_string_lossy(),
                branch_name: &workspace.branch_name,
                initial_workspace_state_sha256: &initial_state,
            })
            .await?;
        match reservation {
            RemoteWorkerReservation::Replay(response) => {
                return Ok(serde_json::from_str(&response)?);
            }
            RemoteWorkerReservation::RecoveryRequired(record) => {
                return self
                    .recover_authoring(
                        &request,
                        &request_sha256,
                        repository,
                        &workspace,
                        record.attempt_path.as_deref(),
                    )
                    .await;
            }
            RemoteWorkerReservation::Started => {}
        }

        let result = self
            .author_inner(&request, repository, &workspace, &provider)
            .await;
        match result {
            Ok(result) => {
                let response = serde_json::to_string(&result)?;
                self.store
                    .sdd_remote_worker_complete_authoring(
                        &request.request_id,
                        &request_sha256,
                        &request.run_id,
                        &result.workspace_state_sha256,
                        &response,
                    )
                    .await?;
                Ok(result)
            }
            Err(error) => {
                let attempt_path = workspace::attempt_path(
                    &workspace.path,
                    &format!("author-{}", &request.request_id[7..]),
                )?;
                let _ = workspace::recover_interrupted_attempt(
                    &repository.path,
                    &workspace.path,
                    &attempt_path,
                )
                .await;
                let _ = self.recover_patches(&request.run_id, &workspace.path).await;
                let _ = workspace::recover_interrupted_create(
                    &repository.path,
                    &workspace.path,
                    &workspace.branch_name,
                )
                .await;
                let result = self
                    .authoring_failure(&request, &workspace.path, &error)
                    .await;
                let response = serde_json::to_string(&result)?;
                self.store
                    .sdd_remote_worker_complete_failure(
                        &request.request_id,
                        &request_sha256,
                        &request.run_id,
                        &bounded_error(&error.to_string()),
                        &response,
                    )
                    .await?;
                Ok(result)
            }
        }
    }

    async fn author_inner(
        &self,
        request: &RemoteAuthoringRequest,
        repository: &ResolvedRepository,
        workspace: &AuthoritativeWorkspace,
        provider: &ProviderAdapter,
    ) -> Result<RemoteAuthoringResult, RemoteWorkerError> {
        if request.source_checkout == "require_clean"
            && !registered_repository_clean(&repository.path).await
        {
            return Err(RemoteWorkerError::Invalid(
                "remote source checkout is dirty; explicitly choose committed_base".into(),
            ));
        }
        workspace::materialize_authoritative(&repository.path, workspace).await?;
        let spec_id: SpecId = request.spec_id.parse().map_err(|_| {
            RemoteWorkerError::Invalid("spec identity changed during authoring".into())
        })?;
        let artifact_set_id: Ulid = request
            .artifact_set_id
            .parse()
            .map_err(|_| RemoteWorkerError::Invalid("artifact-set ULID is invalid".into()))?;
        let root = initialize(&workspace.path, &spec_id, &request.title, artifact_set_id)?;
        let draft = render_spec(
            &spec_id,
            1,
            &request.title,
            None,
            &format!(
                "# {}\n\n## Requirements\n\n- RQ-001: {}\n\n## Acceptance criteria\n\n- AC-001: The authored specification defines verifiable completion conditions.",
                request.title.trim(),
                request.goal.trim()
            ),
        )?;
        let spec_path = root.spec_dir.join("spec.md");
        let draft_hash = atomic_write(&spec_path, draft.as_bytes(), Some(MISSING_HASH))?;

        let attempt_id = format!("author-{}", &request.request_id[7..]);
        let attempt = self
            .create_attempt(
                repository,
                &workspace.path,
                &workspace.base_commit,
                &attempt_id,
                &request.request_id,
                &request_sha256(request)?,
            )
            .await?;
        let staging = attempt.path().join(".agentum/staging/result.txt");
        create_owned_directories(
            attempt.path(),
            staging.parent().expect("staging has parent"),
        )?;
        let body = run_authoring(
            &format!("{}:{}", request.run_id, attempt_id),
            provider,
            &attempt.path().to_string_lossy(),
            &authoring_prompt(&request.title, &request.goal),
            &staging.to_string_lossy(),
        )
        .await?;
        let authored = render_spec(&spec_id, 2, &request.title, None, &body)?;
        if authored.len().saturating_add(4_096) > request.output_limit {
            return Err(RemoteWorkerError::Invalid(
                "authored specification exceeds the negotiated response limit".into(),
            ));
        }
        self.publish(
            &request.request_id,
            &request.run_id,
            &workspace.path,
            vec![(
                root.spec_relative_path.clone(),
                Some(authored.as_bytes().to_vec()),
            )],
            Some((&root.spec_relative_path, &draft_hash)),
        )
        .await?;
        attempt.cleanup().await?;

        let workspace_state_sha256 = workspace_state(&workspace.path).await?;
        let artifact_set_sha256 = artifact_set_state(&workspace.path)?;
        let (spec, content_sha256) = read_text(&spec_path)?;
        let result = RemoteAuthoringResult {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            run_id: request.run_id.clone(),
            spec_id: request.spec_id.clone(),
            spec_revision: 2,
            status: RemotePhaseStatus::Succeeded,
            workspace_state_sha256,
            artifact_set_sha256,
            spec: Some(RemoteArtifactPayload {
                kind: "specification".into(),
                relative_path: root.spec_relative_path,
                content_sha256,
                content: spec,
            }),
            error_code: None,
        };
        if serde_json::to_vec(&result)?.len() > request.output_limit {
            return Err(RemoteWorkerError::Invalid(
                "authoring response exceeds the negotiated output limit".into(),
            ));
        }
        Ok(result)
    }

    async fn recover_authoring(
        &self,
        request: &RemoteAuthoringRequest,
        request_sha256: &str,
        repository: &ResolvedRepository,
        workspace: &AuthoritativeWorkspace,
        attempt_path: Option<&str>,
    ) -> Result<RemoteAuthoringResult, RemoteWorkerError> {
        if let Some(path) = attempt_path {
            workspace::recover_interrupted_attempt(
                &repository.path,
                &workspace.path,
                Path::new(path),
            )
            .await?;
        }
        self.recover_patches(&request.run_id, &workspace.path)
            .await?;
        workspace::recover_interrupted_create(
            &repository.path,
            &workspace.path,
            &workspace.branch_name,
        )
        .await?;
        let result = RemoteAuthoringResult {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            run_id: request.run_id.clone(),
            spec_id: request.spec_id.clone(),
            spec_revision: 1,
            status: RemotePhaseStatus::Failed,
            workspace_state_sha256: sha256(b"recovered-empty-authoring-workspace"),
            artifact_set_sha256: sha256(b"recovered-empty-artifact-set"),
            spec: None,
            error_code: Some("recovered_interrupted_authoring".into()),
        };
        let response = serde_json::to_string(&result)?;
        self.store
            .sdd_remote_worker_complete_failure(
                &request.request_id,
                request_sha256,
                &request.run_id,
                "interrupted authoring was rolled back; retry with a new request",
                &response,
            )
            .await?;
        Ok(result)
    }

    async fn execute_phase(
        &self,
        request: RemotePhaseRequest,
    ) -> Result<RemotePhaseResult, RemoteWorkerError> {
        self.validate_phase_request(&request)?;
        let repository = self.repository(&request.repository_identity_sha256)?;
        if request.artifact_set_id != repository.artifact_set_id {
            return Err(RemoteWorkerError::Invalid(
                "artifact-set identity does not match repository registration".into(),
            ));
        }
        let provider = resolve_provider(&request.provider)
            .map_err(|error| RemoteWorkerError::Invalid(error.to_string()))?;
        let run = self
            .store
            .sdd_remote_worker_run(&request.run_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(request.run_id.clone()))?;
        let authoritative = PathBuf::from(&run.authoritative_path);
        let actual_state = workspace_state(&authoritative).await?;
        if actual_state != request.expected_workspace_state_sha256 {
            return Err(RemoteWorkerError::Invalid(
                "authoritative workspace changed outside the durable checkpoint".into(),
            ));
        }
        let request_sha256 = sha256(serde_json::to_vec(&request)?);
        let reservation = self
            .store
            .sdd_remote_worker_reserve_phase(ReserveRemotePhase {
                request_id: &request.request_id,
                request_sha256: &request_sha256,
                run_id: &request.run_id,
                host_id: &request.host_id,
                repository_identity_sha256: &request.repository_identity_sha256,
                artifact_set_id: &request.artifact_set_id,
                spec_id: &request.spec_id,
                spec_revision: request.spec_revision,
                base_commit: &request.base_commit,
                provider: &request.provider,
                approval_digest: &request.approval_digest,
                phase: phase_name(request.phase),
                completed_phases: i64::from(phase_ordinal(request.phase)),
                expected_workspace_state_sha256: &request.expected_workspace_state_sha256,
                previous_result_sha256: &request.previous_result_sha256,
            })
            .await?;
        match reservation {
            RemoteWorkerReservation::Replay(response) => {
                return Ok(serde_json::from_str(&response)?);
            }
            RemoteWorkerReservation::RecoveryRequired(record) => {
                return self
                    .recover_phase(
                        &request,
                        &request_sha256,
                        repository,
                        &authoritative,
                        record.attempt_path.as_deref(),
                    )
                    .await;
            }
            RemoteWorkerReservation::Started => {}
        }

        let result = self
            .execute_phase_inner(&request, repository, &authoritative, &provider)
            .await;
        match result {
            Ok(result) => {
                let response = serde_json::to_string(&result)?;
                let result_sha256 = sha256(response.as_bytes());
                let next = next_phase_name(request.phase);
                self.store
                    .sdd_remote_worker_complete_phase(
                        &request.request_id,
                        &request_sha256,
                        &request.run_id,
                        next,
                        i64::from(phase_ordinal(request.phase)) + 1,
                        &result.workspace_state_sha256,
                        &result_sha256,
                        request.phase == RemoteLifecyclePhase::Review,
                        &response,
                    )
                    .await?;
                Ok(result)
            }
            Err(error) => {
                if let Some(record) = self
                    .store
                    .sdd_remote_worker_request(&request.request_id)
                    .await?
                {
                    if let Some(path) = record.attempt_path {
                        let _ = workspace::recover_interrupted_attempt(
                            &repository.path,
                            &authoritative,
                            Path::new(&path),
                        )
                        .await;
                    }
                }
                let _ = self.recover_patches(&request.run_id, &authoritative).await;
                let result = self.phase_failure(&request, &authoritative, &error).await;
                let response = serde_json::to_string(&result)?;
                self.store
                    .sdd_remote_worker_complete_failure(
                        &request.request_id,
                        &request_sha256,
                        &request.run_id,
                        &bounded_error(&error.to_string()),
                        &response,
                    )
                    .await?;
                Ok(result)
            }
        }
    }

    async fn execute_phase_inner(
        &self,
        request: &RemotePhaseRequest,
        repository: &ResolvedRepository,
        authoritative: &Path,
        provider: &ProviderAdapter,
    ) -> Result<RemotePhaseResult, RemoteWorkerError> {
        let spec_id: SpecId = request
            .spec_id
            .parse()
            .map_err(|_| RemoteWorkerError::Invalid("spec identity is invalid".into()))?;
        let discovered = discover_specs(authoritative)?
            .ok_or_else(|| RemoteWorkerError::Invalid("artifact set is missing".into()))?;
        let spec = discovered
            .specs
            .iter()
            .find(|spec| spec.header.id == spec_id)
            .ok_or_else(|| {
                RemoteWorkerError::Invalid("approved specification is missing".into())
            })?;
        if spec.header.revision != request.spec_revision {
            return Err(RemoteWorkerError::Invalid(
                "specification revision does not match approval".into(),
            ));
        }
        let spec_directory = authoritative
            .join(".agentum/specs")
            .join(&spec.directory_name);
        let mut artifacts = Vec::new();
        let mut evidence_summary = None;
        match request.phase {
            RemoteLifecyclePhase::Design => {
                artifacts.push(
                    self.execute_text_artifact(
                        request,
                        repository,
                        authoritative,
                        provider,
                        ArtifactKind::Design,
                        &spec_directory,
                        None,
                    )
                    .await?,
                );
            }
            RemoteLifecyclePhase::Planning => {
                artifacts.push(
                    self.execute_text_artifact(
                        request,
                        repository,
                        authoritative,
                        provider,
                        ArtifactKind::Plan,
                        &spec_directory,
                        None,
                    )
                    .await?,
                );
            }
            RemoteLifecyclePhase::Implementation => {
                evidence_summary = Some(
                    self.execute_implementation(
                        request,
                        repository,
                        authoritative,
                        provider,
                        &spec_directory,
                    )
                    .await?,
                );
            }
            RemoteLifecyclePhase::Verification => {
                evidence_summary = Some(
                    self.execute_verification(request, repository, authoritative, &spec_directory)
                        .await?,
                );
            }
            RemoteLifecyclePhase::Review => {
                let verification_response = self
                    .store
                    .sdd_remote_worker_phase_response(&request.run_id, "verification")
                    .await?
                    .ok_or_else(|| {
                        RemoteWorkerError::Invalid("verification evidence is missing".into())
                    })?;
                let verification: RemotePhaseResult = serde_json::from_str(&verification_response)?;
                let evidence = verification.evidence_summary.ok_or_else(|| {
                    RemoteWorkerError::Invalid("verification evidence summary is missing".into())
                })?;
                let verification_evidence: RemoteVerificationEvidence =
                    serde_json::from_str(&evidence)?;
                let review_evidence = serde_json::json!({
                    "schemaVersion": verification_evidence.schema_version,
                    "commandResults": verification_evidence.command_results,
                    "browserResults": verification_evidence.browser_results.iter().map(|result| {
                        serde_json::json!({
                            "checkId": result.check_id,
                            "capturedAt": result.captured_at,
                            "status": result.status,
                            "durationMs": result.duration_ms,
                            "outputExcerpt": result.output_excerpt,
                            "target": result.target,
                            "browser": result.browser,
                            "assertions": result.assertions,
                            "console": result.console,
                            "network": result.network,
                            "blobs": result.blobs.iter().map(|blob| serde_json::json!({
                                "sha256": blob.sha256,
                                "byteLength": blob.byte_length,
                                "mediaType": blob.media_type,
                                "role": blob.role
                            })).collect::<Vec<_>>()
                        })
                    }).collect::<Vec<_>>()
                })
                .to_string();
                artifacts.push(
                    self.execute_text_artifact(
                        request,
                        repository,
                        authoritative,
                        provider,
                        ArtifactKind::Review,
                        &spec_directory,
                        Some(&review_evidence),
                    )
                    .await?,
                );
            }
            RemoteLifecyclePhase::Ready => {
                return Err(RemoteWorkerError::Invalid(
                    "Ready is a checkpoint, not an executable remote phase".into(),
                ));
            }
        }
        let workspace_state_sha256 = workspace_state(authoritative).await?;
        let artifact_set_sha256 = artifact_set_state(authoritative)?;
        let evidence_sha256 = sha256(
            evidence_summary
                .as_deref()
                .unwrap_or(&artifact_set_sha256)
                .as_bytes(),
        );
        let result = RemotePhaseResult {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            phase: request.phase,
            status: RemotePhaseStatus::Succeeded,
            workspace_state_sha256,
            artifact_set_sha256,
            evidence_sha256,
            evidence_summary,
            artifacts,
            error_code: None,
        };
        if serde_json::to_vec(&result)?.len() > request.output_limit {
            return Err(RemoteWorkerError::Invalid(
                "phase response exceeds the negotiated output limit".into(),
            ));
        }
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_text_artifact(
        &self,
        request: &RemotePhaseRequest,
        repository: &ResolvedRepository,
        authoritative: &Path,
        provider: &ProviderAdapter,
        kind: ArtifactKind,
        spec_directory: &Path,
        verification_evidence: Option<&str>,
    ) -> Result<RemoteArtifactPayload, RemoteWorkerError> {
        let role = if kind == ArtifactKind::Review {
            "independent-review"
        } else {
            kind.file_name()
        };
        let attempt_id = format!("{}-{}", role.replace('.', "-"), &request.request_id[7..]);
        let attempt = self
            .create_attempt(
                repository,
                authoritative,
                &request.base_commit,
                &attempt_id,
                &request.request_id,
                &sha256(serde_json::to_vec(request)?),
            )
            .await?;
        let staging = attempt.path().join(".agentum/staging/result.txt");
        create_owned_directories(attempt.path(), staging.parent().expect("staging parent"))?;
        let (operation, begin, end, prompt) = artifact_prompt(
            kind,
            attempt.path(),
            spec_directory
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| RemoteWorkerError::Invalid("spec directory is invalid".into()))?,
            &request.spec_id,
            request.spec_revision,
            verification_evidence,
        )?;
        let content = run_artifact(
            &format!("{}:{}", request.run_id, attempt_id),
            provider,
            operation,
            &attempt.path().to_string_lossy(),
            &prompt,
            &staging.to_string_lossy(),
            begin,
            end,
        )
        .await?;
        validate_text_artifact(kind, &content, &request.spec_id, request.spec_revision)?;
        if content.len().saturating_add(4_096) > request.output_limit {
            return Err(RemoteWorkerError::Invalid(
                "phase artifact exceeds the negotiated response limit".into(),
            ));
        }
        let relative_path = format!(
            ".agentum/specs/{}/{}",
            spec_directory.file_name().unwrap().to_string_lossy(),
            kind.file_name()
        );
        self.publish(
            &request.request_id,
            &request.run_id,
            authoritative,
            vec![(relative_path.clone(), Some(content.as_bytes().to_vec()))],
            None,
        )
        .await?;
        attempt.cleanup().await?;
        Ok(RemoteArtifactPayload {
            kind: phase_name(request.phase).into(),
            relative_path,
            content_sha256: sha256(content.as_bytes()),
            content,
        })
    }

    async fn execute_implementation(
        &self,
        request: &RemotePhaseRequest,
        repository: &ResolvedRepository,
        authoritative: &Path,
        provider: &ProviderAdapter,
        spec_directory: &Path,
    ) -> Result<String, RemoteWorkerError> {
        let (content, _) = read_text(&spec_directory.join("plan.json"))?;
        let spec_id: SpecId = request.spec_id.parse().map_err(|_| {
            RemoteWorkerError::Invalid("plan specification identity is invalid".into())
        })?;
        validate_plan(&content, &spec_id, request.spec_revision)?;
        let plan: PlanArtifact = serde_json::from_str(&content)?;
        let mut completed = HashSet::new();
        let mut evidence = Vec::with_capacity(plan.tasks.len());
        while completed.len() < plan.tasks.len() {
            let task = plan
                .tasks
                .iter()
                .find(|task| {
                    !completed.contains(&task.id)
                        && task
                            .dependencies
                            .iter()
                            .all(|dependency| completed.contains(dependency))
                })
                .ok_or_else(|| RemoteWorkerError::Invalid("plan DAG cannot advance".into()))?;
            evidence.push(
                self.execute_task(request, repository, authoritative, provider, task)
                    .await?,
            );
            completed.insert(task.id.clone());
        }
        Ok(serde_json::to_string(&RemoteImplementationEvidence {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            spec_id: request.spec_id.clone(),
            spec_revision: request.spec_revision,
            tasks: evidence,
        })?)
    }

    async fn execute_task(
        &self,
        request: &RemotePhaseRequest,
        repository: &ResolvedRepository,
        authoritative: &Path,
        provider: &ProviderAdapter,
        task: &PlanTask,
    ) -> Result<RemoteTaskCompletionEvidence, RemoteWorkerError> {
        let attempt_id = format!(
            "task-{}-{}",
            safe_token(&task.id),
            &sha256(format!("{}:{}", request.request_id, task.id))[..16]
        );
        let attempt = self
            .create_attempt(
                repository,
                authoritative,
                &request.base_commit,
                &attempt_id,
                &request.request_id,
                &sha256(serde_json::to_vec(request)?),
            )
            .await?;
        let staging = attempt.path().join(".agentum/staging/result.diff");
        create_owned_directories(attempt.path(), staging.parent().expect("staging parent"))?;
        let prompt = format!(
            "You are an implementation agent in an Agentum-owned workflow. Read the repository and its .agentum spec/design/plan artifacts. Implement only task {:?}: {:?}. Dependencies are already applied. You may propose changes only within these write scopes: {:?}. Satisfy acceptance criteria {:?}. Do not edit files, run Git, commit, push, contact trackers, or emit binary/rename patches. Return one ordinary UTF-8 unified Git diff between literal lines {DIFF_BEGIN} and {DIFF_END}. Agentum alone validates and applies it.",
            task.id, task.objective, task.write_scopes, task.acceptance_criteria
        );
        let diff = run_artifact(
            &format!("{}:{}", request.run_id, attempt_id),
            provider,
            ProviderOperation::ImplementationDiff,
            &attempt.path().to_string_lossy(),
            &prompt,
            &staging.to_string_lossy(),
            DIFF_BEGIN,
            DIFF_END,
        )
        .await?;
        let patch_sha256 = sha256(diff.as_bytes());
        let paths = validate_and_apply_provider_diff(attempt.path(), &diff, task).await?;
        let mut write_set = paths.clone();
        write_set.sort();
        let write_set_sha256 = sha256(serde_json::to_vec(&write_set)?);
        let mut publications = Vec::with_capacity(paths.len());
        for relative in paths {
            let path = attempt.path().join(&relative);
            let postimage = match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                    return Err(RemoteWorkerError::Invalid(format!(
                        "provider produced an unsafe task path: {relative}"
                    )));
                }
                Ok(_) => Some(read_bytes(&path)?.0),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            publications.push((relative, postimage));
        }
        self.publish(
            &request.request_id,
            &request.run_id,
            authoritative,
            publications,
            None,
        )
        .await?;
        attempt.cleanup().await?;
        Ok(RemoteTaskCompletionEvidence {
            task_id: task.id.clone(),
            patch_sha256,
            write_set_sha256,
        })
    }

    async fn execute_verification(
        &self,
        request: &RemotePhaseRequest,
        repository: &ResolvedRepository,
        authoritative: &Path,
        spec_directory: &Path,
    ) -> Result<String, RemoteWorkerError> {
        let (content, _) = read_text(&spec_directory.join("plan.json"))?;
        let spec_id: SpecId = request.spec_id.parse().map_err(|_| {
            RemoteWorkerError::Invalid("plan specification identity is invalid".into())
        })?;
        validate_plan(&content, &spec_id, request.spec_revision)?;
        let plan: PlanArtifact = serde_json::from_str(&content)?;
        let attempt_id = format!("verification-{}", &request.request_id[7..]);
        let attempt = self
            .create_attempt(
                repository,
                authoritative,
                &request.base_commit,
                &attempt_id,
                &request.request_id,
                &sha256(serde_json::to_vec(request)?),
            )
            .await?;
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
        let mut command_results = Vec::with_capacity(commands.len());
        for (index, command) in commands.iter().enumerate() {
            let result = run_verification_command(
                &format!("{}:{}:{index}", request.run_id, attempt_id),
                attempt.path(),
                command,
                index as i64,
            )
            .await?;
            let success = result.status == "succeeded";
            command_results.push(result);
            if !success {
                return Err(RemoteWorkerError::Invalid(
                    "a required verification command did not succeed".into(),
                ));
            }
        }
        let browser_checks = plan
            .tasks
            .iter()
            .flat_map(|task| task.browser_checks.iter().cloned())
            .collect::<Vec<_>>();
        let browser_results = if browser_checks.is_empty() {
            Vec::new()
        } else {
            self.execute_remote_browser_checks(request, &browser_checks)
                .await?
        };
        attempt.cleanup().await?;
        let evidence = serde_json::to_string(&RemoteVerificationEvidence {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            command_results,
            browser_results,
        })?;
        if evidence.len().saturating_add(4_096) > request.output_limit {
            return Err(RemoteWorkerError::Invalid(
                "verification evidence exceeds the negotiated response limit".into(),
            ));
        }
        Ok(evidence)
    }

    async fn execute_remote_browser_checks(
        &self,
        request: &RemotePhaseRequest,
        checks: &[BrowserCheck],
    ) -> Result<Vec<RemoteBrowserCheckResult>, RemoteWorkerError> {
        if checks.len() > 32 {
            return Err(RemoteWorkerError::BrowserVerificationUnavailable(
                "a remote verification phase is limited to 32 browser checks".into(),
            ));
        }
        for check in checks {
            lifecycle::validate_browser_check(check).map_err(|error| {
                RemoteWorkerError::BrowserVerificationUnavailable(error.to_string())
            })?;
        }
        let context_key = format!("remote-sdd-{}", request.request_id);
        let (_, port) = crate::cdp_browser::ensure_local_cdp_browser_for(&context_key)
            .await
            .map_err(|error| {
                RemoteWorkerError::BrowserVerificationUnavailable(error.to_string())
            })?;
        let execution = async {
            let browser_version = crate::cdp_http::cdp_http_json(&format!(
                "{}/json/version",
                crate::cdp_browser::cdp_endpoint_for(port)
            ))
            .await
            .ok()
            .and_then(|value| {
                value
                    .get("Browser")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "Chromium/unknown".into());
            let (browser_name, browser_version) = browser_version
                .split_once('/')
                .map(|(name, version)| (name.to_ascii_lowercase(), version.to_owned()))
                .unwrap_or_else(|| ("chromium".into(), "unknown".into()));
            let mut results = Vec::with_capacity(checks.len());
            for check in checks {
                let context = crate::cdp_driver::run_browser_op(
                    "new_context",
                    &serde_json::json!({ "cdpPort": port }),
                )
                .await
                .map_err(|error| {
                    RemoteWorkerError::BrowserVerificationUnavailable(error.to_string())
                })?;
                let target = context
                    .get("target")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        RemoteWorkerError::BrowserVerificationUnavailable(
                            "browser context returned no target".into(),
                        )
                    })?
                    .to_owned();
                let browser_context_id = context
                    .get("browser_context_id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        RemoteWorkerError::BrowserVerificationUnavailable(
                            "browser context returned no identity".into(),
                        )
                    })?
                    .to_owned();
                let url = reqwest::Url::parse(&check.url).map_err(|error| {
                    RemoteWorkerError::BrowserVerificationUnavailable(error.to_string())
                })?;
                let approved_origin = url.origin().ascii_serialization();
                let origin_guard =
                    crate::cdp_driver::start_sdd_origin_guard(port, &target, &approved_origin)
                        .await
                        .map_err(|error| {
                            RemoteWorkerError::BrowserVerificationUnavailable(error.to_string())
                        })?;
                let item = tokio::time::timeout(
                    Duration::from_millis(check.timeout_ms),
                    execute_remote_browser_check(
                        port,
                        &target,
                        check,
                        &browser_name,
                        &browser_version,
                    ),
                )
                .await
                .map_err(|_| {
                    RemoteWorkerError::BrowserVerificationUnavailable(format!(
                        "browser check {} exceeded {}ms",
                        check.id, check.timeout_ms
                    ))
                })?;
                let guard_result = origin_guard.stop().await.map_err(|error| {
                    RemoteWorkerError::BrowserVerificationUnavailable(error.to_string())
                });
                let _ = crate::cdp_driver::run_browser_op(
                    "close_context",
                    &serde_json::json!({
                        "cdpPort": port,
                        "browser_context_id": browser_context_id
                    }),
                )
                .await;
                let item = item?;
                guard_result?;
                if item.status != "passed" {
                    return Err(RemoteWorkerError::BrowserVerificationUnavailable(format!(
                        "browser check {} did not satisfy every declared assertion",
                        check.id
                    )));
                }
                results.push(item);
            }
            Ok(results)
        }
        .await;
        let _ = crate::cdp_browser::stop_local_cdp_browser_for(&context_key).await;
        execution
    }

    async fn create_attempt(
        &self,
        repository: &ResolvedRepository,
        authoritative: &Path,
        base_commit: &str,
        attempt_id: &str,
        request_id: &str,
        request_sha256: &str,
    ) -> Result<AttemptGuard, RemoteWorkerError> {
        let expected = ["reserved", "materializing", "running"];
        let path = workspace::attempt_path(authoritative, attempt_id)?;
        self.store
            .sdd_remote_worker_mark_stage(
                request_id,
                request_sha256,
                &expected,
                "materializing",
                Some(&path.to_string_lossy()),
            )
            .await?;
        let attempt = workspace::create_attempt(
            &repository.path,
            authoritative,
            attempt_id,
            base_commit,
            None,
        )
        .await?;
        sync_authoritative_overlay(authoritative, &attempt.path).await?;
        self.store
            .sdd_remote_worker_mark_stage(
                request_id,
                request_sha256,
                &["materializing"],
                "running",
                Some(&attempt.path.to_string_lossy()),
            )
            .await?;
        Ok(AttemptGuard::new(&repository.path, attempt))
    }

    async fn publish(
        &self,
        request_id: &str,
        run_id: &str,
        authoritative: &Path,
        publications: Vec<(String, Option<Vec<u8>>)>,
        exact_expected: Option<(&str, &str)>,
    ) -> Result<(), RemoteWorkerError> {
        if publications.is_empty() {
            return Err(RemoteWorkerError::Invalid(
                "an empty authoritative publication is forbidden".into(),
            ));
        }
        let patch_id = Uuid::new_v4().to_string();
        let mut operations = Vec::with_capacity(publications.len());
        let mut preimages = Vec::with_capacity(publications.len());
        for (relative, postimage) in &publications {
            agentum_core::sdd::validate_relative_path(relative).map_err(|_| {
                RemoteWorkerError::Invalid(format!("unsafe publication path: {relative}"))
            })?;
            let target = authoritative.join(relative);
            let (preimage, preimage_hash) = match read_bytes(&target) {
                Ok((bytes, hash)) => (Some(bytes), hash),
                Err(super::artifacts::ArtifactError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    (None, MISSING_HASH.into())
                }
                Err(error) => return Err(error.into()),
            };
            if let Some((expected_path, expected_hash)) = exact_expected
                && expected_path == relative
                && expected_hash != preimage_hash
            {
                return Err(RemoteWorkerError::Invalid(
                    "artifact changed before publication".into(),
                ));
            }
            let postimage_hash = postimage
                .as_ref()
                .map_or_else(|| MISSING_HASH.into(), sha256);
            operations.push(JournalOperation {
                relative_path: relative.clone(),
                preimage_sha256: preimage_hash.clone(),
                postimage_sha256: postimage_hash,
                postimage_base64: postimage
                    .as_ref()
                    .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)),
            });
            preimages.push(JournalPreimage {
                relative_path: relative.clone(),
                sha256: preimage_hash,
                content_base64: preimage
                    .as_ref()
                    .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)),
            });
        }
        self.store
            .sdd_remote_worker_reserve_patch(
                &patch_id,
                request_id,
                run_id,
                &serde_json::to_string(&operations)?,
                &serde_json::to_string(&preimages)?,
            )
            .await?;
        let mut applied = Vec::new();
        for (index, operation) in operations.iter().enumerate() {
            let target = authoritative.join(&operation.relative_path);
            let result = match &operation.postimage_base64 {
                Some(encoded) => {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .map_err(|_| {
                            RemoteWorkerError::Invalid("journal encoding failed".into())
                        })?;
                    atomic_write(&target, &bytes, Some(&operation.preimage_sha256)).map(|_| ())
                }
                None => atomic_remove(&target, &operation.preimage_sha256),
            };
            if let Err(error) = result {
                let rollback =
                    rollback_operations(authoritative, &operations, &preimages, &applied);
                let message = if let Err(rollback) = rollback {
                    format!("publication failed: {error}; rollback failed: {rollback}")
                } else {
                    error.to_string()
                };
                let _ = self
                    .store
                    .sdd_remote_worker_fail_patch(&patch_id, &bounded_error(&message))
                    .await;
                return Err(RemoteWorkerError::Invalid(message));
            }
            applied.push(index);
        }
        self.store
            .sdd_remote_worker_complete_patch(&patch_id)
            .await?;
        Ok(())
    }

    async fn recover_patches(
        &self,
        run_id: &str,
        authoritative: &Path,
    ) -> Result<(), RemoteWorkerError> {
        for patch in self
            .store
            .sdd_remote_worker_unfinished_patches(run_id)
            .await?
        {
            let operations: Vec<JournalOperation> = serde_json::from_str(&patch.operations_json)?;
            let preimages: Vec<JournalPreimage> = serde_json::from_str(&patch.preimages_json)?;
            let all: Vec<usize> = (0..operations.len()).collect();
            rollback_operations(authoritative, &operations, &preimages, &all)?;
            self.store
                .sdd_remote_worker_fail_patch(
                    &patch.patch_id,
                    "recovered after worker interruption",
                )
                .await?;
        }
        Ok(())
    }

    async fn recover_phase(
        &self,
        request: &RemotePhaseRequest,
        request_sha256: &str,
        repository: &ResolvedRepository,
        authoritative: &Path,
        attempt_path: Option<&str>,
    ) -> Result<RemotePhaseResult, RemoteWorkerError> {
        if let Some(path) = attempt_path {
            workspace::recover_interrupted_attempt(
                &repository.path,
                authoritative,
                Path::new(path),
            )
            .await?;
        }
        self.recover_patches(&request.run_id, authoritative).await?;
        let result = RemotePhaseResult {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            phase: request.phase,
            status: RemotePhaseStatus::Failed,
            workspace_state_sha256: workspace_state(authoritative).await?,
            artifact_set_sha256: artifact_set_state(authoritative)?,
            evidence_sha256: sha256(b"interrupted remote phase recovered"),
            evidence_summary: None,
            artifacts: Vec::new(),
            error_code: Some("recovered_interrupted_phase".into()),
        };
        let response = serde_json::to_string(&result)?;
        self.store
            .sdd_remote_worker_complete_failure(
                &request.request_id,
                request_sha256,
                &request.run_id,
                "interrupted phase was rolled back; reopen and retry",
                &response,
            )
            .await?;
        Ok(result)
    }

    async fn authoring_failure(
        &self,
        request: &RemoteAuthoringRequest,
        authoritative: &Path,
        error: &RemoteWorkerError,
    ) -> RemoteAuthoringResult {
        RemoteAuthoringResult {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            run_id: request.run_id.clone(),
            spec_id: request.spec_id.clone(),
            spec_revision: 1,
            status: if error.to_string().contains("canceled") {
                RemotePhaseStatus::Canceled
            } else {
                RemotePhaseStatus::Failed
            },
            workspace_state_sha256: workspace_state(authoritative)
                .await
                .unwrap_or_else(|_| sha256(b"unavailable authoring workspace")),
            artifact_set_sha256: artifact_set_state(authoritative)
                .unwrap_or_else(|_| sha256(b"unavailable authoring artifacts")),
            spec: None,
            error_code: Some(error_code(error)),
        }
    }

    async fn phase_failure(
        &self,
        request: &RemotePhaseRequest,
        authoritative: &Path,
        error: &RemoteWorkerError,
    ) -> RemotePhaseResult {
        RemotePhaseResult {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            phase: request.phase,
            status: if error.to_string().contains("canceled") {
                RemotePhaseStatus::Canceled
            } else {
                RemotePhaseStatus::Failed
            },
            workspace_state_sha256: workspace_state(authoritative)
                .await
                .unwrap_or_else(|_| request.expected_workspace_state_sha256.clone()),
            artifact_set_sha256: artifact_set_state(authoritative)
                .unwrap_or_else(|_| sha256(b"unavailable phase artifacts")),
            evidence_sha256: sha256(error.to_string()),
            evidence_summary: None,
            artifacts: Vec::new(),
            error_code: Some(error_code(error)),
        }
    }

    async fn inspect_delivery(
        &self,
        request: RemoteDeliverySnapshotRequest,
    ) -> Result<RemoteDeliverySnapshotResult, RemoteWorkerError> {
        self.validate_delivery_snapshot_request(&request)?;
        let repository = self.repository(&request.repository_identity_sha256)?;
        if repository.artifact_set_id != request.artifact_set_id {
            return Err(RemoteWorkerError::Invalid(
                "artifact-set identity does not match repository registration".into(),
            ));
        }
        let run = self
            .store
            .sdd_remote_worker_run(&request.run_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(request.run_id.clone()))?;
        self.validate_ready_delivery_run(
            &run,
            &request.host_id,
            &request.repository_identity_sha256,
            &request.artifact_set_id,
            &request.spec_id,
            request.spec_revision,
            &request.base_commit,
            &request.approval_digest,
        )?;
        let authoritative = PathBuf::from(&run.authoritative_path);
        let workspace_state_sha256 = workspace_state(&authoritative).await?;
        if workspace_state_sha256 != run.workspace_state_sha256
            || workspace_state_sha256 != request.expected_workspace_state_sha256
        {
            return Err(RemoteWorkerError::Invalid(
                "Ready workspace changed outside the durable checkpoint".into(),
            ));
        }
        Ok(RemoteDeliverySnapshotResult {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: request.request_id,
            run_id: request.run_id,
            workspace_state_sha256,
            artifact_set_sha256: artifact_set_state(&authoritative)?,
            worktree_identity_sha256: sha256(run.authoritative_path.as_bytes()),
            branch_name: run.branch_name,
            openspec_destination_exists: request
                .openspec_destination
                .as_deref()
                .map(|destination| {
                    super::delivery::openspec_destination_exists(&authoritative, destination)
                        .map_err(|error| RemoteWorkerError::Invalid(error.to_string()))
                })
                .transpose()?,
        })
    }

    async fn execute_delivery_action(
        &self,
        request: RemoteDeliveryActionRequest,
    ) -> Result<RemoteDeliveryActionResult, RemoteWorkerError> {
        self.validate_delivery_action_request(&request)?;
        let repository = self.repository(&request.repository_identity_sha256)?;
        if repository.artifact_set_id != request.artifact_set_id {
            return Err(RemoteWorkerError::Invalid(
                "artifact-set identity does not match repository registration".into(),
            ));
        }
        let run = self
            .store
            .sdd_remote_worker_run(&request.run_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(request.run_id.clone()))?;
        self.validate_ready_delivery_run(
            &run,
            &request.host_id,
            &request.repository_identity_sha256,
            &request.artifact_set_id,
            &request.spec_id,
            request.spec_revision,
            &request.base_commit,
            &request.approval_digest,
        )?;
        let authoritative = PathBuf::from(&run.authoritative_path);
        let actual_state = workspace_state(&authoritative).await?;
        if actual_state != run.workspace_state_sha256 {
            return Err(RemoteWorkerError::Invalid(
                "Ready workspace changed outside the durable delivery ledger".into(),
            ));
        }
        let artifact_set_sha256 = artifact_set_state(&authoritative)?;
        let remote_artifact_hashes = request
            .envelope
            .artifact_hashes
            .iter()
            .filter(|artifact| {
                artifact.kind == "remote_artifact_set"
                    && artifact.relative_path == "agentum+ssh://artifact-set"
            })
            .collect::<Vec<_>>();
        if remote_artifact_hashes.len() != 1
            || remote_artifact_hashes[0].content_hash != artifact_set_sha256
            || request.envelope.worktree_identity != sha256(run.authoritative_path.as_bytes())
            || request.envelope.branch_name != run.branch_name
        {
            return Err(RemoteWorkerError::Invalid(
                "remote delivery preview no longer identifies this authoritative worktree".into(),
            ));
        }
        let request_sha256 = sha256(serde_json::to_vec(&request)?);
        let reservation = self
            .store
            .sdd_remote_worker_reserve_delivery(ReserveRemoteDelivery {
                request_id: &request.request_id,
                request_sha256: &request_sha256,
                run_id: &request.run_id,
                host_id: &request.host_id,
                repository_identity_sha256: &request.repository_identity_sha256,
                artifact_set_id: &request.artifact_set_id,
                spec_id: &request.spec_id,
                spec_revision: request.spec_revision,
                base_commit: &request.base_commit,
                approval_digest: &request.approval_digest,
                preview_digest: &request.preview_digest,
                action_id: &request.action.id,
                dependencies: &request.action.depends_on,
                initial_workspace_state_sha256: &request.envelope.workspace_state_hash,
            })
            .await?;
        match reservation {
            RemoteWorkerReservation::Replay(response) => {
                return Ok(serde_json::from_str(&response)?);
            }
            // Repository delivery primitives reconcile stable action markers
            // (commit trailer, remote head, PR/release marker, export hash), so
            // an interrupted request is safely re-entered with the same ID.
            RemoteWorkerReservation::RecoveryRequired(_) | RemoteWorkerReservation::Started => {}
        }

        let outcome = super::delivery::execute_repository_delivery_action(
            &authoritative,
            &run.branch_name,
            &request.envelope,
            &request.action,
        )
        .await;
        let (status, result, error_code) = match outcome {
            super::delivery::RepositoryDeliveryOutcome::Succeeded(result) => {
                (RemoteDeliveryActionStatus::Succeeded, result, None)
            }
            super::delivery::RepositoryDeliveryOutcome::Failed(result) => (
                RemoteDeliveryActionStatus::Failed,
                result,
                Some("delivery_action_failed".into()),
            ),
            super::delivery::RepositoryDeliveryOutcome::SyncPending(result) => (
                RemoteDeliveryActionStatus::SyncPending,
                result,
                Some("delivery_outcome_ambiguous".into()),
            ),
        };
        let result = RemoteDeliveryActionResult {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            run_id: request.run_id.clone(),
            action_id: request.action.id.clone(),
            status,
            result,
            workspace_state_sha256: workspace_state(&authoritative).await?,
            artifact_set_sha256: artifact_set_state(&authoritative)?,
            error_code,
        };
        let response = serde_json::to_string(&result)?;
        if response.len() > request.output_limit {
            return Err(RemoteWorkerError::Invalid(
                "delivery result exceeds the negotiated output limit".into(),
            ));
        }
        self.store
            .sdd_remote_worker_complete_delivery(
                &request.request_id,
                &request_sha256,
                &request.run_id,
                &result.workspace_state_sha256,
                &response,
            )
            .await?;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_ready_delivery_run(
        &self,
        run: &agentum_store::sdd_remote_worker::RemoteWorkerRunRecord,
        host_id: &str,
        repository_identity_sha256: &str,
        artifact_set_id: &str,
        spec_id: &str,
        spec_revision: i64,
        base_commit: &str,
        approval_digest: &str,
    ) -> Result<(), RemoteWorkerError> {
        if run.status != "ready"
            || run.next_phase != "ready"
            || run.completed_phases != 5
            || run.host_id != host_id
            || run.repository_identity_sha256 != repository_identity_sha256
            || run.artifact_set_id != artifact_set_id
            || run.spec_id != spec_id
            || run.spec_revision != spec_revision
            || run.base_commit != base_commit
            || run.approval_digest.as_deref() != Some(approval_digest)
        {
            return Err(RemoteWorkerError::Invalid(
                "delivery requires the exact approved durable Ready run".into(),
            ));
        }
        Ok(())
    }

    fn validate_delivery_snapshot_request(
        &self,
        request: &RemoteDeliverySnapshotRequest,
    ) -> Result<(), RemoteWorkerError> {
        if request.schema_version != REMOTE_SDD_SCHEMA_VERSION
            || request.host_id != self.config.host_id
            || !valid_prefixed_request_id(&request.request_id, "delivery-inspect-")
            || Uuid::parse_str(&request.run_id).is_err()
            || request.spec_id.parse::<SpecId>().is_err()
            || request.spec_revision < 2
            || !valid_sha256(&request.repository_identity_sha256)
            || request.artifact_set_id.parse::<Ulid>().is_err()
            || !valid_git_object(&request.base_commit)
            || !valid_sha256(&request.approval_digest)
            || !valid_sha256(&request.expected_workspace_state_sha256)
            || request.openspec_destination.as_ref().is_some_and(|path| {
                path.len() > 1_024 || agentum_core::sdd::validate_relative_path(path).is_err()
            })
            || !(1_000..=3_600_000).contains(&request.timeout_ms)
            || !(1_024..=8 * 1024 * 1024).contains(&request.output_limit)
        {
            return Err(RemoteWorkerError::Invalid(
                "delivery inspection request contract is invalid".into(),
            ));
        }
        Ok(())
    }

    fn validate_delivery_action_request(
        &self,
        request: &RemoteDeliveryActionRequest,
    ) -> Result<(), RemoteWorkerError> {
        let digest = super::delivery::preview_digest(&request.envelope)
            .map_err(|error| RemoteWorkerError::Invalid(error.to_string()))?;
        if request.schema_version != REMOTE_SDD_SCHEMA_VERSION
            || request.host_id != self.config.host_id
            || !valid_prefixed_request_id(&request.request_id, "delivery-action-")
            || Uuid::parse_str(&request.run_id).is_err()
            || request.spec_id.parse::<SpecId>().is_err()
            || request.spec_revision < 2
            || !valid_sha256(&request.repository_identity_sha256)
            || request.artifact_set_id.parse::<Ulid>().is_err()
            || !valid_git_object(&request.base_commit)
            || !valid_sha256(&request.approval_digest)
            || !valid_sha256(&request.preview_digest)
            || request.preview_digest != digest
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
            || !request
                .envelope
                .actions
                .iter()
                .any(|action| action == &request.action)
            || !super::delivery::is_repository_delivery_action(&request.action)
            || !(1..=1_000).contains(&request.attempt)
            || !(1_000..=3_600_000).contains(&request.timeout_ms)
            || !(1_024..=8 * 1024 * 1024).contains(&request.output_limit)
        {
            return Err(RemoteWorkerError::Invalid(
                "delivery action request contract is invalid".into(),
            ));
        }
        Ok(())
    }

    fn validate_authoring_request(
        &self,
        request: &RemoteAuthoringRequest,
    ) -> Result<(), RemoteWorkerError> {
        if request.schema_version != REMOTE_SDD_SCHEMA_VERSION
            || request.host_id != self.config.host_id
            || !request.request_id.starts_with("author-")
            || request.request_id.len() != 39
            || Uuid::parse_str(&request.run_id).is_err()
            || request.spec_id.parse::<SpecId>().is_err()
            || !valid_sha256(&request.repository_identity_sha256)
            || request.artifact_set_id.parse::<Ulid>().is_err()
            || !valid_git_object(&request.base_commit)
            || !valid_provider_reference(&request.provider)
            || !matches!(
                request.source_checkout.as_str(),
                "require_clean" | "committed_base"
            )
            || request.title.trim().is_empty()
            || request.title.len() > 256
            || request.title.contains(['\r', '\n'])
            || request.goal.trim().is_empty()
            || request.goal.len() > 32 * 1024
            || !(1_000..=3_600_000).contains(&request.timeout_ms)
            || !(1_024..=8 * 1024 * 1024).contains(&request.output_limit)
        {
            return Err(RemoteWorkerError::Invalid(
                "authoring request contract is invalid".into(),
            ));
        }
        Ok(())
    }

    fn validate_phase_request(
        &self,
        request: &RemotePhaseRequest,
    ) -> Result<(), RemoteWorkerError> {
        if request.schema_version != REMOTE_SDD_SCHEMA_VERSION
            || request.host_id != self.config.host_id
            || !request.request_id.starts_with("remote-")
            || request.request_id.len() != 39
            || Uuid::parse_str(&request.run_id).is_err()
            || request.spec_id.parse::<SpecId>().is_err()
            || request.spec_revision < 2
            || !valid_sha256(&request.repository_identity_sha256)
            || request.artifact_set_id.parse::<Ulid>().is_err()
            || !valid_git_object(&request.base_commit)
            || !valid_provider_reference(&request.provider)
            || !valid_sha256(&request.expected_workspace_state_sha256)
            || !valid_sha256(&request.previous_result_sha256)
            || !valid_sha256(&request.approval_digest)
            || !(1_000..=3_600_000).contains(&request.timeout_ms)
            || !(1_024..=8 * 1024 * 1024).contains(&request.output_limit)
            || request.phase == RemoteLifecyclePhase::Ready
        {
            return Err(RemoteWorkerError::Invalid(
                "phase request contract is invalid".into(),
            ));
        }
        Ok(())
    }
}

fn request_sha256(request: &RemoteAuthoringRequest) -> Result<String, RemoteWorkerError> {
    Ok(sha256(serde_json::to_vec(request)?))
}

fn artifact_prompt(
    kind: ArtifactKind,
    attempt: &Path,
    spec_slug: &str,
    spec_id: &str,
    revision: i64,
    verification_evidence: Option<&str>,
) -> Result<(ProviderOperation, &'static str, &'static str, String), RemoteWorkerError> {
    let root = attempt.join(".agentum/specs").join(spec_slug);
    let spec_path = root.join("spec.md");
    let _ = read_text(&spec_path)?;
    match kind {
        ArtifactKind::Design => Ok((
            ProviderOperation::Design,
            DESIGN_BEGIN,
            DESIGN_END,
            format!(
                "You are the design agent in an Agentum-owned workflow. Read {spec_path:?} and the repository. Do not edit files, change Git, contact networks beyond your model transport, or implement code. Produce design.md with concrete architecture, boundaries, data flow, failure handling, and verification strategy tied to RQ/AC identifiers. Return only Markdown between literal lines {DESIGN_BEGIN} and {DESIGN_END}."
            ),
        )),
        ArtifactKind::Plan => Ok((
            ProviderOperation::Planning,
            PLAN_BEGIN,
            PLAN_END,
            format!(
                "You are the planning agent in an Agentum-owned workflow. Read {spec_path:?}, {:?}, and the repository. Do not edit files or change Git. Return only a JSON object between literal lines {PLAN_BEGIN} and {PLAN_END}. It must have schemaVersion:1, specId:{spec_id:?}, specRevision:{revision}, and a non-empty tasks array. Each task requires id, objective, dependencies, relative traversal-free readScopes/writeScopes, acceptanceCriteria using AC-* ids, verification CommandSpec objects (program,args,cwd,envAllowlist,timeoutMs,outputLimit), browserChecks, risk, and parallelSafe. Commands are direct programs; never generate shell strings or bash -lc. Declare every required browser check as a typed browserCheck; Agentum executes those checks on the registered SSH host and imports bounded, content-addressed evidence, so never replace one with a shell command.",
                root.join("design.md")
            ),
        )),
        ArtifactKind::Review => Ok((
            ProviderOperation::Review,
            REVIEW_BEGIN,
            REVIEW_END,
            format!(
                "You are an independent review agent in a new isolated session. Read {spec_path:?}, {:?}, {:?}, the implemented diff, and this Agentum-owned verification evidence: {}. Do not edit files or change Git. Review every RQ/AC, security boundary, and regression risk. If and only if the work is ready, include a line exactly `Verdict: PASS`; otherwise include `Verdict: FAIL` with blockers. Return only review.md Markdown between literal lines {REVIEW_BEGIN} and {REVIEW_END}.",
                root.join("design.md"),
                root.join("plan.json"),
                verification_evidence.unwrap_or("[]")
            ),
        )),
        _ => Err(RemoteWorkerError::Invalid(
            "phase does not produce a text artifact".into(),
        )),
    }
}

async fn execute_remote_browser_check(
    port: u16,
    target: &str,
    check: &BrowserCheck,
    browser_name: &str,
    browser_version: &str,
) -> Result<RemoteBrowserCheckResult, RemoteWorkerError> {
    let started = std::time::Instant::now();
    let wait_until = match check.wait_until {
        BrowserWaitUntil::Load | BrowserWaitUntil::NetworkIdle => "load",
        BrowserWaitUntil::DomContentLoaded => "domcontentloaded",
    };
    let navigate = crate::cdp_driver::run_browser_op(
        "navigate",
        &serde_json::json!({
            "cdpPort": port,
            "target": target,
            "url": check.url,
            "wait_until": wait_until
        }),
    )
    .await
    .map_err(|error| RemoteWorkerError::BrowserVerificationUnavailable(error.to_string()))?;
    let navigation_ok = navigate.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
    let final_origin_allowed = navigate
        .get("final_url")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| reqwest::Url::parse(value).ok())
        .is_some_and(|url| lifecycle::ensure_sdd_browser_origin_allowed(&url).is_ok());
    if navigation_ok && !final_origin_allowed {
        return Err(RemoteWorkerError::BrowserVerificationUnavailable(
            "browser navigation redirected outside the approved origin policy".into(),
        ));
    }
    let network_idle = if check.wait_until == BrowserWaitUntil::NetworkIdle {
        crate::cdp_driver::sdd_wait_network_idle(port, target, check.timeout_ms)
            .await
            .map_err(|error| RemoteWorkerError::BrowserVerificationUnavailable(error.to_string()))?
    } else {
        true
    };
    let page_status = crate::cdp_driver::sdd_page_status(port, target)
        .await
        .map_err(|error| RemoteWorkerError::BrowserVerificationUnavailable(error.to_string()))?;
    let mut assertion_results = Vec::with_capacity(check.assertions.len());
    let mut passed_count = 0_usize;
    for assertion in &check.assertions {
        let passed = match assertion {
            BrowserCheckAssertion::PageLoaded {
                expected_status, ..
            } => navigation_ok && network_idle && page_status == Some(*expected_status),
            BrowserCheckAssertion::TextPresent { text, .. } => {
                remote_browser_wait_assertion(port, target, "text", text, check.timeout_ms).await?
            }
            BrowserCheckAssertion::SelectorVisible { selector, .. } => {
                remote_browser_wait_assertion(port, target, "selector", selector, check.timeout_ms)
                    .await?
            }
            BrowserCheckAssertion::UrlContains { value, .. } => {
                remote_browser_wait_assertion(port, target, "url", value, check.timeout_ms).await?
            }
        };
        passed_count += usize::from(passed);
        assertion_results.push((assertion.id().to_owned(), passed));
    }
    // Capture after every bounded wait so the immutable image represents the
    // exact page state used to decide the assertions.
    let screenshot = crate::cdp_driver::run_browser_op(
        "screenshot",
        &serde_json::json!({
            "cdpPort": port,
            "target": target,
            "width": check.viewport.width,
            "height": check.viewport.height,
            "deviceScaleFactor": check.viewport.device_scale_milli as f64 / 1000.0
        }),
    )
    .await
    .map_err(|error| RemoteWorkerError::BrowserVerificationUnavailable(error.to_string()))?;
    let screenshot_bytes = screenshot
        .get("image_b64")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            RemoteWorkerError::BrowserVerificationUnavailable(
                "browser screenshot returned no bytes".into(),
            )
        })
        .and_then(|encoded| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|error| {
                    RemoteWorkerError::BrowserVerificationUnavailable(error.to_string())
                })
        })?;
    if screenshot_bytes.is_empty() || screenshot_bytes.len() > 6 * 1024 * 1024 {
        return Err(RemoteWorkerError::BrowserVerificationUnavailable(
            "browser screenshot is empty or exceeds the remote evidence bound".into(),
        ));
    }
    let screenshot_sha256 = sha256(&screenshot_bytes);
    let assertions = assertion_results
        .into_iter()
        .map(|(id, passed)| BrowserAssertion {
            id,
            status: if passed {
                BrowserAssertionStatus::Passed
            } else {
                BrowserAssertionStatus::Failed
            },
            acceptance_criteria: check.acceptance_criteria.clone(),
            evidence_sha256: vec![screenshot_sha256.clone()],
        })
        .collect::<Vec<_>>();
    let console_bytes = br#"{"coverage":"none","reason":"ambient_diagnostics_excluded"}"#;
    let network_bytes = serde_json::to_vec(&serde_json::json!({
        "coverage": "main_document",
        "navigationOk": navigation_ok,
        "networkIdle": network_idle,
        "status": page_status
    }))?;
    let console_sha256 = sha256(console_bytes);
    let network_sha256 = sha256(&network_bytes);
    let blob = |bytes: &[u8], media_type: &str, role: &str| RemoteBrowserBlob {
        sha256: sha256(bytes),
        byte_length: bytes.len() as u64,
        media_type: media_type.into(),
        role: role.into(),
        content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    };
    let captured_at = OffsetDateTime::now_utc().format(&Rfc3339)?;
    let url = reqwest::Url::parse(&check.url)
        .map_err(|error| RemoteWorkerError::BrowserVerificationUnavailable(error.to_string()))?;
    let passed =
        navigation_ok && final_origin_allowed && network_idle && passed_count == assertions.len();
    Ok(RemoteBrowserCheckResult {
        check_id: check.id.clone(),
        captured_at,
        status: if passed { "passed" } else { "failed" }.into(),
        duration_ms: started.elapsed().as_millis().min(i64::MAX as u128) as i64,
        output_excerpt: format!(
            "{}: navigation={} networkIdle={} {}/{} assertions passed; screenshot sha256:{}",
            check.id,
            navigation_ok,
            network_idle,
            passed_count,
            assertions.len(),
            screenshot_sha256
        ),
        target: BrowserTarget {
            origin: url.origin().ascii_serialization(),
            path: "/[redacted]".into(),
            path_redacted: true,
            query_redacted: true,
        },
        browser: BrowserRuntime {
            name: browser_name.into(),
            version: browser_version.into(),
            viewport_width: check.viewport.width,
            viewport_height: check.viewport.height,
            device_scale_milli: check.viewport.device_scale_milli,
        },
        assertions,
        console: BrowserConsoleSummary {
            coverage: BrowserDiagnosticCoverage::None,
            errors: 0,
            warnings: 0,
            transcript_sha256: console_sha256,
        },
        network: BrowserNetworkSummary {
            coverage: BrowserDiagnosticCoverage::MainDocument,
            requests: 1,
            failed_requests: u32::from(
                !navigation_ok || !network_idle || page_status.is_some_and(|status| status >= 400),
            ),
            transcript_sha256: network_sha256,
        },
        blobs: vec![
            blob(&screenshot_bytes, "image/png", "capture"),
            blob(console_bytes, "application/json", "console_transcript"),
            blob(&network_bytes, "application/json", "network_transcript"),
        ],
    })
}

async fn remote_browser_wait_assertion(
    port: u16,
    target: &str,
    condition: &str,
    argument: &str,
    timeout_ms: u64,
) -> Result<bool, RemoteWorkerError> {
    let value = crate::cdp_driver::run_browser_op(
        "wait",
        &serde_json::json!({
            "cdpPort": port,
            "target": target,
            "condition": condition,
            "arg": argument,
            "timeout_ms": timeout_ms
        }),
    )
    .await
    .map_err(|error| RemoteWorkerError::BrowserVerificationUnavailable(error.to_string()))?;
    Ok(
        value.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
            && value.get("timed_out").and_then(serde_json::Value::as_bool) == Some(false),
    )
}

fn validate_text_artifact(
    kind: ArtifactKind,
    content: &str,
    spec_id: &str,
    revision: i64,
) -> Result<(), RemoteWorkerError> {
    if content.trim().is_empty() || content.len() > 2 * 1024 * 1024 {
        return Err(RemoteWorkerError::Invalid(
            "phase artifact is empty or exceeds 2 MiB".into(),
        ));
    }
    match kind {
        ArtifactKind::Design if !content.contains("RQ-") || !content.contains("AC-") => {
            Err(RemoteWorkerError::Invalid(
                "design must trace requirements and acceptance criteria".into(),
            ))
        }
        ArtifactKind::Plan => {
            let id: SpecId = spec_id
                .parse()
                .map_err(|_| RemoteWorkerError::Invalid("spec identity is invalid".into()))?;
            validate_plan(content, &id, revision)?;
            Ok(())
        }
        ArtifactKind::Review => {
            let passes = content
                .lines()
                .filter(|line| line.trim() == "Verdict: PASS")
                .count();
            if passes != 1
                || content.lines().any(|line| line.trim() == "Verdict: FAIL")
                || !content.contains("AC-")
            {
                return Err(RemoteWorkerError::Invalid(
                    "independent review did not return one traced PASS verdict".into(),
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn rollback_operations(
    authoritative: &Path,
    operations: &[JournalOperation],
    preimages: &[JournalPreimage],
    candidates: &[usize],
) -> Result<(), RemoteWorkerError> {
    if operations.len() != preimages.len() {
        return Err(RemoteWorkerError::Invalid(
            "patch recovery evidence is inconsistent".into(),
        ));
    }
    for index in candidates.iter().rev().copied() {
        let operation = operations
            .get(index)
            .ok_or_else(|| RemoteWorkerError::Invalid("patch recovery index is invalid".into()))?;
        let preimage = preimages.get(index).ok_or_else(|| {
            RemoteWorkerError::Invalid("patch recovery preimage is missing".into())
        })?;
        if operation.relative_path != preimage.relative_path
            || operation.preimage_sha256 != preimage.sha256
        {
            return Err(RemoteWorkerError::Invalid(
                "patch recovery evidence identity mismatch".into(),
            ));
        }
        let target = authoritative.join(&operation.relative_path);
        let current = content_hash(&target)?;
        if current == preimage.sha256 {
            continue;
        }
        if current != operation.postimage_sha256 {
            return Err(RemoteWorkerError::Invalid(format!(
                "patch recovery found an external edit at {}",
                operation.relative_path
            )));
        }
        match &preimage.content_base64 {
            Some(encoded) => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|_| RemoteWorkerError::Invalid("invalid recovery preimage".into()))?;
                if sha256(&bytes) != preimage.sha256 {
                    return Err(RemoteWorkerError::Invalid(
                        "recovery preimage digest mismatch".into(),
                    ));
                }
                atomic_write(&target, &bytes, Some(&operation.postimage_sha256))?;
            }
            None => atomic_remove(&target, &operation.postimage_sha256)?,
        }
    }
    Ok(())
}

async fn workspace_state(authoritative: &Path) -> Result<String, RemoteWorkerError> {
    let metadata = std::fs::symlink_metadata(authoritative)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RemoteWorkerError::Invalid(
            "authoritative workspace is not a real directory".into(),
        ));
    }
    let diff = git_output(
        authoritative,
        &["diff", "--binary", "--no-ext-diff", "HEAD", "--", "."],
    )
    .await?;
    let untracked = git_output(
        authoritative,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )
    .await?;
    let mut paths = untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            std::str::from_utf8(path)
                .map(str::to_owned)
                .map_err(|_| RemoteWorkerError::Invalid("non-UTF-8 workspace path".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut material = Vec::new();
    material.extend_from_slice(&diff);
    for relative in paths {
        agentum_core::sdd::validate_relative_path(&relative).map_err(|_| {
            RemoteWorkerError::Invalid(format!("unsafe workspace path: {relative}"))
        })?;
        let (bytes, hash) = read_bytes(&authoritative.join(&relative))?;
        material.extend_from_slice(relative.as_bytes());
        material.push(0);
        material.extend_from_slice(hash.as_bytes());
        material.push(0);
        material.extend_from_slice(&bytes);
        if material.len() > MAX_STATE_BYTES {
            return Err(RemoteWorkerError::Invalid(
                "workspace state exceeds its 64 MiB bound".into(),
            ));
        }
    }
    Ok(sha256(material))
}

fn artifact_set_state(authoritative: &Path) -> Result<String, RemoteWorkerError> {
    let discovered = discover_specs(authoritative)?
        .ok_or_else(|| RemoteWorkerError::Invalid("artifact set is missing".into()))?;
    let value = serde_json::json!({
        "manifest": {
            "format": discovered.manifest.format,
            "schemaVersion": discovered.manifest.schema_version,
            "artifactSetId": discovered.manifest.artifact_set_id.to_string(),
        },
        "specs": discovered.specs.iter().map(|spec| serde_json::json!({
            "id": spec.header.id.to_string(),
            "revision": spec.header.revision,
            "relativePath": spec.relative_path,
            "contentHash": spec.content_hash,
            "later": spec.later_artifacts.iter().map(|artifact| serde_json::json!({
                "kind": artifact.kind,
                "relativePath": artifact.relative_path,
                "contentHash": artifact.content_hash,
            })).collect::<Vec<_>>()
        })).collect::<Vec<_>>()
    });
    Ok(sha256(serde_json::to_vec(&value)?))
}

async fn git_output(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, RemoteWorkerError> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        return Err(RemoteWorkerError::Invalid(format!(
            "Git state inspection failed with status {}",
            output.status
        )));
    }
    if output.stdout.len().saturating_add(output.stderr.len()) > MAX_STATE_BYTES {
        return Err(RemoteWorkerError::Invalid(
            "Git state inspection exceeded its output bound".into(),
        ));
    }
    Ok(output.stdout)
}

async fn registered_repository_ready(path: &Path) -> bool {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return false;
    }
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--is-inside-work-tree"])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    matches!(
        tokio::time::timeout(Duration::from_secs(5), command.output()).await,
        Ok(Ok(output)) if output.status.success() && output.stdout == b"true\n"
    )
}

async fn registered_repository_clean(path: &Path) -> bool {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    matches!(
        tokio::time::timeout(Duration::from_secs(5), command.output()).await,
        Ok(Ok(output)) if output.status.success() && output.stdout.is_empty()
    )
}

async fn resolve_base_commit(path: &Path, base_ref: &str) -> Option<String> {
    if !valid_base_ref(base_ref) {
        return None;
    }
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--verify"])
        .arg(format!("{base_ref}^{{commit}}"))
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() || output.stdout.len() > 256 {
        return None;
    }
    let commit = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    valid_git_object(&commit).then_some(commit)
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

fn next_phase_name(phase: RemoteLifecyclePhase) -> &'static str {
    match phase {
        RemoteLifecyclePhase::Design => "planning",
        RemoteLifecyclePhase::Planning => "implementation",
        RemoteLifecyclePhase::Implementation => "verification",
        RemoteLifecyclePhase::Verification => "review",
        RemoteLifecyclePhase::Review | RemoteLifecyclePhase::Ready => "ready",
    }
}

fn phase_ordinal(phase: RemoteLifecyclePhase) -> u8 {
    match phase {
        RemoteLifecyclePhase::Design => 0,
        RemoteLifecyclePhase::Planning => 1,
        RemoteLifecyclePhase::Implementation => 2,
        RemoteLifecyclePhase::Verification => 3,
        RemoteLifecyclePhase::Review => 4,
        RemoteLifecyclePhase::Ready => 5,
    }
}

fn safe_token(value: &str) -> String {
    let token: String = value
        .chars()
        .filter(|value| value.is_ascii_alphanumeric() || *value == '-')
        .take(48)
        .collect();
    if token.is_empty() {
        "task".into()
    } else {
        token
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_prefixed_request_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn valid_git_object(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && valid_hex(value)
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

fn valid_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_provider_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| !byte.is_ascii_control() && !byte.is_ascii_whitespace())
}

fn error_code(error: &RemoteWorkerError) -> String {
    match error {
        RemoteWorkerError::Provider(super::providers::ProviderError::Canceled) => "canceled",
        RemoteWorkerError::Provider(_) => "provider_failed",
        RemoteWorkerError::Workspace(_) => "workspace_failed",
        RemoteWorkerError::Artifact(_) => "artifact_failed",
        RemoteWorkerError::Lifecycle(_) => "lifecycle_failed",
        RemoteWorkerError::Store(_) => "durable_state_failed",
        RemoteWorkerError::Config(_) => "worker_misconfigured",
        RemoteWorkerError::Invalid(_) => "invalid_lifecycle_output",
        RemoteWorkerError::BrowserVerificationUnavailable(_) => {
            "remote_browser_verification_unavailable"
        }
        RemoteWorkerError::Io(_) => "io_failed",
        RemoteWorkerError::Json(_) => "malformed_json",
        RemoteWorkerError::Time(_) => "time_failed",
        RemoteWorkerError::Path(_) => "path_failed",
    }
    .into()
}

fn bounded_error(value: &str) -> String {
    value.chars().take(2_048).collect()
}

/// Run exactly one subsystem request over bounded length-prefixed JSON. The
/// client deliberately keeps stdin open; EOF after the request is therefore a
/// cancellation signal and terminates provider/verification process trees.
pub async fn serve_stdio(config_path: &Path) -> Result<(), RemoteWorkerError> {
    let worker = RemoteSubsystemWorker::open(config_path).await?;
    let mut stdin = tokio::io::stdin();
    let frame = read_client_frame(&mut stdin).await?;
    let run_id = match &frame {
        RemoteClientFrame::Probe(_) => None,
        RemoteClientFrame::AuthorSpec(request) => Some(request.run_id.clone()),
        RemoteClientFrame::ExecutePhase(request) => Some(request.run_id.clone()),
        RemoteClientFrame::InspectDelivery(request) => Some(request.run_id.clone()),
        RemoteClientFrame::ExecuteDeliveryAction(request) => Some(request.run_id.clone()),
    };
    let timeout_ms = match &frame {
        RemoteClientFrame::Probe(_) => 10_000,
        RemoteClientFrame::AuthorSpec(request) => request.timeout_ms,
        RemoteClientFrame::ExecutePhase(request) => request.timeout_ms,
        RemoteClientFrame::InspectDelivery(request) => request.timeout_ms,
        RemoteClientFrame::ExecuteDeliveryAction(request) => request.timeout_ms,
    };
    let task_worker = worker.clone();
    let mut task = tokio::spawn(async move { task_worker.handle(frame).await });
    let mut eof_byte = [0_u8; 1];
    let result = tokio::select! {
        result = &mut task => result.map_err(|error| RemoteWorkerError::Invalid(error.to_string()))??,
        read = stdin.read(&mut eof_byte) => {
            match read {
                Ok(0) => {
                    if let Some(run_id) = run_id.as_deref() {
                        worker.cancel(run_id);
                    }
                    match tokio::time::timeout(Duration::from_secs(10), &mut task).await {
                        Ok(result) => result.map_err(|error| RemoteWorkerError::Invalid(error.to_string()))??,
                        Err(_) => {
                            task.abort();
                            return Err(RemoteWorkerError::Invalid("cancellation grace period expired".into()));
                        }
                    }
                }
                Ok(_) => return Err(RemoteWorkerError::Invalid("trailing subsystem input".into())),
                Err(error) => return Err(error.into()),
            }
        }
        _ = tokio::time::sleep(Duration::from_millis(timeout_ms.saturating_add(5_000))) => {
            if let Some(run_id) = run_id.as_deref() {
                worker.cancel(run_id);
            }
            task.abort();
            return Err(RemoteWorkerError::Invalid("subsystem request timed out".into()));
        }
    };
    write_server_frame(tokio::io::stdout(), &result).await
}

async fn read_client_frame(
    reader: &mut (impl AsyncRead + Unpin),
) -> Result<RemoteClientFrame, RemoteWorkerError> {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).await?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(RemoteWorkerError::Invalid(
            "subsystem request frame length is invalid".into(),
        ));
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn write_server_frame(
    mut writer: impl AsyncWrite + Unpin,
    frame: &RemoteServerFrame,
) -> Result<(), RemoteWorkerError> {
    let bytes = serde_json::to_vec(frame)?;
    if bytes.is_empty() || bytes.len() > MAX_RESPONSE_BYTES {
        return Err(RemoteWorkerError::Invalid(
            "response frame exceeds the protocol bound".into(),
        ));
    }
    let length = u32::try_from(bytes.len())
        .map_err(|_| RemoteWorkerError::Invalid("response frame is too large".into()))?;
    writer.write_all(&length.to_be_bytes()).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    writer.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    use std::ffi::OsString;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    use super::super::remote::{
        RemoteLifecycleCheckpoint, RemoteLifecycleError, RemoteLifecyclePlan, RemoteSddTransport,
        SequentialRemoteLifecycle,
    };
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[derive(Clone)]
    struct FramedWorkerTransport {
        worker: RemoteSubsystemWorker,
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    impl FramedWorkerTransport {
        async fn exchange(
            &self,
            frame: RemoteClientFrame,
        ) -> Result<RemoteServerFrame, RemoteLifecycleError> {
            let (mut client, mut server) = tokio::io::duplex(256 * 1024);
            let worker = self.worker.clone();
            let server_task = tokio::spawn(async move {
                let request = read_client_frame(&mut server).await?;
                let response = worker.handle(request).await?;
                write_server_frame(server, &response).await
            });

            let bytes = serde_json::to_vec(&frame)
                .map_err(|error| RemoteLifecycleError::Transport(error.to_string()))?;
            client
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .await
                .map_err(|error| RemoteLifecycleError::Transport(error.to_string()))?;
            client
                .write_all(&bytes)
                .await
                .map_err(|error| RemoteLifecycleError::Transport(error.to_string()))?;
            client
                .flush()
                .await
                .map_err(|error| RemoteLifecycleError::Transport(error.to_string()))?;

            let mut header = [0_u8; 4];
            client
                .read_exact(&mut header)
                .await
                .map_err(|_| RemoteLifecycleError::MalformedFrame)?;
            let length = u32::from_be_bytes(header) as usize;
            if length == 0 || length > MAX_RESPONSE_BYTES {
                return Err(RemoteLifecycleError::MalformedFrame);
            }
            let mut response = vec![0_u8; length];
            client
                .read_exact(&mut response)
                .await
                .map_err(|_| RemoteLifecycleError::MalformedFrame)?;
            server_task
                .await
                .map_err(|error| RemoteLifecycleError::Transport(error.to_string()))?
                .map_err(|error| RemoteLifecycleError::Transport(error.to_string()))?;
            serde_json::from_slice(&response).map_err(|_| RemoteLifecycleError::MalformedFrame)
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    impl RemoteSddTransport for FramedWorkerTransport {
        fn execute(
            &self,
            request: RemotePhaseRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<RemotePhaseResult, RemoteLifecycleError>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async move {
                match self
                    .exchange(RemoteClientFrame::ExecutePhase(request))
                    .await?
                {
                    RemoteServerFrame::PhaseResult(result) => Ok(result),
                    _ => Err(RemoteLifecycleError::InvalidResult),
                }
            })
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    struct EnvironmentRestore(Vec<(&'static str, Option<OsString>)>);

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    impl EnvironmentRestore {
        fn set(values: Vec<(&'static str, OsString)>) -> Self {
            let previous = values
                .iter()
                .map(|(name, _)| (*name, std::env::var_os(name)))
                .collect();
            for (name, value) in values {
                // SAFETY: the caller holds the crate-wide TEST_ENV_LOCK until
                // this guard restores every process-wide environment value.
                unsafe { std::env::set_var(name, value) };
            }
            Self(previous)
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    impl Drop for EnvironmentRestore {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..).rev() {
                // SAFETY: the owning test still holds TEST_ENV_LOCK.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    fn git(directory: &Path, arguments: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?}");
    }

    fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        git(directory.path(), &["init", "-q"]);
        git(
            directory.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(
            directory.path(),
            &["config", "user.name", "Remote worker test"],
        );
        std::fs::write(directory.path().join("README.md"), "fixture\n").unwrap();
        git(directory.path(), &["add", "README.md"]);
        git(directory.path(), &["commit", "-qm", "fixture"]);
        directory
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn deterministic_codex(directory: &Path, spec_id: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let script = r##"#!/bin/sh
set -eu
if [ "${1:-}" = "--version" ]; then
  printf '%s\n' 'codex-cli 0.145.0'
  exit 0
fi
if [ "${1:-}" = "login" ] && [ "${2:-}" = "status" ]; then
  printf '%s\n' 'Logged in'
  exit 0
fi
staging=''
prompt=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output-last-message" ]; then
    staging=$2
    shift 2
  else
    prompt=$1
    shift
  fi
done
test -n "$staging"
case "$prompt" in
  *"operation: authoring."*)
    cat > "$staging" <<'AGENTUM_PROVIDER_EOF'
AGENTUM_SPEC_BEGIN
# Refresh Access Tokens

## Requirements

- RQ-001: Refresh an access token without ending the active session.

## Acceptance criteria

- AC-001: The refreshed token is stored while the session remains active.
AGENTUM_SPEC_END
AGENTUM_PROVIDER_EOF
    ;;
  *"operation: design."*)
    cat > "$staging" <<'AGENTUM_PROVIDER_EOF'
AGENTUM_DESIGN_BEGIN
# Design

Update the session record in place for RQ-001 and preserve its active flag.
Verify the resulting source change against AC-001.
AGENTUM_DESIGN_END
AGENTUM_PROVIDER_EOF
    ;;
  *"operation: planning."*)
    cat > "$staging" <<'AGENTUM_PROVIDER_EOF'
AGENTUM_PLAN_BEGIN
{"schemaVersion":1,"specId":"__SPEC_ID__","specRevision":2,"tasks":[{"id":"TSK-001","objective":"Refresh the access token without ending the session","dependencies":[],"readScopes":["src/session-store.js"],"writeScopes":["src/session-store.js"],"acceptanceCriteria":["AC-001"],"verification":[{"program":"git","args":["diff","--check"],"cwd":".","envAllowlist":["PATH"],"timeoutMs":60000,"outputLimit":262144}],"browserChecks":[],"risk":"low","parallelSafe":true}]}
AGENTUM_PLAN_END
AGENTUM_PROVIDER_EOF
    ;;
  *"operation: implementation_diff."*)
    cat > "$staging" <<'AGENTUM_PROVIDER_EOF'
AGENTUM_DIFF_BEGIN
diff --git a/src/session-store.js b/src/session-store.js
--- a/src/session-store.js
+++ b/src/session-store.js
@@ -1,3 +1,3 @@
 export function refreshAccessToken(session, token) {
-  throw new Error("not implemented");
+  return { ...session, accessToken: token };
 }
AGENTUM_DIFF_END
AGENTUM_PROVIDER_EOF
    ;;
  *"operation: independent_review."*)
    cat > "$staging" <<'AGENTUM_PROVIDER_EOF'
AGENTUM_REVIEW_BEGIN
# Independent review

AC-001 is satisfied by the bounded source change and successful verification evidence.

Verdict: PASS
AGENTUM_REVIEW_END
AGENTUM_PROVIDER_EOF
    ;;
  *) exit 9 ;;
esac
"##
        .replace("__SPEC_ID__", spec_id);
        let path = directory.join("codex");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)]
    async fn framed_worker_runs_real_authoring_through_durable_ready_without_local_fallback() {
        if !super::super::providers::isolation_available() {
            return;
        }
        let _environment_lock = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let repository = repository();
        std::fs::create_dir(repository.path().join("src")).unwrap();
        std::fs::write(
            repository.path().join("src/session-store.js"),
            "export function refreshAccessToken(session, token) {\n  throw new Error(\"not implemented\");\n}\n",
        )
        .unwrap();
        git(repository.path(), &["add", "src/session-store.js"]);
        git(repository.path(), &["commit", "-qm", "session fixture"]);
        let base_commit = String::from_utf8(
            std::process::Command::new("git")
                .args([
                    "-C",
                    repository.path().to_str().unwrap(),
                    "rev-parse",
                    "HEAD",
                ])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();

        let spec_id = SpecId::new();
        let provider_home = tempfile::tempdir().unwrap();
        deterministic_codex(provider_home.path(), &spec_id.to_string());
        let mut search_path = vec![provider_home.path().to_path_buf()];
        search_path.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let agentum_home = tempfile::tempdir().unwrap();
        let _environment = EnvironmentRestore::set(vec![
            ("PATH", std::env::join_paths(search_path).unwrap()),
            (
                "AGENTUM_HOME",
                agentum_home.path().as_os_str().to_os_string(),
            ),
        ]);

        let database = agentum_home.path().join("worker.sqlite");
        let store = Store::open(&database).await.unwrap();
        let host_id = Uuid::new_v4().to_string();
        let run_id = Uuid::new_v4().to_string();
        let repository_identity_sha256 = sha256(b"framed-worker-repository");
        let artifact_set_id = Ulid::new().to_string();
        let config = RemoteWorkerConfig {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            host_id: host_id.clone(),
            repositories: vec![RegisteredRepository {
                identity_sha256: repository_identity_sha256.clone(),
                artifact_set_id: artifact_set_id.clone(),
                path: repository.path().to_path_buf(),
            }],
        };
        let worker = RemoteSubsystemWorker::with_store(config.clone(), store.clone())
            .await
            .unwrap();
        let mut transport = FramedWorkerTransport { worker };

        let probe = transport
            .exchange(RemoteClientFrame::Probe(RemoteProbeRequest {
                schema_version: REMOTE_SDD_SCHEMA_VERSION,
                request_id: format!("probe-{}", "1".repeat(32)),
                host_id: host_id.clone(),
                repository_identity_sha256: repository_identity_sha256.clone(),
                provider: "codex".into(),
                base_ref: "HEAD".into(),
                expected_worker_version: WORKER_VERSION.into(),
            }))
            .await
            .unwrap();
        let RemoteServerFrame::ProbeResult(probe) = probe else {
            panic!("worker returned the wrong framed probe response")
        };
        assert!(probe.repository_registered);
        assert!(probe.provider_ready, "{:?}", probe.reason);
        assert_eq!(probe.base_commit.as_deref(), Some(base_commit.as_str()));

        let authoring_request = RemoteAuthoringRequest {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            request_id: format!("author-{}", "2".repeat(32)),
            host_id: host_id.clone(),
            run_id: run_id.clone(),
            spec_id: spec_id.to_string(),
            repository_identity_sha256: repository_identity_sha256.clone(),
            artifact_set_id: artifact_set_id.clone(),
            base_commit: base_commit.clone(),
            provider: "codex".into(),
            source_checkout: "require_clean".into(),
            title: "Refresh Access Tokens".into(),
            goal: "Refresh access tokens without interrupting active sessions".into(),
            timeout_ms: 120_000,
            output_limit: 2 * 1024 * 1024,
        };
        let authored = transport
            .exchange(RemoteClientFrame::AuthorSpec(authoring_request.clone()))
            .await
            .unwrap();
        let RemoteServerFrame::AuthoringResult(authored) = authored else {
            panic!("worker returned the wrong framed authoring response")
        };
        assert_eq!(authored.status, RemotePhaseStatus::Succeeded);
        assert_eq!(authored.spec_revision, 2);
        assert!(authored.spec.as_ref().unwrap().content.contains("AC-001"));
        let replay = transport
            .exchange(RemoteClientFrame::AuthorSpec(authoring_request))
            .await
            .unwrap();
        assert_eq!(replay, RemoteServerFrame::AuthoringResult(authored.clone()));
        assert!(!repository.path().join(".agentum").exists());

        // Recreate the subsystem worker from the same durable SQLite state
        // before the approved lifecycle begins, just as a fresh SSH channel
        // does after a process restart.
        transport = FramedWorkerTransport {
            worker: RemoteSubsystemWorker::with_store(config.clone(), store.clone())
                .await
                .unwrap(),
        };
        let plan = RemoteLifecyclePlan {
            schema_version: REMOTE_SDD_SCHEMA_VERSION,
            host_id: host_id.clone(),
            run_id: run_id.clone(),
            spec_id: spec_id.to_string(),
            spec_revision: 2,
            repository_identity_sha256,
            artifact_set_id,
            base_commit,
            provider: "codex".into(),
            approval_digest: sha256(b"independent human approval"),
            timeout_ms: 120_000,
            output_limit: 2 * 1024 * 1024,
        };
        let coordinator = SequentialRemoteLifecycle;
        let mut checkpoint =
            RemoteLifecycleCheckpoint::initial(&plan, authored.workspace_state_sha256.clone())
                .unwrap();
        let mut phases = Vec::new();
        while !checkpoint.is_ready() {
            let advance = coordinator
                .advance(&transport, &plan, &checkpoint)
                .await
                .unwrap();
            let blocker = if advance.result.status == RemotePhaseStatus::Succeeded {
                None
            } else {
                store
                    .sdd_remote_worker_run(&run_id)
                    .await
                    .unwrap()
                    .and_then(|run| run.blocker)
            };
            assert_eq!(
                advance.result.status,
                RemotePhaseStatus::Succeeded,
                "phase {:?} failed with {:?}: {:?}",
                advance.result.phase,
                advance.result.error_code,
                blocker
            );
            phases.push(advance.result.phase);
            checkpoint = advance.checkpoint;
            if checkpoint.completed_phases == 2 {
                transport = FramedWorkerTransport {
                    worker: RemoteSubsystemWorker::with_store(config.clone(), store.clone())
                        .await
                        .unwrap(),
                };
            }
        }
        assert_eq!(
            phases,
            vec![
                RemoteLifecyclePhase::Design,
                RemoteLifecyclePhase::Planning,
                RemoteLifecyclePhase::Implementation,
                RemoteLifecyclePhase::Verification,
                RemoteLifecyclePhase::Review,
            ]
        );

        let durable = store.sdd_remote_worker_run(&run_id).await.unwrap().unwrap();
        assert_eq!(durable.status, "ready");
        assert_eq!(durable.next_phase, "ready");
        assert_eq!(durable.completed_phases, 5);
        assert_eq!(
            durable.workspace_state_sha256,
            checkpoint.workspace_state_sha256
        );
        let authoritative = PathBuf::from(durable.authoritative_path);
        assert!(authoritative.starts_with(agentum_home.path()));
        assert_eq!(
            std::fs::read_to_string(authoritative.join("src/session-store.js")).unwrap(),
            "export function refreshAccessToken(session, token) {\n  return { ...session, accessToken: token };\n}\n"
        );
        let spec_directory = authoritative
            .join(".agentum/specs")
            .join(spec_id.directory_name("Refresh Access Tokens"));
        for artifact in ["spec.md", "design.md", "plan.json", "review.md"] {
            assert!(
                spec_directory.join(artifact).is_file(),
                "missing {artifact}"
            );
        }
        assert!(!repository.path().join(".agentum").exists());
        assert!(
            std::fs::read_to_string(repository.path().join("src/session-store.js"))
                .unwrap()
                .contains("not implemented")
        );
        let status = std::process::Command::new("git")
            .args([
                "-C",
                repository.path().to_str().unwrap(),
                "status",
                "--porcelain",
            ])
            .output()
            .unwrap();
        assert!(status.status.success());
        assert!(status.stdout.is_empty(), "source checkout was modified");
    }

    #[tokio::test]
    async fn probe_is_fail_closed_for_unregistered_repository() {
        let repository = repository();
        let database = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(database.path()).await.unwrap();
        let identity = sha256(b"registered");
        let host_id = Uuid::new_v4().to_string();
        let worker = RemoteSubsystemWorker::with_store(
            RemoteWorkerConfig {
                schema_version: 1,
                host_id: host_id.clone(),
                repositories: vec![RegisteredRepository {
                    identity_sha256: identity,
                    artifact_set_id: Ulid::new().to_string(),
                    path: repository.path().to_path_buf(),
                }],
            },
            store,
        )
        .await
        .unwrap();
        let result = worker
            .probe(RemoteProbeRequest {
                schema_version: 1,
                request_id: format!("probe-{}", "a".repeat(32)),
                host_id,
                repository_identity_sha256: sha256(b"other"),
                provider: "codex".into(),
                base_ref: "HEAD".into(),
                expected_worker_version: WORKER_VERSION.into(),
            })
            .await;
        assert!(!result.repository_registered);
        assert!(!result.provider_ready);
        assert_eq!(result.reason.as_deref(), Some("repository_not_registered"));
    }

    #[tokio::test]
    async fn workspace_hash_detects_external_edits() {
        let repository = repository();
        let first = workspace_state(repository.path()).await.unwrap();
        std::fs::write(repository.path().join("README.md"), "changed\n").unwrap();
        let second = workspace_state(repository.path()).await.unwrap();
        assert_ne!(first, second);
    }

    #[cfg(unix)]
    #[test]
    fn config_refuses_group_readable_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let repository = repository();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("worker.json");
        let config = RemoteWorkerConfig {
            schema_version: 1,
            host_id: Uuid::new_v4().to_string(),
            repositories: vec![RegisteredRepository {
                identity_sha256: sha256(b"repo"),
                artifact_set_id: Ulid::new().to_string(),
                path: repository.path().to_path_buf(),
            }],
        };
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(RemoteWorkerConfig::load(&path).is_err());
    }
}
