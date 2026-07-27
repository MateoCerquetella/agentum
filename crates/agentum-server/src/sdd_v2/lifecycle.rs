//! Autonomous Agentum-owned SDD lifecycle.
//!
//! The durable store owns phase state. Every provider gets a disposable
//! worktree and isolated session; only validated artifacts or bounded diffs
//! cross into the authoritative worktree.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};

use agentum_core::sdd::{
    ArtifactKind, BrowserCheck, BrowserCheckAssertion, BrowserWaitUntil, CommandSpec, PlanArtifact,
    PlanTask, SpecId, validate_relative_path,
};
use agentum_store::sdd::SddRunRecord;
use agentum_store::sdd::SddSpecRecord;
use agentum_store::sdd_browser_evidence::{
    IssueBrowserEvidenceGrantMutation, NewBrowserEvidence, NewBrowserEvidenceBlobRef,
    NewEvidenceBlob, SubmitBrowserEvidenceMutation,
};
use agentum_store::sdd_runtime::{
    ActivateAttemptMutation, BeginAttemptMutation, FailAttemptMutation, QuarantineRunMutation,
    RecordVerificationMutation, VerificationResultInput,
};
use agentum_store::sdd_runtime::{CompletePatchMutation, FailPatchMutation, ReservePatchMutation};
use base64::Engine as _;
use futures_util::StreamExt;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::AppState;
use crate::routes::sdd_v2::submit_artifact;

use super::artifacts::{
    MISSING_HASH, atomic_remove, atomic_write, content_hash, read_bytes, read_text,
};
use super::evidence::{
    BROWSER_EVIDENCE_SCHEMA_VERSION, BrowserAssertion, BrowserAssertionStatus, BrowserCaptureKind,
    BrowserCaptureRef, BrowserConsoleSummary, BrowserDiagnosticCoverage, BrowserEvidence,
    BrowserNetworkSummary, BrowserRuntime, BrowserTarget, StoredEvidenceBlob, persist_blob,
};
#[cfg(target_os = "windows")]
use super::providers::WINDOWS_LOCAL_SDD_REASON;
use super::providers::{
    DESIGN_BEGIN, DESIGN_END, DIFF_BEGIN, DIFF_END, PLAN_BEGIN, PLAN_END, ProviderAdapter,
    ProviderApprovalBinding, ProviderOperation, REVIEW_BEGIN, REVIEW_END, resolve_provider,
    run_artifact,
};
use super::workspace::{self, AttemptWorkspace};
use super::{artifacts::ArtifactError, providers::ProviderError, workspace::WorkspaceError};

const MAX_OVERLAY_BYTES: usize = 32 * 1024 * 1024;
const MAX_LOCAL_TASK_CONCURRENCY: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("store: {0}")]
    Store(#[from] agentum_store::StoreError),
    #[error("workspace: {0}")]
    Workspace(#[from] WorkspaceError),
    #[error("artifact: {0}")]
    Artifact(#[from] ArtifactError),
    #[error("provider: {0}")]
    Provider(#[from] ProviderError),
    #[error("api: {0}")]
    Api(#[from] crate::error::ApiError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid lifecycle output: {0}")]
    Invalid(String),
    #[error("git operation failed: {0}")]
    Git(String),
}

fn active_workers() -> &'static Mutex<HashSet<String>> {
    static ACTIVE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn active_commands() -> &'static Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>> {
    static ACTIVE: OnceLock<Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>>> =
        OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

struct CommandGuard(String);

impl Drop for CommandGuard {
    fn drop(&mut self) {
        active_commands()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.0);
    }
}

struct WorkerGuard(String);

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        active_workers()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.0);
    }
}

/// Start at most one local worker for a run. The store CAS remains the source
/// of truth; this registry only avoids redundant in-process provider calls.
pub fn spawn(state: AppState, run_id: String) -> bool {
    let inserted = active_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(run_id.clone());
    if !inserted {
        return false;
    }
    tokio::spawn(async move {
        let _guard = WorkerGuard(run_id.clone());
        if let Err(error) = drive_to_ready(&state, &run_id).await {
            tracing::error!(run_id, %error, "SDD lifecycle worker stopped");
            let _ = record_unhandled_failure(&state, &run_id, &error.to_string()).await;
        }
    });
    true
}

pub fn is_active(run_id: &str) -> bool {
    active_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(run_id)
}

/// Cancel verification commands owned by a run. Provider processes have a
/// separate registry because they use a different transport boundary.
pub fn cancel_run(run_id: &str) -> bool {
    let active = active_commands()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prefix = format!("{run_id}:");
    let mut canceled = false;
    for (execution_id, sender) in active.iter() {
        if (execution_id == run_id || execution_id.starts_with(&prefix))
            && sender.send(true).is_ok()
        {
            canceled = true;
        }
    }
    canceled
}

async fn drive_to_ready(state: &AppState, run_id: &str) -> Result<(), LifecycleError> {
    loop {
        let run = state
            .store
            .sdd_get_run(run_id)
            .await?
            .ok_or_else(|| agentum_store::StoreError::NotFound(run_id.into()))?;
        if lifecycle_must_stop(&run) {
            return Ok(());
        }
        if run.status != "queued" && run.status != "running" {
            return Ok(());
        }
        match run.phase.as_str() {
            "design" if run.status == "queued" => {
                execute_artifact_phase(state, &run, ArtifactKind::Design).await?;
            }
            "planning" if run.status == "queued" => {
                execute_artifact_phase(state, &run, ArtifactKind::Plan).await?;
            }
            "implementation" => execute_implementation(state, &run).await?,
            "verification" if run.status == "queued" => execute_verification(state, &run).await?,
            "review" if run.status == "queued" => {
                execute_artifact_phase(state, &run, ArtifactKind::Review).await?;
            }
            "ready" | "completed" => return Ok(()),
            _ => return Ok(()),
        }
    }
}

fn lifecycle_must_stop(run: &SddRunRecord) -> bool {
    run.quarantined != 0
        || matches!(run.phase.as_str(), "ready" | "delivery" | "completed")
        || matches!(
            run.status.as_str(),
            "idle" | "waiting" | "paused" | "blocked" | "failed" | "canceled" | "succeeded"
        )
}

struct PhaseAttempt {
    id: String,
    execution_id: String,
    workspace: AttemptWorkspace,
    staging_path: PathBuf,
    revision: i64,
}

async fn reserve_attempt(
    state: &AppState,
    run: &SddRunRecord,
    task_id: Option<&str>,
    session_role: &str,
) -> Result<PhaseAttempt, LifecycleError> {
    let repository = repository_path(&run.repo_id)?;
    let attempt_id = Uuid::new_v4().to_string();
    let authoritative = Path::new(&run.authoritative_path);
    let attempt_path = workspace::attempt_path(authoritative, &attempt_id)?;
    let session_identity = format!(
        "provider:{}:{}:{}",
        session_role,
        run.run_id,
        Uuid::new_v4()
    );
    let reserve_request = format!("internal-attempt-reserve-{}", Uuid::new_v4());
    let reserve_payload = json!({
        "runId": run.run_id,
        "revision": run.aggregate_revision + 1,
        "phase": run.phase,
        "attemptId": attempt_id,
        "taskId": task_id,
        "status": "queued"
    });
    let reserve_hash = super::sha256(reserve_payload.to_string());
    let reserved_revision = state
        .store
        .sdd_begin_attempt(BeginAttemptMutation {
            request_id: &reserve_request,
            request_hash: &reserve_hash,
            run_id: &run.run_id,
            expected_revision: run.aggregate_revision,
            phase: &run.phase,
            attempt_id: &attempt_id,
            task_id,
            provider: &state
                .store
                .sdd_get_spec(&run.spec_id)
                .await?
                .ok_or_else(|| agentum_store::StoreError::NotFound(run.spec_id.clone()))?
                .provider,
            isolated_path: &attempt_path.to_string_lossy(),
            session_identity: &session_identity,
            response_json: &reserve_payload.to_string(),
        })
        .await?;

    let materialized = async {
        let snapshot_digest = run_source_snapshot_digest(run)?;
        let workspace = workspace::create_attempt(
            &repository,
            authoritative,
            &attempt_id,
            &run.base_commit,
            snapshot_digest.as_deref(),
        )
        .await?;
        sync_authoritative_overlay(authoritative, &workspace.path).await?;
        let staging_directory = workspace
            .path
            .join(".agentum")
            .join("staging")
            .join(&attempt_id);
        create_owned_directories(&workspace.path, &staging_directory)?;
        let staging_path = staging_directory.join("result.txt");
        Ok::<_, LifecycleError>((workspace, staging_path))
    }
    .await;
    let (workspace, staging_path) = match materialized {
        Ok(value) => value,
        Err(error) => {
            let _ =
                workspace::recover_interrupted_attempt(&repository, authoritative, &attempt_path)
                    .await;
            fail_attempt(
                state,
                &run.run_id,
                reserved_revision,
                &attempt_id,
                "failed",
                &error.to_string(),
            )
            .await?;
            return Err(error);
        }
    };

    let activate_request = format!("internal-attempt-start-{}", Uuid::new_v4());
    let activate_payload = json!({
        "runId": run.run_id,
        "revision": reserved_revision + 1,
        "phase": run.phase,
        "attemptId": attempt_id,
        "taskId": task_id,
        "status": "running"
    });
    let activate_hash = super::sha256(activate_payload.to_string());
    let revision = match state
        .store
        .sdd_activate_attempt(ActivateAttemptMutation {
            request_id: &activate_request,
            request_hash: &activate_hash,
            run_id: &run.run_id,
            expected_revision: reserved_revision,
            attempt_id: &attempt_id,
            response_json: &activate_payload.to_string(),
        })
        .await
    {
        Ok(revision) => revision,
        Err(error) => {
            let _ = workspace::remove_attempt(&repository, &workspace.path).await;
            return Err(error.into());
        }
    };
    Ok(PhaseAttempt {
        execution_id: format!("{}:{}", run.run_id, attempt_id),
        id: attempt_id,
        workspace,
        staging_path,
        revision,
    })
}

fn run_source_snapshot_digest(run: &SddRunRecord) -> Result<Option<String>, LifecycleError> {
    let policy: serde_json::Value = serde_json::from_str(&run.policy_json)?;
    let Some(value) = policy.get("sourceSnapshotDigest") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let digest = value.as_str().ok_or_else(|| {
        LifecycleError::Invalid("run policy contains a malformed source snapshot digest".into())
    })?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LifecycleError::Invalid(
            "run policy contains a malformed source snapshot digest".into(),
        ));
    }
    Ok(Some(digest.to_ascii_lowercase()))
}

fn resolve_run_provider(
    spec: &SddSpecRecord,
    run: &SddRunRecord,
) -> Result<ProviderAdapter, LifecycleError> {
    let provider = resolve_provider(&spec.provider).map_err(|error| {
        LifecycleError::Invalid(format!("stored provider is unavailable: {error}"))
    })?;
    let policy: serde_json::Value = serde_json::from_str(&run.policy_json)?;
    let binding = policy.get("provider").ok_or_else(|| {
        LifecycleError::Invalid(
            "run policy does not bind the provider execution contract; reopen the phase".into(),
        )
    })?;
    let approved: ProviderApprovalBinding =
        serde_json::from_value(binding.clone()).map_err(|_| {
            LifecycleError::Invalid("run policy contains a malformed provider binding".into())
        })?;
    if approved != provider.approval_binding() {
        return Err(LifecycleError::Invalid(
            "provider manifest or execution contract changed after approval; reopen the phase"
                .into(),
        ));
    }
    Ok(provider)
}

async fn execute_artifact_phase(
    state: &AppState,
    run: &SddRunRecord,
    kind: ArtifactKind,
) -> Result<(), LifecycleError> {
    let spec = state
        .store
        .sdd_get_spec(&run.spec_id)
        .await?
        .ok_or_else(|| agentum_store::StoreError::NotFound(run.spec_id.clone()))?;
    let role = if kind == ArtifactKind::Review {
        "independent-review"
    } else {
        kind.file_name()
    };
    let provider = resolve_run_provider(&spec, run)?;
    let verification_evidence = if kind == ArtifactKind::Review {
        let records = state.store.sdd_verification_results(&run.run_id).await?;
        let browser = state.store.sdd_browser_evidence(&run.run_id).await?;
        Some(serde_json::to_string(&json!({
            "verificationResults": records
                .iter()
                .map(|record| {
                    json!({
                        "commandIndex": record.command_index,
                        "command": serde_json::from_str::<serde_json::Value>(&record.command_json)
                            .unwrap_or_else(|_| json!({"invalid": true})),
                        "status": record.status,
                        "exitCode": record.exit_code,
                        "outputHash": record.output_hash,
                        "outputSummary": record.output_excerpt,
                        "durationMs": record.duration_ms
                    })
                })
                .collect::<Vec<_>>(),
            "browserEvidence": browser.iter().map(|record| json!({
                "evidenceId": record.evidence_id,
                "attemptId": record.attempt_id,
                "checkId": record.check_id,
                "manifestSha256": record.manifest_sha256,
                "status": record.status,
                "capturedAt": record.captured_at,
                "manifest": record.evidence
            })).collect::<Vec<_>>()
        }))?)
    } else {
        None
    };
    let attempt = reserve_attempt(state, run, None, role).await?;
    let (begin, end, prompt) = match artifact_prompt(
        kind,
        &spec,
        &attempt.workspace.path,
        verification_evidence.as_deref(),
    ) {
        Ok(value) => value,
        Err(error) => {
            reject_attempt(state, run, &attempt, "failed", &error.to_string()).await?;
            return Ok(());
        }
    };
    let output = run_artifact(
        &attempt.execution_id,
        &provider,
        match kind {
            ArtifactKind::Design => ProviderOperation::Design,
            ArtifactKind::Plan => ProviderOperation::Planning,
            ArtifactKind::Review => ProviderOperation::Review,
            _ => unreachable!("phase kind validated by caller"),
        },
        &attempt.workspace.path.to_string_lossy(),
        &prompt,
        &attempt.staging_path.to_string_lossy(),
        begin,
        end,
    )
    .await;
    let content = match output {
        Ok(content) => content,
        Err(error) => {
            reject_attempt(
                state,
                run,
                &attempt,
                if matches!(error, ProviderError::Canceled) {
                    "canceled"
                } else {
                    "failed"
                },
                &error.to_string(),
            )
            .await?;
            return Ok(());
        }
    };
    if let Err(error) = validate_phase_artifact(kind, &content, &spec) {
        reject_attempt(state, run, &attempt, "failed", &error.to_string()).await?;
        return Ok(());
    }
    let current_run = state
        .store
        .sdd_get_run(&run.run_id)
        .await?
        .ok_or_else(|| agentum_store::StoreError::NotFound(run.run_id.clone()))?;
    if current_run.aggregate_revision != attempt.revision || current_run.status != "running" {
        cleanup_attempt(state, run, &attempt.workspace.path).await?;
        return Ok(());
    }
    let target = Path::new(&run.authoritative_path)
        .join(".agentum/specs")
        .join(&spec.slug)
        .join(kind.file_name());
    let expected_hash = match content_hash(&target) {
        Ok(hash) => hash,
        Err(error) => {
            reject_attempt(state, run, &attempt, "failed", &error.to_string()).await?;
            return Ok(());
        }
    };
    let request_id = format!("internal-artifact-{}", Uuid::new_v4());
    let request_hash = super::sha256(
        json!({
            "type": "submitArtifact",
            "kind": kind,
            "content": content,
            "attemptId": attempt.id,
            "expectedContentHash": expected_hash
        })
        .to_string(),
    );
    if let Err(error) = submit_artifact(
        state,
        &current_run,
        &request_id,
        &request_hash,
        attempt.revision,
        kind,
        &content,
        &expected_hash,
        &attempt.id,
    )
    .await
    {
        reject_attempt(state, run, &attempt, "failed", &error.to_string()).await?;
        return Ok(());
    }
    cleanup_attempt(state, run, &attempt.workspace.path).await?;
    Ok(())
}

fn artifact_prompt(
    kind: ArtifactKind,
    spec: &agentum_store::sdd::SddSpecRecord,
    attempt: &Path,
    verification_evidence: Option<&str>,
) -> Result<(&'static str, &'static str, String), LifecycleError> {
    let artifact_root = attempt.join(".agentum/specs").join(&spec.slug);
    let spec_path = artifact_root.join("spec.md");
    let (_, _) = read_text(&spec_path)?;
    match kind {
        ArtifactKind::Design => Ok((
            DESIGN_BEGIN,
            DESIGN_END,
            format!(
                "You are the design agent in an Agentum-owned workflow. Read {:?} and the repository. Do not edit files, change Git, contact networks beyond your model transport, or implement code. Produce design.md with concrete architecture, boundaries, data flow, failure handling, and verification strategy tied to RQ/AC identifiers. Return only Markdown between literal lines {DESIGN_BEGIN} and {DESIGN_END}.",
                spec_path
            ),
        )),
        ArtifactKind::Plan => Ok((
            PLAN_BEGIN,
            PLAN_END,
            format!(
                "You are the planning agent in an Agentum-owned workflow. Read {:?}, {:?}, and the repository. Do not edit files or change Git. Return only a JSON object between literal lines {PLAN_BEGIN} and {PLAN_END}. It must have schemaVersion:1, specId:{:?}, specRevision:{}, and a non-empty tasks array. Each task requires id, objective, dependencies, relative traversal-free readScopes/writeScopes, acceptanceCriteria using AC-* ids, verification CommandSpec objects (program,args,cwd,envAllowlist,timeoutMs,outputLimit), browserChecks, risk, and parallelSafe. browserChecks are optional typed objects with id, an http(s) url, acceptanceCriteria, waitUntil (load|dom_content_loaded|network_idle), viewport {{width,height,deviceScaleMilli}}, timeoutMs, and assertions tagged page_loaded, text_present, selector_visible, or url_contains. Assertion ids use BV- followed by three digits. Commands are direct programs; never generate shell strings or bash -lc.",
                spec_path,
                artifact_root.join("design.md"),
                spec.spec_id,
                spec.current_revision
            ),
        )),
        ArtifactKind::Review => Ok((
            REVIEW_BEGIN,
            REVIEW_END,
            format!(
                "You are an independent review agent in a new isolated session. Read {:?}, {:?}, {:?}, the implemented diff, and this Agentum-owned verification evidence: {}. Do not edit files or change Git. Review every RQ/AC, security boundary, and regression risk. If and only if the work is ready, include a line exactly `Verdict: PASS`; otherwise include `Verdict: FAIL` with blockers. Return only review.md Markdown between literal lines {REVIEW_BEGIN} and {REVIEW_END}.",
                spec_path,
                artifact_root.join("design.md"),
                artifact_root.join("plan.json"),
                verification_evidence.unwrap_or("[]")
            ),
        )),
        _ => Err(LifecycleError::Invalid(
            "phase does not produce a provider artifact".into(),
        )),
    }
}

pub(crate) fn validate_phase_artifact(
    kind: ArtifactKind,
    content: &str,
    spec: &agentum_store::sdd::SddSpecRecord,
) -> Result<(), LifecycleError> {
    if content.trim().is_empty() || content.len() > 2 * 1024 * 1024 {
        return Err(LifecycleError::Invalid(
            "phase artifact is empty or exceeds 2 MiB".into(),
        ));
    }
    match kind {
        ArtifactKind::Design => {
            if !content.contains("RQ-") || !content.contains("AC-") {
                return Err(LifecycleError::Invalid(
                    "design must trace requirements and acceptance criteria".into(),
                ));
            }
        }
        ArtifactKind::Plan => {
            let canonical: SpecId =
                spec.spec_id
                    .parse()
                    .map_err(|error: agentum_core::sdd::SddContractError| {
                        LifecycleError::Invalid(error.to_string())
                    })?;
            validate_plan(content, &canonical, spec.current_revision)?;
        }
        ArtifactKind::Review => {
            let pass_count = content
                .lines()
                .filter(|line| line.trim() == "Verdict: PASS")
                .count();
            if pass_count != 1 || content.lines().any(|line| line.trim() == "Verdict: FAIL") {
                return Err(LifecycleError::Invalid(
                    "independent review did not return one unambiguous PASS verdict".into(),
                ));
            }
            if !content.contains("AC-") {
                return Err(LifecycleError::Invalid(
                    "review must cite acceptance criteria".into(),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn validate_plan(
    content: &str,
    spec_id: &SpecId,
    revision: i64,
) -> Result<(), LifecycleError> {
    let plan: PlanArtifact = serde_json::from_str(content)?;
    if plan.schema_version != 1 || &plan.spec_id != spec_id || plan.spec_revision != revision {
        return Err(LifecycleError::Invalid(
            "plan identity does not match the approved specification".into(),
        ));
    }
    if plan.tasks.is_empty() || plan.tasks.len() > 256 {
        return Err(LifecycleError::Invalid(
            "plan must contain between one and 256 tasks".into(),
        ));
    }
    let ids: HashSet<_> = plan.tasks.iter().map(|task| task.id.as_str()).collect();
    if ids.len() != plan.tasks.len()
        || plan
            .tasks
            .iter()
            .any(|task| task.id.trim().is_empty() || task.objective.trim().is_empty())
    {
        return Err(LifecycleError::Invalid(
            "plan task ids/objectives must be unique and non-empty".into(),
        ));
    }
    let browser_check_count = plan
        .tasks
        .iter()
        .map(|task| task.browser_checks.len())
        .sum::<usize>();
    let browser_check_ids = plan
        .tasks
        .iter()
        .flat_map(|task| task.browser_checks.iter().map(|check| check.id.as_str()))
        .collect::<HashSet<_>>();
    if browser_check_ids.len() != browser_check_count {
        return Err(LifecycleError::Invalid(
            "browser check ids must be unique across the complete plan".into(),
        ));
    }
    for task in &plan.tasks {
        if task.dependencies.len() > 256
            || task.read_scopes.len() > 256
            || task.write_scopes.len() > 256
            || task.acceptance_criteria.len() > 256
            || task.verification.len() > 32
            || task.browser_checks.len() > 32
        {
            return Err(LifecycleError::Invalid(format!(
                "task {} exceeds plan collection limits",
                task.id
            )));
        }
        if task
            .dependencies
            .iter()
            .any(|dependency| dependency == &task.id || !ids.contains(dependency.as_str()))
        {
            return Err(LifecycleError::Invalid(format!(
                "task {} has an invalid dependency",
                task.id
            )));
        }
        if task.write_scopes.is_empty()
            || task
                .read_scopes
                .iter()
                .chain(task.write_scopes.iter())
                .any(|path| validate_relative_path(path).is_err())
        {
            return Err(LifecycleError::Invalid(format!(
                "task {} has missing or unsafe path scopes",
                task.id
            )));
        }
        for command in &task.verification {
            if !verification_command_is_safe(command) {
                return Err(LifecycleError::Invalid(format!(
                    "task {} contains an unsafe verification command",
                    task.id
                )));
            }
        }
        for check in &task.browser_checks {
            validate_browser_check(check).map_err(|message| {
                LifecycleError::Invalid(format!("task {} browser check: {message}", task.id))
            })?;
        }
    }
    ensure_acyclic(&plan.tasks)?;
    Ok(())
}

pub(crate) fn validate_browser_check(check: &BrowserCheck) -> Result<(), String> {
    if check.id.trim().is_empty()
        || check.id.len() > 64
        || !check
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || check.acceptance_criteria.is_empty()
        || check.acceptance_criteria.len() > 64
        || check.acceptance_criteria.iter().any(|value| {
            !value.starts_with("AC-")
                || value.len() > 19
                || value.bytes().any(|byte| {
                    !(byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
                })
        })
        || check.assertions.is_empty()
        || check.assertions.len() > 64
        || !(100..=120_000).contains(&check.timeout_ms)
        || !(1..=16_384).contains(&check.viewport.width)
        || !(1..=16_384).contains(&check.viewport.height)
        || !(100..=8_000).contains(&check.viewport.device_scale_milli)
    {
        return Err(
            "identity, AC references, viewport, timeout, or assertion count is invalid".into(),
        );
    }
    let url = reqwest::Url::parse(&check.url).map_err(|_| "url is invalid")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("url must be credential-free http(s)".into());
    }
    ensure_sdd_browser_origin_allowed(&url)?;
    let mut assertion_ids = HashSet::new();
    for assertion in &check.assertions {
        let id = assertion.id();
        let valid_id = id.strip_prefix("BV-").is_some_and(|suffix| {
            suffix.len() == 3 && suffix.bytes().all(|byte| byte.is_ascii_digit())
        });
        let payload_safe = match assertion {
            BrowserCheckAssertion::PageLoaded {
                expected_status, ..
            } => (100..=599).contains(expected_status),
            BrowserCheckAssertion::TextPresent { text, .. } => {
                !text.is_empty() && text.len() <= 4096 && !text.chars().any(char::is_control)
            }
            BrowserCheckAssertion::SelectorVisible { selector, .. } => {
                !selector.is_empty()
                    && selector.len() <= 2048
                    && !selector.chars().any(char::is_control)
            }
            BrowserCheckAssertion::UrlContains { value, .. } => {
                !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
            }
        };
        if !valid_id || !payload_safe || !assertion_ids.insert(id) {
            return Err("assertion identity or bounded payload is invalid".into());
        }
    }
    Ok(())
}

pub(crate) fn ensure_sdd_browser_origin_allowed(url: &reqwest::Url) -> Result<(), String> {
    let origin = url.origin().ascii_serialization();
    if let Some(configured) = std::env::var_os("AGENTUM_SDD_BROWSER_ALLOWED_ORIGINS") {
        let configured = configured
            .to_str()
            .ok_or_else(|| "AGENTUM_SDD_BROWSER_ALLOWED_ORIGINS is not UTF-8".to_owned())?;
        let mut allowed = HashSet::new();
        for raw in configured
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let candidate = reqwest::Url::parse(raw)
                .map_err(|_| "AGENTUM_SDD_BROWSER_ALLOWED_ORIGINS contains an invalid URL")?;
            if !matches!(candidate.scheme(), "http" | "https")
                || candidate.host_str().is_none()
                || !candidate.username().is_empty()
                || candidate.password().is_some()
                || candidate.path() != "/"
                || candidate.query().is_some()
                || candidate.fragment().is_some()
            {
                return Err(
                    "AGENTUM_SDD_BROWSER_ALLOWED_ORIGINS must contain exact credential-free origins"
                        .into(),
                );
            }
            allowed.insert(candidate.origin().ascii_serialization());
        }
        if allowed.is_empty() || !allowed.contains(&origin) {
            return Err(format!(
                "origin {origin} is not in the SDD browser allowlist"
            ));
        }
        return Ok(());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "browser target has no host".to_owned())?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if loopback {
        Ok(())
    } else {
        Err(format!(
            "origin {origin} is blocked; the default SDD browser policy permits only loopback targets"
        ))
    }
}

fn verification_command_is_safe(command: &CommandSpec) -> bool {
    let program = command.program.trim().to_ascii_lowercase();
    let forbidden_programs = [
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
    let safe_cwd = command.cwd == "." || validate_relative_path(&command.cwd).is_ok();
    let safe_env = command.env_allowlist.len() <= 32
        && command
            .env_allowlist
            .iter()
            .all(|key| verification_env_is_safe(key));
    !program.is_empty()
        && !command.program.contains(['/', '\\'])
        && !forbidden_programs.contains(&program.as_str())
        && command.args.len() <= 256
        && command
            .args
            .iter()
            .all(|argument| !argument.contains('\0') && argument.len() <= 64 * 1024)
        && safe_cwd
        && safe_env
        && command.timeout_ms > 0
        && command.timeout_ms <= 60 * 60 * 1000
        && command.output_limit > 0
        && command.output_limit <= 16 * 1024 * 1024
}

fn verification_env_is_safe(key: &str) -> bool {
    matches!(
        key,
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
}

fn ensure_acyclic(tasks: &[PlanTask]) -> Result<(), LifecycleError> {
    fn visit<'a>(
        id: &'a str,
        tasks: &HashMap<&'a str, &'a PlanTask>,
        visiting: &mut HashSet<&'a str>,
        visited: &mut HashSet<&'a str>,
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
    let by_id: HashMap<_, _> = tasks.iter().map(|task| (task.id.as_str(), task)).collect();
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    if tasks
        .iter()
        .all(|task| visit(&task.id, &by_id, &mut visiting, &mut visited))
    {
        Ok(())
    } else {
        Err(LifecycleError::Invalid(
            "plan task graph has a cycle".into(),
        ))
    }
}

fn repository_path(repo_id: &str) -> Result<PathBuf, LifecycleError> {
    Ok(PathBuf::from(crate::routes::repos::resolve_repo_path(
        repo_id,
    )?))
}

pub(crate) async fn sync_authoritative_overlay(
    authoritative: &Path,
    attempt: &Path,
) -> Result<(), LifecycleError> {
    let diff = git_output(
        authoritative,
        &["diff", "--binary", "--no-ext-diff", "HEAD", "--", "."],
        MAX_OVERLAY_BYTES,
    )
    .await?;
    if !diff.is_empty() {
        git_apply(attempt, &diff).await?;
    }
    let untracked = git_output(
        authoritative,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        MAX_OVERLAY_BYTES,
    )
    .await?;
    for raw in untracked
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
    {
        let relative = std::str::from_utf8(raw)
            .map_err(|_| LifecycleError::Invalid("non-UTF-8 untracked path".into()))?;
        validate_relative_path(relative)
            .map_err(|_| LifecycleError::Invalid(format!("unsafe overlay path: {relative}")))?;
        let source = authoritative.join(relative);
        let metadata = std::fs::symlink_metadata(&source)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(LifecycleError::Invalid(format!(
                "unsupported untracked overlay entry: {relative}"
            )));
        }
        let (bytes, _) = read_bytes(&source)?;
        let destination = attempt.join(relative);
        if let Some(parent) = destination.parent() {
            create_owned_directories(attempt, parent)?;
        }
        atomic_write(&destination, &bytes, Some(MISSING_HASH))?;
    }
    Ok(())
}

pub(crate) fn create_owned_directories(root: &Path, target: &Path) -> Result<(), LifecycleError> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| LifecycleError::Invalid("directory escaped the attempt".into()))?;
    let metadata = std::fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(LifecycleError::Invalid("attempt root is unsafe".into()));
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(part) = component else {
            return Err(LifecycleError::Invalid("unsafe directory component".into()));
        };
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(LifecycleError::Invalid(format!(
                    "unsafe attempt directory: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

async fn git_output(cwd: &Path, args: &[&str], limit: usize) -> Result<Vec<u8>, LifecycleError> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await?;
    if !output.status.success() {
        return Err(LifecycleError::Git(output.status.to_string()));
    }
    if output.stdout.len() > limit || output.stderr.len() > 128 * 1024 {
        return Err(LifecycleError::Invalid(
            "git output exceeded its configured limit".into(),
        ));
    }
    Ok(output.stdout)
}

async fn git_apply(cwd: &Path, patch: &[u8]) -> Result<(), LifecycleError> {
    let mut child = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["apply", "--binary", "--whitespace=nowarn"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| LifecycleError::Git("git apply stdin unavailable".into()))?
        .write_all(patch)
        .await?;
    let status = child.wait().await?;
    if status.success() {
        Ok(())
    } else {
        Err(LifecycleError::Git(status.to_string()))
    }
}

async fn cleanup_attempt(
    state: &AppState,
    run: &SddRunRecord,
    attempt: &Path,
) -> Result<(), LifecycleError> {
    let repository = repository_path(&run.repo_id)?;
    match workspace::remove_attempt(&repository, attempt).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let blocker =
                format!("disposable worktree cleanup failed; recovery evidence preserved: {error}");
            if let Some(current) = state.store.sdd_get_run(&run.run_id).await? {
                if current.quarantined != 0 {
                    return Err(error.into());
                }
                let request_id = format!("internal-cleanup-quarantine-{}", Uuid::new_v4());
                let payload = json!({
                    "runId": run.run_id,
                    "revision": current.aggregate_revision + 1,
                    "status": "blocked",
                    "quarantined": true,
                    "blocker": blocker
                });
                let request_hash = super::sha256(payload.to_string());
                match state
                    .store
                    .sdd_quarantine_run(QuarantineRunMutation {
                        request_id: &request_id,
                        request_hash: &request_hash,
                        run_id: &run.run_id,
                        expected_revision: current.aggregate_revision,
                        blocker: &blocker,
                        response_json: &payload.to_string(),
                    })
                    .await
                {
                    Ok(_) | Err(agentum_store::StoreError::StaleRevision { .. }) => {}
                    Err(store_error) => return Err(store_error.into()),
                }
            }
            Err(error.into())
        }
    }
}

async fn fail_attempt(
    state: &AppState,
    run_id: &str,
    expected_revision: i64,
    attempt_id: &str,
    status: &str,
    blocker: &str,
) -> Result<(), LifecycleError> {
    let request_id = format!("internal-attempt-fail-{}", Uuid::new_v4());
    let payload = json!({
        "runId": run_id,
        "revision": expected_revision + 1,
        "attemptId": attempt_id,
        "status": status,
        "blocker": blocker
    });
    let request_hash = super::sha256(payload.to_string());
    match state
        .store
        .sdd_fail_attempt(FailAttemptMutation {
            request_id: &request_id,
            request_hash: &request_hash,
            run_id,
            expected_revision,
            attempt_id,
            status,
            blocker,
            event_kind: if status == "canceled" {
                "sdd.attempt.canceled"
            } else {
                "sdd.attempt.failed"
            },
            response_json: &payload.to_string(),
        })
        .await
    {
        Ok(_) => Ok(()),
        Err(agentum_store::StoreError::StaleRevision { .. }) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn reject_attempt(
    state: &AppState,
    run: &SddRunRecord,
    attempt: &PhaseAttempt,
    status: &str,
    blocker: &str,
) -> Result<(), LifecycleError> {
    cleanup_attempt(state, run, &attempt.workspace.path).await?;
    fail_attempt(
        state,
        &run.run_id,
        attempt.revision,
        &attempt.id,
        status,
        blocker,
    )
    .await
}

async fn record_unhandled_failure(
    state: &AppState,
    run_id: &str,
    blocker: &str,
) -> Result<(), LifecycleError> {
    let Some(run) = state.store.sdd_get_run(run_id).await? else {
        return Ok(());
    };
    if matches!(
        run.status.as_str(),
        "failed" | "blocked" | "paused" | "canceled" | "succeeded"
    ) {
        return Ok(());
    }
    let request_id = format!("internal-lifecycle-fail-{}", Uuid::new_v4());
    let payload = json!({
        "runId": run_id,
        "revision": run.aggregate_revision + 1,
        "phase": run.phase,
        "status": "failed",
        "blocker": blocker
    });
    let request_hash = super::sha256(payload.to_string());
    let result = state
        .store
        .sdd_transition(agentum_store::sdd::TransitionMutation {
            request_id: &request_id,
            request_hash: &request_hash,
            run_id,
            expected_revision: run.aggregate_revision,
            phase: &run.phase,
            status: "failed",
            blocker: Some(blocker),
            event_kind: "sdd.run.failed",
            response_json: &payload.to_string(),
        })
        .await;
    match result {
        Ok(_) | Err(agentum_store::StoreError::StaleRevision { .. }) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn execute_implementation(
    state: &AppState,
    run: &SddRunRecord,
) -> Result<(), LifecycleError> {
    loop {
        let current_run = state
            .store
            .sdd_get_run(&run.run_id)
            .await?
            .ok_or_else(|| agentum_store::StoreError::NotFound(run.run_id.clone()))?;
        if current_run.phase != "implementation"
            || !matches!(current_run.status.as_str(), "queued" | "running")
        {
            return Ok(());
        }
        let tasks = state.store.sdd_tasks(&run.run_id).await?;
        if tasks.is_empty() {
            return Err(LifecycleError::Invalid(
                "implementation phase has no current plan tasks".into(),
            ));
        }
        let statuses = tasks
            .iter()
            .map(|task| (task.task_id.clone(), task.runtime_status.clone()))
            .collect::<HashMap<_, _>>();
        if statuses.values().all(|status| status == "succeeded") {
            return Err(LifecycleError::Invalid(
                "all tasks succeeded but the run did not enter verification".into(),
            ));
        }
        let planned = tasks
            .iter()
            .map(|record| serde_json::from_str::<PlanTask>(&record.intent_json))
            .collect::<Result<Vec<_>, _>>()?;
        let batch = select_ready_task_batch(&planned, &statuses, MAX_LOCAL_TASK_CONCURRENCY);
        if batch.is_empty() {
            return Err(LifecycleError::Invalid(
                "task DAG has no runnable node".into(),
            ));
        }
        execute_task_batch(state, &current_run, batch).await?;
    }
}

fn select_ready_task_batch(
    tasks: &[PlanTask],
    statuses: &HashMap<String, String>,
    limit: usize,
) -> Vec<PlanTask> {
    let ready = tasks
        .iter()
        .filter(|task| {
            statuses.get(&task.id).is_some_and(|status| {
                matches!(status.as_str(), "idle" | "queued" | "retry_scheduled")
            }) && task.dependencies.iter().all(|dependency| {
                statuses
                    .get(dependency)
                    .is_some_and(|status| status == "succeeded")
            })
        })
        .collect::<Vec<_>>();
    let Some(first) = ready.first() else {
        return Vec::new();
    };
    if !first.parallel_safe || limit <= 1 {
        return vec![(*first).clone()];
    }
    let mut batch = Vec::new();
    for task in ready {
        if batch.len() >= limit {
            break;
        }
        if !task.parallel_safe
            || batch
                .iter()
                .any(|selected| task_scopes_conflict(selected, task))
        {
            continue;
        }
        batch.push(task.clone());
    }
    batch
}

fn task_scopes_conflict(left: &PlanTask, right: &PlanTask) -> bool {
    left.write_scopes.iter().any(|left_write| {
        right
            .write_scopes
            .iter()
            .chain(right.read_scopes.iter())
            .any(|right_scope| scopes_overlap(left_write, right_scope))
    }) || right.write_scopes.iter().any(|right_write| {
        left.read_scopes
            .iter()
            .any(|left_read| scopes_overlap(right_write, left_read))
    })
}

fn scopes_overlap(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

struct PreparedTaskAttempt {
    ordinal: usize,
    task: PlanTask,
    attempt: PhaseAttempt,
}

struct ProducedTaskAttempt {
    ordinal: usize,
    attempt: PhaseAttempt,
    result: Result<Vec<String>, ProducedTaskFailure>,
}

struct ProducedTaskFailure {
    status: &'static str,
    blocker: String,
}

async fn execute_task_batch(
    state: &AppState,
    run: &SddRunRecord,
    tasks: Vec<PlanTask>,
) -> Result<(), LifecycleError> {
    let spec = state
        .store
        .sdd_get_spec(&run.spec_id)
        .await?
        .ok_or_else(|| agentum_store::StoreError::NotFound(run.spec_id.clone()))?;
    let provider = resolve_run_provider(&spec, run)?;
    let mut reservation_run = run.clone();
    let mut prepared = Vec::with_capacity(tasks.len());
    for (ordinal, task) in tasks.into_iter().enumerate() {
        match reserve_attempt(state, &reservation_run, Some(&task.id), "implementation").await {
            Ok(attempt) => prepared.push(PreparedTaskAttempt {
                ordinal,
                task,
                attempt,
            }),
            Err(error) => {
                for reserved in prepared {
                    settle_attempt_current(
                        state,
                        run,
                        &reserved.attempt,
                        "failed",
                        "parallel task batch reservation was interrupted",
                    )
                    .await?;
                }
                return Err(error);
            }
        }
        reservation_run = state
            .store
            .sdd_get_run(&run.run_id)
            .await?
            .ok_or_else(|| agentum_store::StoreError::NotFound(run.run_id.clone()))?;
        if reservation_run.phase != "implementation" || reservation_run.status != "running" {
            for reserved in prepared {
                settle_attempt_current(
                    state,
                    run,
                    &reserved.attempt,
                    "canceled",
                    "run stopped while the parallel task batch was reserved",
                )
                .await?;
            }
            return Ok(());
        }
    }

    let futures = prepared
        .into_iter()
        .map(|prepared| produce_task_attempt(provider.clone(), prepared))
        .collect::<Vec<_>>();
    let mut produced = collect_bounded(futures, MAX_LOCAL_TASK_CONCURRENCY).await;
    produced.sort_by_key(|result| result.ordinal);

    let mut batch_failed = false;
    for produced in produced {
        if batch_failed {
            settle_attempt_current(
                state,
                run,
                &produced.attempt,
                "canceled",
                "another task in the parallel batch failed",
            )
            .await?;
            continue;
        }
        let current = state
            .store
            .sdd_get_run(&run.run_id)
            .await?
            .ok_or_else(|| agentum_store::StoreError::NotFound(run.run_id.clone()))?;
        if current.phase != "implementation" || current.status != "running" {
            settle_attempt_current(
                state,
                run,
                &produced.attempt,
                "canceled",
                "run stopped before the task result could be published",
            )
            .await?;
            batch_failed = true;
            continue;
        }
        let changed_paths = match produced.result {
            Ok(paths) => paths,
            Err(failure) => {
                settle_attempt_current(
                    state,
                    run,
                    &produced.attempt,
                    failure.status,
                    &failure.blocker,
                )
                .await?;
                batch_failed = true;
                continue;
            }
        };
        let changes = match collect_patch_changes(
            Path::new(&run.authoritative_path),
            &produced.attempt.workspace.path,
            &changed_paths,
        ) {
            Ok(changes) if !changes.is_empty() => changes,
            Ok(_) => {
                settle_attempt_current(
                    state,
                    run,
                    &produced.attempt,
                    "failed",
                    "provider diff produced no content change",
                )
                .await?;
                batch_failed = true;
                continue;
            }
            Err(error) => {
                settle_attempt_current(state, run, &produced.attempt, "failed", &error.to_string())
                    .await?;
                batch_failed = true;
                continue;
            }
        };
        if let Err(error) = publish_patch(state, &current, &produced.attempt, &changes).await {
            settle_attempt_current(state, run, &produced.attempt, "failed", &error.to_string())
                .await?;
            batch_failed = true;
            continue;
        }
        cleanup_attempt(state, run, &produced.attempt.workspace.path).await?;
    }
    Ok(())
}

async fn collect_bounded<F, T>(futures: Vec<F>, limit: usize) -> Vec<T>
where
    F: Future<Output = T>,
{
    futures_util::stream::iter(futures)
        .buffer_unordered(limit.max(1))
        .collect()
        .await
}

async fn produce_task_attempt(
    provider: ProviderAdapter,
    prepared: PreparedTaskAttempt,
) -> ProducedTaskAttempt {
    let prompt = format!(
        "You are an implementation agent in an Agentum-owned workflow. Read the repository and its .agentum spec/design/plan artifacts. Implement only task {:?}: {:?}. Dependencies are already applied. You may propose changes only within these write scopes: {:?}. Satisfy acceptance criteria {:?}. Do not edit files, run Git, commit, push, contact trackers, or emit binary/rename patches. Return one ordinary UTF-8 unified Git diff between literal lines {DIFF_BEGIN} and {DIFF_END}. Each changed file must begin with an ordinary `diff --git a/<path> b/<path>` header followed by `---`, `+++`, and `@@` hunk headers. Do not put Markdown fences, prose, `*** Begin Patch`, or `*** End Patch` inside the markers. Agentum alone will validate and apply it.",
        prepared.task.id,
        prepared.task.objective,
        prepared.task.write_scopes,
        prepared.task.acceptance_criteria
    );
    let result = match run_artifact(
        &prepared.attempt.execution_id,
        &provider,
        ProviderOperation::ImplementationDiff,
        &prepared.attempt.workspace.path.to_string_lossy(),
        &prompt,
        &prepared.attempt.staging_path.to_string_lossy(),
        DIFF_BEGIN,
        DIFF_END,
    )
    .await
    {
        Ok(diff) => validate_and_apply_provider_diff(
            &prepared.attempt.workspace.path,
            &diff,
            &prepared.task,
        )
        .await
        .map_err(|error| ProducedTaskFailure {
            status: "failed",
            blocker: error.to_string(),
        }),
        Err(error) => Err(ProducedTaskFailure {
            status: if matches!(error, ProviderError::Canceled) {
                "canceled"
            } else {
                "failed"
            },
            blocker: error.to_string(),
        }),
    };
    ProducedTaskAttempt {
        ordinal: prepared.ordinal,
        attempt: prepared.attempt,
        result,
    }
}

async fn settle_attempt_current(
    state: &AppState,
    run: &SddRunRecord,
    attempt: &PhaseAttempt,
    status: &str,
    blocker: &str,
) -> Result<(), LifecycleError> {
    cleanup_attempt(state, run, &attempt.workspace.path).await?;
    let active = state
        .store
        .sdd_attempts(&run.run_id)
        .await?
        .into_iter()
        .any(|record| {
            record.attempt_id == attempt.id
                && matches!(record.status.as_str(), "queued" | "running")
        });
    if !active {
        return Ok(());
    }
    for _ in 0..3 {
        let current = state
            .store
            .sdd_get_run(&run.run_id)
            .await?
            .ok_or_else(|| agentum_store::StoreError::NotFound(run.run_id.clone()))?;
        let request_id = format!("internal-attempt-settle-{}", Uuid::new_v4());
        let payload = json!({
            "runId": run.run_id,
            "revision": current.aggregate_revision + 1,
            "attemptId": attempt.id,
            "status": status,
            "blocker": blocker
        });
        let request_hash = super::sha256(payload.to_string());
        match state
            .store
            .sdd_fail_attempt(FailAttemptMutation {
                request_id: &request_id,
                request_hash: &request_hash,
                run_id: &run.run_id,
                expected_revision: current.aggregate_revision,
                attempt_id: &attempt.id,
                status,
                blocker,
                event_kind: if status == "canceled" {
                    "sdd.attempt.canceled"
                } else {
                    "sdd.attempt.failed"
                },
                response_json: &payload.to_string(),
            })
            .await
        {
            Ok(_) => return Ok(()),
            Err(agentum_store::StoreError::StaleRevision { .. }) => continue,
            Err(agentum_store::StoreError::InvalidCommand(_)) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    Err(LifecycleError::Invalid(
        "attempt settlement lost three aggregate revision races".into(),
    ))
}

#[derive(Debug)]
struct PatchChange {
    relative_path: String,
    preimage: Option<Vec<u8>>,
    preimage_hash: String,
    postimage: Option<Vec<u8>>,
    postimage_hash: String,
}

pub(crate) async fn validate_and_apply_provider_diff(
    attempt: &Path,
    diff: &str,
    task: &PlanTask,
) -> Result<Vec<String>, LifecycleError> {
    if diff.is_empty()
        || diff.len() > 2 * 1024 * 1024
        || diff.contains('\0')
        || diff.contains("GIT binary patch")
        || diff.lines().any(|line| {
            line.starts_with("rename from ")
                || line.starts_with("rename to ")
                || line.starts_with("copy from ")
                || line.starts_with("copy to ")
                || line.starts_with("Binary files ")
        })
    {
        return Err(LifecycleError::Invalid(
            "provider diff is empty, oversized, binary, or uses unsupported rename/copy syntax"
                .into(),
        ));
    }
    let numstat =
        git_apply_output(attempt, &["--recount", "--numstat", "-z"], diff.as_bytes()).await?;
    let mut paths = Vec::new();
    for record in numstat
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
    {
        let record = std::str::from_utf8(record)
            .map_err(|_| LifecycleError::Invalid("diff path is not UTF-8".into()))?;
        let mut fields = record.splitn(3, '\t');
        let (Some(additions), Some(deletions), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(LifecycleError::Invalid("malformed diff numstat".into()));
        };
        if additions == "-" || deletions == "-" {
            return Err(LifecycleError::Invalid("binary diff is forbidden".into()));
        }
        validate_relative_path(path)
            .map_err(|_| LifecycleError::Invalid(format!("unsafe diff path: {path}")))?;
        if path == ".agentum"
            || path.starts_with(".agentum/")
            || !task
                .write_scopes
                .iter()
                .any(|scope| path == scope || path.starts_with(&format!("{scope}/")))
        {
            return Err(LifecycleError::Invalid(format!(
                "diff path is outside task write scopes: {path}"
            )));
        }
        if !paths.iter().any(|existing| existing == path) {
            paths.push(path.to_owned());
        }
    }
    if paths.is_empty() {
        return Err(LifecycleError::Invalid("provider diff has no files".into()));
    }
    git_apply_output(
        attempt,
        &["--recount", "--check", "--whitespace=nowarn"],
        diff.as_bytes(),
    )
    .await?;
    git_apply_output(
        attempt,
        &["--recount", "--whitespace=nowarn"],
        diff.as_bytes(),
    )
    .await?;
    Ok(paths)
}

async fn git_apply_output(
    cwd: &Path,
    args: &[&str],
    patch: &[u8],
) -> Result<Vec<u8>, LifecycleError> {
    let mut child = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("apply")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| LifecycleError::Git("git apply stdin unavailable".into()))?
        .write_all(patch)
        .await?;
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        return Err(LifecycleError::Git(output.status.to_string()));
    }
    if output.stdout.len() > 4 * 1024 * 1024 {
        return Err(LifecycleError::Invalid(
            "diff metadata exceeded output limit".into(),
        ));
    }
    Ok(output.stdout)
}

fn collect_patch_changes(
    authoritative: &Path,
    attempt: &Path,
    paths: &[String],
) -> Result<Vec<PatchChange>, LifecycleError> {
    let mut changes = Vec::new();
    let mut total = 0usize;
    for relative in paths {
        let authoritative_path = authoritative.join(relative);
        let attempt_path = attempt.join(relative);
        let preimage_hash = content_hash(&authoritative_path)?;
        let preimage = if preimage_hash == MISSING_HASH {
            None
        } else {
            Some(read_bytes(&authoritative_path)?.0)
        };
        let postimage_hash = content_hash(&attempt_path)?;
        let postimage = if postimage_hash == MISSING_HASH {
            None
        } else {
            let bytes = read_bytes(&attempt_path)?.0;
            if bytes.contains(&0) {
                return Err(LifecycleError::Invalid(format!(
                    "binary implementation output is unsupported: {relative}"
                )));
            }
            Some(bytes)
        };
        if preimage_hash == postimage_hash {
            continue;
        }
        total = total
            .saturating_add(preimage.as_ref().map_or(0, Vec::len))
            .saturating_add(postimage.as_ref().map_or(0, Vec::len));
        if total > 32 * 1024 * 1024
            || preimage
                .as_ref()
                .is_some_and(|value| value.len() > 8 * 1024 * 1024)
            || postimage
                .as_ref()
                .is_some_and(|value| value.len() > 8 * 1024 * 1024)
        {
            return Err(LifecycleError::Invalid(
                "patch or preimage exceeds configured limits".into(),
            ));
        }
        changes.push(PatchChange {
            relative_path: relative.clone(),
            preimage,
            preimage_hash,
            postimage,
            postimage_hash,
        });
    }
    Ok(changes)
}

async fn publish_patch(
    state: &AppState,
    run: &SddRunRecord,
    attempt: &PhaseAttempt,
    changes: &[PatchChange],
) -> Result<(), LifecycleError> {
    let patch_id = Uuid::new_v4().to_string();
    let relative_paths: Vec<_> = changes
        .iter()
        .map(|change| change.relative_path.clone())
        .collect();
    let preimage_hashes: Vec<_> = changes
        .iter()
        .map(|change| change.preimage_hash.clone())
        .collect();
    let operations = json!(
        changes
            .iter()
            .map(|change| json!({
                "path": change.relative_path,
                "operation": if change.postimage.is_some() { "write" } else { "delete" },
                "preimageHash": change.preimage_hash,
                "contentHash": change.postimage_hash
            }))
            .collect::<Vec<_>>()
    );
    let preimages = json!(
        changes
            .iter()
            .map(|change| json!({
                "path": change.relative_path,
                "hash": change.preimage_hash,
                "contentBase64": change.preimage.as_ref().map(|bytes| {
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                })
            }))
            .collect::<Vec<_>>()
    );
    let expires_at = (time::OffsetDateTime::now_utc() + time::Duration::minutes(10))
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| LifecycleError::Invalid(error.to_string()))?;
    let reserve_request = format!("internal-patch-reserve-{}", Uuid::new_v4());
    let reserve_payload = json!({
        "runId": run.run_id,
        "revision": run.aggregate_revision + 1,
        "patchId": patch_id,
        "attemptId": attempt.id,
        "paths": relative_paths,
        "status": "pending"
    });
    let reserve_hash = super::sha256(reserve_payload.to_string());
    let reserved_revision = state
        .store
        .sdd_reserve_patch(ReservePatchMutation {
            request_id: &reserve_request,
            request_hash: &reserve_hash,
            run_id: &run.run_id,
            expected_revision: run.aggregate_revision,
            patch_id: &patch_id,
            attempt_id: &attempt.id,
            relative_paths: &relative_paths,
            preimage_hashes: &preimage_hashes,
            operations_json: &operations.to_string(),
            preimages_json: &preimages.to_string(),
            expires_at: &expires_at,
            response_json: &reserve_payload.to_string(),
        })
        .await?;

    let authoritative = Path::new(&run.authoritative_path);
    let mut applied = Vec::new();
    let publication = (|| -> Result<(), LifecycleError> {
        for (index, change) in changes.iter().enumerate() {
            let target = authoritative.join(&change.relative_path);
            match &change.postimage {
                Some(content) => {
                    if let Some(parent) = target.parent() {
                        create_owned_directories(authoritative, parent)?;
                    }
                    atomic_write(&target, content, Some(&change.preimage_hash))?;
                }
                None => atomic_remove(&target, &change.preimage_hash)?,
            }
            applied.push(index);
        }
        Ok(())
    })();
    if let Err(error) = publication {
        let rollback_succeeded = rollback_patch(authoritative, changes, &applied).is_ok();
        let failure_request = format!("internal-patch-fail-{}", Uuid::new_v4());
        let failure_payload = json!({
            "runId": run.run_id,
            "revision": reserved_revision + 1,
            "patchId": patch_id,
            "attemptId": attempt.id,
            "status": if rollback_succeeded { "rolled_back" } else { "quarantined" },
            "error": error.to_string()
        });
        let failure_hash = super::sha256(failure_payload.to_string());
        let _ = state
            .store
            .sdd_fail_patch(FailPatchMutation {
                request_id: &failure_request,
                request_hash: &failure_hash,
                run_id: &run.run_id,
                expected_revision: reserved_revision,
                patch_id: &patch_id,
                attempt_id: &attempt.id,
                error: &error.to_string(),
                rollback_succeeded,
                response_json: &failure_payload.to_string(),
            })
            .await;
        return Err(error);
    }

    let complete_request = format!("internal-patch-complete-{}", Uuid::new_v4());
    let complete_payload = json!({
        "runId": run.run_id,
        "revision": reserved_revision + 1,
        "patchId": patch_id,
        "attemptId": attempt.id,
        "status": "applied"
    });
    let complete_hash = super::sha256(complete_payload.to_string());
    if let Err(error) = state
        .store
        .sdd_complete_patch(CompletePatchMutation {
            request_id: &complete_request,
            request_hash: &complete_hash,
            run_id: &run.run_id,
            expected_revision: reserved_revision,
            patch_id: &patch_id,
            attempt_id: &attempt.id,
            response_json: &complete_payload.to_string(),
        })
        .await
    {
        let rollback_succeeded = rollback_patch(authoritative, changes, &applied).is_ok();
        let failure_request = format!("internal-patch-db-fail-{}", Uuid::new_v4());
        let failure_payload = json!({
            "runId": run.run_id,
            "revision": reserved_revision + 1,
            "patchId": patch_id,
            "attemptId": attempt.id,
            "status": if rollback_succeeded { "rolled_back" } else { "quarantined" },
            "error": error.to_string()
        });
        let failure_hash = super::sha256(failure_payload.to_string());
        let _ = state
            .store
            .sdd_fail_patch(FailPatchMutation {
                request_id: &failure_request,
                request_hash: &failure_hash,
                run_id: &run.run_id,
                expected_revision: reserved_revision,
                patch_id: &patch_id,
                attempt_id: &attempt.id,
                error: &error.to_string(),
                rollback_succeeded,
                response_json: &failure_payload.to_string(),
            })
            .await;
        return Err(error.into());
    }
    Ok(())
}

fn rollback_patch(
    authoritative: &Path,
    changes: &[PatchChange],
    applied: &[usize],
) -> Result<(), LifecycleError> {
    for index in applied.iter().rev() {
        let change = &changes[*index];
        let target = authoritative.join(&change.relative_path);
        match &change.preimage {
            Some(content) => {
                atomic_write(&target, content, Some(&change.postimage_hash))?;
            }
            None => atomic_remove(&target, &change.postimage_hash)?,
        }
    }
    Ok(())
}

async fn execute_verification(state: &AppState, run: &SddRunRecord) -> Result<(), LifecycleError> {
    let spec = state
        .store
        .sdd_get_spec(&run.spec_id)
        .await?
        .ok_or_else(|| agentum_store::StoreError::NotFound(run.spec_id.clone()))?;
    let plan_path = Path::new(&run.authoritative_path)
        .join(".agentum/specs")
        .join(&spec.slug)
        .join("plan.json");
    let (plan_content, _) = read_text(&plan_path)?;
    let canonical: SpecId =
        spec.spec_id
            .parse()
            .map_err(|error: agentum_core::sdd::SddContractError| {
                LifecycleError::Invalid(error.to_string())
            })?;
    validate_plan(&plan_content, &canonical, spec.current_revision)?;
    let plan: PlanArtifact = serde_json::from_str(&plan_content)?;
    if spec.profile == "high_risk"
        && plan
            .tasks
            .iter()
            .any(|task| task.verification.is_empty() && task.browser_checks.is_empty())
    {
        return Err(LifecycleError::Invalid(
            "high-risk plans cannot waive task verification".into(),
        ));
    }

    let mut attempt = reserve_attempt(state, run, None, "verification").await?;
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

    let mut results = Vec::with_capacity(commands.len());
    for (index, command) in commands.iter().enumerate() {
        let execution_id = format!("{}:{}:verification:{}", run.run_id, attempt.id, index);
        let result = match run_verification_command(
            &execution_id,
            &attempt.workspace.path,
            command,
            index as i64,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => {
                reject_attempt(state, run, &attempt, "failed", &error.to_string()).await?;
                return Ok(());
            }
        };
        let terminal = result.status != "succeeded";
        let canceled = result.status == "canceled";
        results.push(result);
        if terminal {
            if canceled {
                cleanup_attempt(state, run, &attempt.workspace.path).await?;
                return Ok(());
            }
            break;
        }
    }

    let browser_checks = plan
        .tasks
        .iter()
        .flat_map(|task| task.browser_checks.iter().cloned())
        .collect::<Vec<_>>();
    if results.iter().all(|result| result.status == "succeeded") && !browser_checks.is_empty() {
        match execute_browser_checks(
            state,
            run,
            &mut attempt,
            &browser_checks,
            commands.len() as i64,
        )
        .await
        {
            Ok((browser_results, revision)) => {
                results.extend(browser_results);
                attempt.revision = revision;
            }
            Err(error) => {
                let current = state.store.sdd_get_run(&run.run_id).await?;
                if current.as_ref().is_some_and(|current| {
                    current.aggregate_revision == attempt.revision
                        && current.phase == "verification"
                        && current.status == "running"
                }) {
                    reject_attempt(state, run, &attempt, "failed", &error.to_string()).await?;
                } else {
                    cleanup_attempt(state, run, &attempt.workspace.path).await?;
                }
                return Ok(());
            }
        }
    }

    let current_run = state
        .store
        .sdd_get_run(&run.run_id)
        .await?
        .ok_or_else(|| agentum_store::StoreError::NotFound(run.run_id.clone()))?;
    if current_run.aggregate_revision != attempt.revision
        || current_run.phase != "verification"
        || current_run.status != "running"
    {
        cleanup_attempt(state, run, &attempt.workspace.path).await?;
        return Ok(());
    }
    let success_status = if spec.control == "interactive" {
        "paused"
    } else {
        "queued"
    };
    let payload = json!({
        "runId": run.run_id,
        "revision": attempt.revision + 1,
        "attemptId": attempt.id,
        "phase": if results.iter().all(|result| result.status == "succeeded") {
            "review"
        } else {
            "verification"
        },
        "status": if results.iter().all(|result| result.status == "succeeded") {
            success_status
        } else {
            "failed"
        },
        "results": results
    });
    let request_id = format!("internal-verification-{}", Uuid::new_v4());
    let request_hash = super::sha256(payload.to_string());
    match state
        .store
        .sdd_record_verification(RecordVerificationMutation {
            request_id: &request_id,
            request_hash: &request_hash,
            run_id: &run.run_id,
            expected_revision: attempt.revision,
            attempt_id: &attempt.id,
            results: &results,
            success_status,
            response_json: &payload.to_string(),
        })
        .await
    {
        Ok(_) | Err(agentum_store::StoreError::StaleRevision { .. }) => {}
        Err(error) => {
            reject_attempt(state, run, &attempt, "failed", &error.to_string()).await?;
            return Ok(());
        }
    }
    cleanup_attempt(state, run, &attempt.workspace.path).await?;
    Ok(())
}

struct PreparedBrowserEvidence {
    check_id: String,
    manifest_sha256: String,
    manifest_json: String,
    status: String,
    captured_at: String,
    evidence_id: String,
    blob_refs: Vec<(String, String)>,
    result: VerificationResultInput,
}

async fn execute_browser_checks(
    state: &AppState,
    run: &SddRunRecord,
    attempt: &mut PhaseAttempt,
    checks: &[BrowserCheck],
    command_index_offset: i64,
) -> Result<(Vec<VerificationResultInput>, i64), LifecycleError> {
    let check_ids = checks
        .iter()
        .map(|check| check.id.clone())
        .collect::<Vec<_>>();
    let grant_id = Uuid::new_v4().to_string();
    let grant_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let token_hash = super::sha256(grant_token.as_bytes());
    let spec = state
        .store
        .sdd_get_spec(&run.spec_id)
        .await?
        .ok_or_else(|| agentum_store::StoreError::NotFound(run.spec_id.clone()))?;
    let scope = json!({
        "schemaVersion": 1,
        "runId": run.run_id,
        "attemptId": attempt.id,
        "specRevision": spec.current_revision,
        "workspaceFingerprint": run.workspace_fingerprint,
        "checkIds": check_ids,
        "maxTotalBytes": 16 * 1024 * 1024
    });
    let expires_at = (time::OffsetDateTime::now_utc() + time::Duration::minutes(15))
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| LifecycleError::Invalid(error.to_string()))?;
    let grant_request = format!("internal-browser-grant-{}", Uuid::new_v4());
    let grant_payload = json!({
        "runId": run.run_id,
        "revision": attempt.revision + 1,
        "attemptId": attempt.id,
        "grantId": grant_id,
        "checkIds": check_ids,
        "expiresAt": expires_at
    });
    let grant_revision = state
        .store
        .sdd_issue_browser_evidence_grant(IssueBrowserEvidenceGrantMutation {
            request_id: &grant_request,
            request_hash: &super::sha256(grant_payload.to_string()),
            run_id: &run.run_id,
            expected_revision: attempt.revision,
            attempt_id: &attempt.id,
            grant_id: &grant_id,
            token_hash: &token_hash,
            scope_json: &scope.to_string(),
            expires_at: &expires_at,
            response_json: &grant_payload.to_string(),
        })
        .await?;
    attempt.revision = grant_revision;

    let context_key = format!("sdd-evidence-{}", attempt.id);
    let execution_id = format!("{}:{}:browser", run.run_id, attempt.id);
    let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    active_commands()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(execution_id.clone(), cancel_tx);
    let _guard = CommandGuard(execution_id);
    let (_, port) = cancellable_browser_call(
        &mut cancel_rx,
        crate::cdp_browser::ensure_local_cdp_browser_for(&context_key),
    )
    .await?;
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

    let mut prepared = Vec::with_capacity(checks.len());
    let mut blobs_by_hash: HashMap<String, StoredEvidenceBlob> = HashMap::new();
    let execution = async {
        for (index, check) in checks.iter().enumerate() {
            let context = cancellable_browser_call(
                &mut cancel_rx,
                crate::cdp_driver::run_browser_op("new_context", &json!({ "cdpPort": port })),
            )
            .await?;
            let target = context
                .get("target")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    LifecycleError::Invalid("browser context returned no target".into())
                })?
                .to_owned();
            let browser_context_id = context
                .get("browser_context_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    LifecycleError::Invalid("browser context returned no identity".into())
                })?
                .to_owned();
            let approved_origin = reqwest::Url::parse(&check.url)
                .map_err(|error| LifecycleError::Invalid(error.to_string()))?
                .origin()
                .ascii_serialization();
            let origin_guard = match cancellable_browser_call(
                &mut cancel_rx,
                crate::cdp_driver::start_sdd_origin_guard(port, &target, &approved_origin),
            )
            .await
            {
                Ok(guard) => guard,
                Err(error) => {
                    let _ = crate::cdp_driver::run_browser_op(
                        "close_context",
                        &json!({ "cdpPort": port, "browser_context_id": browser_context_id }),
                    )
                    .await;
                    return Err(error);
                }
            };
            let item = tokio::time::timeout(
                std::time::Duration::from_millis(check.timeout_ms),
                execute_one_browser_check(
                    &mut cancel_rx,
                    run,
                    attempt,
                    check,
                    command_index_offset + index as i64,
                    spec.current_revision,
                    port,
                    &target,
                    &browser_name,
                    &browser_version,
                    &mut blobs_by_hash,
                ),
            )
            .await
            .map_err(|_| {
                LifecycleError::Invalid(format!(
                    "browser check {} exceeded its total {}ms deadline",
                    check.id, check.timeout_ms
                ))
            });
            let guard_result = origin_guard
                .stop()
                .await
                .map_err(|error| LifecycleError::Invalid(format!("browser origin guard: {error}")));
            let _ = crate::cdp_driver::run_browser_op(
                "close_context",
                &json!({ "cdpPort": port, "browser_context_id": browser_context_id }),
            )
            .await;
            let item = item??;
            guard_result?;
            prepared.push(item);
        }
        Ok::<(), LifecycleError>(())
    }
    .await;
    let _ = crate::cdp_browser::stop_local_cdp_browser_for(&context_key).await;
    execution?;

    let new_blobs = blobs_by_hash
        .values()
        .map(|blob| NewEvidenceBlob {
            sha256: &blob.sha256,
            byte_length: blob.byte_length,
            media_type: &blob.media_type,
            storage_relative_path: &blob.storage_relative_path,
        })
        .collect::<Vec<_>>();
    let evidence_rows = prepared
        .iter()
        .map(|item| NewBrowserEvidence {
            evidence_id: &item.evidence_id,
            check_id: &item.check_id,
            manifest_sha256: &item.manifest_sha256,
            manifest_json: &item.manifest_json,
            status: &item.status,
            captured_at: &item.captured_at,
        })
        .collect::<Vec<_>>();
    let refs = prepared
        .iter()
        .flat_map(|item| {
            item.blob_refs
                .iter()
                .map(move |(hash, role)| NewBrowserEvidenceBlobRef {
                    evidence_id: &item.evidence_id,
                    sha256: hash,
                    role,
                })
        })
        .collect::<Vec<_>>();
    let submit_request = format!("internal-browser-submit-{}", Uuid::new_v4());
    let submit_payload = json!({
        "runId": run.run_id,
        "revision": grant_revision + 1,
        "attemptId": attempt.id,
        "evidence": prepared.iter().map(|item| json!({
            "evidenceId": item.evidence_id,
            "checkId": item.check_id,
            "manifestSha256": item.manifest_sha256,
            "status": item.status
        })).collect::<Vec<_>>()
    });
    let submitted_by = format!("agentum:browser-driver:{}", attempt.id);
    let submitted_revision = state
        .store
        .sdd_submit_browser_evidence(SubmitBrowserEvidenceMutation {
            request_id: &submit_request,
            request_hash: &super::sha256(submit_payload.to_string()),
            run_id: &run.run_id,
            expected_revision: grant_revision,
            attempt_id: &attempt.id,
            grant_token_hash: &token_hash,
            submitted_by: &submitted_by,
            evidence: &evidence_rows,
            blobs: &new_blobs,
            blob_refs: &refs,
            response_json: &submit_payload.to_string(),
        })
        .await?;
    Ok((
        prepared.into_iter().map(|item| item.result).collect(),
        submitted_revision,
    ))
}

async fn cancellable_browser_call<T, F>(
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    future: F,
) -> Result<T, LifecycleError>
where
    F: Future<Output = anyhow::Result<T>>,
{
    tokio::select! {
        result = future => result.map_err(|error| LifecycleError::Invalid(format!("browser check failed: {error}"))),
        changed = cancel.changed() => {
            let _ = changed;
            Err(LifecycleError::Invalid("browser verification canceled".into()))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_one_browser_check(
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    run: &SddRunRecord,
    attempt: &PhaseAttempt,
    check: &BrowserCheck,
    command_index: i64,
    spec_revision: i64,
    port: u16,
    target: &str,
    browser_name: &str,
    browser_version: &str,
    blobs: &mut HashMap<String, StoredEvidenceBlob>,
) -> Result<PreparedBrowserEvidence, LifecycleError> {
    let started = std::time::Instant::now();
    let wait_until = match check.wait_until {
        BrowserWaitUntil::Load | BrowserWaitUntil::NetworkIdle => "load",
        BrowserWaitUntil::DomContentLoaded => "domcontentloaded",
    };
    let navigate = cancellable_browser_call(
        cancel,
        crate::cdp_driver::run_browser_op(
            "navigate",
            &json!({
                "cdpPort": port,
                "target": target,
                "url": check.url,
                "wait_until": wait_until
            }),
        ),
    )
    .await?;
    let navigation_ok = navigate.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
    let final_origin_allowed = navigate
        .get("final_url")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| reqwest::Url::parse(value).ok())
        .is_some_and(|value| ensure_sdd_browser_origin_allowed(&value).is_ok());
    if navigation_ok && !final_origin_allowed {
        return Err(LifecycleError::Invalid(
            "browser navigation redirected outside the approved SDD origin policy".into(),
        ));
    }
    let network_idle = if check.wait_until == BrowserWaitUntil::NetworkIdle {
        cancellable_browser_call(
            cancel,
            crate::cdp_driver::sdd_wait_network_idle(port, target, check.timeout_ms),
        )
        .await?
    } else {
        true
    };
    let status =
        cancellable_browser_call(cancel, crate::cdp_driver::sdd_page_status(port, target)).await?;
    let mut assertions = Vec::with_capacity(check.assertions.len());
    for assertion in &check.assertions {
        let passed = match assertion {
            BrowserCheckAssertion::PageLoaded {
                expected_status, ..
            } => navigation_ok && network_idle && status == Some(*expected_status),
            BrowserCheckAssertion::TextPresent { text, .. } => {
                browser_wait_assertion(cancel, port, target, "text", text, check.timeout_ms).await?
            }
            BrowserCheckAssertion::SelectorVisible { selector, .. } => {
                browser_wait_assertion(cancel, port, target, "selector", selector, check.timeout_ms)
                    .await?
            }
            BrowserCheckAssertion::UrlContains { value, .. } => {
                browser_wait_assertion(cancel, port, target, "url", value, check.timeout_ms).await?
            }
        };
        assertions.push((assertion.id().to_owned(), passed));
    }
    let screenshot = cancellable_browser_call(
        cancel,
        crate::cdp_driver::run_browser_op(
            "screenshot",
            &json!({
                "cdpPort": port,
                "target": target,
                "width": check.viewport.width,
                "height": check.viewport.height,
                "deviceScaleFactor": check.viewport.device_scale_milli as f64 / 1000.0
            }),
        ),
    )
    .await?;
    let screenshot_bytes = screenshot
        .get("image_b64")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| LifecycleError::Invalid("browser screenshot returned no bytes".into()))
        .and_then(|encoded| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|error| LifecycleError::Invalid(error.to_string()))
        })?;
    let screenshot_blob = persist_blob(&screenshot_bytes, "image/png")
        .map_err(|error| LifecycleError::Invalid(error.to_string()))?;
    let console_marker = br#"{"coverage":"none","reason":"ambient_diagnostics_excluded"}"#;
    let network_marker = serde_json::to_vec(&json!({
        "coverage": "main_document",
        "navigationOk": navigation_ok,
        "networkIdle": network_idle,
        "status": status
    }))?;
    let console_blob = persist_blob(console_marker, "application/json")
        .map_err(|error| LifecycleError::Invalid(error.to_string()))?;
    let network_blob = persist_blob(&network_marker, "application/json")
        .map_err(|error| LifecycleError::Invalid(error.to_string()))?;
    for blob in [&screenshot_blob, &console_blob, &network_blob] {
        blobs
            .entry(blob.sha256.clone())
            .or_insert_with(|| blob.clone());
    }
    let captured_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| LifecycleError::Invalid(error.to_string()))?;
    let url = reqwest::Url::parse(&check.url)
        .map_err(|error| LifecycleError::Invalid(error.to_string()))?;
    let evidence_id = Uuid::new_v4().to_string();
    let evidence = BrowserEvidence {
        schema_version: BROWSER_EVIDENCE_SCHEMA_VERSION,
        evidence_id: evidence_id.clone(),
        run_id: run.run_id.clone(),
        attempt_id: attempt.id.clone(),
        check_id: check.id.clone(),
        spec_revision,
        captured_at: captured_at.clone(),
        workspace_fingerprint: run.workspace_fingerprint.clone(),
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
        captures: vec![BrowserCaptureRef {
            kind: BrowserCaptureKind::Screenshot,
            sha256: screenshot_blob.sha256.clone(),
            byte_length: screenshot_blob.byte_length as u64,
            media_type: screenshot_blob.media_type.clone(),
        }],
        assertions: assertions
            .iter()
            .map(|(id, passed)| BrowserAssertion {
                id: id.clone(),
                status: if *passed {
                    BrowserAssertionStatus::Passed
                } else {
                    BrowserAssertionStatus::Failed
                },
                acceptance_criteria: check.acceptance_criteria.clone(),
                evidence_sha256: vec![screenshot_blob.sha256.clone()],
            })
            .collect(),
        console: BrowserConsoleSummary {
            coverage: BrowserDiagnosticCoverage::None,
            errors: 0,
            warnings: 0,
            transcript_sha256: console_blob.sha256.clone(),
        },
        network: BrowserNetworkSummary {
            coverage: BrowserDiagnosticCoverage::MainDocument,
            requests: 1,
            failed_requests: u32::from(
                !navigation_ok || !network_idle || status.is_some_and(|value| value >= 400),
            ),
            transcript_sha256: network_blob.sha256.clone(),
        },
    };
    evidence
        .validate()
        .map_err(|error| LifecycleError::Invalid(error.to_string()))?;
    let manifest_sha256 = evidence
        .digest()
        .map_err(|error| LifecycleError::Invalid(error.to_string()))?;
    let manifest_json = serde_json::to_string(&evidence)?;
    let prerequisites_passed = navigation_ok && final_origin_allowed && network_idle;
    let passed = prerequisites_passed && assertions.iter().all(|(_, passed)| *passed);
    Ok(PreparedBrowserEvidence {
        check_id: check.id.clone(),
        manifest_sha256: manifest_sha256.clone(),
        manifest_json,
        status: if passed { "passed" } else { "failed" }.into(),
        captured_at,
        evidence_id,
        blob_refs: vec![
            (screenshot_blob.sha256.clone(), "capture".into()),
            (console_blob.sha256, "console_transcript".into()),
            (network_blob.sha256, "network_transcript".into()),
        ],
        result: VerificationResultInput {
            command_index,
            command_json: json!({ "type": "browserCheck", "check": check }).to_string(),
            status: if passed { "succeeded" } else { "failed" }.into(),
            exit_code: None,
            output_hash: manifest_sha256,
            output_excerpt: format!(
                "{}: navigation={} networkIdle={} {}/{} assertions passed; screenshot sha256:{}",
                check.id,
                navigation_ok,
                network_idle,
                assertions.iter().filter(|(_, passed)| *passed).count(),
                assertions.len(),
                screenshot_blob.sha256
            ),
            duration_ms: started.elapsed().as_millis().min(i64::MAX as u128) as i64,
        },
    })
}

async fn browser_wait_assertion(
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    port: u16,
    target: &str,
    condition: &str,
    arg: &str,
    timeout_ms: u64,
) -> Result<bool, LifecycleError> {
    let value = cancellable_browser_call(
        cancel,
        crate::cdp_driver::run_browser_op(
            "wait",
            &json!({
                "cdpPort": port,
                "target": target,
                "condition": condition,
                "arg": arg,
                "timeout_ms": timeout_ms
            }),
        ),
    )
    .await?;
    Ok(
        value.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
            && value.get("timed_out").and_then(serde_json::Value::as_bool) == Some(false),
    )
}

pub(crate) async fn run_verification_command(
    execution_id: &str,
    attempt: &Path,
    command: &CommandSpec,
    command_index: i64,
) -> Result<VerificationResultInput, LifecycleError> {
    if !verification_command_is_safe(command) {
        return Err(LifecycleError::Invalid(
            "unsafe verification command reached execution".into(),
        ));
    }
    let isolated = isolate_verification_command(attempt, command)?;
    let command_json = serde_json::to_string(command)?;
    let started = std::time::Instant::now();
    let mut process = tokio::process::Command::new(&isolated.program);
    process
        .args(&isolated.args)
        .current_dir(&isolated.cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    process.process_group(0);
    let mut child = process.spawn()?;
    let pid = child.id();
    let stdout = child.stdout.take().expect("verification stdout is piped");
    let stderr = child.stderr.take().expect("verification stderr is piped");
    let (limit_tx, mut limit_rx) = tokio::sync::mpsc::channel(1);
    let stdout_task = tokio::spawn(read_verification_output(
        stdout,
        command.output_limit,
        limit_tx.clone(),
    ));
    let stderr_limit = command.output_limit.min(512 * 1024);
    let stderr_task = tokio::spawn(read_verification_output(stderr, stderr_limit, limit_tx));
    let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    active_commands()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(execution_id.to_owned(), cancel_tx);
    let _guard = CommandGuard(execution_id.to_owned());
    let timeout = tokio::time::sleep(std::time::Duration::from_millis(command.timeout_ms));
    tokio::pin!(timeout);

    enum Termination {
        Completed(std::process::ExitStatus),
        TimedOut,
        Canceled,
        OutputLimit,
    }
    let termination = tokio::select! {
        status = child.wait() => Termination::Completed(status?),
        _ = &mut timeout => {
            super::providers::terminate_process_tree(&mut child, pid).await;
            Termination::TimedOut
        }
        changed = cancel_rx.changed() => {
            let _ = changed;
            super::providers::terminate_process_tree(&mut child, pid).await;
            Termination::Canceled
        }
        exceeded = wait_for_verification_limit(&mut limit_rx) => {
            let _ = exceeded;
            super::providers::terminate_process_tree(&mut child, pid).await;
            Termination::OutputLimit
        }
    };
    let stdout = stdout_task
        .await
        .map_err(|error| LifecycleError::Invalid(error.to_string()))??;
    let stderr = stderr_task
        .await
        .map_err(|error| LifecycleError::Invalid(error.to_string()))??;
    let mut captured = stdout;
    captured.push(0xff);
    captured.extend_from_slice(&stderr);
    let (status, exit_code) = match termination {
        Termination::Completed(status) if status.success() => ("succeeded", status.code()),
        Termination::Completed(status) => ("failed", status.code()),
        Termination::TimedOut => ("timed_out", None),
        Termination::Canceled => ("canceled", None),
        Termination::OutputLimit => ("failed", None),
    };
    Ok(VerificationResultInput {
        command_index,
        command_json,
        status: status.into(),
        exit_code: exit_code.map(i64::from),
        output_hash: super::sha256(&captured),
        output_excerpt: format!(
            "[redacted] {} bytes captured; status={status}",
            captured.len().saturating_sub(1)
        ),
        duration_ms: started.elapsed().as_millis().min(i64::MAX as u128) as i64,
    })
}

async fn wait_for_verification_limit(receiver: &mut tokio::sync::mpsc::Receiver<usize>) -> usize {
    match receiver.recv().await {
        Some(limit) => limit,
        None => std::future::pending().await,
    }
}

async fn read_verification_output(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
    limit_tx: tokio::sync::mpsc::Sender<usize>,
) -> Result<Vec<u8>, std::io::Error> {
    let mut output = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut output)
        .await?;
    if output.len() > limit {
        output.truncate(limit);
        let _ = limit_tx.send(limit).await;
    }
    Ok(output)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn linked_worktree_git_metadata(
    attempt: &Path,
) -> Result<Option<(PathBuf, PathBuf)>, LifecycleError> {
    let marker = attempt.join(".git");
    let marker_metadata = match std::fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if marker_metadata.file_type().is_symlink() {
        return Err(LifecycleError::Invalid(
            "verification worktree .git marker must not be a symlink".into(),
        ));
    }
    if marker_metadata.is_dir() {
        // A self-contained repository already travels with the writable
        // attempt bind and needs no external metadata exposure.
        return Ok(None);
    }
    if !marker_metadata.is_file() || marker_metadata.len() > 8 * 1024 {
        return Err(LifecycleError::Invalid(
            "verification worktree .git marker is invalid".into(),
        ));
    }
    let marker_content = read_text(&marker)?.0;
    let declared = marker_content
        .strip_prefix("gitdir: ")
        .map(|value| value.trim_end_matches(['\n', '\r']))
        .filter(|value| !value.is_empty() && !value.contains(['\0', '\n', '\r']))
        .map(PathBuf::from)
        .ok_or_else(|| {
            LifecycleError::Invalid("verification worktree .git marker is malformed".into())
        })?;
    let declared = if declared.is_absolute() {
        declared
    } else {
        attempt.join(declared)
    };
    let declared = declared.canonicalize()?;

    let resolve = |flag: &str| -> Result<PathBuf, LifecycleError> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(attempt)
            .args(["rev-parse", "--path-format=absolute", flag])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() || output.stdout.len() > 8 * 1024 {
            return Err(LifecycleError::Invalid(
                "linked worktree Git metadata could not be resolved".into(),
            ));
        }
        let path = std::str::from_utf8(&output.stdout)
            .map_err(|_| LifecycleError::Invalid("Git metadata path is not UTF-8".into()))?
            .trim_end_matches(['\n', '\r']);
        if path.is_empty() || path.contains(['\0', '\n', '\r']) {
            return Err(LifecycleError::Invalid(
                "Git metadata path is malformed".into(),
            ));
        }
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(LifecycleError::Invalid(
                "Git metadata path is not absolute".into(),
            ));
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(LifecycleError::Invalid(
                "Git metadata path is not a regular directory".into(),
            ));
        }
        path.canonicalize().map_err(Into::into)
    };

    let git_dir = resolve("--git-dir")?;
    let common_dir = resolve("--git-common-dir")?;
    if git_dir != declared || !git_dir.starts_with(common_dir.join("worktrees")) {
        return Err(LifecycleError::Invalid(
            "linked worktree Git metadata escaped its common directory".into(),
        ));
    }
    Ok(Some((git_dir, common_dir)))
}

#[cfg(target_os = "linux")]
fn isolate_verification_command(
    attempt: &Path,
    command: &CommandSpec,
) -> Result<CommandSpec, LifecycleError> {
    let bwrap = which::which("bwrap").map_err(|_| {
        LifecycleError::Invalid("bubblewrap is required for isolated verification on Linux".into())
    })?;
    let executable = which::which(&command.program).map_err(|_| {
        LifecycleError::Invalid(format!(
            "verification executable is not installed: {}",
            command.program
        ))
    })?;
    let attempt = attempt.canonicalize()?;
    let git_metadata = linked_worktree_git_metadata(&attempt)?;
    let requested_cwd = if command.cwd == "." {
        attempt.clone()
    } else {
        attempt.join(&command.cwd)
    };
    let requested_cwd = requested_cwd.canonicalize()?;
    if !requested_cwd.starts_with(&attempt) || !requested_cwd.is_dir() {
        return Err(LifecycleError::Invalid(
            "verification cwd escaped the disposable worktree".into(),
        ));
    }
    let relative_cwd = requested_cwd
        .strip_prefix(&attempt)
        .map_err(|_| LifecycleError::Invalid("verification cwd escaped the attempt".into()))?;
    let sandbox_cwd = attempt.join(relative_cwd);
    let account_root = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            LifecycleError::Invalid("an absolute account directory is required".into())
        })?;
    let mut args = vec![
        "--die-with-parent".into(),
        "--new-session".into(),
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
        "--unshare-net".into(),
        "--ro-bind".into(),
        "/".into(),
        "/".into(),
        "--tmpfs".into(),
        account_root.to_string_lossy().into_owned(),
        "--tmpfs".into(),
        "/tmp".into(),
        "--tmpfs".into(),
        "/var/tmp".into(),
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
    ];
    add_verification_runtime_mounts(&mut args, &account_root);
    add_sandbox_parents(&mut args, &attempt);
    if let Some((_, common_dir)) = &git_metadata {
        add_sandbox_parents(&mut args, common_dir);
        args.extend([
            "--ro-bind".into(),
            common_dir.to_string_lossy().into_owned(),
            common_dir.to_string_lossy().into_owned(),
        ]);
    }
    args.extend([
        "--bind".into(),
        attempt.to_string_lossy().into_owned(),
        attempt.to_string_lossy().into_owned(),
        "--setenv".into(),
        "HOME".into(),
        account_root.to_string_lossy().into_owned(),
        "--setenv".into(),
        "XDG_CONFIG_HOME".into(),
        account_root.join(".config").to_string_lossy().into_owned(),
        "--setenv".into(),
        "XDG_CACHE_HOME".into(),
        account_root
            .join(".cache/agentum-verification")
            .to_string_lossy()
            .into_owned(),
        "--setenv".into(),
        "TMPDIR".into(),
        "/tmp".into(),
        "--setenv".into(),
        "PATH".into(),
        format!(
            "{}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            account_root.join(".cargo/bin").to_string_lossy()
        ),
    ]);
    for key in &command.env_allowlist {
        if key == "PATH" {
            continue;
        }
        if let Some(value) = std::env::var_os(key) {
            args.extend([
                "--setenv".into(),
                key.clone(),
                value.to_string_lossy().into_owned(),
            ]);
        }
    }
    args.extend([
        "--chdir".into(),
        sandbox_cwd.to_string_lossy().into_owned(),
        "--".into(),
        executable.to_string_lossy().into_owned(),
    ]);
    args.extend(command.args.clone());
    Ok(CommandSpec {
        program: bwrap.to_string_lossy().into_owned(),
        args,
        cwd: attempt.to_string_lossy().into_owned(),
        env_allowlist: Vec::new(),
        timeout_ms: command.timeout_ms,
        output_limit: command.output_limit,
    })
}

#[cfg(target_os = "linux")]
fn add_verification_runtime_mounts(args: &mut Vec<String>, account_root: &Path) {
    let mounts = [
        (account_root.join(".rustup"), account_root.join(".rustup")),
        (
            account_root.join(".cargo/bin"),
            account_root.join(".cargo/bin"),
        ),
        (
            account_root.join(".cargo/registry"),
            account_root.join(".cargo/registry"),
        ),
        (
            account_root.join(".cargo/git"),
            account_root.join(".cargo/git"),
        ),
    ];
    for (source, target) in mounts {
        if !source.exists() {
            continue;
        }
        add_sandbox_parents(args, &target);
        args.push("--ro-bind".into());
        args.push(source.to_string_lossy().into_owned());
        args.push(target.to_string_lossy().into_owned());
    }
    args.extend([
        "--setenv".into(),
        "RUSTUP_HOME".into(),
        account_root.join(".rustup").to_string_lossy().into_owned(),
        "--setenv".into(),
        "CARGO_HOME".into(),
        account_root.join(".cargo").to_string_lossy().into_owned(),
    ]);
}

#[cfg(target_os = "linux")]
fn add_sandbox_parents(args: &mut Vec<String>, target: &Path) {
    let Some(parent) = target.parent() else {
        return;
    };
    let mut current = PathBuf::from("/");
    for component in parent.components() {
        if let std::path::Component::Normal(part) = component {
            current.push(part);
            args.push("--dir".into());
            args.push(current.to_string_lossy().into_owned());
        }
    }
}

#[cfg(target_os = "macos")]
fn isolate_verification_command(
    attempt: &Path,
    command: &CommandSpec,
) -> Result<CommandSpec, LifecycleError> {
    let sandbox_exec = Path::new("/usr/bin/sandbox-exec");
    if !sandbox_exec.is_file() {
        return Err(LifecycleError::Invalid(
            "sandbox-exec is required for isolated verification on macOS".into(),
        ));
    }
    let executable = which::which(&command.program).map_err(|_| {
        LifecycleError::Invalid(format!(
            "verification executable is not installed: {}",
            command.program
        ))
    })?;
    let attempt = attempt.canonicalize()?;
    let git_metadata = linked_worktree_git_metadata(&attempt)?;
    let requested_cwd = if command.cwd == "." {
        attempt.clone()
    } else {
        attempt.join(&command.cwd)
    };
    let requested_cwd = requested_cwd.canonicalize()?;
    if !requested_cwd.starts_with(&attempt) || !requested_cwd.is_dir() {
        return Err(LifecycleError::Invalid(
            "verification cwd escaped the disposable worktree".into(),
        ));
    }
    let runtime = attempt.join(".agentum-verification-runtime");
    std::fs::create_dir_all(&runtime)?;
    let account_root = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            LifecycleError::Invalid("an absolute account directory is required".into())
        })?;
    let mut readable = vec![
        PathBuf::from("/System"),
        PathBuf::from("/usr"),
        PathBuf::from("/bin"),
        PathBuf::from("/sbin"),
        PathBuf::from("/Library"),
        PathBuf::from("/opt/homebrew"),
        PathBuf::from("/usr/local"),
        attempt.clone(),
        executable.clone(),
        account_root.join(".rustup"),
        account_root.join(".cargo"),
    ];
    if let Some((git_dir, common_dir)) = git_metadata {
        readable.push(git_dir);
        readable.push(common_dir);
    }
    let mut profile = String::from(
        "(version 1)\n(deny default)\n(allow process-exec)\n(allow process-fork)\n(allow signal (target same-sandbox))\n(allow sysctl-read)\n(allow mach-lookup (global-name \"com.apple.system.opendirectoryd.libinfo\") (global-name \"com.apple.cfprefsd.agent\"))\n(allow file-read-metadata)\n",
    );
    for path in readable.into_iter().filter(|path| path.exists()) {
        profile.push_str(&format!(
            "(allow file-read-data (subpath {}))\n",
            verification_seatbelt_literal(&path)?
        ));
    }
    profile.push_str(&format!(
        "(allow file-write* (subpath {}))\n",
        verification_seatbelt_literal(&attempt)?
    ));
    profile.push_str("(allow file-write-data (literal \"/dev/null\"))\n");

    let mut args = vec![
        "-p".into(),
        profile,
        "--".into(),
        "/usr/bin/env".into(),
        format!("HOME={}", runtime.display()),
        format!("TMPDIR={}", runtime.display()),
        format!("XDG_CONFIG_HOME={}", runtime.join("config").display()),
        format!("XDG_CACHE_HOME={}", runtime.join("cache").display()),
        executable.to_string_lossy().into_owned(),
    ];
    args.extend(command.args.clone());
    Ok(CommandSpec {
        program: sandbox_exec.to_string_lossy().into_owned(),
        args,
        cwd: requested_cwd.to_string_lossy().into_owned(),
        env_allowlist: command.env_allowlist.clone(),
        timeout_ms: command.timeout_ms,
        output_limit: command.output_limit,
    })
}

#[cfg(target_os = "macos")]
fn verification_seatbelt_literal(path: &Path) -> Result<String, LifecycleError> {
    let value = path.to_string_lossy();
    if value.contains(['\0', '\n', '\r']) {
        return Err(LifecycleError::Invalid(
            "verification sandbox path contains control characters".into(),
        ));
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

#[cfg(target_os = "windows")]
fn isolate_verification_command(
    _attempt: &Path,
    _command: &CommandSpec,
) -> Result<CommandSpec, LifecycleError> {
    Err(LifecycleError::Invalid(WINDOWS_LOCAL_SDD_REASON.into()))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn isolate_verification_command(
    _attempt: &Path,
    _command: &CommandSpec,
) -> Result<CommandSpec, LifecycleError> {
    Err(LifecycleError::Invalid(
        "Agentum does not provide a verification filesystem sandbox for this platform.".into(),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_local_sdd_boundary_rejects_verification_before_runtime_files() {
        let directory = tempfile::tempdir().unwrap();
        let command = CommandSpec {
            program: "cargo.exe".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            env_allowlist: vec!["PATH".into()],
            timeout_ms: 10_000,
            output_limit: 32 * 1024,
        };
        let result = isolate_verification_command(directory.path(), &command);
        assert!(matches!(
            result,
            Err(LifecycleError::Invalid(ref reason)) if reason == WINDOWS_LOCAL_SDD_REASON
        ));
        assert!(
            !directory
                .path()
                .join(".agentum-verification-runtime")
                .exists()
        );
    }

    fn scheduler_task(
        id: &str,
        read_scopes: &[&str],
        write_scopes: &[&str],
        parallel_safe: bool,
    ) -> PlanTask {
        PlanTask {
            id: id.into(),
            objective: format!("Implement {id}"),
            dependencies: Vec::new(),
            read_scopes: read_scopes.iter().map(|scope| (*scope).into()).collect(),
            write_scopes: write_scopes.iter().map(|scope| (*scope).into()).collect(),
            acceptance_criteria: vec!["AC-001".into()],
            verification: Vec::new(),
            browser_checks: Vec::new(),
            risk: "low".into(),
            parallel_safe,
        }
    }

    #[tokio::test]
    async fn provider_diff_recounts_hunk_metadata_without_relaxing_scope() {
        let repository = tempfile::tempdir().unwrap();
        std::fs::write(repository.path().join("session.txt"), "active\nold-token\n").unwrap();
        git_output(repository.path(), &["init", "--quiet"], 1024)
            .await
            .unwrap();
        let task = scheduler_task("TSK-001", &["session.txt"], &["session.txt"], true);
        let diff = r#"diff --git a/session.txt b/session.txt
--- a/session.txt
+++ b/session.txt
@@ -1,2 +1,2 @@
 active
-old-token
+new-token
+still-active
"#;

        let changed = validate_and_apply_provider_diff(repository.path(), diff, &task)
            .await
            .unwrap();

        assert_eq!(changed, ["session.txt"]);
        assert_eq!(
            std::fs::read_to_string(repository.path().join("session.txt")).unwrap(),
            "active\nnew-token\nstill-active\n"
        );
    }

    fn run_at(phase: &str, status: &str) -> SddRunRecord {
        SddRunRecord {
            run_id: "run-autopilot".into(),
            spec_id: "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            repo_id: "repo".into(),
            phase: phase.into(),
            status: status.into(),
            aggregate_revision: 2,
            base_ref: "HEAD".into(),
            base_commit: "deadbeef".into(),
            branch_name: "agentum/spc-01arz3ndektsv4rrffq69g5fav-example".into(),
            authoritative_path: "/tmp/authoritative".into(),
            workspace_fingerprint: "fingerprint".into(),
            policy_json: r#"{"control":"autopilot"}"#.into(),
            blocker: None,
            quarantined: 0,
            created_at: "2026-07-27T12:00:00Z".into(),
            updated_at: "2026-07-27T12:00:00Z".into(),
        }
    }

    #[test]
    fn autopilot_lifecycle_stops_at_human_exceptions_ready_and_delivery() {
        assert!(!lifecycle_must_stop(&run_at("design", "queued")));
        for (phase, status) in [
            ("design", "waiting"),
            ("implementation", "blocked"),
            ("verification", "paused"),
            ("ready", "queued"),
            ("delivery", "queued"),
            ("completed", "queued"),
        ] {
            assert!(
                lifecycle_must_stop(&run_at(phase, status)),
                "lifecycle crossed gate {phase}/{status}"
            );
        }
        let mut quarantined = run_at("design", "queued");
        quarantined.quarantined = 1;
        assert!(lifecycle_must_stop(&quarantined));
    }

    #[test]
    fn plan_validation_rejects_cycles_shells_and_unsafe_paths() {
        let spec_id = SpecId::new();
        let base = json!({
            "schemaVersion": 1,
            "specId": spec_id,
            "specRevision": 2,
            "tasks": [{
                "id": "T-001",
                "objective": "Implement",
                "dependencies": [],
                "readScopes": ["src"],
                "writeScopes": ["src/lib.rs"],
                "acceptanceCriteria": ["AC-001"],
                "verification": [{
                    "program": "cargo", "args": ["test"], "cwd": ".",
                    "envAllowlist": [], "timeoutMs": 60000, "outputLimit": 1024
                }],
                "risk": "low",
                "parallelSafe": true
            }]
        });
        assert!(validate_plan(&base.to_string(), &spec_id, 2).is_ok());
        let mut valid = base;
        assert!(validate_plan(&valid.to_string(), &spec_id, 2).is_ok());
        valid["tasks"][0]["verification"][0]["program"] = json!("bash");
        assert!(validate_plan(&valid.to_string(), &spec_id, 2).is_err());
        valid["tasks"][0]["verification"][0]["program"] = json!("cargo");
        valid["tasks"][0]["writeScopes"] = json!(["../outside"]);
        assert!(validate_plan(&valid.to_string(), &spec_id, 2).is_err());
    }

    #[test]
    fn browser_plan_validation_is_bounded_unique_and_loopback_by_default() {
        let spec_id = SpecId::new();
        let mut plan = json!({
            "schemaVersion": 1,
            "specId": spec_id,
            "specRevision": 2,
            "tasks": [{
                "id": "T-001",
                "objective": "Verify browser",
                "dependencies": [],
                "readScopes": ["src"],
                "writeScopes": ["src/lib.rs"],
                "acceptanceCriteria": ["AC-001"],
                "verification": [],
                "browserChecks": [{
                    "id": "browser-session",
                    "url": "http://127.0.0.1:3000/session?secret=redacted-at-capture",
                    "acceptanceCriteria": ["AC-001"],
                    "waitUntil": "load",
                    "viewport": { "width": 1280, "height": 720, "deviceScaleMilli": 1000 },
                    "timeoutMs": 10000,
                    "assertions": [{
                        "type": "page_loaded", "id": "BV-001", "expectedStatus": 200
                    }]
                }],
                "risk": "low",
                "parallelSafe": true
            }]
        });
        assert!(validate_plan(&plan.to_string(), &spec_id, 2).is_ok());
        let duplicate = plan["tasks"][0].clone();
        plan["tasks"].as_array_mut().unwrap().push(duplicate);
        plan["tasks"][1]["id"] = json!("T-002");
        assert!(validate_plan(&plan.to_string(), &spec_id, 2).is_err());
        plan["tasks"].as_array_mut().unwrap().pop();
        plan["tasks"][0]["browserChecks"][0]["url"] =
            json!("http://169.254.169.254/latest/meta-data");
        assert!(validate_plan(&plan.to_string(), &spec_id, 2).is_err());
    }

    #[test]
    fn scheduler_parallelizes_only_independent_tasks_and_serializes_conflicts() {
        let tasks = vec![
            scheduler_task("T-001", &["src/auth"], &["src/auth/token.rs"], true),
            scheduler_task("T-002", &["src/auth"], &["src/auth"], true),
            scheduler_task("T-003", &["config"], &["tests/token.rs"], true),
            scheduler_task("T-004", &["docs"], &["docs/guide.md"], false),
        ];
        let mut statuses = tasks
            .iter()
            .map(|task| (task.id.clone(), "queued".to_owned()))
            .collect::<HashMap<_, _>>();

        let first = select_ready_task_batch(&tasks, &statuses, 4);
        assert_eq!(
            first
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["T-001", "T-003"]
        );

        statuses.insert("T-001".into(), "succeeded".into());
        statuses.insert("T-003".into(), "succeeded".into());
        let second = select_ready_task_batch(&tasks, &statuses, 4);
        assert_eq!(
            second
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["T-002"]
        );

        statuses.insert("T-002".into(), "succeeded".into());
        let third = select_ready_task_batch(&tasks, &statuses, 4);
        assert_eq!(
            third
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["T-004"]
        );
    }

    #[test]
    fn scope_conflicts_cover_parent_and_read_write_overlap_without_prefix_collisions() {
        let writer = scheduler_task("T-001", &[], &["src/auth"], true);
        let nested_writer = scheduler_task("T-002", &[], &["src/auth/token.rs"], true);
        let reader = scheduler_task("T-003", &["src/auth/token.rs"], &["tests"], true);
        let independent = scheduler_task("T-004", &["src/config"], &["docs"], true);

        assert!(task_scopes_conflict(&writer, &nested_writer));
        assert!(task_scopes_conflict(&writer, &reader));
        assert!(!task_scopes_conflict(&writer, &independent));
        assert!(!scopes_overlap("src/auth", "src/authentication"));
    }

    #[tokio::test]
    async fn bounded_runner_polls_independent_tasks_in_parallel() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let futures = (0..6)
            .map(|ordinal| {
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    ordinal
                }
            })
            .collect::<Vec<_>>();

        let mut results = collect_bounded(futures, 2).await;
        results.sort_unstable();
        assert_eq!(results, (0..6).collect::<Vec<_>>());
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn verification_command_rejects_shells_and_sensitive_environment() {
        let mut command = CommandSpec {
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            env_allowlist: vec!["PATH".into(), "CI".into()],
            timeout_ms: 60_000,
            output_limit: 1024,
        };
        assert!(verification_command_is_safe(&command));
        command.env_allowlist.push("OPENAI_API_KEY".into());
        assert!(!verification_command_is_safe(&command));
        command.env_allowlist = Vec::new();
        command.program = "pwsh".into();
        assert!(!verification_command_is_safe(&command));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn verification_rejects_a_linked_git_marker_before_resolving_metadata() {
        use std::os::unix::fs::symlink;

        let attempt = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), attempt.path().join(".git")).unwrap();

        assert!(matches!(
            linked_worktree_git_metadata(attempt.path()),
            Err(LifecycleError::Invalid(reason)) if reason.contains("must not be a symlink")
        ));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn verification_runs_directly_in_the_os_sandbox() {
        if which::which("bwrap").is_err() || which::which("git").is_err() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let command = CommandSpec {
            program: "git".into(),
            args: vec!["--version".into()],
            cwd: ".".into(),
            env_allowlist: vec!["PATH".into()],
            timeout_ms: 10_000,
            output_limit: 32 * 1024,
        };
        let result = run_verification_command("test:verification", directory.path(), &command, 0)
            .await
            .unwrap();
        assert_eq!(result.status, "succeeded");
        assert!(result.output_excerpt.starts_with("[redacted]"));
        assert!(!result.output_hash.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn verification_cancellation_terminates_the_process_group() {
        if which::which("bwrap").is_err() || which::which("sleep").is_err() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let command = CommandSpec {
            program: "sleep".into(),
            args: vec!["30".into()],
            cwd: ".".into(),
            env_allowlist: vec![],
            timeout_ms: 30_000,
            output_limit: 1024,
        };
        let run = tokio::spawn(async move {
            run_verification_command("verify-cancel:attempt:0", directory.path(), &command, 0).await
        });
        for _ in 0..100 {
            if cancel_run("verify-cancel") {
                break;
            }
            tokio::task::yield_now().await;
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("cancellation must not wait for the verification timeout")
            .unwrap()
            .unwrap();
        assert_eq!(result.status, "canceled");
        assert!(!cancel_run("verify-cancel"));
    }
}
