//! Release-grade SDD provider conformance.
//!
//! Contract-only probes are useful developer feedback, but they cannot prove
//! that a model transport can complete Agentum's Standard + Guarded workflow.
//! This module runs every phase against a disposable copy of the public demo
//! fixture, validates and publishes the real artifacts, applies only the
//! bounded provider diff, executes verification in the OS sandbox, and starts
//! review in a separate provider process. Reports contain hashes and stable
//! status only; provider/model output is never persisted in CI artifacts.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use agentum_core::sdd::{PlanArtifact, PlanTask, SpecId};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use super::artifacts::{self, MISSING_HASH};
use super::credentials::SddCredentialVault;
use super::lifecycle::{self, LifecycleError};
use super::providers::{
    self, BundledProvider, CustomProviderAdapter, CustomProviderConformanceCase,
    CustomProviderConformanceEvidence, CustomProviderError, ProviderAdapter, ProviderError,
    ProviderOperation, SddProviderAdapter,
};

const REPORT_SCHEMA_VERSION: u32 = 1;
const TITLE: &str = "Refresh access tokens";
const GOAL: &str = "Refresh access tokens without interrupting active sessions";
const FIXTURE_FILES: [&str; 4] = [
    "README.md",
    "package.json",
    "src/session-store.js",
    "test/session-store.test.js",
];
const CONFORMANCE_TEST: &str = r#"import assert from "node:assert/strict";
import test from "node:test";

import { SessionStore } from "../src/session-store.js";

test("refreshing an access token preserves the active session", () => {
  const sessions = new SessionStore();
  sessions.start("session-1", "access-token-1");

  sessions.refreshAccessToken("session-1", "access-token-2");

  assert.equal(sessions.isActive("session-1"), true);
  assert.equal(sessions.accessToken("session-1"), "access-token-2");
});
"#;

#[derive(Debug, thiserror::Error)]
pub enum ProviderConformanceError {
    #[error("provider contract: {0}")]
    Provider(#[from] ProviderError),
    #[error("custom provider: {0}")]
    Custom(#[from] CustomProviderError),
    #[error("lifecycle: {0}")]
    Lifecycle(#[from] LifecycleError),
    #[error("artifact: {0}")]
    Artifact(#[from] artifacts::ArtifactError),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("provider conformance failed: {0}")]
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderConformanceReport {
    pub schema_version: u32,
    pub suite: String,
    pub provider_id: String,
    pub provider_version: String,
    pub source_revision: String,
    pub fixture_sha256: String,
    pub approval_digest: String,
    pub profile: String,
    pub control: String,
    pub terminal_phase: String,
    pub delivery_performed: bool,
    pub completed_at_unix: i64,
    pub cases: Vec<CustomProviderConformanceEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderConformanceBundle {
    pub schema_version: u32,
    pub suite: String,
    pub source_revision: String,
    pub reports: Vec<ProviderConformanceReport>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryCheckpoint {
    schema_version: u32,
    provider_id: String,
    spec_id: String,
    phase: String,
    approval_digest: String,
    source_revision: String,
}

struct Fixture {
    _root: tempfile::TempDir,
    repository: PathBuf,
    runtime: PathBuf,
    source_fixture_sha256: String,
    fixture_sha256: String,
}

/// Execute all bundled providers sequentially. Release qualification uses a
/// concurrency of one intentionally: credentials, rate limits, and provider
/// caches are shared resources on the hardened conformance host.
pub async fn run_bundled_suite(
    provider_ids: &[String],
    source_revision: &str,
) -> Result<ProviderConformanceBundle, ProviderConformanceError> {
    validate_source_revision(source_revision)?;
    let requested: Vec<String> = if provider_ids.is_empty() {
        providers::BUNDLED_IDS
            .iter()
            .map(|value| (*value).to_owned())
            .collect()
    } else {
        provider_ids.to_vec()
    };
    let declared: BTreeSet<_> = requested.iter().map(String::as_str).collect();
    if declared.len() != requested.len() {
        return Err(ProviderConformanceError::Failed(
            "provider ids must be unique".into(),
        ));
    }

    let mut reports = Vec::with_capacity(requested.len());
    for id in requested {
        let bundled = BundledProvider::get(&id).ok_or_else(|| {
            ProviderConformanceError::Failed(format!("unknown bundled provider: {id}"))
        })?;
        let capability = providers::probe_provider(bundled).await;
        if !capability.available {
            return Err(ProviderConformanceError::Failed(format!(
                "{} is unavailable: {}",
                capability.descriptor.id,
                capability.reason.as_deref().unwrap_or("probe failed")
            )));
        }
        let version = capability.version.ok_or_else(|| {
            ProviderConformanceError::Failed(format!("{id} did not report a version"))
        })?;
        reports.push(
            run_provider_suite(ProviderAdapter::Bundled(bundled), &version, source_revision)
                .await?,
        );
    }
    Ok(ProviderConformanceBundle {
        schema_version: REPORT_SCHEMA_VERSION,
        suite: providers::CUSTOM_PROVIDER_CONFORMANCE_SUITE.into(),
        source_revision: source_revision.into(),
        reports,
    })
}

/// Run an unapproved custom manifest through the exact same lifecycle. The
/// approval is published only after every case succeeds and is signed with the
/// installation key held by `vault`.
pub async fn run_custom_suite(
    directory: &Path,
    id: &str,
    source_revision: &str,
    vault: &dyn SddCredentialVault,
) -> Result<ProviderConformanceBundle, ProviderConformanceError> {
    validate_source_revision(source_revision)?;
    if !id
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ProviderConformanceError::Failed(
            "custom provider id is invalid".into(),
        ));
    }
    let manifest_path = directory.join(format!("{id}.json"));
    let (manifest, _) = artifacts::read_bytes(&manifest_path)?;
    let adapter = providers::validate_custom_provider_manifest(&manifest, id)?;
    probe_unapproved_custom(&adapter).await?;
    let report = run_provider_suite(
        ProviderAdapter::Custom(adapter.clone()),
        &adapter.version,
        source_revision,
    )
    .await?;
    let receipt = providers::signed_conformance_receipt_for(
        &adapter,
        report.cases.clone(),
        &report.fixture_sha256,
        source_revision,
        vault,
    )?;
    let bytes = pretty_json(&receipt)?;
    let receipt_path = directory.join(format!("{id}.approval.json"));
    let expected = artifacts::content_hash(&receipt_path)?;
    artifacts::atomic_write(&receipt_path, &bytes, Some(&expected))?;
    // Reload through the authenticated path before reporting success. This is
    // the same check future Agentum runs perform after a process restart.
    providers::load_custom_provider_from_directory_with_vault(
        directory,
        &format!("custom:{id}"),
        vault,
    )?;
    Ok(ProviderConformanceBundle {
        schema_version: REPORT_SCHEMA_VERSION,
        suite: providers::CUSTOM_PROVIDER_CONFORMANCE_SUITE.into(),
        source_revision: source_revision.into(),
        reports: vec![report],
    })
}

pub fn publish_report(
    path: &Path,
    bundle: &ProviderConformanceBundle,
) -> Result<(), ProviderConformanceError> {
    validate_report(bundle, &bundle.source_revision, &[])?;
    let expected = artifacts::content_hash(path)?;
    artifacts::atomic_write(path, &pretty_json(bundle)?, Some(&expected))?;
    Ok(())
}

/// Validate a redacted report before release packaging. The actual gate is the
/// runner's successful process exit; this second check prevents a report for a
/// different source revision or incomplete provider set from being reused.
pub fn verify_report_file(
    path: &Path,
    source_revision: &str,
    required_provider_ids: &[String],
) -> Result<(), ProviderConformanceError> {
    let (content, _) = artifacts::read_text(path)?;
    let bundle: ProviderConformanceBundle = serde_json::from_str(&content)?;
    validate_report(&bundle, source_revision, required_provider_ids)
}

pub fn verify_checkpoint_file(
    path: &Path,
    expected_hash: &str,
) -> Result<(), ProviderConformanceError> {
    let (content, hash) = artifacts::read_text(path)?;
    if hash != expected_hash {
        return Err(ProviderConformanceError::Failed(
            "recovery checkpoint hash changed".into(),
        ));
    }
    let checkpoint: RecoveryCheckpoint = serde_json::from_str(&content)?;
    if checkpoint.schema_version != REPORT_SCHEMA_VERSION
        || checkpoint.phase != "waiting_spec_approval"
        || checkpoint.provider_id.is_empty()
        || checkpoint.spec_id.parse::<SpecId>().is_err()
        || !valid_sha256(&checkpoint.approval_digest)
        || checkpoint.source_revision.is_empty()
    {
        return Err(ProviderConformanceError::Failed(
            "recovery checkpoint is malformed".into(),
        ));
    }
    Ok(())
}

async fn run_provider_suite(
    adapter: ProviderAdapter,
    provider_version: &str,
    source_revision: &str,
) -> Result<ProviderConformanceReport, ProviderConformanceError> {
    providers::validate_provider_contract(&adapter)?;
    let descriptor = adapter.descriptor();
    let fixture = prepare_fixture()?;
    let repository = fixture.repository.canonicalize()?;
    let repository_text = repository.to_string_lossy().into_owned();
    let staging_root = repository.join(".agentum-provider-staging");
    std::fs::create_dir(&staging_root)?;
    let original_fixture_after_copy = hash_source_fixture()?;
    if original_fixture_after_copy != fixture.source_fixture_sha256 {
        return Err(ProviderConformanceError::Failed(
            "the checked-in fixture changed while conformance was running".into(),
        ));
    }

    let mut evidence = Vec::new();

    // Cancellation is exercised before save. The canceled authoring attempt
    // must leave no project-owned artifact root behind.
    let cancellation_execution = format!("conformance:{}:cancel", descriptor.id);
    let cancel_adapter = adapter.clone();
    let cancel_cwd = repository_text.clone();
    let cancel_staging = staging_root.join("cancel.out");
    let cancel_staging_text = cancel_staging.to_string_lossy().into_owned();
    let cancellation_execution_for_task = cancellation_execution.clone();
    let cancel_task = tokio::spawn(async move {
        providers::run_artifact(
            &cancellation_execution_for_task,
            &cancel_adapter,
            ProviderOperation::Authoring,
            &cancel_cwd,
            &providers::authoring_prompt(TITLE, GOAL),
            &cancel_staging_text,
            providers::SPEC_BEGIN,
            providers::SPEC_END,
        )
        .await
    });
    let mut cancellation_requested = false;
    for _ in 0..500 {
        if providers::cancel_run(&cancellation_execution) {
            cancellation_requested = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    if !cancellation_requested {
        cancel_task.abort();
        return Err(ProviderConformanceError::Failed(
            "provider exited before process-tree cancellation could be requested".into(),
        ));
    }
    match cancel_task.await {
        Ok(Err(ProviderError::Canceled)) => {}
        Ok(other) => {
            return Err(ProviderConformanceError::Failed(format!(
                "provider cancellation returned {other:?}"
            )));
        }
        Err(error) => {
            return Err(ProviderConformanceError::Failed(format!(
                "provider cancellation task failed: {error}"
            )));
        }
    }
    if repository.join(".agentum").exists() {
        return Err(ProviderConformanceError::Failed(
            "canceling before save created .agentum".into(),
        ));
    }
    push_evidence(
        &mut evidence,
        CustomProviderConformanceCase::Cancellation,
        &descriptor.id,
        source_revision,
        &fixture.fixture_sha256,
        b"process-tree-canceled-before-save:no-artifact-root",
    );

    let spec_body = run_phase(
        &adapter,
        ProviderOperation::Authoring,
        "authoring",
        &repository_text,
        &providers::authoring_prompt(TITLE, GOAL),
        &staging_root,
    )
    .await?;
    let spec_id = SpecId::new();
    let spec = artifacts::render_spec(&spec_id, 1, TITLE, None, &spec_body)?;
    let artifact_root = artifacts::initialize(&repository, &spec_id, TITLE, Ulid::new())?;
    let spec_path = artifact_root.spec_dir.join("spec.md");
    artifacts::atomic_write(&spec_path, spec.as_bytes(), Some(MISSING_HASH))?;
    let (_, spec_hash) = artifacts::read_text(&spec_path)?;
    push_evidence(
        &mut evidence,
        CustomProviderConformanceCase::Authoring,
        &descriptor.id,
        source_revision,
        &fixture.fixture_sha256,
        spec.as_bytes(),
    );

    // Standard + Guarded must stop here. Persist the hash-bound gate outside
    // the repository and prove a fresh process can recover it before allowing
    // design to start.
    for later in ["design.md", "plan.json", "review.md"] {
        if artifact_root.spec_dir.join(later).exists() {
            return Err(ProviderConformanceError::Failed(
                "a downstream artifact existed before spec approval".into(),
            ));
        }
    }
    let workspace_fingerprint = super::sha256(format!(
        "{}:{}:{}",
        fixture.fixture_sha256, descriptor.id, source_revision
    ));
    let approval_digest = super::sha256(format!(
        "{spec_hash}:profile=standard:control=guarded:{workspace_fingerprint}"
    ));
    let checkpoint = RecoveryCheckpoint {
        schema_version: REPORT_SCHEMA_VERSION,
        provider_id: descriptor.id.clone(),
        spec_id: spec_id.to_string(),
        phase: "waiting_spec_approval".into(),
        approval_digest: approval_digest.clone(),
        source_revision: source_revision.into(),
    };
    let checkpoint_path = fixture.runtime.join("approval-checkpoint.json");
    let checkpoint_hash = artifacts::atomic_write(
        &checkpoint_path,
        &pretty_json(&checkpoint)?,
        Some(MISSING_HASH),
    )?;
    verify_checkpoint_in_fresh_process(&checkpoint_path, &checkpoint_hash).await?;
    push_evidence(
        &mut evidence,
        CustomProviderConformanceCase::GuardedApproval,
        &descriptor.id,
        source_revision,
        &fixture.fixture_sha256,
        approval_digest.as_bytes(),
    );
    push_evidence(
        &mut evidence,
        CustomProviderConformanceCase::RestartRecovery,
        &descriptor.id,
        source_revision,
        &fixture.fixture_sha256,
        checkpoint_hash.as_bytes(),
    );

    let design_prompt = format!(
        "Read {} and the repository. Do not edit files or change Git. Produce a concrete design tied to RQ-* and AC-* identifiers, covering API behavior, active-session preservation, failure handling, and verification. Return only Markdown between literal lines {} and {}.",
        spec_path.display(),
        providers::DESIGN_BEGIN,
        providers::DESIGN_END
    );
    let design = run_phase(
        &adapter,
        ProviderOperation::Design,
        "design",
        &repository_text,
        &design_prompt,
        &staging_root,
    )
    .await?;
    if !design.contains("RQ-") || !design.contains("AC-") {
        return Err(ProviderConformanceError::Failed(
            "design did not trace RQ/AC identifiers".into(),
        ));
    }
    artifacts::atomic_write(
        &artifact_root.spec_dir.join("design.md"),
        design.as_bytes(),
        Some(MISSING_HASH),
    )?;
    push_evidence(
        &mut evidence,
        CustomProviderConformanceCase::Design,
        &descriptor.id,
        source_revision,
        &fixture.fixture_sha256,
        design.as_bytes(),
    );

    let plan_prompt = format!(
        "Read {}, {}, and the repository. Do not edit files or change Git. Return exactly one task that changes only src/session-store.js, implements refreshAccessToken while preserving active sessions, and traces AC-001. Return only a JSON object between literal lines {} and {}. The object must use schemaVersion 1, specId {:?}, and specRevision 1. The one task must include every field: id \"TSK-001\", a concrete objective, dependencies [], readScopes [\"src/session-store.js\",\"test/refresh-token.conformance.test.js\"], writeScopes [\"src/session-store.js\"], acceptanceCriteria [\"AC-001\"], verification exactly [{{\"program\":\"node\",\"args\":[\"--test\"],\"cwd\":\".\",\"envAllowlist\":[\"PATH\"],\"timeoutMs\":120000,\"outputLimit\":1048576}}], browserChecks [], risk \"low\", and parallelSafe true. Do not omit, rename, or add fields.",
        spec_path.display(),
        artifact_root.spec_dir.join("design.md").display(),
        providers::PLAN_BEGIN,
        providers::PLAN_END,
        spec_id.to_string()
    );
    let plan_json = run_phase(
        &adapter,
        ProviderOperation::Planning,
        "planning",
        &repository_text,
        &plan_prompt,
        &staging_root,
    )
    .await?;
    lifecycle::validate_plan(&plan_json, &spec_id, 1)?;
    let plan: PlanArtifact = serde_json::from_str(&plan_json)?;
    let task = validate_conformance_plan(&plan)?;
    artifacts::atomic_write(
        &artifact_root.spec_dir.join("plan.json"),
        plan_json.as_bytes(),
        Some(MISSING_HASH),
    )?;
    push_evidence(
        &mut evidence,
        CustomProviderConformanceCase::Planning,
        &descriptor.id,
        source_revision,
        &fixture.fixture_sha256,
        plan_json.as_bytes(),
    );

    let initial_verification = lifecycle::run_verification_command(
        &format!("conformance:{}:verification-before", descriptor.id),
        &repository,
        &task.verification[0],
        0,
    )
    .await?;
    if initial_verification.status != "failed" {
        return Err(ProviderConformanceError::Failed(
            "fixture did not prove the acceptance test fails before implementation".into(),
        ));
    }

    let implementation_prompt = format!(
        "Read the repository and Agentum artifacts below {}. Implement only task {:?}. Do not edit tests, .agentum, provider settings, or Git metadata; do not commit or push. Return one ordinary UTF-8 unified Git diff changing only src/session-store.js between literal lines {} and {}. The first line after the begin marker must be exactly `diff --git a/src/session-store.js b/src/session-store.js`, followed by ordinary `--- a/src/session-store.js`, `+++ b/src/session-store.js`, and `@@` hunk headers. Do not use Markdown fences, prose, `*** Begin Patch`, or `*** End Patch` inside the markers. Preserve the existing class and public methods. The implementation must make the existing refreshAccessToken conformance test pass without deactivating the session.",
        artifact_root.spec_dir.display(),
        task.id,
        providers::DIFF_BEGIN,
        providers::DIFF_END
    );
    let diff = run_phase(
        &adapter,
        ProviderOperation::ImplementationDiff,
        "implementation",
        &repository_text,
        &implementation_prompt,
        &staging_root,
    )
    .await?;
    let changed = lifecycle::validate_and_apply_provider_diff(&repository, &diff, task).await?;
    if changed != ["src/session-store.js"] {
        return Err(ProviderConformanceError::Failed(
            "implementation changed files outside the exact conformance scope".into(),
        ));
    }
    push_evidence(
        &mut evidence,
        CustomProviderConformanceCase::ImplementationDiff,
        &descriptor.id,
        source_revision,
        &fixture.fixture_sha256,
        diff.as_bytes(),
    );

    let verification = lifecycle::run_verification_command(
        &format!("conformance:{}:verification-after", descriptor.id),
        &repository,
        &task.verification[0],
        1,
    )
    .await?;
    if verification.status != "succeeded" {
        return Err(ProviderConformanceError::Failed(format!(
            "post-implementation verification status was {}",
            verification.status
        )));
    }
    let verification_binding = serde_json::to_vec(&(
        initial_verification.output_hash,
        verification.output_hash,
        verification.status,
    ))?;
    push_evidence(
        &mut evidence,
        CustomProviderConformanceCase::Verification,
        &descriptor.id,
        source_revision,
        &fixture.fixture_sha256,
        &verification_binding,
    );

    let review_prompt = format!(
        "This is an independent review in a new isolated provider session. Read {}, {}, {}, and the current repository diff. Verification succeeded with evidence hash {}. Do not edit files or change Git. Review RQ/AC traceability, active-session preservation, regressions, and scope. If and only if ready, include one line exactly `Verdict: PASS`; otherwise `Verdict: FAIL`. Return only Markdown between literal lines {} and {}.",
        spec_path.display(),
        artifact_root.spec_dir.join("design.md").display(),
        artifact_root.spec_dir.join("plan.json").display(),
        super::sha256(&verification_binding),
        providers::REVIEW_BEGIN,
        providers::REVIEW_END
    );
    let review = run_phase(
        &adapter,
        ProviderOperation::Review,
        "review",
        &repository_text,
        &review_prompt,
        &staging_root,
    )
    .await?;
    let pass_count = review
        .lines()
        .filter(|line| line.trim() == "Verdict: PASS")
        .count();
    if pass_count != 1
        || review.lines().any(|line| line.trim() == "Verdict: FAIL")
        || !review.contains("AC-")
    {
        return Err(ProviderConformanceError::Failed(
            "independent review did not produce one AC-traced PASS".into(),
        ));
    }
    artifacts::atomic_write(
        &artifact_root.spec_dir.join("review.md"),
        review.as_bytes(),
        Some(MISSING_HASH),
    )?;
    push_evidence(
        &mut evidence,
        CustomProviderConformanceCase::IndependentReview,
        &descriptor.id,
        source_revision,
        &fixture.fixture_sha256,
        review.as_bytes(),
    );

    let malformed_prompt = "Return the single word sample between literal lines AGENTUM_MALFORMED_SAMPLE_BEGIN and AGENTUM_MALFORMED_SAMPLE_END. Do not use any other marker.";
    let malformed_staging = staging_root.join("malformed.out");
    let malformed = providers::run_artifact(
        &format!("conformance:{}:malformed", descriptor.id),
        &adapter,
        ProviderOperation::Review,
        &repository_text,
        malformed_prompt,
        &malformed_staging.to_string_lossy(),
        "AGENTUM_UNDISCLOSED_EXPECTED_BEGIN_01KCONFORMANCE",
        "AGENTUM_UNDISCLOSED_EXPECTED_END_01KCONFORMANCE",
    )
    .await;
    if !matches!(malformed, Err(ProviderError::MalformedOutput)) {
        return Err(ProviderConformanceError::Failed(format!(
            "malformed provider output was not rejected: {malformed:?}"
        )));
    }
    push_evidence(
        &mut evidence,
        CustomProviderConformanceCase::MalformedOutput,
        &descriptor.id,
        source_revision,
        &fixture.fixture_sha256,
        b"undisclosed-envelope-rejected",
    );

    remove_staging_root(&staging_root)?;
    remove_optional_runtime_root(&repository.join(".agentum-verification-runtime"))?;
    let status = git_output(
        &repository,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?;
    let status_lines: BTreeSet<_> = status.lines().collect();
    if !status_lines.contains(" M src/session-store.js")
        || status_lines
            .iter()
            .any(|line| !line.starts_with("?? .agentum/") && *line != " M src/session-store.js")
    {
        return Err(ProviderConformanceError::Failed(format!(
            "Ready worktree has unexpected changes: {}",
            super::sha256(status.as_bytes())
        )));
    }
    for forbidden in [
        "ai",
        concat!(".agentum", "-harness"),
        ".planning",
        ".claude",
        ".cursor",
        ".gemini",
        "openspec",
        "opencode.json",
        ".aider.conf.yml",
    ] {
        if repository.join(forbidden).exists() {
            return Err(ProviderConformanceError::Failed(format!(
                "provider polluted the fixture with {forbidden}"
            )));
        }
    }
    if git_output(&repository, &["log", "-1", "--format=%s"])?.trim()
        != "Agentum conformance baseline"
    {
        return Err(ProviderConformanceError::Failed(
            "provider changed Git history or performed delivery".into(),
        ));
    }
    push_evidence(
        &mut evidence,
        CustomProviderConformanceCase::ReadyNoDelivery,
        &descriptor.id,
        source_revision,
        &fixture.fixture_sha256,
        status.as_bytes(),
    );

    if hash_source_fixture()? != fixture.source_fixture_sha256 {
        return Err(ProviderConformanceError::Failed(
            "the source demo fixture was modified".into(),
        ));
    }
    validate_case_set(&evidence)?;
    Ok(ProviderConformanceReport {
        schema_version: REPORT_SCHEMA_VERSION,
        suite: providers::CUSTOM_PROVIDER_CONFORMANCE_SUITE.into(),
        provider_id: descriptor.id,
        provider_version: provider_version.into(),
        source_revision: source_revision.into(),
        fixture_sha256: fixture.fixture_sha256,
        approval_digest,
        profile: "standard".into(),
        control: "guarded".into(),
        terminal_phase: "ready".into(),
        delivery_performed: false,
        completed_at_unix: time::OffsetDateTime::now_utc().unix_timestamp(),
        cases: evidence,
    })
}

async fn run_phase(
    adapter: &ProviderAdapter,
    operation: ProviderOperation,
    phase: &str,
    cwd: &str,
    prompt: &str,
    staging_root: &Path,
) -> Result<String, ProviderConformanceError> {
    let (begin, end) = match operation {
        ProviderOperation::Authoring => (providers::SPEC_BEGIN, providers::SPEC_END),
        ProviderOperation::Design => (providers::DESIGN_BEGIN, providers::DESIGN_END),
        ProviderOperation::Planning => (providers::PLAN_BEGIN, providers::PLAN_END),
        ProviderOperation::ImplementationDiff => (providers::DIFF_BEGIN, providers::DIFF_END),
        ProviderOperation::Review => (providers::REVIEW_BEGIN, providers::REVIEW_END),
    };
    let staging = staging_root.join(format!("{phase}.out"));
    Ok(providers::run_artifact(
        &format!("conformance:{}:{phase}", adapter.descriptor().id),
        adapter,
        operation,
        cwd,
        prompt,
        &staging.to_string_lossy(),
        begin,
        end,
    )
    .await?)
}

fn validate_conformance_plan(plan: &PlanArtifact) -> Result<&PlanTask, ProviderConformanceError> {
    if plan.tasks.len() != 1 {
        return Err(ProviderConformanceError::Failed(
            "conformance plan must contain exactly one bounded task".into(),
        ));
    }
    let task = &plan.tasks[0];
    if task.write_scopes != ["src/session-store.js"]
        || !task
            .acceptance_criteria
            .iter()
            .any(|value| value == "AC-001")
        || task.verification.is_empty()
        || task.verification[0].program != "node"
        || task.verification[0].args != ["--test"]
        || task.verification[0].cwd != "."
    {
        return Err(ProviderConformanceError::Failed(
            "plan did not preserve the exact write scope, AC, and typed verification command"
                .into(),
        ));
    }
    Ok(task)
}

fn prepare_fixture() -> Result<Fixture, ProviderConformanceError> {
    let source = fixture_source();
    let source_fixture_sha256 = hash_fixture_at(&source, false)?;
    let root = tempfile::Builder::new()
        .prefix("agentum-provider-conformance.")
        .tempdir()?;
    let repository = root.path().join("authoritative");
    let runtime = root.path().join("runtime");
    std::fs::create_dir(&repository)?;
    std::fs::create_dir(&runtime)?;
    for relative in FIXTURE_FILES {
        let from = source.join(relative);
        let (content, _) = artifacts::read_bytes(&from)?;
        let to = repository.join(relative);
        std::fs::create_dir_all(to.parent().expect("fixture file has a parent"))?;
        artifacts::atomic_write(&to, &content, Some(MISSING_HASH))?;
    }
    let conformance_test = repository.join("test/refresh-token.conformance.test.js");
    artifacts::atomic_write(
        &conformance_test,
        CONFORMANCE_TEST.as_bytes(),
        Some(MISSING_HASH),
    )?;
    git_success(&repository, &["init", "--quiet"])?;
    git_success(&repository, &["add", "--all"])?;
    git_success(
        &repository,
        &[
            "-c",
            "user.name=Agentum Conformance",
            "-c",
            "user.email=conformance@invalid.agentum",
            "commit",
            "--quiet",
            "-m",
            "Agentum conformance baseline",
        ],
    )?;
    let fixture_sha256 = hash_fixture_at(&repository, true)?;
    Ok(Fixture {
        _root: root,
        repository,
        runtime,
        source_fixture_sha256,
        fixture_sha256,
    })
}

fn fixture_source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("agentum-server is a workspace crate")
        .join("examples/sdd-demo")
}

fn hash_source_fixture() -> Result<String, ProviderConformanceError> {
    hash_fixture_at(&fixture_source(), false)
}

fn hash_fixture_at(
    root: &Path,
    include_conformance_test: bool,
) -> Result<String, ProviderConformanceError> {
    let mut inputs: Vec<&str> = FIXTURE_FILES.to_vec();
    if include_conformance_test {
        inputs.push("test/refresh-token.conformance.test.js");
    }
    let mut binding = Vec::new();
    for relative in inputs {
        let path = root.join(relative);
        let (content, _) = artifacts::read_bytes(&path)?;
        binding.extend_from_slice(relative.as_bytes());
        binding.push(0);
        binding.extend_from_slice(&content);
        binding.push(0xff);
    }
    Ok(super::sha256(binding))
}

fn remove_staging_root(path: &Path) -> Result<(), ProviderConformanceError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ProviderConformanceError::Failed(
            "provider staging root was replaced with an unsafe object".into(),
        ));
    }
    std::fs::remove_dir_all(path)?;
    Ok(())
}

fn remove_optional_runtime_root(path: &Path) -> Result<(), ProviderConformanceError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ProviderConformanceError::Failed(
            "verification runtime root was replaced with an unsafe object".into(),
        ));
    }
    std::fs::remove_dir_all(path)?;
    Ok(())
}

async fn probe_unapproved_custom(
    adapter: &CustomProviderAdapter,
) -> Result<(), ProviderConformanceError> {
    if !providers::isolation_available() {
        return Err(ProviderConformanceError::Failed(
            "an Agentum-enforced provider OS sandbox is unavailable".into(),
        ));
    }
    let descriptor = adapter.descriptor();
    let phase = adapter.phase_command(
        ProviderOperation::Authoring,
        "/agentum/conformance",
        "probe",
        "/agentum/conformance/staging",
        "/agentum/provider",
    );
    let executable = which::which(&phase.program).map_err(|_| {
        ProviderConformanceError::Failed("custom provider executable is not installed".into())
    })?;
    let mut command = tokio::process::Command::new(executable);
    command
        .args(&descriptor.version_probe)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for key in &phase.env_allowlist {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .map_err(|_| ProviderConformanceError::Failed("custom version probe timed out".into()))??;
    if !output.status.success() {
        return Err(ProviderConformanceError::Failed(
            "custom version probe exited unsuccessfully".into(),
        ));
    }
    let mut bounded = output.stdout;
    bounded.extend_from_slice(&output.stderr);
    bounded.truncate(4096);
    let reported = String::from_utf8_lossy(&bounded);
    if !reported.contains(&adapter.version) {
        return Err(ProviderConformanceError::Failed(
            "custom version probe does not match the manifest version".into(),
        ));
    }
    Ok(())
}

async fn verify_checkpoint_in_fresh_process(
    path: &Path,
    expected_hash: &str,
) -> Result<(), ProviderConformanceError> {
    let executable = std::env::current_exe()?;
    let status = tokio::process::Command::new(executable)
        .arg("verify-checkpoint")
        .arg("--checkpoint")
        .arg(path)
        .arg("--expected-hash")
        .arg(expected_hash)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await?;
    if !status.success() {
        return Err(ProviderConformanceError::Failed(
            "fresh-process checkpoint recovery failed".into(),
        ));
    }
    Ok(())
}

fn push_evidence(
    evidence: &mut Vec<CustomProviderConformanceEvidence>,
    case: CustomProviderConformanceCase,
    provider_id: &str,
    source_revision: &str,
    fixture_sha256: &str,
    result: &[u8],
) {
    evidence.push(CustomProviderConformanceEvidence {
        case,
        evidence_sha256: super::sha256(
            [
                provider_id.as_bytes(),
                source_revision.as_bytes(),
                fixture_sha256.as_bytes(),
                format!("{case:?}").as_bytes(),
                super::sha256(result).as_bytes(),
            ]
            .concat(),
        ),
    });
}

fn validate_case_set(
    evidence: &[CustomProviderConformanceEvidence],
) -> Result<(), ProviderConformanceError> {
    let expected: BTreeSet<_> = [
        CustomProviderConformanceCase::Authoring,
        CustomProviderConformanceCase::GuardedApproval,
        CustomProviderConformanceCase::Design,
        CustomProviderConformanceCase::Planning,
        CustomProviderConformanceCase::ImplementationDiff,
        CustomProviderConformanceCase::Verification,
        CustomProviderConformanceCase::IndependentReview,
        CustomProviderConformanceCase::MalformedOutput,
        CustomProviderConformanceCase::Cancellation,
        CustomProviderConformanceCase::RestartRecovery,
        CustomProviderConformanceCase::ReadyNoDelivery,
    ]
    .into_iter()
    .collect();
    let actual: BTreeSet<_> = evidence.iter().map(|entry| entry.case).collect();
    let hashes: BTreeSet<_> = evidence
        .iter()
        .map(|entry| entry.evidence_sha256.as_str())
        .collect();
    if actual != expected
        || evidence.len() != expected.len()
        || hashes.len() != evidence.len()
        || evidence
            .iter()
            .any(|entry| !valid_sha256(&entry.evidence_sha256))
    {
        return Err(ProviderConformanceError::Failed(
            "conformance evidence is incomplete, duplicated, or malformed".into(),
        ));
    }
    Ok(())
}

fn validate_report(
    bundle: &ProviderConformanceBundle,
    source_revision: &str,
    required_provider_ids: &[String],
) -> Result<(), ProviderConformanceError> {
    validate_source_revision(source_revision)?;
    if bundle.schema_version != REPORT_SCHEMA_VERSION
        || bundle.suite != providers::CUSTOM_PROVIDER_CONFORMANCE_SUITE
        || bundle.source_revision != source_revision
        || bundle.reports.is_empty()
    {
        return Err(ProviderConformanceError::Failed(
            "report header is incomplete or bound to another source revision".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    for report in &bundle.reports {
        if report.schema_version != REPORT_SCHEMA_VERSION
            || report.suite != bundle.suite
            || report.source_revision != source_revision
            || report.provider_id.is_empty()
            || report.provider_version.is_empty()
            || !valid_sha256(&report.fixture_sha256)
            || !valid_sha256(&report.approval_digest)
            || report.profile != "standard"
            || report.control != "guarded"
            || report.terminal_phase != "ready"
            || report.delivery_performed
            || report.completed_at_unix <= 0
            || !ids.insert(report.provider_id.as_str())
        {
            return Err(ProviderConformanceError::Failed(
                "report contains a malformed or duplicate provider result".into(),
            ));
        }
        validate_case_set(&report.cases)?;
    }
    let required: BTreeSet<_> = required_provider_ids.iter().map(String::as_str).collect();
    if required.len() != required_provider_ids.len()
        || required.iter().any(|provider| !ids.contains(provider))
    {
        return Err(ProviderConformanceError::Failed(
            "report does not include every required provider".into(),
        ));
    }
    Ok(())
}

fn validate_source_revision(value: &str) -> Result<(), ProviderConformanceError> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(char::is_control)
        || value.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(ProviderConformanceError::Failed(
            "source revision must be a bounded non-whitespace value".into(),
        ));
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn git_success(cwd: &Path, args: &[&str]) -> Result<(), ProviderConformanceError> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(ProviderConformanceError::Failed(format!(
            "git operation failed: {}",
            output.status
        )));
    }
    Ok(())
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String, ProviderConformanceError> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(ProviderConformanceError::Failed(format!(
            "git operation failed: {}",
            output.status
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| ProviderConformanceError::Failed("git output was not UTF-8".into()))
}

fn pretty_json(value: &impl Serialize) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn complete_evidence() -> Vec<CustomProviderConformanceEvidence> {
        [
            CustomProviderConformanceCase::Authoring,
            CustomProviderConformanceCase::GuardedApproval,
            CustomProviderConformanceCase::Design,
            CustomProviderConformanceCase::Planning,
            CustomProviderConformanceCase::ImplementationDiff,
            CustomProviderConformanceCase::Verification,
            CustomProviderConformanceCase::IndependentReview,
            CustomProviderConformanceCase::MalformedOutput,
            CustomProviderConformanceCase::Cancellation,
            CustomProviderConformanceCase::RestartRecovery,
            CustomProviderConformanceCase::ReadyNoDelivery,
        ]
        .into_iter()
        .map(|case| CustomProviderConformanceEvidence {
            case,
            evidence_sha256: super::super::sha256(format!("case:{case:?}")),
        })
        .collect()
    }

    #[test]
    fn fixture_copy_is_disposable_failing_and_zero_pollution() {
        let before = hash_source_fixture().unwrap();
        let fixture = prepare_fixture().unwrap();
        assert!(!fixture.repository.join(".agentum").exists());
        assert!(!fixture.repository.join(".claude").exists());
        assert!(
            fixture
                .repository
                .join("test/refresh-token.conformance.test.js")
                .is_file()
        );
        let output = std::process::Command::new("node")
            .arg("--test")
            .current_dir(&fixture.repository)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(hash_source_fixture().unwrap(), before);
    }

    #[test]
    fn report_validation_is_source_and_complete_case_bound() {
        let report = ProviderConformanceReport {
            schema_version: REPORT_SCHEMA_VERSION,
            suite: providers::CUSTOM_PROVIDER_CONFORMANCE_SUITE.into(),
            provider_id: "codex".into(),
            provider_version: "1.2.3".into(),
            source_revision: "source-1".into(),
            fixture_sha256: super::super::sha256(b"fixture"),
            approval_digest: super::super::sha256(b"approval"),
            profile: "standard".into(),
            control: "guarded".into(),
            terminal_phase: "ready".into(),
            delivery_performed: false,
            completed_at_unix: 1,
            cases: complete_evidence(),
        };
        let bundle = ProviderConformanceBundle {
            schema_version: REPORT_SCHEMA_VERSION,
            suite: providers::CUSTOM_PROVIDER_CONFORMANCE_SUITE.into(),
            source_revision: "source-1".into(),
            reports: vec![report],
        };
        assert!(validate_report(&bundle, "source-1", &["codex".into()]).is_ok());
        assert!(validate_report(&bundle, "source-2", &["codex".into()]).is_err());
        assert!(validate_report(&bundle, "source-1", &["claude".into()]).is_err());
        let mut incomplete = bundle;
        incomplete.reports[0].cases.pop();
        assert!(validate_report(&incomplete, "source-1", &["codex".into()]).is_err());
    }

    #[test]
    fn checkpoint_is_hash_bound() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("checkpoint.json");
        let checkpoint = RecoveryCheckpoint {
            schema_version: REPORT_SCHEMA_VERSION,
            provider_id: "codex".into(),
            spec_id: SpecId::new().to_string(),
            phase: "waiting_spec_approval".into(),
            approval_digest: super::super::sha256(b"approval"),
            source_revision: "source-1".into(),
        };
        let hash = artifacts::atomic_write(
            &path,
            &pretty_json(&checkpoint).unwrap(),
            Some(MISSING_HASH),
        )
        .unwrap();
        assert!(verify_checkpoint_file(&path, &hash).is_ok());
        assert!(verify_checkpoint_file(&path, &super::super::sha256(b"wrong")).is_err());
    }

    /// Provider sandboxes intentionally retain network egress for their model
    /// transports. This adversarial probe therefore uses the real loopback HTTP
    /// boundary: possession of the address alone must not grant a model the
    /// human approval or delivery capability.
    #[tokio::test]
    async fn provider_origin_http_cannot_approve_or_deliver() {
        const UI_TOKEN: &str = "test-only-provider-boundary-ui-capability";
        let directory = tempfile::tempdir().unwrap();
        let store = agentum_store::Store::open(&directory.path().join("boundary.sqlite"))
            .await
            .unwrap();
        let (bus, _) = tokio::sync::broadcast::channel::<agentum_core::Event>(16);
        let mut state = crate::AppState::new(store, bus);
        // Exercise the dangerous configuration as well: even `--no-auth`
        // cannot turn loopback provenance into a human SDD identity.
        state.no_auth = true;
        state.embedded_ui_token = Some(Arc::new(UI_TOKEN.into()));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                crate::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });

        let endpoint = format!("http://{address}/api/sdd/runs/provider-probe/commands");
        let client = reqwest::Client::new();
        for command in [
            serde_json::json!({
                "type": "decideApproval",
                "requestId": "provider-approval-probe",
                "expectedRevision": 0,
                "approvalId": "missing",
                "digest": "missing",
                "decision": "approve"
            }),
            serde_json::json!({
                "type": "previewDelivery",
                "requestId": "provider-preview-probe",
                "expectedRevision": 0,
                "actions": [{ "type": "commit", "message": "provider probe" }]
            }),
            serde_json::json!({
                "type": "confirmDelivery",
                "requestId": "provider-confirm-probe",
                "expectedRevision": 0,
                "previewToken": "missing",
                "actions": ["missing"]
            }),
        ] {
            let denied = client.post(&endpoint).json(&command).send().await.unwrap();
            assert_eq!(denied.status().as_u16(), 401);

            let guessed = client
                .post(&endpoint)
                .bearer_auth("provider-does-not-know-ui-capability")
                .json(&command)
                .send()
                .await
                .unwrap();
            assert_eq!(guessed.status().as_u16(), 401);

            // The Tauri-delivered capability crosses authentication, proving
            // that the denials above are the trust boundary rather than a dead
            // route. The intentionally absent run then returns 404.
            let ui = client
                .post(&endpoint)
                .bearer_auth(UI_TOKEN)
                .json(&command)
                .send()
                .await
                .unwrap();
            assert_eq!(ui.status().as_u16(), 404);
        }

        server.abort();
        let _ = server.await;
    }
}
