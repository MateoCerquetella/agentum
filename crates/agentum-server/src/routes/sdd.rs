//! `/api/sdd` — the sole authoritative specification workflow contract.

use std::path::{Path as FsPath, PathBuf};
use std::time::Duration;

use agentum_core::sdd::{
    ArtifactKind, ExternalReference, PlanArtifact, SpecId, WorkflowControl, WorkflowProfile,
    validate_relative_path,
};
use agentum_store::sdd::{
    ApprovalDecisionMutation, ArtifactMutation, DiscoveredSpecInput, ExternalSpecMutation,
    NewSddAggregate, NewSddCreateSaga, NewSddDiscoveredRun, NewSddExternalLink, NewSddImportJob,
    NewSddRunArtifact, NewSddRunCreateSaga, ReconcileDiscoveredSpecs, SddCreateSagaRecord,
    SddEventRecord, TransitionMutation,
};
use agentum_store::sdd_delivery::{ConfirmDelivery, NewDeliveryAction, NewDeliveryPreview};
use agentum_store::sdd_remote_projection::NewSddRemoteProjection;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Extension, Path, Query, State, WebSocketUpgrade};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthActor;
use crate::error::ApiError;
use crate::sdd::artifacts::{
    self, ArtifactError, MISSING_HASH, atomic_remove, atomic_write, content_hash,
    discover_manifest, discover_specs, initialize, read_bytes, read_text, render_spec,
    validate_existing_root,
};
use crate::sdd::delivery::{
    DeliveryActionRequest, DeliveryArtifactHash, DeliveryPreviewEnvelope, PreparedDeliveryAction,
    bind_openspec_exports, bind_tracker_mutations, prepare_actions, preview_digest, preview_token,
    validate_openspec_exports, validate_preview_token, validate_tracker_mutations,
    workspace_state_hash,
};
use crate::sdd::jira::JiraError;
use crate::sdd::providers::{
    BUNDLED_IDS, BundledProvider, ProviderAdapter, authoring_prompt, probe_custom_provider,
    probe_provider, provider_isolation_capability, resolve_provider, run_authoring,
};
use crate::sdd::remote::{
    REMOTE_SDD_SCHEMA_VERSION, RemoteAuthoringRequest, RemoteAuthoringResult,
    RemoteDeliverySnapshotRequest, RemoteDeliverySnapshotResult, RemoteLifecycleCheckpoint,
    RemoteLifecyclePlan, RemotePhaseStatus, RemoteSddAuthoringTransport, RemoteSddProbeTransport,
    RemoteSddTransport,
};
use crate::sdd::sha256;
use crate::sdd::sources::{
    NormalizedSource, SourceError, WorkItemSource, import_openspec, normalize_markdown_intake,
    normalize_work_item,
};
use crate::sdd::workspace::{self, WorkspaceError};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sdd/capabilities", get(capabilities))
        .route(
            "/api/sdd/repos/{repo_id}/remote-capability",
            get(remote_capability),
        )
        .route(
            "/api/sdd/integrations/jira/oauth/start",
            post(start_jira_oauth),
        )
        .route(
            "/api/sdd/integrations/jira/oauth/redeem",
            post(redeem_jira_oauth),
        )
        .route(
            "/api/sdd/integrations/jira/api-token/connect",
            post(connect_jira_api_token),
        )
        .route(
            "/api/sdd/integrations/jira/connections",
            get(jira_connections),
        )
        .route(
            "/api/sdd/integrations/jira/connections/{connection_id}/select-site",
            post(select_jira_site),
        )
        .route(
            "/api/sdd/repos/{repo_id}/specs",
            get(list_specs).post(create_spec),
        )
        .route(
            "/api/sdd/repos/{repo_id}/sources/preview",
            post(preview_source),
        )
        .route("/api/sdd/specs/{spec_id}", get(get_spec))
        .route("/api/sdd/specs/{spec_id}/runs", post(create_run))
        .route("/api/sdd/runs/{run_id}", get(get_run))
        .route("/api/sdd/runs/{run_id}/commands", post(command))
        .route("/api/sdd/runs/{run_id}/artifacts", get(get_artifacts))
        .route(
            "/api/sdd/runs/{run_id}/evidence/{evidence_id}/blobs/{sha256}",
            get(get_evidence_blob),
        )
        .route("/api/sdd/runs/{run_id}/events", get(get_events))
        .route("/api/sdd/events", get(events_socket))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteCapabilityQuery {
    provider: String,
    #[serde(default = "head")]
    base_ref: String,
}

fn reject_unavailable_local_provider_execution(provider: &str) -> Result<(), ApiError> {
    let capability = provider_isolation_capability();
    if capability.available {
        return Ok(());
    }
    Err(ApiError::Custom(
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({
            "error": "provider_capability_unavailable",
            "provider": provider,
            "reason": capability.reason_code,
            "message": capability.reason.unwrap_or("Agentum provider isolation is unavailable."),
            "localProviderExecution": capability
        }),
    ))
}

/// Repository-scoped remote capability is never inferred from the existence
/// of an SSH host. It is true only after the fixed subsystem answers a typed
/// protocol probe, confirms this repository registration, proves the selected
/// provider is currently usable, and reports the exact desktop version.
async fn remote_capability(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
    Query(query): Query<RemoteCapabilityQuery>,
) -> Result<Json<Value>, ApiError> {
    let provider = query.provider.trim();
    if provider.is_empty() || provider.len() > 160 {
        return Err(ApiError::BadRequest("provider is invalid".into()));
    }
    let Some(host_id) = crate::routes::repos::resolve_repo_host_id(&repo_id)? else {
        return Ok(Json(json!({
            "schemaVersion": 1,
            "available": false,
            "reason": "repository_is_local"
        })));
    };
    // Resolve the registry row as well so an unknown repo cannot be probed by
    // supplying a valid host id through a stale caller-side cache.
    let _ = crate::routes::repos::resolve_repo_path(&repo_id)?;
    let repository_identity_sha256 = sha256(repo_id.as_bytes());
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("host not found: {host_id}")))?;
    let transport = crate::sdd::remote_lifecycle::client_for_host(host)
        .map_err(|_| ApiError::BadRequest("remote SDD host is not a valid SSH host".into()))?;
    let probe = match RemoteSddProbeTransport::probe(
        transport.as_ref(),
        &repository_identity_sha256,
        provider,
        query.base_ref.trim(),
    )
    .await
    {
        Ok(probe) => probe,
        Err(error) => {
            tracing::warn!(
                repo_id,
                host_id = %host_id,
                error = %error,
                "remote SDD fixed-subsystem probe failed"
            );
            return Ok(Json(json!({
                "schemaVersion": 1,
                "available": false,
                "hostId": host_id,
                "repositoryIdentitySha256": repository_identity_sha256,
                "reason": "remote_subsystem_unavailable",
                "localFallback": false
            })));
        }
    };
    let version_matches = probe.worker_version == state.version;
    let worker_ready = probe.repository_registered
        && probe.provider_ready
        && probe.artifact_set_id.is_some()
        && probe.base_commit.is_some()
        && version_matches;
    let available = worker_ready;
    let reason = if !version_matches {
        Some("worker_version_mismatch")
    } else {
        probe.reason.as_deref()
    };
    Ok(Json(json!({
        "schemaVersion": 1,
        "available": available,
        "workerReady": worker_ready,
        "hostId": host_id,
        "repositoryIdentitySha256": repository_identity_sha256,
        "workerVersion": probe.worker_version,
        "repositoryRegistered": probe.repository_registered,
        "artifactSetId": probe.artifact_set_id,
        "baseCommit": probe.base_commit,
        "providerReady": probe.provider_ready,
        "projectionReady": true,
        "localFallback": false,
        "reason": reason
    })))
}

async fn capabilities(State(state): State<AppState>) -> Json<Value> {
    let local_provider_execution = provider_isolation_capability();
    let mut providers = Vec::with_capacity(BUNDLED_IDS.len());
    for id in BUNDLED_IDS {
        if let Some(provider) = BundledProvider::get(id) {
            providers.push(probe_provider(provider).await);
        }
    }
    let github_probe = probe_github_source().await;
    let github_available = github_probe.is_ok();
    let github_reason = github_probe.err();
    let linear_probe = probe_linear_source(&state).await;
    let (linear_available, linear_connection_id, linear_reason) = match linear_probe {
        Ok(connection_id) => (true, Some(connection_id), None),
        Err(reason) => (false, None, Some(reason)),
    };
    let jira_selected = match crate::sdd::jira::selected_connection(&state).await {
        Ok(Some(mut connection)) => {
            connection.delivery_write_authorized =
                crate::sdd::jira::delivery_write_authorized(&state, &connection.connection_id)
                    .await
                    .unwrap_or(false);
            Ok(Some(connection))
        }
        other => other,
    };
    let jira_broker = crate::sdd::jira::broker_configured();
    let jira_api_token_fallback = crate::sdd::jira::api_token_fallback_enabled();
    let (jira_available, jira_connection, jira_reason) = match jira_selected {
        Ok(Some(connection)) if !connection.selected_site_id.is_empty() => {
            (true, Some(connection), None)
        }
        Ok(Some(connection)) => (
            false,
            Some(connection),
            Some("Select one authorized Jira Cloud site before importing an issue".to_owned()),
        ),
        Ok(None) if !jira_broker && !jira_api_token_fallback => (
            false,
            None,
            Some("Jira OAuth broker is not configured and the advanced local API-token fallback is disabled".to_owned()),
        ),
        Ok(None) => (
            false,
            None,
            Some("Connect Jira Cloud before importing an issue".to_owned()),
        ),
        Err(_) => (
            false,
            None,
            Some("The secure credential vault is unavailable or locked".to_owned()),
        ),
    };
    let browser_evidence_available = crate::cdp_browser::local_browser_runtime_available();
    Json(json!({
        "schemaVersion": 1,
        "providers": providers,
        "providerAliases": { "cursor": "agent" },
        "localProviderExecution": local_provider_execution,
        "sources": [
            { "id": "description", "available": true },
            { "id": "socratic", "available": true },
            { "id": "markdown", "available": true, "preview": true },
            {
                "id": "github",
                "available": github_available,
                "preview": true,
                "reason": github_reason
            },
            {
                "id": "linear",
                "available": linear_available,
                "preview": true,
                "connectionId": linear_connection_id,
                "reason": linear_reason
            },
            {
                "id": "jira",
                "available": jira_available,
                "preview": true,
                "connection": jira_connection,
                "brokerConfigured": jira_broker,
                "apiTokenFallbackEnabled": jira_api_token_fallback,
                "reason": jira_reason
            },
            { "id": "openspec", "available": true, "preview": true }
        ],
        "remoteLifecycle": true,
        "remoteWorker": {
            "schemaVersion": 1,
            "protocol": "agentum-sdd-v1",
            "projectionReady": true,
            "automaticallyDeployed": false
        },
        "delivery": true,
        "readyLifecycle": true,
        "browserEvidence": {
            "available": browser_evidence_available,
            "reason": (!browser_evidence_available).then_some(
                "No supported local Chrome/Chromium executable is installed; typed command verification remains available."
            )
        }
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartJiraOauthBody {
    request_id: String,
    #[serde(default)]
    expected_revision: i64,
}

async fn start_jira_oauth(
    State(state): State<AppState>,
    Json(body): Json<StartJiraOauthBody>,
) -> Result<Json<Value>, ApiError> {
    validate_request_id(&body.request_id)?;
    if body.expected_revision != 0 {
        return Err(stale(0, body.expected_revision));
    }
    let started = crate::sdd::jira::start_oauth(&state, &body.request_id)
        .await
        .map_err(jira_error)?;
    Ok(Json(
        serde_json::to_value(started).map_err(|error| ApiError::Internal(error.to_string()))?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RedeemJiraOauthBody {
    request_id: String,
    flow_id: String,
    expected_revision: i64,
}

async fn redeem_jira_oauth(
    State(state): State<AppState>,
    Json(body): Json<RedeemJiraOauthBody>,
) -> Result<Json<Value>, ApiError> {
    validate_request_id(&body.request_id)?;
    let connection = crate::sdd::jira::redeem_oauth(&state, &body.flow_id, body.expected_revision)
        .await
        .map_err(jira_error)?;
    Ok(Json(json!({ "connection": connection })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectJiraApiTokenBody {
    request_id: String,
    email: String,
    api_token: String,
    site_url: String,
    acknowledge_risk: bool,
    #[serde(default)]
    expected_revision: i64,
}

async fn connect_jira_api_token(
    State(state): State<AppState>,
    Json(body): Json<ConnectJiraApiTokenBody>,
) -> Result<Json<Value>, ApiError> {
    validate_request_id(&body.request_id)?;
    let connection = crate::sdd::jira::connect_api_token(
        &state,
        &body.email,
        &body.api_token,
        &body.site_url,
        body.acknowledge_risk,
        body.expected_revision,
    )
    .await
    .map_err(jira_error)?;
    Ok(Json(json!({ "connection": connection })))
}

async fn jira_connections(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let connections = crate::sdd::jira::connections(&state)
        .await
        .map_err(jira_error)?;
    Ok(Json(json!({ "connections": connections })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SelectJiraSiteBody {
    request_id: String,
    site_id: String,
    expected_credential_revision: i64,
}

async fn select_jira_site(
    State(state): State<AppState>,
    Path(connection_id): Path<String>,
    Json(body): Json<SelectJiraSiteBody>,
) -> Result<Json<Value>, ApiError> {
    validate_request_id(&body.request_id)?;
    let connection = crate::sdd::jira::select_site(
        &state,
        &connection_id,
        &body.site_id,
        body.expected_credential_revision,
    )
    .await
    .map_err(jira_error)?;
    Ok(Json(json!({ "connection": connection })))
}

fn jira_error(error: JiraError) -> ApiError {
    match error {
        JiraError::BrokerUnavailable(message) => ApiError::Custom(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "error": "jira_broker_unavailable", "message": message }),
        ),
        JiraError::Vault => source_unavailable(
            "jira",
            "The secure credential vault is unavailable or locked",
        ),
        JiraError::ApiTokenDisabled => ApiError::Custom(
            StatusCode::FORBIDDEN,
            json!({ "error": "jira_api_token_disabled", "message": error.to_string() }),
        ),
        JiraError::ApiTokenRejected => ApiError::Custom(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "jira_api_token_rejected", "message": error.to_string() }),
        ),
        JiraError::Flow => ApiError::Custom(
            StatusCode::CONFLICT,
            json!({ "error": "jira_oauth_flow_invalid", "message": error.to_string() }),
        ),
        JiraError::Ambiguous => ApiError::Custom(
            StatusCode::CONFLICT,
            json!({ "error": "jira_oauth_sync_pending", "message": error.to_string() }),
        ),
        JiraError::BrokerResponse => ApiError::Custom(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "jira_broker_response_invalid", "message": error.to_string() }),
        ),
        JiraError::Connection => ApiError::Custom(
            StatusCode::CONFLICT,
            json!({ "error": "jira_connection_invalid", "message": error.to_string() }),
        ),
        JiraError::IssueKey => ApiError::BadRequest(error.to_string()),
        JiraError::IssueRead => source_unavailable(
            "jira",
            "Jira could not return that issue from the selected Cloud site",
        ),
        JiraError::Store(error) => error.into(),
    }
}

async fn probe_linear_source(state: &AppState) -> Result<String, String> {
    let vault = state.sdd_credentials.clone();
    tokio::task::spawn_blocking(move || {
        crate::sdd::credentials::get_linear_credential(vault.as_ref(), None)
    })
    .await
    .map_err(|_| "Linear credential-vault probe failed".to_owned())?
    .map_err(|_| "The secure credential vault is unavailable or locked".to_owned())?
    .map(|credential| credential.connection_id)
    .ok_or_else(|| {
        "Connect and select one Linear workspace in Settings; legacy plaintext credentials are refused"
            .to_owned()
    })
}

async fn probe_github_source() -> Result<(), String> {
    let program = std::env::var("AGENTUM_GH_BIN").unwrap_or_else(|_| "gh".into());
    let executable = which::which(program).map_err(|_| {
        "GitHub CLI is not installed; authenticated read-only import is unavailable".to_owned()
    })?;
    let mut command = tokio::process::Command::new(executable);
    command
        .args(["auth", "status", "--hostname", "github.com"])
        .env_clear()
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    for key in ["PATH", "HOME", "USERPROFILE", "GH_TOKEN", "GITHUB_TOKEN"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .map_err(|_| "GitHub authentication probe timed out".to_owned())?
        .map_err(|_| "GitHub authentication probe could not start".to_owned())?;
    if !output.status.success()
        || output.stdout.len().saturating_add(output.stderr.len()) > 64 * 1024
    {
        return Err(
            "GitHub CLI is installed but an authenticated github.com account was not verified"
                .into(),
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum CreateSpecSource {
    Socratic {
        context: String,
    },
    Markdown {
        markdown: String,
    },
    Github {
        url: String,
        #[serde(default, rename = "expectedSourceRevision")]
        expected_source_revision: Option<String>,
    },
    Linear {
        identifier: String,
        #[serde(default, rename = "connectionId")]
        connection_id: Option<String>,
        #[serde(default, rename = "expectedSourceRevision")]
        expected_source_revision: Option<String>,
    },
    Jira {
        #[serde(rename = "connectionId")]
        connection_id: String,
        #[serde(rename = "siteId")]
        site_id: String,
        key: String,
        #[serde(default, rename = "expectedSourceRevision")]
        expected_source_revision: Option<String>,
    },
    Openspec {
        path: String,
        #[serde(default, rename = "expectedSourceRevision")]
        expected_source_revision: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewSourceBody {
    title: String,
    source: CreateSpecSource,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredSourceReference<'a> {
    kind: &'a str,
    source_revision: &'a str,
    source_path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_reference: Option<&'a ExternalReference>,
}

#[derive(Debug, Clone)]
struct PreparedSource {
    normalized: Option<NormalizedSource>,
    authoring_context: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateSpecBody {
    request_id: String,
    #[serde(default)]
    expected_revision: i64,
    title: String,
    goal: String,
    #[serde(default = "standard")]
    profile: WorkflowProfile,
    #[serde(default = "guarded")]
    control: WorkflowControl,
    provider: String,
    #[serde(default = "head")]
    base_ref: String,
    #[serde(default)]
    source_checkout: SourceCheckout,
    /// Unit-test seam for exercising the persistence boundary without
    /// requiring a provider binary. Production request bodies cannot bypass
    /// provider authoring; serde simply ignores this field outside tests.
    #[cfg(test)]
    #[serde(default)]
    spec_markdown: Option<String>,
    #[serde(default)]
    source: Option<CreateSpecSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteCreatePublicationIntent {
    spec_id: String,
    spec_ulid: String,
    title: String,
    slug: String,
    profile: String,
    control: String,
    provider: String,
    base_ref: String,
    base_commit: String,
    branch_name: String,
    authoritative_path: String,
    attempt_id: String,
    attempt_path: String,
    approval_id: String,
    repository_identity_sha256: String,
    artifact_set_id: String,
    worker_version: String,
    source_checkout: String,
    source_ref_json: Option<String>,
    import_job: Option<RemoteCreateImportIntent>,
    initial_spec_content: String,
    initial_spec_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteCreateImportIntent {
    source_kind: String,
    source_hash: String,
    preview_json: String,
    disposition: String,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum SourceCheckout {
    #[default]
    RequireClean,
    CommittedBase,
    Snapshot,
}

impl From<SourceCheckout> for workspace::SourceCheckoutMode {
    fn from(value: SourceCheckout) -> Self {
        match value {
            SourceCheckout::RequireClean => Self::RequireClean,
            SourceCheckout::CommittedBase => Self::CommittedBase,
            SourceCheckout::Snapshot => Self::Snapshot,
        }
    }
}

fn standard() -> WorkflowProfile {
    WorkflowProfile::Standard
}
fn guarded() -> WorkflowControl {
    WorkflowControl::Guarded
}
fn head() -> String {
    "HEAD".into()
}

async fn preview_source(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
    Json(body): Json<PreviewSourceBody>,
) -> Result<Json<Value>, ApiError> {
    if crate::routes::repos::resolve_repo_host_id(&repo_id)?.is_some() {
        return Err(ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "error": "source_capability_unavailable",
                "source": "remote",
                "message": "source preview is unavailable until the sequential SSH SDD lifecycle is installed"
            }),
        ));
    }
    let repository = PathBuf::from(crate::routes::repos::resolve_repo_path(&repo_id)?);
    let prepared = prepare_source(
        &state,
        &repo_id,
        &repository,
        body.title.trim(),
        Some(&body.source),
        "",
    )
    .await?;
    let normalized = prepared.normalized.ok_or_else(|| {
        ApiError::BadRequest("description intake does not require a source preview".into())
    })?;
    let preview_digest = sha256(
        serde_json::to_vec(&json!({
            "repoId": repo_id,
            "kind": normalized.kind,
            "sourceRevision": normalized.source_revision,
            "sourcePath": normalized.source_path,
            "markdownHash": sha256(normalized.markdown.as_bytes()),
            "designHash": normalized.design.as_deref().map(|value| sha256(value.as_bytes())),
            "tasks": normalized.tasks,
            "diagnostics": normalized.diagnostics
        }))
        .map_err(|error| ApiError::Internal(error.to_string()))?,
    );
    Ok(Json(json!({
        "kind": normalized.kind,
        "title": normalized.title,
        "markdown": normalized.markdown,
        "sourceRevision": normalized.source_revision,
        "sourcePath": normalized.source_path,
        "externalReference": normalized.external_reference,
        "designAvailable": normalized.design.is_some(),
        "taskCount": normalized.tasks.len(),
        "diagnostics": normalized.diagnostics,
        "previewDigest": preview_digest
    })))
}

async fn prepare_source(
    state: &AppState,
    repo_id: &str,
    repository: &FsPath,
    title: &str,
    source: Option<&CreateSpecSource>,
    additional_goal: &str,
) -> Result<PreparedSource, ApiError> {
    let Some(source) = source else {
        let normalized = normalize_markdown_intake(title, additional_goal).map_err(source_error)?;
        return Ok(PreparedSource {
            normalized: None,
            authoring_context: normalized.markdown,
        });
    };

    let normalized = match source {
        CreateSpecSource::Socratic { context } => {
            let mut normalized = normalize_markdown_intake(title, context).map_err(source_error)?;
            normalized.kind = "socratic".into();
            normalized.source_path = "inline:socratic".into();
            normalized
        }
        CreateSpecSource::Markdown { markdown } => {
            normalize_markdown_intake(title, markdown).map_err(source_error)?
        }
        CreateSpecSource::Github {
            url,
            expected_source_revision,
        } => {
            let github_program = std::env::var("AGENTUM_GH_BIN").unwrap_or_else(|_| "gh".into());
            if which::which(github_program).is_err() {
                return Err(source_unavailable(
                    "github",
                    "GitHub CLI is not installed; authenticated read-only import is unavailable",
                ));
            }
            let (slug, number) = crate::task_sink::github_slug_and_number_from_issue_url(url)
                .ok_or_else(|| {
                    ApiError::BadRequest(
                        "GitHub source must be an HTTPS github.com issue URL".into(),
                    )
                })?;
            let workdir = repository.to_string_lossy();
            let fetched = tokio::time::timeout(
                Duration::from_secs(30),
                crate::routes::github::fetch_github_issue(
                    state,
                    Some(repo_id),
                    &workdir,
                    &number,
                    Some(&slug),
                ),
            )
            .await
            .map_err(|_| {
                source_unavailable("github", "GitHub source fetch timed out after 30 seconds")
            })??;
            let key = format!("{slug}#{number}");
            let normalized = normalize_work_item(WorkItemSource {
                provider: "github",
                connection_id: "gh-cli:github.com",
                site_id: None,
                external_id: &number,
                key: Some(&key),
                url: &fetched.url,
                source_revision: &fetched.updated_at,
                title: &fetched.title,
                body: &fetched.body,
            })
            .map_err(source_error)?;
            require_expected_source_revision(
                "github",
                expected_source_revision.as_deref(),
                &normalized.source_revision,
            )?;
            normalized
        }
        CreateSpecSource::Linear {
            identifier,
            connection_id,
            expected_source_revision,
        } => {
            let requested_connection = connection_id.clone();
            let vault = state.sdd_credentials.clone();
            let credential = tokio::task::spawn_blocking(move || {
                crate::sdd::credentials::get_linear_credential(
                    vault.as_ref(),
                    requested_connection.as_deref(),
                )
            })
            .await
            .map_err(|_| source_unavailable("linear", "Linear credential-vault read failed"))?
            .map_err(|_| {
                source_unavailable(
                    "linear",
                    "The secure credential vault is unavailable or locked",
                )
            })?
            .ok_or_else(|| {
                source_unavailable(
                    "linear",
                    "The selected Linear workspace has no credential in the secure vault",
                )
            })?;
            let fetched = crate::linear::fetch_issue_source_with_token(
                identifier,
                &credential.connection_id,
                credential.token(),
            )
            .await
            .map_err(|_| {
                source_unavailable(
                    "linear",
                    "Linear could not return that issue from the selected workspace",
                )
            })?;
            let normalized = normalize_work_item(WorkItemSource {
                provider: "linear",
                connection_id: &fetched.connection_id,
                site_id: None,
                external_id: &fetched.id,
                key: Some(&fetched.identifier),
                url: &fetched.url,
                source_revision: &fetched.updated_at,
                title: &fetched.title,
                body: &fetched.description,
            })
            .map_err(source_error)?;
            require_expected_source_revision(
                "linear",
                expected_source_revision.as_deref(),
                &normalized.source_revision,
            )?;
            normalized
        }
        CreateSpecSource::Jira {
            connection_id,
            site_id,
            key,
            expected_source_revision,
        } => {
            let fetched = crate::sdd::jira::fetch_issue(state, connection_id, site_id, key)
                .await
                .map_err(jira_error)?;
            let normalized = normalize_work_item(WorkItemSource {
                provider: "jira",
                connection_id: &fetched.connection_id,
                site_id: Some(&fetched.site_id),
                external_id: &fetched.id,
                key: Some(&fetched.key),
                url: &fetched.url,
                source_revision: &fetched.updated_at,
                title: &fetched.title,
                body: &fetched.description,
            })
            .map_err(source_error)?;
            require_expected_source_revision(
                "jira",
                expected_source_revision.as_deref(),
                &normalized.source_revision,
            )?;
            normalized
        }
        CreateSpecSource::Openspec {
            path,
            expected_source_revision,
        } => {
            let repository = repository.to_path_buf();
            let path = path.clone();
            let normalized =
                tokio::task::spawn_blocking(move || import_openspec(&repository, &path))
                    .await
                    .map_err(|error| {
                        ApiError::Internal(format!("OpenSpec importer failed: {error}"))
                    })?
                    .map_err(source_error)?;
            require_expected_source_revision(
                "openspec",
                expected_source_revision.as_deref(),
                &normalized.source_revision,
            )?;
            normalized
        }
    };

    let additional_goal = additional_goal.trim();
    let authoring_context =
        if additional_goal.is_empty() || additional_goal == normalized.markdown.trim() {
            normalized.markdown.clone()
        } else {
            format!(
                "{}\n## Additional Agentum authoring constraints\n\n{}\n",
                normalized.markdown.trim(),
                additional_goal
            )
        };
    Ok(PreparedSource {
        normalized: Some(normalized),
        authoring_context,
    })
}

fn require_expected_source_revision(
    source: &str,
    expected: Option<&str>,
    current: &str,
) -> Result<(), ApiError> {
    if expected.is_some_and(|expected| expected != current) {
        return Err(ApiError::Custom(
            StatusCode::CONFLICT,
            json!({
                "error": "source_revision_changed",
                "source": source,
                "expectedSourceRevision": expected,
                "currentSourceRevision": current,
                "message": "the source changed after preview; preview the current revision before creating the spec"
            }),
        ));
    }
    Ok(())
}

fn source_unavailable(source: &str, message: &str) -> ApiError {
    ApiError::Custom(
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({
            "error": "source_capability_unavailable",
            "source": source,
            "message": message
        }),
    )
}

fn source_error(error: SourceError) -> ApiError {
    let (status, code) = match &error {
        SourceError::UnsafeReference(_) | SourceError::Malformed(_) => {
            (StatusCode::BAD_REQUEST, "invalid_source")
        }
        SourceError::Unsupported(_) => (StatusCode::UNPROCESSABLE_ENTITY, "unsupported_source"),
        SourceError::TooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, "source_too_large"),
        SourceError::Changed(_) => (StatusCode::CONFLICT, "source_changed"),
        SourceError::Io(_) | SourceError::Artifact(_) => {
            (StatusCode::BAD_REQUEST, "source_read_failed")
        }
    };
    ApiError::Custom(
        status,
        json!({ "error": code, "message": error.to_string() }),
    )
}

async fn create_spec(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
    Json(body): Json<CreateSpecBody>,
) -> Result<Response, ApiError> {
    validate_request_id(&body.request_id)?;
    let request_hash = request_digest(&body)?;
    if body.expected_revision != 0 {
        return Err(stale(0, body.expected_revision));
    }
    let idempotency_scope = format!("repo:{repo_id}:create_spec");
    if let Some(response) = state
        .store
        .sdd_idempotent_response(&idempotency_scope, &body.request_id, &request_hash)
        .await?
    {
        return Ok((StatusCode::OK, Json(response)).into_response());
    }
    if let Some(saga) = state
        .store
        .sdd_create_saga(&repo_id, &body.request_id)
        .await?
    {
        if saga.request_hash != request_hash {
            return Err(agentum_store::StoreError::IdempotencyConflict(idempotency_scope).into());
        }
        return Err(ApiError::Conflict(format!(
            "spec creation {} is {}; use a new requestId to retry a failed creation",
            saga.run_id, saga.stage
        )));
    }
    if body.title.trim().is_empty() {
        return Err(ApiError::BadRequest("title is required".into()));
    }
    if body.provider.trim().is_empty() || body.provider.len() > 160 {
        return Err(ApiError::BadRequest("provider is invalid".into()));
    }
    // A remote repository is never allowed to fall through to the local
    // provider, source, Git, or filesystem path. Until remote results have a
    // transactional desktop projection, reject before resolving or probing
    // any client-local provider configuration.
    if let Some(host_id) = crate::routes::repos::resolve_repo_host_id(&repo_id)? {
        return create_remote_spec(&state, &repo_id, host_id, &body, &request_hash).await;
    }
    reject_unavailable_local_provider_execution(&body.provider)?;
    let provider = validate_provider(&body.provider)?;
    let provider_capability = match &provider {
        ProviderAdapter::Bundled(provider) => probe_provider(*provider).await,
        ProviderAdapter::Custom(_) => {
            probe_custom_provider(&body.provider)
                .await
                .map_err(|error| {
                    ApiError::Custom(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        json!({
                            "error": "provider_capability_unavailable",
                            "provider": body.provider,
                            "message": error.to_string()
                        }),
                    )
                })?
        }
    };
    if !provider_capability.available {
        return Err(ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "error": "provider_capability_unavailable",
                "provider": body.provider,
                "message": provider_capability.reason.unwrap_or_else(|| "provider is unavailable".into())
            }),
        ));
    }
    let repository = PathBuf::from(crate::routes::repos::resolve_repo_path(&repo_id)?);
    let title = body.title.trim();
    let prepared_source = prepare_source(
        &state,
        &repo_id,
        &repository,
        title,
        body.source.as_ref(),
        body.goal.trim(),
    )
    .await?;
    let spec_id = SpecId::new();
    let run_id = Uuid::new_v4().to_string();
    let approval_id = Uuid::new_v4().to_string();
    let attempt_id = Uuid::new_v4().to_string();
    let source_line = prepared_source.normalized.as_ref().map(|source| {
        format!(
            "{}:{}@{}",
            source.kind, source.source_path, source.source_revision
        )
    });
    let initial_goal = prepared_source
        .normalized
        .as_ref()
        .map(|source| {
            format!(
                "Author the immutable {} source snapshot {}.",
                source.kind, source.source_revision
            )
        })
        .unwrap_or_else(|| body.goal.trim().to_owned());
    let initial_content = render_spec(
        &spec_id,
        1,
        title,
        source_line.as_deref(),
        &initial_spec_body(title, &initial_goal),
    )
    .map_err(artifact_error)?;

    let workspace = workspace::plan_authoritative(
        &repo_id,
        &repository,
        &run_id,
        &spec_id,
        title,
        body.base_ref.trim(),
        body.source_checkout.into(),
    )
    .await
    .map_err(workspace_error)?;
    let attempt_path =
        workspace::attempt_path(&workspace.path, &attempt_id).map_err(workspace_error)?;
    let source_manifest = discover_manifest(&repository).map_err(artifact_error)?;
    let candidate_manifest = source_manifest.clone().unwrap_or_default();
    let candidate_artifact_set_id = candidate_manifest.artifact_set_id.to_string();
    let artifact_set_id = state
        .store
        .sdd_reserve_create(NewSddCreateSaga {
            repo_id: &repo_id,
            request_id: &body.request_id,
            request_hash: &request_hash,
            spec_id: &spec_id.to_string(),
            run_id: &run_id,
            repository_path: &repository.to_string_lossy(),
            authoritative_path: &workspace.path.to_string_lossy(),
            branch_name: &workspace.branch_name,
            attempt_id: &attempt_id,
            attempt_path: &attempt_path.to_string_lossy(),
            artifact_set_id: &candidate_artifact_set_id,
            artifact_set_required: source_manifest.is_some(),
        })
        .await?;
    let artifact_set_id = artifact_set_id.parse().map_err(|_| {
        ApiError::Internal("stored repository artifact-set identity is invalid".into())
    })?;
    if let Err(error) = workspace::materialize_authoritative(&repository, &workspace).await {
        abort_create(
            &state,
            &repo_id,
            &body.request_id,
            &request_hash,
            &repository,
            &workspace,
            None,
            &error.to_string(),
        )
        .await;
        return Err(workspace_error(error));
    }
    if let Err(error) = state
        .store
        .sdd_update_create_stage(
            &repo_id,
            &body.request_id,
            &request_hash,
            &["reserved"],
            "workspace_ready",
            None,
        )
        .await
    {
        abort_create(
            &state,
            &repo_id,
            &body.request_id,
            &request_hash,
            &repository,
            &workspace,
            None,
            &error.to_string(),
        )
        .await;
        return Err(error.into());
    }
    let artifact_root = match initialize(&workspace.path, &spec_id, title, artifact_set_id) {
        Ok(value) => value,
        Err(error) => {
            abort_create(
                &state,
                &repo_id,
                &body.request_id,
                &request_hash,
                &repository,
                &workspace,
                None,
                &error.to_string(),
            )
            .await;
            return Err(artifact_error(error));
        }
    };
    let spec_path = artifact_root.spec_dir.join("spec.md");
    let initial_hash =
        match atomic_write(&spec_path, initial_content.as_bytes(), Some(MISSING_HASH)) {
            Ok(value) => value,
            Err(error) => {
                abort_create(
                    &state,
                    &repo_id,
                    &body.request_id,
                    &request_hash,
                    &repository,
                    &workspace,
                    None,
                    &error.to_string(),
                )
                .await;
                return Err(artifact_error(error));
            }
        };
    let attempt = match workspace::create_attempt(
        &repository,
        &workspace.path,
        &attempt_id,
        &workspace.base_commit,
        workspace.snapshot_digest.as_deref(),
    )
    .await
    {
        Ok(attempt) => attempt,
        Err(error) => {
            abort_create(
                &state,
                &repo_id,
                &body.request_id,
                &request_hash,
                &repository,
                &workspace,
                Some(&attempt_path),
                &error.to_string(),
            )
            .await;
            return Err(workspace_error(error));
        }
    };
    let attempt_root = match initialize(&attempt.path, &spec_id, title, artifact_set_id) {
        Ok(root) => root,
        Err(error) => {
            abort_create(
                &state,
                &repo_id,
                &body.request_id,
                &request_hash,
                &repository,
                &workspace,
                Some(&attempt.path),
                &error.to_string(),
            )
            .await;
            return Err(artifact_error(error));
        }
    };
    if let Err(error) = atomic_write(
        &attempt_root.spec_dir.join("spec.md"),
        initial_content.as_bytes(),
        Some(MISSING_HASH),
    ) {
        abort_create(
            &state,
            &repo_id,
            &body.request_id,
            &request_hash,
            &repository,
            &workspace,
            Some(&attempt.path),
            &error.to_string(),
        )
        .await;
        return Err(artifact_error(error));
    }
    let staging_dir = attempt_root.root.join("staging");
    if let Err(error) = std::fs::create_dir(&staging_dir) {
        abort_create(
            &state,
            &repo_id,
            &body.request_id,
            &request_hash,
            &repository,
            &workspace,
            Some(&attempt.path),
            &error.to_string(),
        )
        .await;
        return Err(ApiError::Internal(error.to_string()));
    }
    let staging_path = staging_dir.join("spec-output.md");
    if let Err(error) = state
        .store
        .sdd_update_create_stage(
            &repo_id,
            &body.request_id,
            &request_hash,
            &["workspace_ready"],
            "authoring",
            None,
        )
        .await
    {
        abort_create(
            &state,
            &repo_id,
            &body.request_id,
            &request_hash,
            &repository,
            &workspace,
            Some(&attempt.path),
            &error.to_string(),
        )
        .await;
        return Err(error.into());
    }
    #[cfg(test)]
    let injected_spec = body
        .spec_markdown
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    #[cfg(not(test))]
    let injected_spec: Option<&str> = None;
    let authored_body = if let Some(submitted) = injected_spec {
        submitted.to_owned()
    } else {
        match run_authoring(
            &run_id,
            &provider,
            &attempt.path.to_string_lossy(),
            &authoring_prompt(title, &prepared_source.authoring_context),
            &staging_path.to_string_lossy(),
        )
        .await
        {
            Ok(content) => content,
            Err(error) => {
                abort_create(
                    &state,
                    &repo_id,
                    &body.request_id,
                    &request_hash,
                    &repository,
                    &workspace,
                    Some(&attempt.path),
                    &error.to_string(),
                )
                .await;
                return Err(ApiError::BadRequest(format!(
                    "{} authoring failed: {error}",
                    body.provider
                )));
            }
        }
    };
    if let Err(error) = workspace::remove_attempt(&repository, &attempt.path).await {
        abort_create(
            &state,
            &repo_id,
            &body.request_id,
            &request_hash,
            &repository,
            &workspace,
            Some(&attempt.path),
            &format!("authoring attempt cleanup failed: {error}"),
        )
        .await;
        return Err(ApiError::Internal(
            "authoring attempt cleanup failed; creation was quarantined for recovery".into(),
        ));
    }
    if let Err(error) = state
        .store
        .sdd_update_create_stage(
            &repo_id,
            &body.request_id,
            &request_hash,
            &["authoring"],
            "publishing",
            None,
        )
        .await
    {
        abort_create(
            &state,
            &repo_id,
            &body.request_id,
            &request_hash,
            &repository,
            &workspace,
            None,
            &error.to_string(),
        )
        .await;
        return Err(error.into());
    }
    let spec_content = match render_spec(&spec_id, 2, title, source_line.as_deref(), &authored_body)
    {
        Ok(content) => content,
        Err(error) => {
            abort_create(
                &state,
                &repo_id,
                &body.request_id,
                &request_hash,
                &repository,
                &workspace,
                None,
                &error.to_string(),
            )
            .await;
            return Err(artifact_error(error));
        }
    };
    let spec_hash = match atomic_write(&spec_path, spec_content.as_bytes(), Some(&initial_hash)) {
        Ok(hash) => hash,
        Err(error) => {
            abort_create(
                &state,
                &repo_id,
                &body.request_id,
                &request_hash,
                &repository,
                &workspace,
                None,
                &error.to_string(),
            )
            .await;
            return Err(artifact_error(error));
        }
    };
    let profile = profile_name(body.profile);
    let control = control_name(body.control);
    let policy = json!({
        "profile": profile,
        "control": control,
        "deliveryRequired": true,
        "implementationEnabled": true,
        "sourceCheckout": match body.source_checkout {
            SourceCheckout::RequireClean => "require_clean",
            SourceCheckout::CommittedBase => "committed_base",
            SourceCheckout::Snapshot => "snapshot",
        },
        "sourceSnapshotDigest": workspace.snapshot_digest.as_deref(),
        "provider": provider.approval_binding()
    });
    let digest = approval_digest(
        &spec_id,
        2,
        &[(&artifact_root.spec_relative_path, &spec_hash)],
        &policy,
        &workspace.fingerprint,
    );
    let submitted_by = format!("agent:{}:{}", body.provider, attempt_id);
    let attempt_path_string = attempt.path.to_string_lossy().into_owned();
    let attempt_session_identity = format!("provider:{}:{}", body.provider, attempt_id);
    let response = json!({
        "specId": spec_id,
        "runId": run_id,
        "revision": 1,
        "specRevision": 2,
        "phase": "specification",
        "status": "waiting",
        "nextAction": "Spec approval required",
        "artifactSetId": artifact_root.manifest.artifact_set_id,
        "authoritativePath": workspace.path,
        "approval": {
            "approvalId": approval_id,
            "purpose": "specification",
            "digest": digest,
            "status": "pending"
        }
    });
    let response_json =
        serde_json::to_string(&response).map_err(|error| ApiError::Internal(error.to_string()))?;
    let source_json = prepared_source
        .normalized
        .as_ref()
        .map(|source| {
            serde_json::to_string(&StoredSourceReference {
                kind: &source.kind,
                source_revision: &source.source_revision,
                source_path: &source.source_path,
                external_reference: source.external_reference.as_ref(),
            })
        })
        .transpose()
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let external_link = prepared_source
        .normalized
        .as_ref()
        .and_then(|source| source.external_reference.as_ref())
        .map(|reference| NewSddExternalLink {
            provider: &reference.provider,
            connection_id: &reference.connection_id,
            site_id: reference.site_id.as_deref(),
            external_id: &reference.external_id,
            key: reference.key.as_deref(),
            url: &reference.url,
            source_revision: &reference.source_revision,
        });
    let import_preview_json = prepared_source
        .normalized
        .as_ref()
        .filter(|source| matches!(source.kind.as_str(), "markdown" | "openspec" | "socratic"))
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let import_job = prepared_source
        .normalized
        .as_ref()
        .zip(import_preview_json.as_deref())
        .filter(|(source, _)| matches!(source.kind.as_str(), "markdown" | "openspec" | "socratic"))
        .map(|(source, preview_json)| NewSddImportJob {
            source_kind: &source.kind,
            source_hash: &source.source_revision,
            preview_json,
            disposition: "imported_revision",
        });
    let directory_name = spec_id.directory_name(title);
    let create_result = state
        .store
        .sdd_create_aggregate(NewSddAggregate {
            request_id: &body.request_id,
            request_hash: &request_hash,
            spec_id: &spec_id.to_string(),
            spec_ulid: spec_id.ulid(),
            repo_id: &repo_id,
            title,
            slug: &directory_name,
            profile,
            control,
            provider: &body.provider,
            source_ref_json: source_json.as_deref(),
            external_link,
            import_job,
            initial_spec_content: &initial_content,
            initial_spec_hash: &initial_hash,
            spec_content: &spec_content,
            spec_hash: &spec_hash,
            spec_revision: 2,
            submitted_by: &submitted_by,
            attempt_id: &attempt_id,
            attempt_path: &attempt_path_string,
            attempt_session_identity: &attempt_session_identity,
            run_id: &run_id,
            base_ref: body.base_ref.trim(),
            base_commit: &workspace.base_commit,
            branch_name: &workspace.branch_name,
            authoritative_path: &workspace.path.to_string_lossy(),
            workspace_fingerprint: &workspace.fingerprint,
            policy_json: &policy.to_string(),
            approval_id: &approval_id,
            approval_digest: &digest,
            response_json: &response_json,
            remote_projection: None,
        })
        .await;
    if let Err(error) = create_result {
        abort_create(
            &state,
            &repo_id,
            &body.request_id,
            &request_hash,
            &repository,
            &workspace,
            None,
            &error.to_string(),
        )
        .await;
        return Err(error.into());
    }
    Ok((StatusCode::CREATED, Json(response)).into_response())
}

async fn create_remote_spec(
    state: &AppState,
    repo_id: &str,
    host_id: Uuid,
    body: &CreateSpecBody,
    request_hash: &str,
) -> Result<Response, ApiError> {
    if body.base_ref.trim().is_empty() {
        return Err(ApiError::BadRequest("baseRef is required".into()));
    }
    if matches!(body.source_checkout, SourceCheckout::Snapshot) {
        return Err(ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "error": "remote_snapshot_unavailable",
                "message": "Remote SDD accepts require_clean or committed_base; snapshot overlays are not transferred.",
                "localFallback": false
            }),
        ));
    }
    let title = body.title.trim();
    let (normalized_source, authoring_context) = match body.source.as_ref() {
        None => {
            let normalized =
                normalize_markdown_intake(title, body.goal.trim()).map_err(source_error)?;
            (None, normalized.markdown)
        }
        Some(CreateSpecSource::Socratic { context }) => {
            let mut normalized = normalize_markdown_intake(title, context).map_err(source_error)?;
            normalized.kind = "socratic".into();
            normalized.source_path = "inline:socratic".into();
            let context = append_remote_goal(&normalized.markdown, body.goal.trim());
            (Some(normalized), context)
        }
        Some(CreateSpecSource::Markdown { markdown }) => {
            let normalized = normalize_markdown_intake(title, markdown).map_err(source_error)?;
            let context = append_remote_goal(&normalized.markdown, body.goal.trim());
            (Some(normalized), context)
        }
        Some(source) => {
            let kind = match source {
                CreateSpecSource::Github { .. } => "github",
                CreateSpecSource::Linear { .. } => "linear",
                CreateSpecSource::Jira { .. } => "jira",
                CreateSpecSource::Openspec { .. } => "openspec",
                CreateSpecSource::Socratic { .. } | CreateSpecSource::Markdown { .. } => {
                    unreachable!("handled above")
                }
            };
            return Err(ApiError::Custom(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({
                    "error": "remote_source_capability_unavailable",
                    "source": kind,
                    "message": "This source requires an explicit remote source adapter; it will not be fetched on the desktop for a remote repository.",
                    "localFallback": false
                }),
            ));
        }
    };
    if authoring_context.trim().is_empty() || authoring_context.len() > 32 * 1024 {
        return Err(ApiError::BadRequest(
            "remote authoring context must be between 1 byte and 32 KiB".into(),
        ));
    }

    let repository_identity_sha256 = sha256(repo_id.as_bytes());
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("host not found: {host_id}")))?;
    let client = crate::sdd::remote_lifecycle::client_for_host(host)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let probe = RemoteSddProbeTransport::probe(
        client.as_ref(),
        &repository_identity_sha256,
        body.provider.trim(),
        body.base_ref.trim(),
    )
    .await
    .map_err(|error| {
        ApiError::Custom(
            StatusCode::BAD_GATEWAY,
            json!({
                "error": "remote_subsystem_unavailable",
                "message": error.to_string(),
                "localFallback": false
            }),
        )
    })?;
    let artifact_set_id = probe
        .artifact_set_id
        .clone()
        .filter(|value| value.len() == 26 && value.parse::<ulid::Ulid>().is_ok());
    let base_commit = probe.base_commit.clone().filter(|value| {
        matches!(value.len(), 40 | 64)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if probe.worker_version != state.version
        || !probe.repository_registered
        || !probe.provider_ready
        || artifact_set_id.is_none()
        || base_commit.is_none()
        || probe.reason.is_some()
    {
        return Err(ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "error": "remote_capability_unavailable",
                "workerVersion": probe.worker_version,
                "repositoryRegistered": probe.repository_registered,
                "providerReady": probe.provider_ready,
                "reason": probe.reason,
                "localFallback": false
            }),
        ));
    }
    let artifact_set_id = artifact_set_id.expect("checked above");
    let base_commit = base_commit.expect("checked above");
    let spec_id = SpecId::new();
    let spec_id_string = spec_id.to_string();
    let directory_name = spec_id.directory_name(title);
    let run_id = Uuid::new_v4().to_string();
    let approval_id = Uuid::new_v4().to_string();
    let attempt_id = Uuid::new_v4().to_string();
    let branch_name = format!("agentum/{}", directory_name);
    let authoritative_path = format!("agentum+ssh://{host_id}/{run_id}/authoritative");
    let attempt_path = format!("agentum+ssh://{host_id}/{run_id}/attempts/{attempt_id}");
    state
        .store
        .sdd_reserve_create(NewSddCreateSaga {
            repo_id,
            request_id: &body.request_id,
            request_hash,
            spec_id: &spec_id_string,
            run_id: &run_id,
            repository_path: &format!(
                "agentum+ssh://{host_id}/repository/{repository_identity_sha256}"
            ),
            authoritative_path: &authoritative_path,
            branch_name: &branch_name,
            attempt_id: &attempt_id,
            attempt_path: &attempt_path,
            artifact_set_id: &artifact_set_id,
            artifact_set_required: true,
        })
        .await?;
    let source_checkout = match body.source_checkout {
        SourceCheckout::RequireClean => "require_clean",
        SourceCheckout::CommittedBase => "committed_base",
        SourceCheckout::Snapshot => unreachable!("rejected above"),
    };
    let author_material = serde_json::json!({
        "runId": run_id,
        "specId": spec_id_string,
        "artifactSetId": artifact_set_id,
        "baseCommit": base_commit,
        "provider": body.provider,
        "title": title,
        "goal": authoring_context,
        "sourceCheckout": source_checkout,
    });
    let author_request_id = format!(
        "author-{}",
        &sha256(serde_json::to_vec(&author_material).expect("author request serializes"))[..32]
    );
    let author_request = RemoteAuthoringRequest {
        schema_version: REMOTE_SDD_SCHEMA_VERSION,
        request_id: author_request_id,
        host_id: host_id.to_string(),
        run_id: run_id.clone(),
        spec_id: spec_id_string.clone(),
        repository_identity_sha256: repository_identity_sha256.clone(),
        artifact_set_id: artifact_set_id.clone(),
        base_commit: base_commit.clone(),
        provider: body.provider.trim().to_owned(),
        source_checkout: source_checkout.into(),
        title: title.to_owned(),
        goal: authoring_context.clone(),
        timeout_ms: 15 * 60 * 1000,
        output_limit: 8 * 1024 * 1024,
    };
    let initial_content = render_spec(
        &spec_id,
        1,
        title,
        None,
        &format!(
            "# {title}\n\n## Requirements\n\n- RQ-001: {}\n\n## Acceptance criteria\n\n- AC-001: The authored specification defines verifiable completion conditions.",
            authoring_context.trim()
        ),
    )
    .map_err(artifact_error)?;
    let source_ref_json = normalized_source
        .as_ref()
        .map(|source| {
            serde_json::to_string(&StoredSourceReference {
                kind: &source.kind,
                source_revision: &source.source_revision,
                source_path: &source.source_path,
                external_reference: None,
            })
        })
        .transpose()
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let import_job = normalized_source
        .as_ref()
        .map(|source| {
            serde_json::to_string(source).map(|preview_json| RemoteCreateImportIntent {
                source_kind: source.kind.clone(),
                source_hash: source.source_revision.clone(),
                preview_json,
                disposition: "imported_revision".into(),
            })
        })
        .transpose()
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let publication_intent = RemoteCreatePublicationIntent {
        spec_id: spec_id_string.clone(),
        spec_ulid: spec_id.ulid().into(),
        title: title.into(),
        slug: directory_name.clone(),
        profile: profile_name(body.profile).into(),
        control: control_name(body.control).into(),
        provider: body.provider.trim().into(),
        base_ref: body.base_ref.trim().into(),
        base_commit: base_commit.clone(),
        branch_name: branch_name.clone(),
        authoritative_path: authoritative_path.clone(),
        attempt_id: attempt_id.clone(),
        attempt_path: attempt_path.clone(),
        approval_id: approval_id.clone(),
        repository_identity_sha256: repository_identity_sha256.clone(),
        artifact_set_id: artifact_set_id.clone(),
        worker_version: probe.worker_version.clone(),
        source_checkout: source_checkout.into(),
        source_ref_json,
        import_job,
        initial_spec_hash: sha256(initial_content.as_bytes()),
        initial_spec_content: initial_content,
    };
    let author_request_json = serde_json::to_string(&author_request)
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let publication_intent_json = serde_json::to_string(&publication_intent)
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    state
        .store
        .sdd_prepare_remote_create(
            repo_id,
            &body.request_id,
            request_hash,
            &host_id.to_string(),
            &author_request_json,
            &publication_intent_json,
        )
        .await?;
    let authored =
        match RemoteSddAuthoringTransport::author(client.as_ref(), author_request.clone()).await {
            Ok(result) => result,
            Err(error) => {
                mark_create_failed(
                    state,
                    repo_id,
                    &body.request_id,
                    request_hash,
                    &error.to_string(),
                )
                .await;
                return Err(ApiError::Custom(
                    StatusCode::BAD_GATEWAY,
                    json!({
                        "error": "remote_authoring_failed",
                        "message": error.to_string(),
                        "localFallback": false
                    }),
                ));
            }
        };
    let author_result_json =
        serde_json::to_string(&authored).map_err(|error| ApiError::Internal(error.to_string()))?;
    state
        .store
        .sdd_record_remote_authoring_result(
            repo_id,
            &body.request_id,
            request_hash,
            &author_result_json,
        )
        .await?;
    publish_remote_authoring_result(
        state,
        repo_id,
        &body.request_id,
        request_hash,
        &author_request,
        &publication_intent,
        &authored,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn publish_remote_authoring_result(
    state: &AppState,
    repo_id: &str,
    request_id: &str,
    request_hash: &str,
    author_request: &RemoteAuthoringRequest,
    intent: &RemoteCreatePublicationIntent,
    authored: &RemoteAuthoringResult,
) -> Result<Response, ApiError> {
    let spec_id = match intent.spec_id.parse::<SpecId>() {
        Ok(spec_id) => spec_id,
        Err(error) => {
            return Err(fail_remote_publication(
                state,
                repo_id,
                request_id,
                request_hash,
                &error.to_string(),
            )
            .await);
        }
    };
    let expected_relative_path = format!(".agentum/specs/{}/spec.md", intent.slug);
    let expected_authoritative_path = format!(
        "agentum+ssh://{}/{}/authoritative",
        author_request.host_id, author_request.run_id
    );
    let expected_attempt_path = format!(
        "agentum+ssh://{}/{}/attempts/{}",
        author_request.host_id, author_request.run_id, intent.attempt_id
    );
    let expected_branch = format!("agentum/{}", intent.slug);
    let structurally_valid = author_request.schema_version == REMOTE_SDD_SCHEMA_VERSION
        && authored.schema_version == REMOTE_SDD_SCHEMA_VERSION
        && authored.request_id == author_request.request_id
        && authored.run_id == author_request.run_id
        && authored.spec_id == author_request.spec_id
        && authored.spec_revision == 2
        && authored.status == RemotePhaseStatus::Succeeded
        && authored.error_code.is_none()
        && author_request.spec_id == intent.spec_id
        && intent.spec_ulid == spec_id.ulid()
        && intent.slug == spec_id.directory_name(&intent.title)
        && intent.authoritative_path == expected_authoritative_path
        && intent.attempt_path == expected_attempt_path
        && intent.branch_name == expected_branch
        && author_request.repository_identity_sha256 == intent.repository_identity_sha256
        && author_request.artifact_set_id == intent.artifact_set_id
        && author_request.base_commit == intent.base_commit
        && author_request.provider == intent.provider
        && author_request.source_checkout == intent.source_checkout
        && author_request.title == intent.title
        && author_request.timeout_ms == 15 * 60 * 1000
        && author_request.output_limit == 8 * 1024 * 1024
        && intent.worker_version == state.version
        && intent.repository_identity_sha256 == sha256(repo_id.as_bytes())
        && intent.artifact_set_id.len() == 26
        && intent.artifact_set_id.parse::<ulid::Ulid>().is_ok()
        && matches!(intent.profile.as_str(), "standard" | "high_risk")
        && matches!(
            intent.control.as_str(),
            "guarded" | "interactive" | "autopilot"
        )
        && matches!(
            intent.source_checkout.as_str(),
            "require_clean" | "committed_base"
        )
        && is_lower_sha256(&intent.repository_identity_sha256)
        && is_lower_sha256(&intent.initial_spec_hash)
        && is_git_object_id(&intent.base_commit)
        && is_lower_sha256(&authored.workspace_state_sha256)
        && is_lower_sha256(&authored.artifact_set_sha256)
        && sha256(intent.initial_spec_content.as_bytes()) == intent.initial_spec_hash;
    if !structurally_valid {
        return Err(fail_remote_publication(
            state,
            repo_id,
            request_id,
            request_hash,
            "remote authoring identity or publication intent failed validation",
        )
        .await);
    }
    let Some(spec_payload) = authored.spec.as_ref() else {
        let code = authored
            .error_code
            .as_deref()
            .unwrap_or("remote_authoring_failed");
        return Err(fail_remote_publication(state, repo_id, request_id, request_hash, code).await);
    };
    let initial_header = artifacts::parse_spec(&intent.initial_spec_content)
        .map(|(header, _)| header)
        .map_err(artifact_error);
    let authored_header = artifacts::parse_spec(&spec_payload.content)
        .map(|(header, _)| header)
        .map_err(artifact_error);
    let (initial_header, authored_header) = match (initial_header, authored_header) {
        (Ok(initial), Ok(authored)) => (initial, authored),
        (Err(error), _) | (_, Err(error)) => {
            mark_create_failed(state, repo_id, request_id, request_hash, &error.to_string()).await;
            return Err(error);
        }
    };
    if spec_payload.kind != "specification"
        || spec_payload.relative_path != expected_relative_path
        || spec_payload.content.is_empty()
        || spec_payload.content.len() > 8 * 1024 * 1024
        || !is_lower_sha256(&spec_payload.content_sha256)
        || sha256(spec_payload.content.as_bytes()) != spec_payload.content_sha256
        || initial_header.id != spec_id
        || initial_header.revision != 1
        || initial_header.title != intent.title
        || authored_header.id != spec_id
        || authored_header.revision != 2
        || authored_header.title != intent.title
    {
        return Err(fail_remote_publication(
            state,
            repo_id,
            request_id,
            request_hash,
            "remote authored specification failed validation",
        )
        .await);
    }
    if intent
        .source_ref_json
        .as_deref()
        .is_some_and(|raw| serde_json::from_str::<serde_json::Value>(raw).is_err())
        || intent.import_job.as_ref().is_some_and(|job| {
            !is_lower_sha256(&job.source_hash)
                || serde_json::from_str::<serde_json::Value>(&job.preview_json).is_err()
                || job.disposition != "imported_revision"
        })
    {
        return Err(fail_remote_publication(
            state,
            repo_id,
            request_id,
            request_hash,
            "remote source publication intent failed validation",
        )
        .await);
    }

    let policy = json!({
        "profile": intent.profile,
        "control": intent.control,
        "deliveryRequired": true,
        "implementationEnabled": true,
        "sourceCheckout": intent.source_checkout,
        "provider": {
            "id": intent.provider,
            "version": intent.worker_version,
            "transport": "agentum-sdd-v1",
            "resultTransport": "typed_remote",
            "isolation": "external_remote_worktree",
            "cancellation": "fixed_subsystem_process_tree",
            "timeoutMs": 15 * 60 * 1000,
            "outputLimit": 8 * 1024 * 1024
        }
    });
    let workspace_fingerprint = sha256(
        serde_json::to_vec(&json!({
            "hostId": author_request.host_id,
            "repositoryIdentitySha256": intent.repository_identity_sha256,
            "artifactSetId": intent.artifact_set_id,
            "baseCommit": intent.base_commit,
            "workspaceStateSha256": authored.workspace_state_sha256,
            "artifactSetSha256": authored.artifact_set_sha256,
        }))
        .expect("workspace fingerprint serializes"),
    );
    let digest = approval_digest(
        &spec_id,
        2,
        &[(&expected_relative_path, &spec_payload.content_sha256)],
        &policy,
        &workspace_fingerprint,
    );
    let plan = RemoteLifecyclePlan {
        schema_version: REMOTE_SDD_SCHEMA_VERSION,
        host_id: author_request.host_id.clone(),
        run_id: author_request.run_id.clone(),
        spec_id: intent.spec_id.clone(),
        spec_revision: 2,
        repository_identity_sha256: intent.repository_identity_sha256.clone(),
        artifact_set_id: intent.artifact_set_id.clone(),
        base_commit: intent.base_commit.clone(),
        provider: intent.provider.clone(),
        approval_digest: digest.clone(),
        timeout_ms: 15 * 60 * 1000,
        output_limit: 8 * 1024 * 1024,
    };
    let checkpoint =
        RemoteLifecycleCheckpoint::initial(&plan, authored.workspace_state_sha256.clone())
            .map_err(|error| ApiError::Internal(error.to_string()))?;
    let plan_json =
        serde_json::to_string(&plan).map_err(|error| ApiError::Internal(error.to_string()))?;
    let checkpoint_json = serde_json::to_string(&checkpoint)
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let response = json!({
        "specId": spec_id,
        "runId": author_request.run_id,
        "revision": 1,
        "specRevision": 2,
        "phase": "specification",
        "status": "waiting",
        "nextAction": "Spec approval required",
        "artifactSetId": intent.artifact_set_id,
        "authoritativePath": intent.authoritative_path,
        "remote": {
            "hostId": author_request.host_id,
            "workerVersion": intent.worker_version,
            "baseCommit": intent.base_commit,
            "sequential": true,
            "localFallback": false
        },
        "approval": {
            "approvalId": intent.approval_id,
            "purpose": "specification",
            "digest": digest,
            "status": "pending"
        }
    });
    let response_json = response.to_string();
    let import_job = intent.import_job.as_ref().map(|job| NewSddImportJob {
        source_kind: &job.source_kind,
        source_hash: &job.source_hash,
        preview_json: &job.preview_json,
        disposition: &job.disposition,
    });
    let submitted_by = format!("remote-agent:{}:{}", intent.provider, intent.attempt_id);
    let session_identity = format!("remote:authoring:{}", intent.attempt_id);
    let policy_json = policy.to_string();
    state
        .store
        .sdd_create_aggregate(NewSddAggregate {
            request_id,
            request_hash,
            spec_id: &intent.spec_id,
            spec_ulid: &intent.spec_ulid,
            repo_id,
            title: &intent.title,
            slug: &intent.slug,
            profile: &intent.profile,
            control: &intent.control,
            provider: &intent.provider,
            source_ref_json: intent.source_ref_json.as_deref(),
            external_link: None,
            import_job,
            initial_spec_content: &intent.initial_spec_content,
            initial_spec_hash: &intent.initial_spec_hash,
            spec_content: &spec_payload.content,
            spec_hash: &spec_payload.content_sha256,
            spec_revision: 2,
            submitted_by: &submitted_by,
            attempt_id: &intent.attempt_id,
            attempt_path: &intent.attempt_path,
            attempt_session_identity: &session_identity,
            run_id: &author_request.run_id,
            base_ref: &intent.base_ref,
            base_commit: &intent.base_commit,
            branch_name: &intent.branch_name,
            authoritative_path: &intent.authoritative_path,
            workspace_fingerprint: &workspace_fingerprint,
            policy_json: &policy_json,
            approval_id: &intent.approval_id,
            approval_digest: &digest,
            response_json: &response_json,
            remote_projection: Some(NewSddRemoteProjection {
                host_id: &author_request.host_id,
                repository_identity_sha256: &intent.repository_identity_sha256,
                artifact_set_id: &intent.artifact_set_id,
                worker_version: &intent.worker_version,
                plan_json: &plan_json,
                checkpoint_json: &checkpoint_json,
                specification_content: &spec_payload.content,
            }),
        })
        .await?;
    Ok((StatusCode::CREATED, Json(response)).into_response())
}

pub(crate) async fn recover_remote_create(
    state: &AppState,
    saga: &SddCreateSagaRecord,
) -> Result<(), ApiError> {
    let Some(intent_record) = state
        .store
        .sdd_remote_create_intent(&saga.repo_id, &saga.request_id)
        .await?
    else {
        // `reserved` precedes the atomically committed intent and therefore
        // proves that no SSH authoring request could have started.
        mark_create_failed(
            state,
            &saga.repo_id,
            &saga.request_id,
            &saga.request_hash,
            "desktop restarted before the remote authoring intent was committed",
        )
        .await;
        return Ok(());
    };
    if intent_record.status == "completed" {
        return Ok(());
    }
    if intent_record.status == "failed" {
        mark_create_failed(
            state,
            &saga.repo_id,
            &saga.request_id,
            &saga.request_hash,
            "remote create intent was already failed",
        )
        .await;
        return Ok(());
    }
    let author_request: RemoteAuthoringRequest =
        serde_json::from_str(&intent_record.author_request_json).map_err(|error| {
            ApiError::Internal(format!("invalid remote authoring recovery intent: {error}"))
        })?;
    let publication_intent: RemoteCreatePublicationIntent =
        serde_json::from_str(&intent_record.publication_intent_json).map_err(|error| {
            ApiError::Internal(format!(
                "invalid remote publication recovery intent: {error}"
            ))
        })?;
    if author_request.host_id != intent_record.host_id {
        mark_create_failed(
            state,
            &saga.repo_id,
            &saga.request_id,
            &saga.request_hash,
            "remote recovery host identity changed",
        )
        .await;
        return Err(ApiError::Internal(
            "remote recovery host identity changed".into(),
        ));
    }
    let host_id = intent_record
        .host_id
        .parse::<Uuid>()
        .map_err(|_| ApiError::Internal("remote recovery host id is invalid".into()))?;
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("host not found: {host_id}")))?;
    let client = crate::sdd::remote_lifecycle::client_for_host(host)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let probe = RemoteSddProbeTransport::probe(
        client.as_ref(),
        &author_request.repository_identity_sha256,
        &author_request.provider,
        &publication_intent.base_ref,
    )
    .await
    .map_err(|error| {
        ApiError::Custom(
            StatusCode::BAD_GATEWAY,
            json!({
                "error": "remote_recovery_probe_failed",
                "message": error.to_string(),
                "localFallback": false
            }),
        )
    })?;
    if probe.worker_version != state.version
        || probe.worker_version != publication_intent.worker_version
        || !probe.repository_registered
        || !probe.provider_ready
        || probe.reason.is_some()
        || probe.artifact_set_id.as_deref() != Some(publication_intent.artifact_set_id.as_str())
        || probe.base_commit.as_deref() != Some(publication_intent.base_commit.as_str())
    {
        return Err(ApiError::Custom(
            StatusCode::PRECONDITION_FAILED,
            json!({
                "error": "remote_recovery_capability_changed",
                "expectedWorkerVersion": publication_intent.worker_version,
                "actualWorkerVersion": probe.worker_version,
                "expectedArtifactSetId": publication_intent.artifact_set_id,
                "actualArtifactSetId": probe.artifact_set_id,
                "expectedBaseCommit": publication_intent.base_commit,
                "actualBaseCommit": probe.base_commit,
                "repositoryRegistered": probe.repository_registered,
                "providerReady": probe.provider_ready,
                "reason": probe.reason,
                "localFallback": false
            }),
        ));
    }
    let authored = if intent_record.status == "prepared" {
        let result = RemoteSddAuthoringTransport::author(client.as_ref(), author_request.clone())
            .await
            .map_err(|error| {
                ApiError::Custom(
                    StatusCode::BAD_GATEWAY,
                    json!({
                        "error": "remote_authoring_recovery_failed",
                        "message": error.to_string(),
                        "localFallback": false
                    }),
                )
            })?;
        let result_json = serde_json::to_string(&result)
            .map_err(|error| ApiError::Internal(error.to_string()))?;
        state
            .store
            .sdd_record_remote_authoring_result(
                &saga.repo_id,
                &saga.request_id,
                &saga.request_hash,
                &result_json,
            )
            .await?;
        result
    } else if intent_record.status == "authored" {
        let raw = intent_record.author_result_json.as_deref().ok_or_else(|| {
            ApiError::Internal("authored remote create intent has no result".into())
        })?;
        let result: RemoteAuthoringResult = serde_json::from_str(raw).map_err(|error| {
            ApiError::Internal(format!("invalid remote authoring recovery result: {error}"))
        })?;
        state
            .store
            .sdd_record_remote_authoring_result(
                &saga.repo_id,
                &saga.request_id,
                &saga.request_hash,
                raw,
            )
            .await?;
        result
    } else {
        return Err(ApiError::Internal(format!(
            "unsupported remote create recovery state: {}",
            intent_record.status
        )));
    };
    publish_remote_authoring_result(
        state,
        &saga.repo_id,
        &saga.request_id,
        &saga.request_hash,
        &author_request,
        &publication_intent,
        &authored,
    )
    .await?;
    Ok(())
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_git_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

async fn fail_remote_publication(
    state: &AppState,
    repo_id: &str,
    request_id: &str,
    request_hash: &str,
    reason: &str,
) -> ApiError {
    mark_create_failed(state, repo_id, request_id, request_hash, reason).await;
    ApiError::Custom(
        StatusCode::BAD_GATEWAY,
        json!({
            "error": "remote_authoring_result_invalid",
            "message": reason,
            "localFallback": false
        }),
    )
}

fn append_remote_goal(source: &str, goal: &str) -> String {
    if goal.is_empty() || goal == source.trim() {
        source.to_owned()
    } else {
        format!(
            "{}\n\n## Additional Agentum authoring constraints\n\n{}\n",
            source.trim(),
            goal
        )
    }
}

async fn mark_create_failed(
    state: &AppState,
    repo_id: &str,
    request_id: &str,
    request_hash: &str,
    error: &str,
) {
    let mut summary_end = error.len().min(512);
    while !error.is_char_boundary(summary_end) {
        summary_end -= 1;
    }
    let summary = &error[..summary_end];
    let _ = state
        .store
        .sdd_fail_remote_create_intent(repo_id, request_id)
        .await;
    let _ = state
        .store
        .sdd_update_create_stage(
            repo_id,
            request_id,
            request_hash,
            &[
                "reserved",
                "workspace_ready",
                "authoring",
                "publishing",
                "recovery_required",
            ],
            "failed",
            Some(summary),
        )
        .await;
}

async fn mark_create_recovery_required(
    state: &AppState,
    repo_id: &str,
    request_id: &str,
    request_hash: &str,
    error: &str,
) {
    let mut summary_end = error.len().min(512);
    while !error.is_char_boundary(summary_end) {
        summary_end -= 1;
    }
    let summary = &error[..summary_end];
    let _ = state
        .store
        .sdd_update_create_stage(
            repo_id,
            request_id,
            request_hash,
            &[
                "reserved",
                "workspace_ready",
                "authoring",
                "publishing",
                "failed",
                "recovery_required",
            ],
            "recovery_required",
            Some(summary),
        )
        .await;
}

#[allow(clippy::too_many_arguments)]
async fn abort_create(
    state: &AppState,
    repo_id: &str,
    request_id: &str,
    request_hash: &str,
    repository: &FsPath,
    workspace: &workspace::AuthoritativeWorkspace,
    attempt: Option<&FsPath>,
    error: &str,
) {
    let mut recovery_failures = Vec::new();
    if let Some(attempt) = attempt {
        if let Err(cleanup) = workspace::remove_attempt(repository, attempt).await {
            recovery_failures.push(format!("attempt cleanup: {cleanup}"));
        }
    }
    if let Err(cleanup) =
        workspace::compensate_create(repository, &workspace.path, &workspace.branch_name).await
    {
        recovery_failures.push(format!("authoritative cleanup: {cleanup}"));
    }
    if recovery_failures.is_empty() {
        mark_create_failed(state, repo_id, request_id, request_hash, error).await;
    } else {
        mark_create_recovery_required(
            state,
            repo_id,
            request_id,
            request_hash,
            &format!("{error}; {}", recovery_failures.join("; ")),
        )
        .await;
    }
}

fn discovered_spec_matches(
    left: &artifacts::DiscoveredSpecArtifact,
    right: &artifacts::DiscoveredSpecArtifact,
) -> bool {
    left.header == right.header
        && left.directory_name == right.directory_name
        && left.content_hash == right.content_hash
        && left.content == right.content
        && left.later_artifacts.len() == right.later_artifacts.len()
        && left
            .later_artifacts
            .iter()
            .zip(&right.later_artifacts)
            .all(|(left, right)| {
                left.kind == right.kind
                    && left.file_name == right.file_name
                    && left.relative_path == right.relative_path
                    && left.content_hash == right.content_hash
                    && left.content == right.content
            })
}

fn publish_discovered_run_artifacts(
    destination: &FsPath,
    discovered: &artifacts::DiscoveredSpecArtifact,
    artifact_set_id: ulid::Ulid,
) -> Result<(), ArtifactError> {
    let expected_spec_dir = destination
        .join(".agentum/specs")
        .join(&discovered.directory_name);
    let spec_dir = match std::fs::symlink_metadata(&expected_spec_dir) {
        Ok(_) => {
            let manifest = validate_existing_root(
                destination,
                &discovered.header.id,
                &discovered.directory_name,
            )?;
            if manifest.artifact_set_id != artifact_set_id {
                return Err(ArtifactError::Collision(format!(
                    "repository artifact set is {}, expected {artifact_set_id}",
                    manifest.artifact_set_id
                )));
            }
            expected_spec_dir
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            initialize(
                destination,
                &discovered.header.id,
                &discovered.header.title,
                artifact_set_id,
            )?
            .spec_dir
        }
        Err(error) => return Err(error.into()),
    };
    publish_exact_artifact(&spec_dir.join("spec.md"), &discovered.content)?;
    let present: std::collections::HashSet<_> = discovered
        .later_artifacts
        .iter()
        .map(|artifact| artifact.file_name.as_str())
        .collect();
    for artifact in &discovered.later_artifacts {
        publish_exact_artifact(&spec_dir.join(&artifact.file_name), &artifact.content)?;
    }
    for absent in ["design.md", "plan.json", "decisions.md", "review.md"] {
        if present.contains(absent) {
            continue;
        }
        let path = spec_dir.join(absent);
        let current = content_hash(&path)?;
        if current != MISSING_HASH {
            atomic_remove(&path, &current)?;
        }
    }
    let verified = discover_specs(destination)?
        .and_then(|set| {
            set.specs
                .into_iter()
                .find(|candidate| candidate.header.id == discovered.header.id)
        })
        .ok_or_else(|| {
            ArtifactError::InvalidSpec("published discovered specification is missing".into())
        })?;
    if !discovered_spec_matches(&verified, discovered) {
        return Err(ArtifactError::ContentChanged {
            expected: discovered.content_hash.clone(),
            current: verified.content_hash,
        });
    }
    Ok(())
}

fn publish_exact_artifact(path: &FsPath, content: &str) -> Result<(), ArtifactError> {
    let expected = content_hash(path)?;
    let desired = sha256(content.as_bytes());
    if expected != desired {
        let published = atomic_write(path, content.as_bytes(), Some(&expected))?;
        if published != desired {
            return Err(ArtifactError::ContentChanged {
                expected: desired,
                current: published,
            });
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn abort_run_create(
    state: &AppState,
    spec: &agentum_store::sdd::SddSpecRecord,
    body: &CreateRunBody,
    request_hash: &str,
    repository: &FsPath,
    workspace: &workspace::AuthoritativeWorkspace,
    attempt: Option<&FsPath>,
    error: &str,
) {
    let mut recovery_failures = Vec::new();
    if let Some(attempt) = attempt {
        if let Err(cleanup) =
            workspace::recover_interrupted_attempt(repository, &workspace.path, attempt).await
        {
            recovery_failures.push(format!("attempt cleanup: {cleanup}"));
        }
    }
    if let Err(cleanup) =
        workspace::compensate_create(repository, &workspace.path, &workspace.branch_name).await
    {
        recovery_failures.push(format!("authoritative cleanup: {cleanup}"));
    }
    let detail = if recovery_failures.is_empty() {
        error.to_owned()
    } else {
        format!("{error}; {}", recovery_failures.join("; "))
    };
    let mut end = detail.len().min(512);
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    let stage = if recovery_failures.is_empty() {
        "failed"
    } else {
        "recovery_required"
    };
    let _ = state
        .store
        .sdd_update_run_create_stage(
            &spec.spec_id,
            &body.request_id,
            request_hash,
            &[
                "reserved",
                "workspace_ready",
                "publishing",
                "recovery_required",
            ],
            stage,
            Some(&detail[..end]),
        )
        .await;
}

async fn list_specs(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // Resolve first: an unknown repo must not look like a valid empty project.
    let repository = PathBuf::from(crate::routes::repos::resolve_repo_path(&repo_id)?);
    if crate::routes::repos::resolve_repo_host_id(&repo_id)?.is_some() {
        let specs = state.store.sdd_list_specs(&repo_id).await?;
        return Ok(Json(json!({ "specs": specs })));
    }
    for run in state.store.sdd_runs_for_repo(&repo_id).await? {
        reconcile_external_spec(&state, &run.run_id).await?;
    }
    reconcile_discovered_specs(&state, &repo_id, &repository).await?;
    let specs = state.store.sdd_list_specs(&repo_id).await?;
    Ok(Json(json!({ "specs": specs })))
}

async fn reconcile_discovered_specs(
    state: &AppState,
    repo_id: &str,
    repository: &FsPath,
) -> Result<(), ApiError> {
    if crate::routes::repos::resolve_repo_host_id(repo_id)?.is_some() {
        return Err(ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "error": "remote_sdd_unavailable",
                "message": "remote artifact discovery requires the fixed agentum-sdd-v1 SSH subsystem"
            }),
        ));
    }
    let repository = repository.to_path_buf();
    let discovered = tokio::task::spawn_blocking(move || discover_specs(&repository))
        .await
        .map_err(|error| ApiError::Internal(format!("artifact discovery task failed: {error}")))?
        .map_err(artifact_error)?;
    let Some(discovered) = discovered else {
        return Ok(());
    };
    let source_json: Vec<Option<String>> = discovered
        .specs
        .iter()
        .map(|spec| {
            spec.header.source.as_ref().map(|source| {
                json!({
                    "kind": "artifact",
                    "source": source,
                    "relativePath": spec.relative_path
                })
                .to_string()
            })
        })
        .collect();
    let spec_ids: Vec<String> = discovered
        .specs
        .iter()
        .map(|spec| spec.header.id.to_string())
        .collect();
    let inputs: Vec<_> = discovered
        .specs
        .iter()
        .zip(source_json.iter())
        .zip(spec_ids.iter())
        .map(|((spec, source), spec_id)| DiscoveredSpecInput {
            spec_id,
            spec_ulid: spec.header.id.ulid(),
            title: &spec.header.title,
            slug: &spec.directory_name,
            source_ref_json: source.as_deref(),
            revision: spec.header.revision,
            content_hash: &spec.content_hash,
            content: &spec.content,
        })
        .collect();
    let artifact_set_id = discovered.manifest.artifact_set_id.to_string();
    state
        .store
        .sdd_reconcile_discovered_specs(ReconcileDiscoveredSpecs {
            repo_id,
            artifact_set_id: &artifact_set_id,
            specs: &inputs,
        })
        .await?;
    Ok(())
}

async fn get_spec(
    State(state): State<AppState>,
    Path(spec_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let canonical: SpecId =
        spec_id
            .parse()
            .map_err(|error: agentum_core::sdd::SddContractError| {
                ApiError::BadRequest(error.to_string())
            })?;
    let spec = state
        .store
        .sdd_get_spec(&canonical.to_string())
        .await?
        .ok_or_else(|| ApiError::NotFound(canonical.to_string()))?;
    let run = state
        .store
        .sdd_latest_run_for_spec(&canonical.to_string())
        .await?;
    Ok(Json(json!({ "spec": spec, "run": run })))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRunBody {
    request_id: String,
    expected_revision: i64,
    profile: WorkflowProfile,
    control: WorkflowControl,
    provider: String,
    base_ref: String,
    source_checkout: SourceCheckout,
}

async fn create_run(
    State(state): State<AppState>,
    Path(spec_id): Path<String>,
    Json(body): Json<CreateRunBody>,
) -> Result<Json<Value>, ApiError> {
    validate_request_id(&body.request_id)?;
    let canonical: SpecId =
        spec_id
            .parse()
            .map_err(|error: agentum_core::sdd::SddContractError| {
                ApiError::BadRequest(error.to_string())
            })?;
    let canonical_id = canonical.to_string();
    let request_hash = request_digest(&body)?;
    let idempotency_scope = format!("spec:{canonical_id}:create_run");
    if let Some(response) = state
        .store
        .sdd_idempotent_response(&idempotency_scope, &body.request_id, &request_hash)
        .await?
    {
        return Ok(Json(response));
    }
    let spec = state
        .store
        .sdd_get_spec(&canonical_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(canonical_id.clone()))?;
    if spec.aggregate_revision != body.expected_revision {
        return Err(stale(spec.aggregate_revision, body.expected_revision));
    }
    // A discovered remote spec must not cause provider discovery or Git/filesystem
    // work on the desktop. The remote projection gate is checked from the
    // authoritative repository registration before any local adapter lookup.
    if crate::routes::repos::resolve_repo_host_id(&spec.repo_id)?.is_some() {
        return Err(ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "error": "remote_discovered_run_unavailable",
                "message": "Remote runs must be created through New Spec so authoring and the initial checkpoint are atomic.",
                "localFallback": false
            }),
        ));
    }
    reject_unavailable_local_provider_execution(&body.provider)?;
    if let Some(run) = state.store.sdd_latest_run_for_spec(&canonical_id).await? {
        return Ok(Json(json!({ "run": run, "reused": true })));
    }
    if spec.provider != "unassigned" {
        return Err(ApiError::Conflict(
            "configured specification has no run and requires recovery instead of reconstruction"
                .into(),
        ));
    }
    if body.base_ref.trim().is_empty() {
        return Err(ApiError::BadRequest("baseRef is required".into()));
    }
    if let Some(saga) = state
        .store
        .sdd_run_create_saga(&canonical_id, &body.request_id)
        .await?
    {
        if saga.request_hash != request_hash {
            return Err(agentum_store::StoreError::IdempotencyConflict(idempotency_scope).into());
        }
        return Err(ApiError::Conflict(format!(
            "first run {} is {}; use a new requestId only after recovery completes",
            saga.run_id, saga.stage
        )));
    }
    let provider = validate_provider(&body.provider)?;
    let capability = match &provider {
        ProviderAdapter::Bundled(provider) => probe_provider(*provider).await,
        ProviderAdapter::Custom(_) => {
            probe_custom_provider(&body.provider)
                .await
                .map_err(|error| {
                    ApiError::Custom(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        json!({
                            "error": "provider_capability_unavailable",
                            "provider": body.provider,
                            "message": error.to_string()
                        }),
                    )
                })?
        }
    };
    if !capability.available {
        return Err(ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "error": "provider_capability_unavailable",
                "provider": body.provider,
                "message": capability.reason.unwrap_or_else(|| "provider is unavailable".into())
            }),
        ));
    }
    let repository = PathBuf::from(crate::routes::repos::resolve_repo_path(&spec.repo_id)?);
    reconcile_discovered_specs(&state, &spec.repo_id, &repository).await?;
    let spec = state
        .store
        .sdd_get_spec(&canonical_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(canonical_id.clone()))?;
    if spec.aggregate_revision != body.expected_revision {
        return Err(stale(spec.aggregate_revision, body.expected_revision));
    }
    let discovered = tokio::task::spawn_blocking({
        let repository = repository.clone();
        move || discover_specs(&repository)
    })
    .await
    .map_err(|error| ApiError::Internal(format!("artifact discovery task failed: {error}")))?
    .map_err(artifact_error)?
    .ok_or_else(|| ApiError::Conflict("repository has no Agentum artifact root".into()))?;
    let discovered_spec = discovered
        .specs
        .iter()
        .find(|candidate| candidate.header.id == canonical)
        .cloned()
        .ok_or_else(|| ApiError::Conflict("discovered specification no longer exists".into()))?;
    let (stored_hash, stored_content) = state
        .store
        .sdd_spec_revision_content(&canonical_id, spec.current_revision)
        .await?
        .ok_or_else(|| ApiError::Internal("current specification revision is missing".into()))?;
    if stored_hash != discovered_spec.content_hash || stored_content != discovered_spec.content {
        return Err(ApiError::Conflict(
            "filesystem and immutable specification revision are not reconciled".into(),
        ));
    }

    let run_id = Uuid::new_v4().to_string();
    let attempt_id = Uuid::new_v4().to_string();
    let approval_id = Uuid::new_v4().to_string();
    let workspace = workspace::plan_authoritative(
        &spec.repo_id,
        &repository,
        &run_id,
        &canonical,
        &spec.title,
        body.base_ref.trim(),
        body.source_checkout.into(),
    )
    .await
    .map_err(workspace_error)?;
    let attempt_path =
        workspace::attempt_path(&workspace.path, &attempt_id).map_err(workspace_error)?;
    state
        .store
        .sdd_reserve_discovered_run(NewSddRunCreateSaga {
            spec_id: &canonical_id,
            repo_id: &spec.repo_id,
            request_id: &body.request_id,
            request_hash: &request_hash,
            run_id: &run_id,
            expected_spec_revision: spec.current_revision,
            expected_spec_hash: &stored_hash,
            expected_aggregate_revision: body.expected_revision,
            repository_path: &repository.to_string_lossy(),
            authoritative_path: &workspace.path.to_string_lossy(),
            branch_name: &workspace.branch_name,
            attempt_id: &attempt_id,
            attempt_path: &attempt_path.to_string_lossy(),
        })
        .await?;
    if let Err(error) = workspace::materialize_authoritative(&repository, &workspace).await {
        abort_run_create(
            &state,
            &spec,
            &body,
            &request_hash,
            &repository,
            &workspace,
            None,
            &error.to_string(),
        )
        .await;
        return Err(workspace_error(error));
    }
    if let Err(error) = state
        .store
        .sdd_update_run_create_stage(
            &canonical_id,
            &body.request_id,
            &request_hash,
            &["reserved"],
            "workspace_ready",
            None,
        )
        .await
    {
        abort_run_create(
            &state,
            &spec,
            &body,
            &request_hash,
            &repository,
            &workspace,
            None,
            &error.to_string(),
        )
        .await;
        return Err(error.into());
    }
    let current_discovery = tokio::task::spawn_blocking({
        let repository = repository.clone();
        move || discover_specs(&repository)
    })
    .await
    .map_err(|error| ApiError::Internal(format!("artifact discovery task failed: {error}")))?
    .map_err(artifact_error)?;
    let unchanged = current_discovery.as_ref().and_then(|set| {
        set.specs
            .iter()
            .find(|candidate| candidate.header.id == canonical)
            .filter(|candidate| discovered_spec_matches(candidate, &discovered_spec))
    });
    if unchanged.is_none() {
        abort_run_create(
            &state,
            &spec,
            &body,
            &request_hash,
            &repository,
            &workspace,
            None,
            "repository artifacts changed during first-run materialization",
        )
        .await;
        return Err(ApiError::Conflict(
            "repository artifacts changed during first-run materialization".into(),
        ));
    }
    if let Err(error) = publish_discovered_run_artifacts(
        &workspace.path,
        &discovered_spec,
        discovered.manifest.artifact_set_id,
    ) {
        abort_run_create(
            &state,
            &spec,
            &body,
            &request_hash,
            &repository,
            &workspace,
            None,
            &error.to_string(),
        )
        .await;
        return Err(artifact_error(error));
    }
    let attempt = match workspace::create_attempt(
        &repository,
        &workspace.path,
        &attempt_id,
        &workspace.base_commit,
        workspace.snapshot_digest.as_deref(),
    )
    .await
    {
        Ok(attempt) => attempt,
        Err(error) => {
            abort_run_create(
                &state,
                &spec,
                &body,
                &request_hash,
                &repository,
                &workspace,
                Some(&attempt_path),
                &error.to_string(),
            )
            .await;
            return Err(workspace_error(error));
        }
    };
    if let Err(error) = publish_discovered_run_artifacts(
        &attempt.path,
        &discovered_spec,
        discovered.manifest.artifact_set_id,
    ) {
        abort_run_create(
            &state,
            &spec,
            &body,
            &request_hash,
            &repository,
            &workspace,
            Some(&attempt.path),
            &error.to_string(),
        )
        .await;
        return Err(artifact_error(error));
    }
    if let Err(error) = workspace::remove_attempt(&repository, &attempt.path).await {
        abort_run_create(
            &state,
            &spec,
            &body,
            &request_hash,
            &repository,
            &workspace,
            Some(&attempt.path),
            &error.to_string(),
        )
        .await;
        return Err(ApiError::Internal(
            "discovered artifact attempt cleanup failed; recovery is required".into(),
        ));
    }
    if let Err(error) = state
        .store
        .sdd_update_run_create_stage(
            &canonical_id,
            &body.request_id,
            &request_hash,
            &["workspace_ready"],
            "publishing",
            None,
        )
        .await
    {
        abort_run_create(
            &state,
            &spec,
            &body,
            &request_hash,
            &repository,
            &workspace,
            None,
            &error.to_string(),
        )
        .await;
        return Err(error.into());
    }

    let profile = profile_name(body.profile);
    let control = control_name(body.control);
    let later_files: Vec<_> = discovered_spec
        .later_artifacts
        .iter()
        .map(|artifact| artifact.file_name.as_str())
        .collect();
    let policy = json!({
        "profile": profile,
        "control": control,
        "deliveryRequired": true,
        "implementationEnabled": true,
        "sourceCheckout": source_checkout_name(body.source_checkout),
        "sourceSnapshotDigest": workspace.snapshot_digest.as_deref(),
        "provider": provider.approval_binding(),
        "discoveredArtifactDisposition": {
            "laterArtifacts": later_files,
            "status": "historical_unapproved_reopen_from_specification"
        }
    });
    let mut artifact_values = vec![(
        format!(".agentum/specs/{}/spec.md", discovered_spec.directory_name),
        discovered_spec.content_hash.clone(),
        "specification".to_owned(),
    )];
    artifact_values.extend(discovered_spec.later_artifacts.iter().map(|artifact| {
        (
            artifact.relative_path.clone(),
            artifact.content_hash.clone(),
            artifact.kind.clone(),
        )
    }));
    // This is a specification approval. Later-phase files discovered alongside
    // it are preserved as historical, explicitly unapproved evidence and are
    // reopened from specification before they can become workflow authority.
    // Binding those historical bytes here would imply approval even though the
    // external-edit reconciler intentionally watches only spec.md in this phase.
    let digest_values = [(artifact_values[0].0.as_str(), artifact_values[0].1.as_str())];
    let digest = approval_digest(
        &canonical,
        spec.current_revision,
        &digest_values,
        &policy,
        &workspace.fingerprint,
    );
    let response = json!({
        "specId": canonical_id,
        "runId": run_id,
        "revision": 1,
        "specRevision": spec.current_revision,
        "specAggregateRevision": body.expected_revision + 1,
        "phase": "specification",
        "status": "waiting",
        "nextAction": if matches!(body.control, WorkflowControl::Autopilot) {
            "Start to authorize the current digest"
        } else {
            "Spec approval required"
        },
        "authoritativePath": workspace.path,
        "preservedLaterArtifacts": later_files,
        "downstreamDisposition": "historical_unapproved_reopen_from_specification",
        "approval": {
            "approvalId": approval_id,
            "purpose": "specification",
            "digest": digest,
            "status": "pending"
        }
    });
    let response_json = response.to_string();
    let store_artifacts: Vec<_> = artifact_values
        .iter()
        .map(|(path, hash, kind)| NewSddRunArtifact {
            kind,
            relative_path: path,
            content_hash: hash,
        })
        .collect();
    let submitted_by = format!("agentum:filesystem-discovery:{attempt_id}");
    let attempt_identity = format!("filesystem-import:{attempt_id}");
    let published = state
        .store
        .sdd_publish_discovered_run(NewSddDiscoveredRun {
            spec_id: &canonical_id,
            repo_id: &spec.repo_id,
            request_id: &body.request_id,
            request_hash: &request_hash,
            expected_aggregate_revision: body.expected_revision,
            expected_spec_revision: spec.current_revision,
            expected_spec_hash: &stored_hash,
            profile,
            control,
            provider: &body.provider,
            run_id: &run_id,
            base_ref: body.base_ref.trim(),
            base_commit: &workspace.base_commit,
            branch_name: &workspace.branch_name,
            authoritative_path: &workspace.path.to_string_lossy(),
            workspace_fingerprint: &workspace.fingerprint,
            policy_json: &policy.to_string(),
            attempt_id: &attempt_id,
            attempt_path: &attempt_path.to_string_lossy(),
            attempt_session_identity: &attempt_identity,
            submitted_by: &submitted_by,
            artifacts: &store_artifacts,
            approval_id: &approval_id,
            approval_digest: &digest,
            response_json: &response_json,
        })
        .await;
    if let Err(error) = published {
        if let Some(replayed) = state
            .store
            .sdd_idempotent_response(&idempotency_scope, &body.request_id, &request_hash)
            .await?
        {
            return Ok(Json(replayed));
        }
        abort_run_create(
            &state,
            &spec,
            &body,
            &request_hash,
            &repository,
            &workspace,
            None,
            &error.to_string(),
        )
        .await;
        return Err(error.into());
    }
    Ok(Json(response))
}

async fn get_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    reconcile_external_spec(&state, &run_id).await?;
    let snapshot = state
        .store
        .sdd_snapshot(&run_id)
        .await?
        .ok_or(ApiError::NotFound(run_id))?;
    let delivery = if let Some(preview) = state
        .store
        .sdd_latest_delivery_preview_for_run(&snapshot.run.run_id)
        .await?
    {
        let actions = state
            .store
            .sdd_delivery_actions(&preview.preview_id)
            .await?;
        Some(json!({ "preview": preview, "actions": actions }))
    } else {
        None
    };
    let browser_evidence = state
        .store
        .sdd_browser_evidence(&snapshot.run.run_id)
        .await?;
    let mut value =
        serde_json::to_value(&snapshot).map_err(|error| ApiError::Internal(error.to_string()))?;
    value["delivery"] =
        serde_json::to_value(delivery).map_err(|error| ApiError::Internal(error.to_string()))?;
    value["browserEvidence"] = serde_json::to_value(browser_evidence)
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    value["remote"] = serde_json::to_value(state.store.sdd_remote_run(&snapshot.run.run_id).await?)
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    Ok(Json(value))
}

async fn get_evidence_blob(
    State(state): State<AppState>,
    Path((run_id, evidence_id, sha256)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    let metadata = state
        .store
        .sdd_browser_evidence_blob(&run_id, &evidence_id, &sha256)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("{evidence_id}/{sha256}")))?;
    let bytes = crate::sdd::evidence::read_blob(&metadata.storage_relative_path, &sha256)
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    if bytes.len() as i64 != metadata.byte_length {
        return Err(ApiError::Internal(
            "browser evidence blob length no longer matches durable metadata".into(),
        ));
    }
    let content_type = HeaderValue::from_str(&metadata.media_type)
        .map_err(|_| ApiError::Internal("invalid stored evidence media type".into()))?;
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum RunCommand {
    StartAuthoring {
        request_id: String,
        expected_revision: i64,
    },
    StartRun {
        request_id: String,
        expected_revision: i64,
    },
    SubmitArtifact {
        request_id: String,
        expected_revision: i64,
        kind: ArtifactKind,
        content: String,
        expected_content_hash: String,
        attempt_id: String,
    },
    DecideApproval {
        request_id: String,
        expected_revision: i64,
        approval_id: String,
        digest: String,
        decision: String,
        #[serde(default)]
        reason: Option<String>,
    },
    Pause {
        request_id: String,
        expected_revision: i64,
    },
    Resume {
        request_id: String,
        expected_revision: i64,
    },
    Retry {
        request_id: String,
        expected_revision: i64,
    },
    ResolveBlock {
        request_id: String,
        expected_revision: i64,
    },
    Cancel {
        request_id: String,
        expected_revision: i64,
    },
    ReopenPhase {
        request_id: String,
        expected_revision: i64,
        phase: String,
    },
    PreviewDelivery {
        request_id: String,
        expected_revision: i64,
        actions: Vec<DeliveryActionRequest>,
    },
    ConfirmDelivery {
        request_id: String,
        expected_revision: i64,
        preview_token: String,
        actions: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy)]
enum RunCommandClass {
    /// Provider output enters through Agentum's in-process adapter/broker. The
    /// public command remains authenticated so an arbitrary local process
    /// cannot impersonate an attempt.
    ArtifactSubmission,
    /// Pause/resume/retry/cancel/reopen and authoring controls are user-facing
    /// lifecycle controls, not provider callbacks.
    RunControl,
    /// Commands that grant approval or authorize external side effects.
    HumanAuthorization,
}

impl RunCommandClass {
    fn label(self) -> &'static str {
        match self {
            Self::ArtifactSubmission => "artifact submission",
            Self::RunControl => "run control",
            Self::HumanAuthorization => "human authorization",
        }
    }
}

impl RunCommand {
    fn request(&self) -> (&str, i64) {
        match self {
            Self::StartAuthoring {
                request_id,
                expected_revision,
            }
            | Self::StartRun {
                request_id,
                expected_revision,
            }
            | Self::SubmitArtifact {
                request_id,
                expected_revision,
                ..
            }
            | Self::DecideApproval {
                request_id,
                expected_revision,
                ..
            }
            | Self::Pause {
                request_id,
                expected_revision,
            }
            | Self::Resume {
                request_id,
                expected_revision,
            }
            | Self::Retry {
                request_id,
                expected_revision,
            }
            | Self::ResolveBlock {
                request_id,
                expected_revision,
            }
            | Self::Cancel {
                request_id,
                expected_revision,
            }
            | Self::ReopenPhase {
                request_id,
                expected_revision,
                ..
            }
            | Self::PreviewDelivery {
                request_id,
                expected_revision,
                ..
            }
            | Self::ConfirmDelivery {
                request_id,
                expected_revision,
                ..
            } => (request_id, *expected_revision),
        }
    }

    fn class(&self) -> RunCommandClass {
        match self {
            Self::SubmitArtifact { .. } => RunCommandClass::ArtifactSubmission,
            Self::DecideApproval { .. }
            | Self::StartRun { .. }
            | Self::PreviewDelivery { .. }
            | Self::ConfirmDelivery { .. } => RunCommandClass::HumanAuthorization,
            Self::StartAuthoring { .. }
            | Self::Pause { .. }
            | Self::Resume { .. }
            | Self::Retry { .. }
            | Self::ResolveBlock { .. }
            | Self::Cancel { .. }
            | Self::ReopenPhase { .. } => RunCommandClass::RunControl,
        }
    }
}

async fn command(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(run_id): Path<String>,
    Json(command): Json<RunCommand>,
) -> Result<Json<Value>, ApiError> {
    // Defense in depth: the auth middleware denies the entire SDD surface to
    // unauthenticated callers. Keep the command boundary explicit as well so a
    // future alternate router or direct handler composition cannot turn a
    // loopback/provider identity into a human mutation actor.
    let command_class = command.class();
    if !actor.can_mutate_sdd() {
        return Err(ApiError::Forbidden(format!(
            "SDD {} requires an authenticated human capability",
            command_class.label()
        )));
    }
    let (request_id, expected_revision) = command.request();
    validate_request_id(request_id)?;
    let request_hash = request_digest(&command)?;
    let idempotency_scope = format!("run:{run_id}");
    if let Some(response) = state
        .store
        .sdd_idempotent_response(&idempotency_scope, request_id, &request_hash)
        .await?
    {
        return Ok(Json(response));
    }
    // Reconcile bytes before authorizing any new mutation. A filesystem edit
    // invalidates the caller's expected revision and all prior approvals.
    reconcile_external_spec(&state, &run_id).await?;
    let run = state
        .store
        .sdd_get_run(&run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(run_id.clone()))?;
    if run.aggregate_revision != expected_revision {
        return Err(stale(run.aggregate_revision, expected_revision));
    }
    match command {
        RunCommand::SubmitArtifact {
            request_id,
            expected_revision,
            kind,
            content,
            expected_content_hash,
            attempt_id,
        } => {
            ensure_nonterminal(&run, "submit an artifact")?;
            submit_artifact(
                &state,
                &run,
                &request_id,
                &request_hash,
                expected_revision,
                kind,
                &content,
                &expected_content_hash,
                &attempt_id,
            )
            .await
        }
        RunCommand::DecideApproval {
            request_id,
            expected_revision,
            approval_id,
            digest,
            decision,
            reason,
        } => {
            if run.status != "waiting" {
                return Err(ApiError::Conflict(
                    "approval is only valid while the run is waiting".into(),
                ));
            }
            let pending = state
                .store
                .sdd_pending_approval(&run.run_id)
                .await?
                .ok_or_else(|| ApiError::Conflict("no approval is pending".into()))?;
            if workflow_control(&run)? == "autopilot" && pending.purpose == "specification" {
                return Err(ApiError::Conflict(
                    "Autopilot specification authorization requires the explicit Start command"
                        .into(),
                ));
            }
            if pending.approval_id != approval_id {
                return Err(ApiError::Conflict(
                    "approval is no longer the active request".into(),
                ));
            }
            let approved_phase = approved_phase(&pending.purpose)?;
            let approved_status = approved_status(&pending.purpose)?;
            let response = json!({
                "runId": run_id,
                "revision": expected_revision + 1,
                "decision": decision,
                "purpose": pending.purpose,
                "phase": if decision == "approve" { approved_phase } else { run.phase.as_str() },
                "status": if decision == "approve" { approved_status } else { "blocked" }
            });
            state
                .store
                .sdd_decide_approval(ApprovalDecisionMutation {
                    request_id: &request_id,
                    request_hash: &request_hash,
                    run_id: &run_id,
                    expected_revision,
                    approval_id: &approval_id,
                    digest: &digest,
                    actor_id: &actor.id,
                    decision: &decision,
                    reason: reason.as_deref(),
                    response_json: &response.to_string(),
                })
                .await?;
            if decision == "approve"
                && approved_status == "queued"
                && lifecycle_execution_enabled(&run)
            {
                spawn_run_execution(&state, &run.run_id).await?;
            }
            Ok(Json(response))
        }
        RunCommand::Pause {
            request_id,
            expected_revision,
        } => {
            ensure_nonterminal(&run, "pause")?;
            if run.status == "paused" || run.status == "pausing" {
                return Err(ApiError::Conflict(
                    "run is already paused or pausing".into(),
                ));
            }
            let response = transition(
                &state,
                &run,
                &request_id,
                &request_hash,
                expected_revision,
                &run.phase,
                "paused",
                None,
                "sdd.run.paused",
            )
            .await?;
            crate::sdd::providers::cancel_run(&run.run_id);
            cancel_run_execution(&state, &run.run_id).await;
            Ok(response)
        }
        RunCommand::Resume {
            request_id,
            expected_revision,
        } => {
            if run.status != "paused" {
                return Err(ApiError::Conflict("only a paused run can resume".into()));
            }
            let resume_status = if state
                .store
                .sdd_pending_approval(&run.run_id)
                .await?
                .is_some()
            {
                "waiting"
            } else {
                if run.phase != "specification"
                    && !state
                        .store
                        .sdd_current_spec_is_approved(&run.run_id)
                        .await?
                {
                    return Err(ApiError::Conflict(
                        "the current specification revision is not approved".into(),
                    ));
                }
                "queued"
            };
            let response = transition(
                &state,
                &run,
                &request_id,
                &request_hash,
                expected_revision,
                &run.phase,
                resume_status,
                None,
                "sdd.run.queued",
            )
            .await?;
            if resume_status == "queued" && lifecycle_execution_enabled(&run) {
                spawn_run_execution(&state, &run.run_id).await?;
            }
            Ok(response)
        }
        RunCommand::Retry {
            request_id,
            expected_revision,
        } => {
            if run.status != "failed" && run.status != "retry_scheduled" {
                return Err(ApiError::Conflict(
                    "only a failed or scheduled run can retry".into(),
                ));
            }
            let response = transition(
                &state,
                &run,
                &request_id,
                &request_hash,
                expected_revision,
                &run.phase,
                "queued",
                None,
                "sdd.run.queued",
            )
            .await?;
            if lifecycle_execution_enabled(&run) {
                spawn_run_execution(&state, &run.run_id).await?;
            }
            Ok(response)
        }
        RunCommand::ResolveBlock {
            request_id,
            expected_revision,
        } => {
            if run.status != "blocked" {
                return Err(ApiError::Conflict(
                    "only a blocked run can be resolved".into(),
                ));
            }
            if run.phase == "specification"
                && !state
                    .store
                    .sdd_current_spec_is_approved(&run.run_id)
                    .await?
            {
                return Err(ApiError::Conflict(
                    "a rejected or invalid specification must be revised and approved".into(),
                ));
            }
            let response = transition(
                &state,
                &run,
                &request_id,
                &request_hash,
                expected_revision,
                &run.phase,
                "queued",
                None,
                "sdd.run.queued",
            )
            .await?;
            if lifecycle_execution_enabled(&run) {
                spawn_run_execution(&state, &run.run_id).await?;
            }
            Ok(response)
        }
        RunCommand::Cancel {
            request_id,
            expected_revision,
        } => {
            ensure_nonterminal(&run, "cancel")?;
            let response = transition(
                &state,
                &run,
                &request_id,
                &request_hash,
                expected_revision,
                &run.phase,
                "canceled",
                None,
                "sdd.run.canceled",
            )
            .await?;
            crate::sdd::providers::cancel_run(&run.run_id);
            cancel_run_execution(&state, &run.run_id).await;
            Ok(response)
        }
        RunCommand::StartAuthoring {
            request_id,
            expected_revision,
        } => {
            if run.phase != "specification"
                || !matches!(run.status.as_str(), "idle" | "paused" | "blocked")
            {
                return Err(ApiError::Conflict(
                    "authoring can start only from an idle, paused, or blocked specification"
                        .into(),
                ));
            }
            transition(
                &state,
                &run,
                &request_id,
                &request_hash,
                expected_revision,
                "specification",
                "queued",
                None,
                "sdd.authoring.queued",
            )
            .await
        }
        RunCommand::StartRun {
            request_id,
            expected_revision,
        } => {
            ensure_nonterminal(&run, "start")?;
            if let Some(pending) = state.store.sdd_pending_approval(&run.run_id).await? {
                if workflow_control(&run)? != "autopilot" {
                    return Err(ApiError::Conflict("spec approval required".into()));
                }
                if run.status != "waiting" {
                    return Err(ApiError::Conflict(
                        "Autopilot Start cannot resolve a paused or blocked exception".into(),
                    ));
                }
                if !lifecycle_execution_enabled(&run) {
                    return Err(ApiError::Conflict(
                        "autonomous lifecycle execution is not enabled for this run".into(),
                    ));
                }
                let next_phase = approved_phase(&pending.purpose)?;
                let response = json!({
                    "runId": run_id,
                    "revision": expected_revision + 1,
                    "phase": next_phase,
                    "status": "queued",
                    "authorization": {
                        "source": "explicit_start",
                        "approvalId": pending.approval_id,
                        "purpose": pending.purpose,
                        "digest": pending.digest
                    }
                });
                state
                    .store
                    .sdd_authorize_autopilot_start(ApprovalDecisionMutation {
                        request_id: &request_id,
                        request_hash: &request_hash,
                        run_id: &run_id,
                        expected_revision,
                        approval_id: &pending.approval_id,
                        digest: &pending.digest,
                        actor_id: &actor.id,
                        decision: "approve",
                        reason: Some(
                            "explicit Autopilot Start authorized the pending hash-bound digest",
                        ),
                        response_json: &response.to_string(),
                    })
                    .await?;
                spawn_run_execution(&state, &run.run_id).await?;
                return Ok(Json(response));
            }
            if !state
                .store
                .sdd_current_spec_is_approved(&run.run_id)
                .await?
            {
                return Err(ApiError::Conflict(
                    "the current specification revision is not approved".into(),
                ));
            }
            if !matches!(run.status.as_str(), "idle" | "queued" | "paused") {
                return Err(ApiError::Conflict(
                    "run can start only from idle, queued, or paused; resolve blocked exceptions explicitly"
                        .into(),
                ));
            }
            if !lifecycle_execution_enabled(&run) {
                return Err(ApiError::Conflict(
                    "autonomous lifecycle execution is not enabled for this run".into(),
                ));
            }
            let response = transition(
                &state,
                &run,
                &request_id,
                &request_hash,
                expected_revision,
                &run.phase,
                "queued",
                None,
                "sdd.run.started",
            )
            .await?;
            spawn_run_execution(&state, &run.run_id).await?;
            Ok(response)
        }
        RunCommand::ReopenPhase {
            request_id,
            expected_revision,
            phase,
        } => {
            validate_phase(&phase)?;
            if phase_rank(&phase) >= phase_rank(&run.phase) {
                return Err(ApiError::Conflict(
                    "a phase can only be reopened to an earlier lifecycle phase".into(),
                ));
            }
            transition(
                &state,
                &run,
                &request_id,
                &request_hash,
                expected_revision,
                &phase,
                "paused",
                None,
                "sdd.phase.reopened",
            )
            .await
        }
        RunCommand::PreviewDelivery {
            request_id,
            expected_revision,
            actions,
        } => {
            if run.phase != "ready" || run.status != "succeeded" || run.quarantined != 0 {
                return Err(ApiError::Conflict(
                    "delivery preview requires a non-quarantined Ready run".into(),
                ));
            }
            let mut prepared = prepare_actions(actions).map_err(delivery_error)?;
            validate_delivery_capabilities(&state, &run.spec_id, &prepared).await?;
            bind_tracker_mutations(&state, &run, &mut prepared)
                .await
                .map_err(delivery_error)?;
            bind_openspec_exports(&state, &run, &mut prepared)
                .await
                .map_err(delivery_error)?;
            let spec = state
                .store
                .sdd_get_spec(&run.spec_id)
                .await?
                .ok_or_else(|| ApiError::NotFound(run.spec_id.clone()))?;
            let remote_snapshot = inspect_remote_delivery_state(
                &state,
                &run,
                &request_id,
                openspec_delivery_destination(&prepared),
            )
            .await?;
            let (workspace_state, worktree_identity, branch_name) =
                if let Some(snapshot) = remote_snapshot.as_ref() {
                    (
                        snapshot.workspace_state_sha256.clone(),
                        snapshot.worktree_identity_sha256.clone(),
                        snapshot.branch_name.clone(),
                    )
                } else {
                    (
                        workspace_state_hash(FsPath::new(&run.authoritative_path))
                            .await
                            .map_err(delivery_error)?,
                        sha256(run.authoritative_path.as_bytes()),
                        run.branch_name.clone(),
                    )
                };
            let mut artifact_hashes = state
                .store
                .sdd_artifacts(&run.run_id)
                .await?
                .into_iter()
                .map(|artifact| DeliveryArtifactHash {
                    kind: artifact.kind,
                    relative_path: artifact.relative_path,
                    content_hash: artifact.content_hash,
                })
                .collect::<Vec<_>>();
            let evidence_hashes = state
                .store
                .sdd_browser_evidence_manifest_hashes(&run.run_id)
                .await?;
            artifact_hashes.push(DeliveryArtifactHash {
                kind: "browser_evidence".into(),
                relative_path: "agentum://browser-evidence".into(),
                content_hash: sha256(
                    serde_json::to_vec(&evidence_hashes)
                        .map_err(|error| ApiError::Internal(error.to_string()))?,
                ),
            });
            if let Some(snapshot) = remote_snapshot.as_ref() {
                artifact_hashes.push(DeliveryArtifactHash {
                    kind: "remote_artifact_set".into(),
                    relative_path: "agentum+ssh://artifact-set".into(),
                    content_hash: snapshot.artifact_set_sha256.clone(),
                });
            }
            artifact_hashes.sort_by(|left, right| {
                (&left.kind, &left.relative_path).cmp(&(&right.kind, &right.relative_path))
            });
            let envelope = DeliveryPreviewEnvelope {
                schema_version: 1,
                actor_id: actor.id.clone(),
                repo_id: run.repo_id.clone(),
                spec_id: run.spec_id.clone(),
                spec_revision: spec.current_revision,
                run_id: run.run_id.clone(),
                run_revision: expected_revision + 1,
                base_commit: run.base_commit.clone(),
                branch_name,
                worktree_identity,
                workspace_fingerprint: run.workspace_fingerprint.clone(),
                workspace_state_hash: workspace_state,
                artifact_hashes,
                actions: prepared,
            };
            let digest = preview_digest(&envelope).map_err(delivery_error)?;
            let preview_id = Uuid::new_v4().to_string();
            let token = preview_token(&preview_id, &digest);
            let token_hash = sha256(token.as_bytes());
            let expires_at = (time::OffsetDateTime::now_utc() + time::Duration::minutes(15))
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|error| ApiError::Internal(error.to_string()))?;
            let actions_json = serde_json::to_string(&envelope)
                .map_err(|error| ApiError::Internal(error.to_string()))?;
            let response = json!({
                "runId": run.run_id,
                "revision": expected_revision + 1,
                "phase": "ready",
                "status": "succeeded",
                "previewId": preview_id,
                "previewToken": token,
                "digest": digest,
                "specRevision": spec.current_revision,
                "expiresAt": expires_at,
                "workspaceStateHash": envelope.workspace_state_hash,
                "artifactHashes": envelope.artifact_hashes,
                "actions": envelope.actions
            });
            let event = json!({
                "runId": run.run_id,
                "revision": expected_revision + 1,
                "phase": "ready",
                "status": "succeeded",
                "previewId": preview_id,
                "digest": digest,
                "specRevision": spec.current_revision,
                "expiresAt": expires_at,
                "actions": envelope.actions
            });
            state
                .store
                .sdd_create_delivery_preview(NewDeliveryPreview {
                    request_id: &request_id,
                    request_hash: &request_hash,
                    run_id: &run.run_id,
                    expected_revision,
                    actor_id: &actor.id,
                    preview_id: response["previewId"].as_str().expect("preview id"),
                    token_hash: &token_hash,
                    digest: response["digest"].as_str().expect("digest"),
                    spec_revision: spec.current_revision,
                    actions_json: &actions_json,
                    expires_at: response["expiresAt"].as_str().expect("expiry"),
                    event_json: &event.to_string(),
                    response_json: &response.to_string(),
                })
                .await?;
            Ok(Json(response))
        }
        RunCommand::ConfirmDelivery {
            request_id,
            expected_revision,
            preview_token,
            actions,
        } => {
            if preview_token.trim().is_empty()
                || preview_token.len() > 512
                || actions.is_empty()
                || actions.iter().any(|action| action.trim().is_empty())
            {
                return Err(ApiError::BadRequest(
                    "previewToken and at least one delivery action id are required".into(),
                ));
            }
            if run.phase != "ready" || run.status != "succeeded" || run.quarantined != 0 {
                return Err(ApiError::Conflict(
                    "delivery confirmation requires a non-quarantined Ready run".into(),
                ));
            }
            let token_hash = sha256(preview_token.as_bytes());
            let preview = state
                .store
                .sdd_delivery_preview_by_token_hash(&token_hash)
                .await?
                .ok_or_else(|| ApiError::BadRequest("delivery preview token is invalid".into()))?;
            validate_preview_token(&preview, &preview_token).map_err(delivery_error)?;
            if preview.run_id != run.run_id || preview.actor_id != actor.id {
                return Err(ApiError::Forbidden(
                    "delivery preview belongs to a different run or actor".into(),
                ));
            }
            let envelope: DeliveryPreviewEnvelope = serde_json::from_str(&preview.actions_json)
                .map_err(|error| ApiError::Internal(error.to_string()))?;
            if envelope.actor_id != actor.id {
                return Err(ApiError::Forbidden(
                    "delivery digest belongs to a different actor".into(),
                ));
            }
            if preview_digest(&envelope).map_err(delivery_error)? != preview.digest {
                return Err(ApiError::Conflict(
                    "delivery preview digest no longer matches its immutable intent".into(),
                ));
            }
            if preview.status == "pending" {
                validate_tracker_mutations(&state, &run, &envelope.actions)
                    .await
                    .map_err(delivery_error)?;
                validate_openspec_exports(&state, &run, &envelope.actions)
                    .await
                    .map_err(delivery_error)?;
                let remote_snapshot = inspect_remote_delivery_state(
                    &state,
                    &run,
                    &request_id,
                    openspec_delivery_destination(&envelope.actions),
                )
                .await?;
                let current_workspace = if let Some(snapshot) = remote_snapshot.as_ref() {
                    if snapshot.worktree_identity_sha256 != envelope.worktree_identity
                        || snapshot.branch_name != envelope.branch_name
                    {
                        return Err(ApiError::Custom(
                            StatusCode::PRECONDITION_FAILED,
                            json!({
                                "error": "delivery_preview_stale",
                                "message": "remote worktree identity changed after preview"
                            }),
                        ));
                    }
                    snapshot.workspace_state_sha256.clone()
                } else {
                    workspace_state_hash(FsPath::new(&run.authoritative_path))
                        .await
                        .map_err(delivery_error)?
                };
                if current_workspace != envelope.workspace_state_hash {
                    return Err(ApiError::Custom(
                        StatusCode::PRECONDITION_FAILED,
                        json!({
                            "error": "delivery_preview_stale",
                            "message": "workspace changed after preview; create a new preview"
                        }),
                    ));
                }
                let mut current_artifacts = state
                    .store
                    .sdd_artifacts(&run.run_id)
                    .await?
                    .into_iter()
                    .map(|artifact| DeliveryArtifactHash {
                        kind: artifact.kind,
                        relative_path: artifact.relative_path,
                        content_hash: artifact.content_hash,
                    })
                    .collect::<Vec<_>>();
                let evidence_hashes = state
                    .store
                    .sdd_browser_evidence_manifest_hashes(&run.run_id)
                    .await?;
                current_artifacts.push(DeliveryArtifactHash {
                    kind: "browser_evidence".into(),
                    relative_path: "agentum://browser-evidence".into(),
                    content_hash: sha256(
                        serde_json::to_vec(&evidence_hashes)
                            .map_err(|error| ApiError::Internal(error.to_string()))?,
                    ),
                });
                if let Some(snapshot) = remote_snapshot.as_ref() {
                    current_artifacts.push(DeliveryArtifactHash {
                        kind: "remote_artifact_set".into(),
                        relative_path: "agentum+ssh://artifact-set".into(),
                        content_hash: snapshot.artifact_set_sha256.clone(),
                    });
                }
                let offered = envelope
                    .artifact_hashes
                    .iter()
                    .map(|artifact| {
                        (
                            &artifact.kind,
                            &artifact.relative_path,
                            &artifact.content_hash,
                        )
                    })
                    .collect::<std::collections::BTreeSet<_>>();
                let actual = current_artifacts
                    .iter()
                    .map(|artifact| {
                        (
                            &artifact.kind,
                            &artifact.relative_path,
                            &artifact.content_hash,
                        )
                    })
                    .collect::<std::collections::BTreeSet<_>>();
                if actual != offered {
                    return Err(ApiError::Custom(
                        StatusCode::PRECONDITION_FAILED,
                        json!({
                            "error": "delivery_preview_stale",
                            "message": "artifact hashes changed after preview; create a new preview"
                        }),
                    ));
                }
            }
            let selected =
                select_delivery_actions(&envelope.actions, &actions, preview.status == "pending")?;
            let serialized = selected
                .iter()
                .map(|action| {
                    serde_json::to_string(action)
                        .map(|intent_json| (action, intent_json))
                        .map_err(|error| ApiError::Internal(error.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let borrowed = serialized
                .iter()
                .map(|(action, intent_json)| NewDeliveryAction {
                    action_id: &action.id,
                    action_type: &action.kind,
                    intent_json,
                })
                .collect::<Vec<_>>();
            let response = json!({
                "runId": run.run_id,
                "revision": expected_revision + 1,
                "phase": "ready",
                "status": "succeeded",
                "previewId": preview.preview_id,
                "digest": preview.digest,
                "deliveryActions": selected.iter().map(|action| json!({
                    "actionId": action.id,
                    "type": action.kind,
                    "status": "pending"
                })).collect::<Vec<_>>()
            });
            state
                .store
                .sdd_confirm_delivery(ConfirmDelivery {
                    request_id: &request_id,
                    request_hash: &request_hash,
                    run_id: &run.run_id,
                    expected_revision,
                    actor_id: &actor.id,
                    token_hash: &token_hash,
                    digest: &preview.digest,
                    selected: &borrowed,
                    response_json: &response.to_string(),
                })
                .await?;
            crate::sdd::delivery::spawn(state.clone(), preview.preview_id);
            Ok(Json(response))
        }
    }
}

async fn spawn_run_execution(state: &AppState, run_id: &str) -> Result<(), ApiError> {
    if state.store.sdd_remote_run(run_id).await?.is_some() {
        crate::sdd::remote_lifecycle::spawn(state.clone(), run_id.to_owned());
    } else {
        crate::sdd::lifecycle::spawn(state.clone(), run_id.to_owned());
    }
    Ok(())
}

async fn cancel_run_execution(state: &AppState, run_id: &str) {
    if state
        .store
        .sdd_remote_run(run_id)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        let _ = crate::sdd::remote_lifecycle::cancel_run(state, run_id).await;
    } else {
        crate::sdd::lifecycle::cancel_run(run_id);
    }
}

fn openspec_delivery_destination(actions: &[PreparedDeliveryAction]) -> Option<String> {
    actions
        .iter()
        .find(|action| action.kind == "openspec_export")
        .and_then(|action| action.openspec_export.as_ref())
        .map(|preview| preview.destination.clone())
}

async fn inspect_remote_delivery_state(
    state: &AppState,
    run: &agentum_store::sdd::SddRunRecord,
    request_key: &str,
    openspec_destination: Option<String>,
) -> Result<Option<RemoteDeliverySnapshotResult>, ApiError> {
    let Some(projection) = state.store.sdd_remote_run(&run.run_id).await? else {
        return Ok(None);
    };
    let plan: RemoteLifecyclePlan = serde_json::from_str(&projection.plan_json)
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let checkpoint: RemoteLifecycleCheckpoint =
        serde_json::from_str(&projection.checkpoint_json)
            .map_err(|error| ApiError::Internal(error.to_string()))?;
    if !checkpoint.is_ready()
        || projection.status != "succeeded"
        || plan.run_id != run.run_id
        || plan.spec_id != run.spec_id
        || plan.base_commit != run.base_commit
        || plan.host_id != projection.host_id
        || plan.repository_identity_sha256 != projection.repository_identity_sha256
        || plan.artifact_set_id != projection.artifact_set_id
        || checkpoint.run_id != run.run_id
        || checkpoint.approval_digest != plan.approval_digest
    {
        return Err(ApiError::Conflict(
            "remote delivery requires the exact durable Ready projection".into(),
        ));
    }
    let host_id = Uuid::parse_str(&projection.host_id)
        .map_err(|_| ApiError::Internal("remote delivery host identity is invalid".into()))?;
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(projection.host_id.clone()))?;
    let client = crate::sdd::remote_lifecycle::client_for_host(host).map_err(|error| {
        ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "error": "remote_delivery_transport_unavailable",
                "message": error.to_string(),
                "localFallback": false
            }),
        )
    })?;
    let material = serde_json::to_vec(&json!({
        "runId": run.run_id,
        "requestKey": request_key,
        "workspace": checkpoint.workspace_state_sha256,
        "openspecDestination": openspec_destination,
    }))
    .map_err(|error| ApiError::Internal(error.to_string()))?;
    let request = RemoteDeliverySnapshotRequest {
        schema_version: REMOTE_SDD_SCHEMA_VERSION,
        request_id: format!("delivery-inspect-{}", &sha256(material)[..32]),
        host_id: plan.host_id,
        run_id: plan.run_id,
        spec_id: plan.spec_id,
        spec_revision: plan.spec_revision,
        repository_identity_sha256: plan.repository_identity_sha256,
        artifact_set_id: plan.artifact_set_id,
        base_commit: plan.base_commit,
        approval_digest: plan.approval_digest,
        expected_workspace_state_sha256: checkpoint.workspace_state_sha256,
        openspec_destination,
        timeout_ms: plan.timeout_ms.min(120_000),
        output_limit: plan.output_limit,
    };
    let validation_request = request.clone();
    let result = tokio::time::timeout(
        Duration::from_millis(request.timeout_ms),
        RemoteSddTransport::inspect_delivery(client.as_ref(), request),
    )
    .await
    .map_err(|_| {
        ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "error": "remote_delivery_inspection_timed_out",
                "localFallback": false
            }),
        )
    })?
    .map_err(|error| {
        ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "error": "remote_delivery_inspection_failed",
                "message": error.to_string(),
                "localFallback": false
            }),
        )
    })?;
    crate::sdd::remote::validate_delivery_snapshot_result(&validation_request, &result).map_err(
        |error| {
            ApiError::Custom(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({
                    "error": "remote_delivery_inspection_invalid",
                    "message": error.to_string(),
                    "localFallback": false
                }),
            )
        },
    )?;
    if result.branch_name != run.branch_name {
        return Err(ApiError::Conflict(
            "remote delivery branch changed after Ready".into(),
        ));
    }
    if result.openspec_destination_exists == Some(true) {
        return Err(ApiError::Conflict(
            "OpenSpec export destination already exists; one-shot export never overwrites".into(),
        ));
    }
    Ok(Some(result))
}

fn delivery_error(error: crate::sdd::delivery::DeliveryError) -> ApiError {
    match error {
        crate::sdd::delivery::DeliveryError::Invalid(message) => ApiError::BadRequest(message),
        crate::sdd::delivery::DeliveryError::Command(message) => ApiError::Conflict(message),
        crate::sdd::delivery::DeliveryError::Conflict(message) => ApiError::Conflict(message),
        crate::sdd::delivery::DeliveryError::Precondition(message) => ApiError::Custom(
            StatusCode::PRECONDITION_FAILED,
            json!({ "error": "delivery_precondition_failed", "message": message }),
        ),
        crate::sdd::delivery::DeliveryError::TransitionChoiceRequired {
            provider,
            target,
            choices,
        } => ApiError::Custom(
            StatusCode::CONFLICT,
            json!({
                "error": "tracker_transition_choice_required",
                "provider": provider,
                "target": target,
                "choices": choices
            }),
        ),
        crate::sdd::delivery::DeliveryError::Store(error) => error.into(),
        other => ApiError::Internal(other.to_string()),
    }
}

async fn validate_delivery_capabilities(
    state: &AppState,
    spec_id: &str,
    actions: &[PreparedDeliveryAction],
) -> Result<(), ApiError> {
    let needs_tracker = actions.iter().any(|action| {
        matches!(
            action.kind.as_str(),
            "tracker_comment" | "tracker_status" | "tracker_field_update"
        )
    });
    let tracker_link = if needs_tracker {
        Some(
            state
                .store
                .sdd_external_link_for_spec(spec_id)
                .await?
                .ok_or_else(|| {
                    ApiError::BadRequest(
                        "tracker delivery requires an imported external work-item reference".into(),
                    )
                })?,
        )
    } else {
        None
    };
    if let Some(link) = tracker_link.as_ref() {
        if !matches!(link.provider.as_str(), "github" | "linear" | "jira") {
            return Err(ApiError::Custom(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({
                    "error": "delivery_capability_unavailable",
                    "capability": "tracker_mutation",
                    "provider": link.provider,
                    "message": "this tracker provider has no delivery adapter"
                }),
            ));
        }
        if matches!(link.provider.as_str(), "linear" | "jira")
            && !state.sdd_credentials.status().available
        {
            return Err(ApiError::Custom(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({
                    "error": "delivery_capability_unavailable",
                    "capability": "secure_credential_vault",
                    "provider": link.provider,
                    "message": "tracker delivery requires an available secure credential vault"
                }),
            ));
        }
    }
    let needs_gh = actions
        .iter()
        .any(|action| matches!(action.kind.as_str(), "pull_request" | "release"))
        || tracker_link
            .as_ref()
            .is_some_and(|link| link.provider == "github");
    if needs_gh && which::which("gh").is_err() {
        return Err(ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "error": "delivery_capability_unavailable",
                "capability": "github_cli",
                "message": "GitHub CLI is required for the selected delivery actions"
            }),
        ));
    }
    Ok(())
}

fn select_delivery_actions<'a>(
    offered: &'a [PreparedDeliveryAction],
    selected_ids: &[String],
    first_confirmation: bool,
) -> Result<Vec<&'a PreparedDeliveryAction>, ApiError> {
    let selected_count = selected_ids.len();
    let selected_ids = selected_ids
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    if selected_ids.len() != selected_count {
        return Err(ApiError::BadRequest(
            "delivery action ids must be unique".into(),
        ));
    }
    let selected = offered
        .iter()
        .filter(|action| selected_ids.contains(action.id.as_str()))
        .collect::<Vec<_>>();
    if selected.len() != selected_ids.len() {
        return Err(ApiError::BadRequest(
            "one or more delivery action ids were not offered by the preview".into(),
        ));
    }
    if first_confirmation {
        for action in &selected {
            if let Some(missing) = action
                .depends_on
                .iter()
                .find(|dependency| !selected_ids.contains(dependency.as_str()))
            {
                return Err(ApiError::BadRequest(format!(
                    "delivery action {} requires selected dependency {missing}",
                    action.id
                )));
            }
        }
    }
    Ok(selected)
}

fn ensure_nonterminal(
    run: &agentum_store::sdd::SddRunRecord,
    action: &str,
) -> Result<(), ApiError> {
    if run.quarantined != 0 {
        Err(ApiError::Conflict(format!(
            "cannot {action} a quarantined run; preserve and resolve its recovery evidence first"
        )))
    } else if run.phase == "completed"
        || matches!(run.status.as_str(), "canceled" | "failed" | "succeeded")
    {
        Err(ApiError::Conflict(format!(
            "cannot {action} from terminal state {} / {}",
            run.phase, run.status
        )))
    } else {
        Ok(())
    }
}

fn lifecycle_execution_enabled(run: &agentum_store::sdd::SddRunRecord) -> bool {
    serde_json::from_str::<Value>(&run.policy_json)
        .ok()
        .and_then(|policy| policy.get("implementationEnabled").and_then(Value::as_bool))
        .unwrap_or(false)
}

fn workflow_control(run: &agentum_store::sdd::SddRunRecord) -> Result<&'static str, ApiError> {
    let policy = serde_json::from_str::<Value>(&run.policy_json)
        .map_err(|_| ApiError::Internal("run policy is malformed".into()))?;
    // Reading the approval-bound run policy avoids trusting a mutable caller
    // field or an execution default for this authorization decision.
    match policy.get("control").and_then(Value::as_str) {
        Some("guarded") => Ok("guarded"),
        Some("interactive") => Ok("interactive"),
        Some("autopilot") => Ok("autopilot"),
        _ => Err(ApiError::Internal(
            "run policy has no recognized control mode".into(),
        )),
    }
}

fn approved_phase(purpose: &str) -> Result<&'static str, ApiError> {
    match purpose {
        "specification" => Ok("design"),
        "design" => Ok("planning"),
        "planning" => Ok("implementation"),
        "implementation" => Ok("verification"),
        "verification" => Ok("review"),
        "review" => Ok("ready"),
        _ => Err(ApiError::Internal("unknown approval purpose".into())),
    }
}

fn approved_status(purpose: &str) -> Result<&'static str, ApiError> {
    match purpose {
        "specification" | "design" | "planning" | "implementation" | "verification" => Ok("queued"),
        "review" => Ok("succeeded"),
        _ => Err(ApiError::Internal("unknown approval purpose".into())),
    }
}

#[allow(clippy::too_many_arguments)]
async fn transition(
    state: &AppState,
    run: &agentum_store::sdd::SddRunRecord,
    request_id: &str,
    request_hash: &str,
    expected_revision: i64,
    phase: &str,
    status: &str,
    blocker: Option<&str>,
    event_kind: &str,
) -> Result<Json<Value>, ApiError> {
    let response = json!({
        "runId": run.run_id,
        "revision": expected_revision + 1,
        "phase": phase,
        "status": status
    });
    state
        .store
        .sdd_transition(TransitionMutation {
            request_id,
            request_hash,
            run_id: &run.run_id,
            expected_revision,
            phase,
            status,
            blocker,
            event_kind,
            response_json: &response.to_string(),
        })
        .await?;
    Ok(Json(response))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn submit_artifact(
    state: &AppState,
    run: &agentum_store::sdd::SddRunRecord,
    request_id: &str,
    request_hash: &str,
    expected_revision: i64,
    kind: ArtifactKind,
    content: &str,
    expected_content_hash: &str,
    attempt_id: &str,
) -> Result<Json<Value>, ApiError> {
    if attempt_id.trim().is_empty() {
        return Err(ApiError::BadRequest("attemptId is required".into()));
    }
    let policy: Value = serde_json::from_str(&run.policy_json)
        .map_err(|error| ApiError::Internal(format!("invalid run policy: {error}")))?;
    if kind != ArtifactKind::Specification
        && policy.get("implementationEnabled").and_then(Value::as_bool) != Some(true)
    {
        return Err(ApiError::Conflict(
            "implementation artifacts are not available for this run".into(),
        ));
    }
    let expected_phase = match kind {
        ArtifactKind::Specification => "specification",
        ArtifactKind::Design => "design",
        ArtifactKind::Plan => "planning",
        ArtifactKind::Decisions => "design",
        ArtifactKind::Review => "review",
    };
    if run.phase != expected_phase || run.status != "running" {
        return Err(ApiError::Conflict(format!(
            "{} can be submitted only by a running {expected_phase} attempt",
            kind.file_name()
        )));
    }
    let spec = state
        .store
        .sdd_get_spec(&run.spec_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(run.spec_id.clone()))?;
    let Some((attempt_provider, attempt_status)) = state
        .store
        .sdd_attempt_for_run(&run.run_id, attempt_id)
        .await?
    else {
        return Err(ApiError::Forbidden(
            "attempt does not belong to this run".into(),
        ));
    };
    if attempt_provider != spec.provider || attempt_status != "running" {
        return Err(ApiError::Forbidden(
            "attempt is not an active provider attempt for this run".into(),
        ));
    }
    let canonical: SpecId =
        spec.spec_id
            .parse()
            .map_err(|error: agentum_core::sdd::SddContractError| {
                ApiError::Internal(error.to_string())
            })?;
    let file_name = kind.file_name();
    let relative_path = format!(".agentum/specs/{}/{file_name}", spec.slug);
    validate_relative_path(&relative_path)
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let target = PathBuf::from(&run.authoritative_path).join(&relative_path);
    let (published_content, new_spec_revision) = match kind {
        ArtifactKind::Specification => (
            render_spec(
                &canonical,
                spec.current_revision + 1,
                &spec.title,
                None,
                content,
            )
            .map_err(artifact_error)?,
            Some(spec.current_revision + 1),
        ),
        ArtifactKind::Plan => {
            validate_plan(content, &canonical, spec.current_revision)?;
            (normalize_text(content), None)
        }
        _ if content.trim().is_empty() => {
            return Err(ApiError::BadRequest(
                "artifact content cannot be empty".into(),
            ));
        }
        _ => (normalize_text(content), None),
    };
    let preimage = match content_hash(&target).map_err(artifact_error)? {
        hash if hash == MISSING_HASH => None,
        _ => Some(read_bytes(&target).map_err(artifact_error)?.0),
    };
    let new_hash = atomic_write(
        &target,
        published_content.as_bytes(),
        Some(expected_content_hash),
    )
    .map_err(artifact_error)?;
    let high_risk_approval =
        spec.profile == "high_risk" && matches!(kind, ArtifactKind::Design | ArtifactKind::Plan);
    let approval_purpose = match kind {
        ArtifactKind::Specification => Some("specification"),
        ArtifactKind::Design if high_risk_approval => Some("design"),
        ArtifactKind::Plan if high_risk_approval => Some("planning"),
        _ => None,
    };
    let (next_phase, next_status) = match kind {
        ArtifactKind::Specification => ("specification", "waiting"),
        ArtifactKind::Design if high_risk_approval => ("design", "waiting"),
        ArtifactKind::Design if spec.control == "interactive" => ("planning", "paused"),
        ArtifactKind::Design => ("planning", "queued"),
        ArtifactKind::Plan if high_risk_approval => ("planning", "waiting"),
        ArtifactKind::Plan if spec.control == "interactive" => ("implementation", "paused"),
        ArtifactKind::Plan => ("implementation", "queued"),
        ArtifactKind::Decisions => ("design", "running"),
        ArtifactKind::Review => ("ready", "succeeded"),
    };
    let approval_id = approval_purpose.map(|_| Uuid::new_v4().to_string());
    let (review_evidence_hashes_json, review_evidence_digest) = if kind == ArtifactKind::Review {
        let hashes = state
            .store
            .sdd_browser_evidence_manifest_hashes(&run.run_id)
            .await?;
        let encoded = serde_json::to_string(&hashes)
            .map_err(|error| ApiError::Internal(error.to_string()))?;
        let digest = sha256(encoded.as_bytes());
        (Some(encoded), Some(digest))
    } else {
        (None, None)
    };
    let approval_digest_value = if approval_purpose.is_some() {
        let mut digest_artifacts: Vec<(String, String)> = state
            .store
            .sdd_artifacts(&run.run_id)
            .await?
            .into_iter()
            .map(|artifact| (artifact.relative_path, artifact.content_hash))
            .collect();
        digest_artifacts.retain(|(path, _)| path != &relative_path);
        digest_artifacts.push((relative_path.clone(), new_hash.clone()));
        let digest_refs: Vec<(&str, &str)> = digest_artifacts
            .iter()
            .map(|(path, hash)| (path.as_str(), hash.as_str()))
            .collect();
        Some(approval_digest(
            &canonical,
            new_spec_revision.unwrap_or(spec.current_revision),
            &digest_refs,
            &policy,
            &run.workspace_fingerprint,
        ))
    } else {
        None
    };
    let response = json!({
        "runId": run.run_id,
        "revision": expected_revision + 1,
        "phase": next_phase,
        "status": next_status,
        "artifact": {
            "kind": kind,
            "path": relative_path,
            "contentHash": new_hash,
            "evidenceDigest": review_evidence_digest
        },
        "approval": approval_id.as_ref().zip(approval_digest_value.as_ref()).map(|(id, digest)| json!({
            "approvalId": id, "digest": digest, "purpose": approval_purpose, "status": "pending"
        }))
    });
    let submitted_by = format!("agent:{}:{}", spec.provider, attempt_id);
    let result = state
        .store
        .sdd_submit_artifact(ArtifactMutation {
            request_id,
            request_hash,
            run_id: &run.run_id,
            expected_revision,
            kind: artifact_kind_name(kind),
            relative_path: &relative_path,
            content_hash: &new_hash,
            content: matches!(kind, ArtifactKind::Specification | ArtifactKind::Plan)
                .then_some(published_content.as_str()),
            attempt_id,
            submitted_by: &submitted_by,
            approval_id: approval_id.as_deref(),
            approval_digest: approval_digest_value.as_deref(),
            approval_purpose,
            evidence_digest: review_evidence_digest.as_deref(),
            evidence_manifest_hashes_json: review_evidence_hashes_json.as_deref(),
            next_phase,
            next_status,
            response_json: &response.to_string(),
        })
        .await;
    if let Err(error) = result {
        compensate_artifact(&target, preimage.as_deref(), &new_hash);
        return Err(error.into());
    }
    Ok(Json(response))
}

fn compensate_artifact(target: &FsPath, preimage: Option<&[u8]>, published_hash: &str) {
    match preimage {
        Some(bytes) => {
            let _ = atomic_write(target, bytes, Some(published_hash));
        }
        None => {
            let _ = atomic_remove(target, published_hash);
        }
    }
}

async fn get_artifacts(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    reconcile_external_spec(&state, &run_id).await?;
    let run = state
        .store
        .sdd_get_run(&run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(run_id.clone()))?;
    let records = state.store.sdd_artifacts(&run_id).await?;
    if state.store.sdd_remote_run(&run_id).await?.is_some() {
        let payloads = state
            .store
            .sdd_remote_artifact_payloads(&run_id)
            .await?
            .into_iter()
            .map(|payload| (payload.artifact_revision_id.clone(), payload))
            .collect::<std::collections::HashMap<_, _>>();
        let mut artifacts = Vec::with_capacity(records.len());
        for record in records {
            let payload = payloads.get(&record.artifact_revision_id).ok_or_else(|| {
                ApiError::Internal(format!(
                    "remote artifact payload is missing: {}",
                    record.artifact_revision_id
                ))
            })?;
            let actual_hash = sha256(payload.content.as_bytes());
            artifacts.push(json!({
                "metadata": record,
                "content": payload.content,
                "externallyModified": actual_hash != payload.content_sha256
                    || actual_hash != record.content_hash,
                "actualContentHash": actual_hash,
                "remoteProjected": true
            }));
        }
        return Ok(Json(json!({ "artifacts": artifacts })));
    }
    let mut artifacts = Vec::with_capacity(records.len());
    for record in records {
        validate_relative_path(&record.relative_path)
            .map_err(|error| ApiError::Internal(error.to_string()))?;
        let path = PathBuf::from(&run.authoritative_path).join(&record.relative_path);
        match read_text(&path) {
            Ok((content, actual_hash)) => artifacts.push(json!({
                "metadata": record,
                "content": content,
                "externallyModified": actual_hash != record.content_hash,
                "actualContentHash": actual_hash
            })),
            Err(error) => artifacts.push(json!({
                "metadata": record,
                "content": "",
                "externallyModified": true,
                "actualContentHash": sha256(format!("unreadable:{error}")),
                "readError": error.to_string()
            })),
        }
    }
    Ok(Json(json!({ "artifacts": artifacts })))
}

async fn reconcile_external_spec(state: &AppState, run_id: &str) -> Result<(), ApiError> {
    let run = state
        .store
        .sdd_get_run(run_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(run_id.to_owned()))?;
    let spec = state
        .store
        .sdd_get_spec(&run.spec_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(run.spec_id.clone()))?;
    // Remote authoritative paths are host-owned identifiers, never desktop
    // filesystem paths. Even a restored or manually imported DB row remains
    // fail-closed until the remote checkpoint projection is complete.
    if crate::routes::repos::resolve_repo_host_id(&run.repo_id)?.is_some() {
        if state.store.sdd_remote_run(run_id).await?.is_some() {
            return Ok(());
        }
        return Err(ApiError::Conflict(
            "remote run is missing its authoritative desktop projection".into(),
        ));
    }
    let Some(metadata) = state
        .store
        .sdd_artifacts(run_id)
        .await?
        .into_iter()
        .find(|artifact| artifact.kind == "specification")
    else {
        return Ok(());
    };
    let canonical: SpecId =
        spec.spec_id
            .parse()
            .map_err(|error: agentum_core::sdd::SddContractError| {
                ApiError::Internal(error.to_string())
            })?;
    if let Err(error) = artifacts::validate_existing_root(
        FsPath::new(&run.authoritative_path),
        &canonical,
        &spec.slug,
    ) {
        let sentinel = sha256(format!("invalid-root:{error}"));
        return block_invalid_external(state, &run, &sentinel, &error.to_string()).await;
    }
    let path = PathBuf::from(&run.authoritative_path).join(&metadata.relative_path);
    let (content, actual_hash) = match read_text(&path) {
        Ok(value) => value,
        Err(error) => {
            let sentinel = sha256(format!("unreadable:{error}"));
            return block_invalid_external(state, &run, &sentinel, &error.to_string()).await;
        }
    };
    if actual_hash == metadata.content_hash {
        return Ok(());
    }
    let (header, _) = match artifacts::parse_spec(&content) {
        Ok(parsed) => parsed,
        Err(error) => {
            return block_invalid_external(state, &run, &actual_hash, &error.to_string()).await;
        }
    };
    if header.id != canonical {
        return block_invalid_external(
            state,
            &run,
            &actual_hash,
            "external spec attempted to change its canonical id",
        )
        .await;
    }
    if header.revision != spec.current_revision + 1 {
        return block_invalid_external(
            state,
            &run,
            &actual_hash,
            &format!(
                "external spec revision must be {} (found {})",
                spec.current_revision + 1,
                header.revision
            ),
        )
        .await;
    }
    // Stop new provider/lifecycle work before competing for the aggregate CAS.
    // A phase transition already in flight may still win once, so retry only
    // this exact validated revision and content hash against freshly loaded
    // aggregate revisions. This gives a user edit priority without weakening
    // the revision/hash binding or overwriting their file.
    crate::sdd::providers::cancel_run(run_id);
    crate::sdd::lifecycle::cancel_run(run_id);
    let mut expected_run_revision = run.aggregate_revision;
    for _ in 0..16 {
        match state
            .store
            .sdd_import_external_spec(ExternalSpecMutation {
                run_id,
                expected_run_revision,
                spec_revision: header.revision,
                title: &header.title,
                relative_path: &metadata.relative_path,
                content_hash: &actual_hash,
                content: &content,
            })
            .await
        {
            Ok(_) => return Ok(()),
            Err(agentum_store::StoreError::StaleRevision { .. }) => {
                let current_spec = state
                    .store
                    .sdd_get_spec(&run.spec_id)
                    .await?
                    .ok_or_else(|| ApiError::NotFound(run.spec_id.clone()))?;
                let imported = state
                    .store
                    .sdd_artifacts(run_id)
                    .await?
                    .into_iter()
                    .find(|artifact| artifact.kind == "specification")
                    .is_some_and(|artifact| artifact.content_hash == actual_hash);
                if current_spec.current_revision == header.revision && imported {
                    return Ok(());
                }
                if header.revision != current_spec.current_revision + 1 {
                    return Err(ApiError::Conflict(
                        "external specification changed while its revision was being imported"
                            .into(),
                    ));
                }
                expected_run_revision = state
                    .store
                    .sdd_get_run(run_id)
                    .await?
                    .ok_or_else(|| ApiError::NotFound(run_id.to_owned()))?
                    .aggregate_revision;
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(ApiError::Conflict(
        "external specification import could not quiesce the active lifecycle; retry".into(),
    ))
}

async fn block_invalid_external(
    state: &AppState,
    run: &agentum_store::sdd::SddRunRecord,
    actual_hash: &str,
    reason: &str,
) -> Result<(), ApiError> {
    let blocker = format!("invalid external specification: {reason}");
    if run.phase == "completed"
        || matches!(run.status.as_str(), "canceled" | "failed" | "succeeded")
    {
        return Err(ApiError::Conflict(blocker));
    }
    if run.status == "blocked" && run.blocker.as_deref() == Some(blocker.as_str()) {
        return Ok(());
    }
    let request_id = format!(
        "external-invalid:{}:{}",
        run.run_id,
        &actual_hash[..16.min(actual_hash.len())]
    );
    let request_hash = sha256(format!("external-invalid:{}:{actual_hash}", run.run_id));
    if state
        .store
        .sdd_idempotent_response(&format!("run:{}", run.run_id), &request_id, &request_hash)
        .await?
        .is_some()
    {
        return Ok(());
    }
    let response = json!({
        "runId": run.run_id,
        "revision": run.aggregate_revision + 1,
        "phase": run.phase,
        "status": "blocked",
        "blocker": blocker,
        "actualContentHash": actual_hash
    });
    state
        .store
        .sdd_transition(TransitionMutation {
            request_id: &request_id,
            request_hash: &request_hash,
            run_id: &run.run_id,
            expected_revision: run.aggregate_revision,
            phase: &run.phase,
            status: "blocked",
            blocker: Some(&blocker),
            event_kind: "sdd.spec.external_revision_invalid",
            response_json: &response.to_string(),
        })
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    #[serde(default)]
    after: i64,
}

async fn get_events(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Query(query): Query<EventQuery>,
) -> Result<Json<Value>, ApiError> {
    if state.store.sdd_get_run(&run_id).await?.is_none() {
        return Err(ApiError::NotFound(run_id));
    }
    let events = state
        .store
        .sdd_events_after(&run_id, query.after.max(0), 500)
        .await?;
    Ok(Json(json!({ "events": wire_events(events) })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsSocketQuery {
    repo_id: String,
    #[serde(default)]
    after: i64,
}

async fn events_socket(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<EventsSocketQuery>,
) -> Result<Response, ApiError> {
    let _ = crate::routes::repos::resolve_repo_path(&query.repo_id)?;
    Ok(ws
        .on_upgrade(move |socket| stream_events(socket, state, query))
        .into_response())
}

async fn stream_events(mut socket: WebSocket, state: AppState, query: EventsSocketQuery) {
    let mut cursor = query.after.max(0);
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tick.tick().await;
        let events = match state
            .store
            .sdd_repo_events_after(&query.repo_id, cursor, 200)
            .await
        {
            Ok(events) => events,
            Err(_) => break,
        };
        for event in events {
            cursor = event.cursor;
            let wire = wire_event(event);
            if socket
                .send(Message::Text(wire.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
        }
    }
}

fn wire_events(events: Vec<SddEventRecord>) -> Vec<Value> {
    events.into_iter().map(wire_event).collect()
}

fn wire_event(event: SddEventRecord) -> Value {
    json!({
        "cursor": event.cursor,
        "eventId": event.event_id,
        "repoId": event.repo_id,
        "specId": event.spec_id,
        "runId": event.run_id,
        "revision": event.aggregate_revision,
        "kind": event.kind,
        "payload": serde_json::from_str::<Value>(&event.payload_json).unwrap_or(Value::Null),
        "createdAt": event.created_at
    })
}

fn initial_spec_body(title: &str, goal: &str) -> String {
    format!(
        "# {title}\n\n## Goal\n\n{goal}\n\n## Requirements\n\n- RQ-001 Agentum must satisfy the stated goal without unrelated repository changes.\n\n## Acceptance criteria\n\n- AC-001 The goal is demonstrably satisfied and existing behavior remains intact."
    )
}

fn approval_digest(
    spec_id: &SpecId,
    spec_revision: i64,
    artifacts: &[(&str, &str)],
    policy: &Value,
    workspace_fingerprint: &str,
) -> String {
    let mut sorted = artifacts.to_vec();
    sorted.sort_unstable();
    sha256(
        serde_json::to_vec(&json!({
            "specId": spec_id,
            "specRevision": spec_revision,
            "artifacts": sorted,
            "policy": policy,
            "workspaceFingerprint": workspace_fingerprint
        }))
        .expect("digest payload serializes"),
    )
}

fn request_digest(value: &impl Serialize) -> Result<String, ApiError> {
    serde_json::to_vec(value)
        .map(sha256)
        .map_err(|error| ApiError::Internal(format!("could not bind request: {error}")))
}

fn validate_plan(content: &str, spec_id: &SpecId, spec_revision: i64) -> Result<(), ApiError> {
    crate::sdd::lifecycle::validate_plan(content, spec_id, spec_revision)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let plan: PlanArtifact = serde_json::from_str(content)
        .map_err(|error| ApiError::BadRequest(format!("invalid plan.json: {error}")))?;
    if &plan.spec_id != spec_id || plan.spec_revision != spec_revision || plan.schema_version != 1 {
        return Err(ApiError::BadRequest(
            "plan identity or revision does not match the run".into(),
        ));
    }
    let ids: std::collections::HashSet<_> =
        plan.tasks.iter().map(|task| task.id.as_str()).collect();
    if ids.len() != plan.tasks.len() || plan.tasks.iter().any(|task| task.id.trim().is_empty()) {
        return Err(ApiError::BadRequest(
            "plan task ids must be non-empty and unique".into(),
        ));
    }
    for task in &plan.tasks {
        for path in task.read_scopes.iter().chain(&task.write_scopes) {
            validate_relative_path(path)
                .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        }
        if task
            .dependencies
            .iter()
            .any(|dependency| !ids.contains(dependency.as_str()))
        {
            return Err(ApiError::BadRequest(format!(
                "task {} has an unknown dependency",
                task.id
            )));
        }
    }
    // DFS catches self-dependencies and longer cycles.
    fn visit<'a>(
        id: &'a str,
        tasks: &std::collections::HashMap<&'a str, &'a agentum_core::sdd::PlanTask>,
        visiting: &mut std::collections::HashSet<&'a str>,
        visited: &mut std::collections::HashSet<&'a str>,
    ) -> bool {
        if visited.contains(id) {
            return false;
        }
        if !visiting.insert(id) {
            return true;
        }
        let cycle = tasks[id]
            .dependencies
            .iter()
            .any(|dependency| visit(dependency, tasks, visiting, visited));
        visiting.remove(id);
        visited.insert(id);
        cycle
    }
    let tasks: std::collections::HashMap<_, _> = plan
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect();
    let mut visiting = std::collections::HashSet::new();
    let mut visited = std::collections::HashSet::new();
    if tasks
        .keys()
        .any(|id| visit(id, &tasks, &mut visiting, &mut visited))
    {
        return Err(ApiError::BadRequest(
            "plan task graph contains a cycle".into(),
        ));
    }
    Ok(())
}

fn phase_rank(phase: &str) -> usize {
    match phase {
        "specification" => 0,
        "design" => 1,
        "planning" => 2,
        "implementation" => 3,
        "verification" => 4,
        "review" => 5,
        "ready" => 6,
        "delivery" => 7,
        "completed" => 8,
        _ => usize::MAX,
    }
}

fn normalize_text(content: &str) -> String {
    let mut value = content.replace("\r\n", "\n").trim().to_owned();
    value.push('\n');
    value
}

fn validate_provider(provider: &str) -> Result<ProviderAdapter, ApiError> {
    if provider.starts_with("custom:") {
        resolve_provider(provider).map_err(|error| {
            ApiError::Custom(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({
                    "error": "provider_capability_unavailable",
                    "provider": provider,
                    "message": error.to_string()
                }),
            )
        })
    } else {
        BundledProvider::get(provider)
            .map(ProviderAdapter::Bundled)
            .ok_or_else(|| ApiError::BadRequest(format!("unsupported SDD provider: {provider}")))
    }
}

fn validate_request_id(request_id: &str) -> Result<(), ApiError> {
    if request_id.trim().is_empty() || request_id.len() > 128 {
        Err(ApiError::BadRequest(
            "requestId must contain 1..128 characters".into(),
        ))
    } else {
        Ok(())
    }
}

fn validate_phase(phase: &str) -> Result<(), ApiError> {
    if matches!(
        phase,
        "specification" | "design" | "planning" | "implementation" | "verification" | "review"
    ) {
        Ok(())
    } else {
        Err(ApiError::BadRequest("phase cannot be reopened".into()))
    }
}

fn profile_name(value: WorkflowProfile) -> &'static str {
    match value {
        WorkflowProfile::Standard => "standard",
        WorkflowProfile::HighRisk => "high_risk",
    }
}

fn control_name(value: WorkflowControl) -> &'static str {
    match value {
        WorkflowControl::Guarded => "guarded",
        WorkflowControl::Interactive => "interactive",
        WorkflowControl::Autopilot => "autopilot",
    }
}

fn source_checkout_name(value: SourceCheckout) -> &'static str {
    match value {
        SourceCheckout::RequireClean => "require_clean",
        SourceCheckout::CommittedBase => "committed_base",
        SourceCheckout::Snapshot => "snapshot",
    }
}

fn artifact_kind_name(value: ArtifactKind) -> &'static str {
    match value {
        ArtifactKind::Specification => "specification",
        ArtifactKind::Design => "design",
        ArtifactKind::Plan => "plan",
        ArtifactKind::Decisions => "decisions",
        ArtifactKind::Review => "review",
    }
}

fn stale(current: i64, expected: i64) -> ApiError {
    ApiError::Custom(
        StatusCode::CONFLICT,
        json!({
            "error": "stale_revision",
            "expectedRevision": expected,
            "currentRevision": current
        }),
    )
}

fn artifact_error(error: ArtifactError) -> ApiError {
    match error {
        ArtifactError::ContentChanged { expected, current } => ApiError::Custom(
            StatusCode::PRECONDITION_FAILED,
            json!({ "error": "content_hash_mismatch", "expectedHash": expected, "currentHash": current }),
        ),
        ArtifactError::InvalidSpec(message) | ArtifactError::InvalidText(message) => {
            ApiError::BadRequest(message)
        }
        ArtifactError::UnsafeRoot(message)
        | ArtifactError::UnownedRoot(message)
        | ArtifactError::Collision(message) => ApiError::Conflict(message),
        other => ApiError::Internal(other.to_string()),
    }
}

fn workspace_error(error: WorkspaceError) -> ApiError {
    match error {
        WorkspaceError::DirtySource
        | WorkspaceError::SnapshotChanged
        | WorkspaceError::UnsafeRepository(_)
        | WorkspaceError::Collision(_) => ApiError::Conflict(error.to_string()),
        WorkspaceError::Git(_) | WorkspaceError::UnsupportedSnapshot(_) => {
            ApiError::BadRequest(error.to_string())
        }
        other => ApiError::Internal(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::future::Future;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use crate::sdd::evidence::{
        BrowserAssertion, BrowserAssertionStatus, BrowserConsoleSummary, BrowserDiagnosticCoverage,
        BrowserNetworkSummary, BrowserRuntime, BrowserTarget,
    };
    use crate::sdd::remote::{
        RemoteArtifactPayload, RemoteBrowserBlob, RemoteBrowserCheckResult,
        RemoteDeliveryActionRequest, RemoteDeliveryActionResult, RemoteDeliveryActionStatus,
        RemoteDeliverySnapshotRequest, RemoteDeliverySnapshotResult, RemoteImplementationEvidence,
        RemoteLifecycleError, RemoteLifecyclePhase, RemotePhaseRequest, RemotePhaseResult,
        RemoteProbeResult, RemoteTaskCompletionEvidence, RemoteVerificationEvidence,
    };
    use agentum_core::sdd::CommandSpec;
    use agentum_core::{Event, HostKind, NewHost, SshAuth};
    use agentum_store::sdd_runtime::VerificationResultInput;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use base64::Engine as _;
    use tokio::sync::broadcast;
    use tower::ServiceExt;

    const TEST_UI_TOKEN: &str = "test-only-embedded-ui-capability";

    struct TestEnvironmentFixture(Vec<(&'static str, Option<OsString>)>);

    impl TestEnvironmentFixture {
        fn set(values: Vec<(&'static str, OsString)>) -> Self {
            let previous = values
                .iter()
                .map(|(name, _)| (*name, std::env::var_os(name)))
                .collect();
            for (name, value) in values {
                // SAFETY: every caller holds TEST_ENV_LOCK for the complete
                // fixture lifetime, including all awaited work.
                unsafe { std::env::set_var(name, value) };
            }
            Self(previous)
        }
    }

    impl Drop for TestEnvironmentFixture {
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

    /// A deterministic installed/authenticated provider probe for local route
    /// tests. Production still performs the real executable, version, and
    /// authentication checks; the clean CI runner deliberately has no model
    /// credentials or provider installation.
    #[cfg(unix)]
    struct CodexProbeFixture {
        _environment: TestEnvironmentFixture,
        _directory: tempfile::TempDir,
    }

    #[cfg(unix)]
    impl CodexProbeFixture {
        fn install(agentum_home: &FsPath) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let executable = directory.path().join("codex");
            std::fs::write(
                &executable,
                "#!/bin/sh\nprintf '%s\\n' 'codex-cli 0.145.0'\n",
            )
            .unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

            let original_path = std::env::var_os("PATH");
            let mut paths = vec![directory.path().to_path_buf()];
            if let Some(existing) = original_path.as_ref() {
                paths.extend(std::env::split_paths(existing));
            }
            let fixture_path = std::env::join_paths(paths).unwrap();
            let environment = TestEnvironmentFixture::set(vec![
                ("AGENTUM_HOME", agentum_home.as_os_str().to_os_string()),
                ("PATH", fixture_path),
                (
                    "OPENAI_API_KEY",
                    OsString::from("agentum-test-provider-key"),
                ),
            ]);
            Self {
                _environment: environment,
                _directory: directory,
            }
        }
    }

    struct TestRemoteClient {
        host_id: Uuid,
        artifact_set_id: String,
        base_commit: String,
        worker_version: Mutex<String>,
        block_author: AtomicBool,
        author_started: tokio::sync::Notify,
        author_release: tokio::sync::Notify,
        author_calls: AtomicUsize,
        spec_slug: Mutex<Option<String>>,
        block_next_phase: AtomicBool,
        phase_cancel_requested: AtomicBool,
        phase_release: tokio::sync::Notify,
        phase_calls: AtomicUsize,
        phase_returns: AtomicUsize,
        phases: Mutex<Vec<RemoteLifecyclePhase>>,
        canceled: AtomicUsize,
        delivery_branch: Mutex<Option<String>>,
        delivery_inspections: AtomicUsize,
        delivery_requests: Mutex<Vec<RemoteDeliveryActionRequest>>,
        ambiguous_push_once: AtomicBool,
    }

    impl TestRemoteClient {
        fn new(host_id: Uuid) -> Self {
            Self {
                host_id,
                artifact_set_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
                base_commit: "a".repeat(40),
                worker_version: Mutex::new(env!("CARGO_PKG_VERSION").into()),
                block_author: AtomicBool::new(false),
                author_started: tokio::sync::Notify::new(),
                author_release: tokio::sync::Notify::new(),
                author_calls: AtomicUsize::new(0),
                spec_slug: Mutex::new(None),
                block_next_phase: AtomicBool::new(false),
                phase_cancel_requested: AtomicBool::new(false),
                phase_release: tokio::sync::Notify::new(),
                phase_calls: AtomicUsize::new(0),
                phase_returns: AtomicUsize::new(0),
                phases: Mutex::new(Vec::new()),
                canceled: AtomicUsize::new(0),
                delivery_branch: Mutex::new(None),
                delivery_inspections: AtomicUsize::new(0),
                delivery_requests: Mutex::new(Vec::new()),
                ambiguous_push_once: AtomicBool::new(true),
            }
        }

        fn probe_result(
            &self,
            repository_identity_sha256: &str,
            provider: &str,
            base_ref: &str,
        ) -> RemoteProbeResult {
            let worker_version = self
                .worker_version
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            let material = format!(
                "{}\n{}\n{}\n{}\n{}",
                self.host_id,
                repository_identity_sha256,
                provider,
                base_ref,
                env!("CARGO_PKG_VERSION")
            );
            RemoteProbeResult {
                schema_version: REMOTE_SDD_SCHEMA_VERSION,
                request_id: format!("probe-{}", &sha256(material)[..32]),
                host_id: self.host_id.to_string(),
                worker_version,
                repository_registered: true,
                artifact_set_id: Some(self.artifact_set_id.clone()),
                base_commit: Some(self.base_commit.clone()),
                provider_ready: true,
                reason: None,
            }
        }

        fn phase_result(
            &self,
            request: RemotePhaseRequest,
        ) -> Result<RemotePhaseResult, RemoteLifecycleError> {
            let slug = self
                .spec_slug
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
                .ok_or(RemoteLifecycleError::InvalidResult)?;
            let relative = |name: &str| format!(".agentum/specs/{slug}/{name}");
            let mut artifacts = Vec::new();
            let mut evidence_summary = None;
            match request.phase {
                RemoteLifecyclePhase::Design => {
                    let content = concat!(
                        "# Design\n\n",
                        "RQ-001 and AC-001 use an atomic token swap while existing sessions remain valid.\n\n",
                        "Failure handling is rollback-safe and verification includes a typed browser check.\n",
                    )
                    .to_owned();
                    artifacts.push(RemoteArtifactPayload {
                        kind: "design".into(),
                        relative_path: relative("design.md"),
                        content_sha256: sha256(content.as_bytes()),
                        content,
                    });
                }
                RemoteLifecyclePhase::Planning => {
                    let content = json!({
                        "schemaVersion": 1,
                        "specId": request.spec_id.clone(),
                        "specRevision": request.spec_revision,
                        "tasks": [{
                            "id": "T-001",
                            "objective": "Refresh tokens without interrupting active sessions",
                            "dependencies": [],
                            "readScopes": ["src/session.rs"],
                            "writeScopes": ["src/session.rs"],
                            "acceptanceCriteria": ["AC-001"],
                            "verification": [],
                            "browserChecks": [{
                                "id": "browser-session-refresh",
                                "url": "http://127.0.0.1:3000/session",
                                "acceptanceCriteria": ["AC-001"],
                                "waitUntil": "load",
                                "viewport": {
                                    "width": 1280,
                                    "height": 720,
                                    "deviceScaleMilli": 1000
                                },
                                "timeoutMs": 1000,
                                "assertions": [{
                                    "type": "page_loaded",
                                    "id": "BV-001",
                                    "expectedStatus": 200
                                }]
                            }],
                            "risk": "medium",
                            "parallelSafe": false
                        }]
                    })
                    .to_string();
                    artifacts.push(RemoteArtifactPayload {
                        kind: "plan".into(),
                        relative_path: relative("plan.json"),
                        content_sha256: sha256(content.as_bytes()),
                        content,
                    });
                }
                RemoteLifecyclePhase::Implementation => {
                    let evidence = RemoteImplementationEvidence {
                        schema_version: REMOTE_SDD_SCHEMA_VERSION,
                        request_id: request.request_id.clone(),
                        spec_id: request.spec_id.clone(),
                        spec_revision: request.spec_revision,
                        tasks: vec![RemoteTaskCompletionEvidence {
                            task_id: "T-001".into(),
                            patch_sha256: sha256(b"remote implementation patch"),
                            write_set_sha256: sha256(b"src/session.rs"),
                        }],
                    };
                    evidence_summary = Some(
                        serde_json::to_string(&evidence)
                            .map_err(|_| RemoteLifecycleError::InvalidResult)?,
                    );
                }
                RemoteLifecyclePhase::Verification => {
                    let command = CommandSpec {
                        program: "git".into(),
                        args: vec!["diff".into(), "--check".into()],
                        cwd: ".".into(),
                        env_allowlist: vec!["PATH".into()],
                        timeout_ms: 60_000,
                        output_limit: 256 * 1024,
                    };
                    let screenshot = b"bounded remote screenshot";
                    let console = br#"{"coverage":"none"}"#;
                    let network = br#"{"coverage":"main_document","status":200}"#;
                    let screenshot_sha256 = sha256(screenshot);
                    let blob = |bytes: &[u8], media_type: &str, role: &str| RemoteBrowserBlob {
                        sha256: sha256(bytes),
                        byte_length: bytes.len() as u64,
                        media_type: media_type.into(),
                        role: role.into(),
                        content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                    };
                    let evidence = RemoteVerificationEvidence {
                        schema_version: REMOTE_SDD_SCHEMA_VERSION,
                        command_results: vec![VerificationResultInput {
                            command_index: 0,
                            command_json: serde_json::to_string(&command)
                                .map_err(|_| RemoteLifecycleError::InvalidResult)?,
                            status: "succeeded".into(),
                            exit_code: Some(0),
                            output_hash: sha256(b"git diff --check passed"),
                            output_excerpt: "git diff --check passed".into(),
                            duration_ms: 5,
                        }],
                        browser_results: vec![RemoteBrowserCheckResult {
                            check_id: "browser-session-refresh".into(),
                            captured_at: "2026-07-27T18:30:00Z".into(),
                            status: "passed".into(),
                            duration_ms: 12,
                            output_excerpt: "BV-001 passed with host-resident evidence".into(),
                            target: BrowserTarget {
                                origin: "http://127.0.0.1:3000".into(),
                                path: "/session".into(),
                                path_redacted: true,
                                query_redacted: true,
                            },
                            browser: BrowserRuntime {
                                name: "chromium".into(),
                                version: "130.0.1".into(),
                                viewport_width: 1280,
                                viewport_height: 720,
                                device_scale_milli: 1000,
                            },
                            assertions: vec![BrowserAssertion {
                                id: "BV-001".into(),
                                status: BrowserAssertionStatus::Passed,
                                acceptance_criteria: vec!["AC-001".into()],
                                evidence_sha256: vec![screenshot_sha256],
                            }],
                            console: BrowserConsoleSummary {
                                coverage: BrowserDiagnosticCoverage::None,
                                errors: 0,
                                warnings: 0,
                                transcript_sha256: sha256(console),
                            },
                            network: BrowserNetworkSummary {
                                coverage: BrowserDiagnosticCoverage::MainDocument,
                                requests: 1,
                                failed_requests: 0,
                                transcript_sha256: sha256(network),
                            },
                            blobs: vec![
                                blob(screenshot, "image/png", "capture"),
                                blob(console, "application/json", "console_transcript"),
                                blob(network, "application/json", "network_transcript"),
                            ],
                        }],
                    };
                    evidence_summary = Some(
                        serde_json::to_string(&evidence)
                            .map_err(|_| RemoteLifecycleError::InvalidResult)?,
                    );
                }
                RemoteLifecyclePhase::Review => {
                    let content = concat!(
                        "# Independent review\n\n",
                        "Verdict: PASS\n\n",
                        "AC-001 is satisfied by the implementation and bound verification evidence.\n",
                    )
                    .to_owned();
                    artifacts.push(RemoteArtifactPayload {
                        kind: "review".into(),
                        relative_path: relative("review.md"),
                        content_sha256: sha256(content.as_bytes()),
                        content,
                    });
                }
                RemoteLifecyclePhase::Ready => return Err(RemoteLifecycleError::AlreadyReady),
            }
            let evidence_sha256 = evidence_summary
                .as_deref()
                .map_or_else(|| sha256(b"no remote phase evidence"), sha256);
            Ok(RemotePhaseResult {
                schema_version: REMOTE_SDD_SCHEMA_VERSION,
                request_id: request.request_id,
                phase: request.phase,
                status: RemotePhaseStatus::Succeeded,
                workspace_state_sha256: sha256(format!(
                    "{}:{:?}:workspace",
                    request.run_id, request.phase
                )),
                artifact_set_sha256: sha256(format!(
                    "{}:{:?}:artifacts",
                    request.run_id, request.phase
                )),
                evidence_sha256,
                evidence_summary,
                artifacts,
                error_code: None,
            })
        }
    }

    impl crate::sdd::remote::RemoteSddProbeTransport for TestRemoteClient {
        fn probe(
            &self,
            repository_identity_sha256: &str,
            provider: &str,
            base_ref: &str,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            RemoteProbeResult,
                            crate::sdd::remote::RemoteLifecycleError,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            let result = self.probe_result(repository_identity_sha256, provider, base_ref);
            Box::pin(async move { Ok(result) })
        }
    }

    impl crate::sdd::remote::RemoteSddAuthoringTransport for TestRemoteClient {
        fn author(
            &self,
            request: RemoteAuthoringRequest,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            RemoteAuthoringResult,
                            crate::sdd::remote::RemoteLifecycleError,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async move {
                self.author_calls.fetch_add(1, Ordering::SeqCst);
                self.author_started.notify_waiters();
                if self.block_author.load(Ordering::SeqCst) {
                    self.author_release.notified().await;
                }
                let spec_id: SpecId = request
                    .spec_id
                    .parse()
                    .map_err(|_| crate::sdd::remote::RemoteLifecycleError::InvalidResult)?;
                *self
                    .spec_slug
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) =
                    Some(spec_id.directory_name(&request.title));
                let content = render_spec(
                    &spec_id,
                    2,
                    &request.title,
                    None,
                    "## Requirements\n\n- RQ-001 Preserve active sessions.\n\n## Acceptance criteria\n\n- AC-001 Active sessions remain usable.",
                )
                .map_err(|_| crate::sdd::remote::RemoteLifecycleError::InvalidResult)?;
                Ok(RemoteAuthoringResult {
                    schema_version: REMOTE_SDD_SCHEMA_VERSION,
                    request_id: request.request_id,
                    run_id: request.run_id,
                    spec_id: request.spec_id,
                    spec_revision: 2,
                    status: RemotePhaseStatus::Succeeded,
                    workspace_state_sha256: sha256(b"remote-workspace-state"),
                    artifact_set_sha256: sha256(b"remote-artifact-set-state"),
                    spec: Some(crate::sdd::remote::RemoteArtifactPayload {
                        kind: "specification".into(),
                        relative_path: format!(
                            ".agentum/specs/{}/spec.md",
                            spec_id.directory_name(&request.title)
                        ),
                        content_sha256: sha256(content.as_bytes()),
                        content,
                    }),
                    error_code: None,
                })
            })
        }

        fn cancel(&self, _request_id: &str) -> bool {
            self.canceled.fetch_add(1, Ordering::SeqCst);
            self.author_release.notify_waiters();
            true
        }
    }

    impl crate::sdd::remote::RemoteSddTransport for TestRemoteClient {
        fn execute(
            &self,
            request: crate::sdd::remote::RemotePhaseRequest,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            crate::sdd::remote::RemotePhaseResult,
                            crate::sdd::remote::RemoteLifecycleError,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async move {
                self.phase_calls.fetch_add(1, Ordering::SeqCst);
                self.phases
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(request.phase);
                if self.block_next_phase.swap(false, Ordering::SeqCst) {
                    self.phase_release.notified().await;
                }
                let result = if self.phase_cancel_requested.swap(false, Ordering::SeqCst) {
                    Err(RemoteLifecycleError::Canceled)
                } else {
                    self.phase_result(request)
                };
                self.phase_returns.fetch_add(1, Ordering::SeqCst);
                result
            })
        }

        fn cancel(&self, _request_id: &str) -> bool {
            self.canceled.fetch_add(1, Ordering::SeqCst);
            self.phase_cancel_requested.store(true, Ordering::SeqCst);
            self.phase_release.notify_waiters();
            true
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
            Box::pin(async move {
                self.delivery_inspections.fetch_add(1, Ordering::SeqCst);
                let branch_name = self
                    .delivery_branch
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone()
                    .ok_or(RemoteLifecycleError::InvalidResult)?;
                Ok(RemoteDeliverySnapshotResult {
                    schema_version: REMOTE_SDD_SCHEMA_VERSION,
                    request_id: request.request_id,
                    run_id: request.run_id,
                    workspace_state_sha256: request.expected_workspace_state_sha256,
                    artifact_set_sha256: sha256(b"fixture remote delivery artifacts"),
                    worktree_identity_sha256: sha256(b"fixture remote authoritative worktree"),
                    branch_name,
                    openspec_destination_exists: request
                        .openspec_destination
                        .as_ref()
                        .map(|_| false),
                })
            })
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
            Box::pin(async move {
                self.delivery_requests
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(request.clone());
                let ambiguous = request.action.kind == "push"
                    && self.ambiguous_push_once.swap(false, Ordering::SeqCst);
                let (status, error_code) = if ambiguous {
                    (
                        RemoteDeliveryActionStatus::SyncPending,
                        Some("delivery_outcome_ambiguous".into()),
                    )
                } else {
                    (RemoteDeliveryActionStatus::Succeeded, None)
                };
                Ok(RemoteDeliveryActionResult {
                    schema_version: REMOTE_SDD_SCHEMA_VERSION,
                    request_id: request.request_id,
                    run_id: request.run_id,
                    action_id: request.action.id,
                    status,
                    result: json!({
                        "summary": if ambiguous {
                            "fixture outcome ambiguous"
                        } else {
                            "fixture remote action reconciled"
                        },
                        "localFallback": false
                    }),
                    workspace_state_sha256: sha256(b"fixture post-delivery workspace"),
                    artifact_set_sha256: sha256(b"fixture remote delivery artifacts"),
                    error_code,
                })
            })
        }
    }

    #[test]
    fn digest_binds_policy_artifacts_revision_and_workspace() {
        let id = SpecId::new();
        let base = approval_digest(
            &id,
            1,
            &[("spec.md", "a")],
            &json!({"control":"guarded"}),
            "w",
        );
        assert_ne!(
            base,
            approval_digest(
                &id,
                2,
                &[("spec.md", "a")],
                &json!({"control":"guarded"}),
                "w"
            )
        );
        assert_ne!(
            base,
            approval_digest(
                &id,
                1,
                &[("spec.md", "b")],
                &json!({"control":"guarded"}),
                "w"
            )
        );
        assert_ne!(
            base,
            approval_digest(
                &id,
                1,
                &[("spec.md", "a")],
                &json!({"control":"interactive"}),
                "w"
            )
        );
        assert_ne!(
            base,
            approval_digest(
                &id,
                1,
                &[("spec.md", "a")],
                &json!({"control":"guarded"}),
                "other"
            )
        );
    }

    #[tokio::test]
    async fn command_handler_defense_rejects_untrusted_actor_without_auth_middleware() {
        let directory = tempfile::tempdir().unwrap();
        let store = agentum_store::Store::open(&directory.path().join("defense.sqlite"))
            .await
            .unwrap();
        let (bus, _) = broadcast::channel::<Event>(16);
        let state = AppState::new(store, bus);
        // Compose the SDD router directly to prove its own command check does
        // not depend solely on the outer application auth middleware.
        let app = super::router().with_state(state).layer(axum::Extension(
            crate::auth::AuthActor::unauthenticated_local(),
        ));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sdd/runs/missing/commands")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "type": "decideApproval",
                            "requestId": "direct-handler-probe",
                            "expectedRevision": 0,
                            "approvalId": "missing",
                            "digest": "missing",
                            "decision": "approve"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn autopilot_start_authorizes_current_digest_but_not_other_control_or_gates() {
        let database = tempfile::tempdir().unwrap();
        let _repository_registration = crate::routes::repos::register_test_repo(
            "repo-autopilot",
            database.path().to_string_lossy(),
        );
        let store = agentum_store::Store::open(&database.path().join("sdd.sqlite"))
            .await
            .unwrap();
        let spec_id = SpecId::new();
        let spec_id_text = spec_id.to_string();
        let timestamp = "2026-07-27T12:00:00Z";
        sqlx::query(
            "INSERT INTO sdd_specs
             (spec_id, spec_ulid, repo_id, title, slug, profile, control, provider,
              current_revision, aggregate_revision, created_at, updated_at)
             VALUES (?, ?, 'repo-autopilot', 'Autopilot', ?, 'standard', 'autopilot',
                     'codex', 2, 1, ?, ?)",
        )
        .bind(&spec_id_text)
        .bind(spec_id.ulid())
        .bind(spec_id.directory_name("Autopilot"))
        .bind(timestamp)
        .bind(timestamp)
        .execute(store.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sdd_spec_revisions
             (spec_id, revision, content_hash, content, submitted_by, imported_external, created_at)
             VALUES (?, 2, 'spec-hash', '# Spec', 'agent:codex:author', 0, ?)",
        )
        .bind(&spec_id_text)
        .bind(timestamp)
        .execute(store.pool())
        .await
        .unwrap();
        let autopilot_policy = json!({
            "profile": "standard",
            "control": "autopilot",
            "deliveryRequired": true,
            "implementationEnabled": true,
            // Deliberately malformed so the background lifecycle exits before
            // invoking a provider; route authorization is the subject here.
            "provider": {}
        })
        .to_string();
        let guarded_policy = json!({
            "profile": "standard",
            "control": "guarded",
            "deliveryRequired": true,
            "implementationEnabled": true,
            "provider": {}
        })
        .to_string();
        for (run_id, phase, status, revision, policy) in [
            (
                "run-autopilot",
                "specification",
                "waiting",
                1_i64,
                &autopilot_policy,
            ),
            (
                "run-blocked",
                "specification",
                "blocked",
                4_i64,
                &autopilot_policy,
            ),
            ("run-ready", "ready", "succeeded", 8_i64, &autopilot_policy),
            (
                "run-guarded",
                "specification",
                "waiting",
                1_i64,
                &guarded_policy,
            ),
        ] {
            sqlx::query(
                "INSERT INTO sdd_runs
                 (run_id, spec_id, repo_id, phase, status, aggregate_revision, base_ref,
                  base_commit, branch_name, authoritative_path, workspace_fingerprint,
                  policy_json, created_at, updated_at)
                 VALUES (?, ?, 'repo-autopilot', ?, ?, ?, 'HEAD', 'deadbeef', ?, ?, 'fp', ?, ?, ?)",
            )
            .bind(run_id)
            .bind(&spec_id_text)
            .bind(phase)
            .bind(status)
            .bind(revision)
            .bind(format!("agentum/{run_id}"))
            .bind(database.path().join(run_id).to_string_lossy().to_string())
            .bind(policy)
            .bind(timestamp)
            .bind(timestamp)
            .execute(store.pool())
            .await
            .unwrap();
        }
        for (approval_id, run_id, digest) in [
            ("approval-autopilot", "run-autopilot", "digest-autopilot"),
            ("approval-blocked", "run-blocked", "digest-blocked"),
            ("approval-guarded", "run-guarded", "digest-guarded"),
        ] {
            sqlx::query(
                "INSERT INTO sdd_approval_requests
                 (approval_id, run_id, purpose, digest, requested_revision, requested_by,
                  status, created_at)
                 VALUES (?, ?, 'specification', ?, 2, 'agent:codex:author', 'pending', ?)",
            )
            .bind(approval_id)
            .bind(run_id)
            .bind(digest)
            .bind(timestamp)
            .execute(store.pool())
            .await
            .unwrap();
        }
        let app = test_app(store.clone());
        for (decision, request_id) in [
            ("approve", "direct-autopilot-approve"),
            ("reject", "direct-autopilot-reject"),
        ] {
            let rejected = app
                .clone()
                .oneshot(json_request(
                    "POST",
                    "/api/sdd/runs/run-autopilot/commands",
                    &json!({
                        "type": "decideApproval",
                        "requestId": request_id,
                        "expectedRevision": 1,
                        "approvalId": "approval-autopilot",
                        "digest": "digest-autopilot",
                        "decision": decision,
                        "reason": "direct approval must be refused"
                    }),
                ))
                .await
                .unwrap();
            assert_eq!(rejected.status(), StatusCode::CONFLICT, "{decision}");
        }
        let still_pending: String = sqlx::query_scalar(
            "SELECT status FROM sdd_approval_requests WHERE approval_id = 'approval-autopilot'",
        )
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(still_pending, "pending");
        let start = json!({
            "type": "startRun",
            "requestId": "explicit-autopilot-start",
            "expectedRevision": 1
        });
        let authorized = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/sdd/runs/run-autopilot/commands",
                &start,
            ))
            .await
            .unwrap();
        assert_eq!(authorized.status(), StatusCode::OK);
        let authorized = response_json(authorized).await;
        assert_eq!(authorized["phase"], "design");
        assert_eq!(authorized["status"], "queued");
        assert_eq!(authorized["authorization"]["source"], "explicit_start");
        assert_eq!(authorized["authorization"]["digest"], "digest-autopilot");
        let approval_status: String = sqlx::query_scalar(
            "SELECT status FROM sdd_approval_requests WHERE approval_id = 'approval-autopilot'",
        )
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(approval_status, "approved");
        let recorded_digest: String = sqlx::query_scalar(
            "SELECT digest FROM sdd_approval_decisions WHERE approval_id = 'approval-autopilot'",
        )
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(recorded_digest, "digest-autopilot");
        let replay = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/sdd/runs/run-autopilot/commands",
                &start,
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(response_json(replay).await, authorized);

        for (run_id, request_id, revision) in [
            ("run-blocked", "blocked-start", 4_i64),
            ("run-ready", "ready-start", 8_i64),
            ("run-guarded", "guarded-start", 1_i64),
        ] {
            let rejected = app
                .clone()
                .oneshot(json_request(
                    "POST",
                    &format!("/api/sdd/runs/{run_id}/commands"),
                    &json!({
                        "type": "startRun",
                        "requestId": request_id,
                        "expectedRevision": revision
                    }),
                ))
                .await
                .unwrap();
            assert_eq!(rejected.status(), StatusCode::CONFLICT, "{run_id}");
        }
        for approval_id in ["approval-blocked", "approval-guarded"] {
            let status: String = sqlx::query_scalar(
                "SELECT status FROM sdd_approval_requests WHERE approval_id = ?",
            )
            .bind(approval_id)
            .fetch_one(store.pool())
            .await
            .unwrap();
            assert_eq!(status, "pending");
        }
    }

    #[test]
    fn plan_requires_matching_acyclic_dag_and_safe_paths() {
        let id = SpecId::new();
        let valid = json!({
            "schemaVersion": 1,
            "specId": id,
            "specRevision": 1,
            "tasks": [{
                "id": "T-1", "objective": "Do it", "dependencies": [],
                "readScopes": ["src/lib.rs"], "writeScopes": ["src/lib.rs"],
                "acceptanceCriteria": ["AC-001"], "verification": [],
                "risk": "low", "parallelSafe": true
            }]
        });
        assert!(validate_plan(&valid.to_string(), &id, 1).is_ok());
        let unsafe_plan = valid.to_string().replace("src/lib.rs", "../outside");
        assert!(validate_plan(&unsafe_plan, &id, 1).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_create_projects_authoring_without_local_repository_writes() {
        let repository = tempfile::tempdir().unwrap();
        let database = tempfile::tempdir().unwrap();
        let store = agentum_store::Store::open(&database.path().join("remote-create.sqlite"))
            .await
            .unwrap();
        let host = store
            .create_host(NewHost {
                name: "remote-sdd-test".into(),
                kind: HostKind::Ssh {
                    user: "agentum".into(),
                    hostname: "fixture.invalid".into(),
                    port: 22,
                    auth: SshAuth::Agent,
                },
            })
            .await
            .unwrap();
        let _registration = crate::routes::repos::register_test_remote_repo(
            "repo-remote-projected",
            repository.path().to_string_lossy(),
            host.id,
        );
        let client = Arc::new(TestRemoteClient::new(host.id));
        crate::sdd::remote_lifecycle::register_test_remote_client(host.id, client.clone());
        let response = test_app(store.clone())
            .oneshot(json_request(
                "POST",
                "/api/sdd/repos/repo-remote-projected/specs",
                &json!({
                    "requestId": "remote-create-projected",
                    "expectedRevision": 0,
                    "title": "Remote refresh",
                    "goal": "Refresh tokens without interrupting sessions.",
                    "provider": "codex",
                    "baseRef": "HEAD"
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = response_json(response).await;
        assert_eq!(body["status"], "waiting");
        assert_eq!(body["nextAction"], "Spec approval required");
        assert_eq!(body["remote"]["localFallback"], false);
        assert_eq!(client.author_calls.load(Ordering::SeqCst), 1);
        assert!(!repository.path().join(".agentum").exists());
        assert_eq!(
            store
                .sdd_list_specs("repo-remote-projected")
                .await
                .unwrap()
                .len(),
            1
        );
        let run_id = body["runId"].as_str().unwrap();
        let projection = store.sdd_remote_run(run_id).await.unwrap().unwrap();
        assert_eq!(projection.status, "waiting");
        let payloads = store.sdd_remote_artifact_payloads(run_id).await.unwrap();
        assert_eq!(payloads.len(), 1);
        assert!(payloads[0].content.contains("RQ-001"));
        let intent = store
            .sdd_remote_create_intent("repo-remote-projected", "remote-create-projected")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(intent.status, "completed");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_create_recovery_reprobes_and_refuses_a_changed_worker() {
        let database = tempfile::tempdir().unwrap();
        let store = agentum_store::Store::open(&database.path().join("remote-recovery.sqlite"))
            .await
            .unwrap();
        let host = store
            .create_host(NewHost {
                name: "remote-recovery-test".into(),
                kind: HostKind::Ssh {
                    user: "agentum".into(),
                    hostname: "fixture.invalid".into(),
                    port: 22,
                    auth: SshAuth::Agent,
                },
            })
            .await
            .unwrap();
        let repository = tempfile::tempdir().unwrap();
        let _registration = crate::routes::repos::register_test_remote_repo(
            "repo-remote-recovery",
            repository.path().to_string_lossy(),
            host.id,
        );
        let client = Arc::new(TestRemoteClient::new(host.id));
        client.block_author.store(true, Ordering::SeqCst);
        crate::sdd::remote_lifecycle::register_test_remote_client(host.id, client.clone());
        let (bus, _) = broadcast::channel::<Event>(32);
        let mut state = AppState::new(store.clone(), bus);
        state.no_auth = true;
        state.embedded_ui_token = Some(Arc::new(TEST_UI_TOKEN.into()));
        let app = crate::router(state.clone());
        let create = tokio::spawn(async move {
            app.oneshot(json_request(
                "POST",
                "/api/sdd/repos/repo-remote-recovery/specs",
                &json!({
                    "requestId": "remote-create-interrupted",
                    "expectedRevision": 0,
                    "title": "Recover remote authoring",
                    "goal": "Finish the exact authoring request after restart.",
                    "provider": "codex",
                    "baseRef": "HEAD"
                }),
            ))
            .await
        });
        for _ in 0..100 {
            if client.author_calls.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(client.author_calls.load(Ordering::SeqCst), 1);
        create.abort();
        let _ = create.await;

        let claimed = store.sdd_claim_interrupted_creates().await.unwrap();
        assert_eq!(claimed.len(), 1);
        *client
            .worker_version
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = "0.0.0-changed".into();
        client.block_author.store(false, Ordering::SeqCst);
        let error = recover_remote_create(&state, &claimed[0])
            .await
            .expect_err("changed worker must fail closed");
        assert!(
            error
                .to_string()
                .contains("remote_recovery_capability_changed")
        );
        assert_eq!(client.author_calls.load(Ordering::SeqCst), 1);
        let saga = store
            .sdd_create_saga("repo-remote-recovery", "remote-create-interrupted")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saga.stage, "recovery_required");
        assert!(!repository.path().join(".agentum").exists());
    }

    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)] // serializes the task-specific AGENTUM_HOME seam
    async fn remote_desktop_projection_recovers_and_reaches_ready_with_browser_evidence() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let old_agentum_home = std::env::var_os("AGENTUM_HOME");
        let repository = tempfile::tempdir().unwrap();
        let agentum_home = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("AGENTUM_HOME", agentum_home.path());
        }
        let database_path = agentum_home.path().join("remote-projection.sqlite");
        let store = agentum_store::Store::open(&database_path).await.unwrap();
        let host = store
            .create_host(NewHost {
                name: "remote-projection-lifecycle".into(),
                kind: HostKind::Ssh {
                    user: "agentum".into(),
                    hostname: "fixture.invalid".into(),
                    port: 22,
                    auth: SshAuth::Agent,
                },
            })
            .await
            .unwrap();
        let _registration = crate::routes::repos::register_test_remote_repo(
            "repo-remote-lifecycle",
            repository.path().to_string_lossy(),
            host.id,
        );
        let client = Arc::new(TestRemoteClient::new(host.id));
        crate::sdd::remote_lifecycle::register_test_remote_client(host.id, client.clone());

        let app = test_app(store.clone());
        let created = app
            .oneshot(json_request(
                "POST",
                "/api/sdd/repos/repo-remote-lifecycle/specs",
                &json!({
                    "requestId": "remote-full-projection-create",
                    "expectedRevision": 0,
                    "title": "Remote refresh lifecycle",
                    "goal": "Refresh tokens without interrupting active sessions.",
                    "profile": "standard",
                    "control": "guarded",
                    "provider": "codex",
                    "baseRef": "HEAD"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = response_json(created).await;
        let run_id = created["runId"].as_str().unwrap().to_owned();
        assert_eq!(created["remote"]["localFallback"], false);
        assert!(
            created["authoritativePath"]
                .as_str()
                .unwrap()
                .starts_with("agentum+ssh://")
        );
        assert!(!repository.path().join(".agentum").exists());
        assert_eq!(std::fs::read_dir(repository.path()).unwrap().count(), 0);

        // Reopen the SQLite database and reconstruct the router before the
        // approval. This is the first desktop restart boundary: the draft,
        // approval digest, remote plan, and checkpoint must be durable.
        drop(store);
        let store = agentum_store::Store::open(&database_path).await.unwrap();
        let restarted = test_app(store.clone());
        let restored = restarted
            .clone()
            .oneshot(get_request(format!("/api/sdd/runs/{run_id}")))
            .await
            .unwrap();
        assert_eq!(restored.status(), StatusCode::OK);
        let restored = response_json(restored).await;
        assert_eq!(restored["run"]["status"], "waiting");
        assert_eq!(restored["approval"]["purpose"], "specification");
        assert_eq!(restored["remote"]["status"], "waiting");

        // Interrupt the first design request after its durable reservation,
        // then run the same boot recovery mutation used by the server. The
        // resumed coordinator must retry the identical phase and continue.
        client.block_next_phase.store(true, Ordering::SeqCst);
        let approved = restarted
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/sdd/runs/{run_id}/commands"),
                &json!({
                    "type": "decideApproval",
                    "requestId": "remote-full-projection-approve",
                    "expectedRevision": restored["run"]["aggregateRevision"],
                    "approvalId": restored["approval"]["approvalId"],
                    "digest": restored["approval"]["digest"],
                    "decision": "approve"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(approved.status(), StatusCode::OK);
        for _ in 0..200 {
            let projection = store.sdd_remote_run(&run_id).await.unwrap().unwrap();
            if client.phase_calls.load(Ordering::SeqCst) == 1
                && projection.status == "running"
                && projection.active_request_id.is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let active_request = store
            .sdd_remote_run(&run_id)
            .await
            .unwrap()
            .unwrap()
            .active_request_id
            .expect("design request should be durably reserved");
        assert_eq!(
            client
                .phases
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_slice(),
            &[RemoteLifecyclePhase::Design]
        );
        let (duplicate_bus, _) = broadcast::channel::<Event>(8);
        let duplicate_state = AppState::new(store.clone(), duplicate_bus);
        for _ in 0..32 {
            crate::sdd::remote_lifecycle::spawn(duplicate_state.clone(), run_id.clone());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(
            client.phase_calls.load(Ordering::SeqCst),
            1,
            "rapid duplicate spawn requests must retain one transport owner"
        );
        assert_eq!(store.sdd_recover_interrupted_runs().await.unwrap(), 1);
        assert!(crate::sdd::remote::RemoteSddTransport::cancel(
            client.as_ref(),
            &active_request
        ));
        for _ in 0..200 {
            if client.phase_returns.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(client.phase_returns.load(Ordering::SeqCst), 1);
        let interrupted = store.sdd_get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(interrupted.status, "paused");
        assert_eq!(
            store.sdd_remote_run(&run_id).await.unwrap().unwrap().status,
            "paused"
        );
        // The detached task belongs to the simulated pre-restart process. It
        // exits after observing the canceled transport and stale CAS.
        for _ in 0..20 {
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        drop(restarted);
        let recovered_store = agentum_store::Store::open(&database_path).await.unwrap();
        let recovered_app = test_app(recovered_store.clone());
        let resumed = recovered_app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/sdd/runs/{run_id}/commands"),
                &json!({
                    "type": "resume",
                    "requestId": "remote-full-projection-resume",
                    "expectedRevision": interrupted.aggregate_revision
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resumed.status(), StatusCode::OK);

        let mut terminal = None;
        for _ in 0..1_000 {
            let run = recovered_store.sdd_get_run(&run_id).await.unwrap().unwrap();
            if matches!(run.status.as_str(), "succeeded" | "failed" | "blocked") {
                terminal = Some(run);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let terminal = terminal.expect("remote lifecycle should reach a terminal phase");
        assert_eq!(
            (terminal.phase.as_str(), terminal.status.as_str()),
            ("ready", "succeeded"),
            "remote lifecycle stopped with blocker {:?}",
            terminal.blocker
        );
        let projection = recovered_store
            .sdd_remote_run(&run_id)
            .await
            .unwrap()
            .unwrap();
        let checkpoint: RemoteLifecycleCheckpoint =
            serde_json::from_str(&projection.checkpoint_json).unwrap();
        assert_eq!(projection.status, "succeeded");
        assert!(projection.active_request_id.is_none());
        assert!(checkpoint.is_ready());
        assert_eq!(checkpoint.completed_phases, 5);
        assert_eq!(
            client
                .phases
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_slice(),
            &[
                RemoteLifecyclePhase::Design,
                RemoteLifecyclePhase::Design,
                RemoteLifecyclePhase::Planning,
                RemoteLifecyclePhase::Implementation,
                RemoteLifecyclePhase::Verification,
                RemoteLifecyclePhase::Review,
            ]
        );
        assert_eq!(client.author_calls.load(Ordering::SeqCst), 1);

        let snapshot = recovered_store
            .sdd_snapshot(&run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.tasks.len(), 1);
        assert_eq!(snapshot.tasks[0].runtime_status, "succeeded");
        assert_eq!(snapshot.verification.len(), 2);
        assert!(
            snapshot
                .verification
                .iter()
                .all(|result| result.status == "succeeded")
        );
        let implementation_session = snapshot
            .attempts
            .iter()
            .find(|attempt| {
                attempt
                    .session_identity
                    .starts_with("remote:implementation:")
            })
            .unwrap()
            .session_identity
            .clone();
        let review_session = snapshot
            .attempts
            .iter()
            .find(|attempt| attempt.session_identity.starts_with("remote:review:"))
            .unwrap()
            .session_identity
            .clone();
        assert_ne!(implementation_session, review_session);
        let browser_evidence = recovered_store.sdd_browser_evidence(&run_id).await.unwrap();
        assert_eq!(browser_evidence.len(), 1);
        assert_eq!(browser_evidence[0].status, "passed");
        assert_eq!(browser_evidence[0].blobs.len(), 3);

        let artifacts = recovered_app
            .clone()
            .oneshot(get_request(format!("/api/sdd/runs/{run_id}/artifacts")))
            .await
            .unwrap();
        assert_eq!(artifacts.status(), StatusCode::OK);
        let artifacts = response_json(artifacts).await;
        assert_eq!(artifacts["artifacts"].as_array().unwrap().len(), 4);
        assert!(
            artifacts["artifacts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|artifact| {
                    artifact["remoteProjected"] == true && artifact["externallyModified"] == false
                })
        );
        let final_run = recovered_app
            .clone()
            .oneshot(get_request(format!("/api/sdd/runs/{run_id}")))
            .await
            .unwrap();
        let final_run = response_json(final_run).await;
        assert_eq!(final_run["run"]["phase"], "ready");
        assert_eq!(final_run["browserEvidence"].as_array().unwrap().len(), 1);

        // Deliver through the same registered typed remote client. The
        // customer checkout remains empty; treating the agentum+ssh URI as a
        // local path would make this flow fail instead of producing the
        // successful typed results asserted below.
        *client
            .delivery_branch
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(terminal.branch_name.clone());
        let preview = recovered_app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/sdd/runs/{run_id}/commands"),
                &json!({
                    "type": "previewDelivery",
                    "requestId": "remote-delivery-preview",
                    "expectedRevision": final_run["run"]["aggregateRevision"],
                    "actions": [
                        { "type": "commit", "message": "Deliver remote fixture" },
                        { "type": "push", "remote": "origin" }
                    ]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(preview.status(), StatusCode::OK);
        let preview = response_json(preview).await;
        assert_eq!(client.delivery_inspections.load(Ordering::SeqCst), 1);
        assert!(
            preview["artifactHashes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|hash| {
                    hash["kind"] == "remote_artifact_set"
                        && hash["relativePath"] == "agentum+ssh://artifact-set"
                })
        );
        let preview_token = preview["previewToken"].as_str().unwrap().to_owned();
        let commit_id = preview["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["type"] == "commit")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let push_id = preview["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["type"] == "push")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let confirmed = recovered_app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/sdd/runs/{run_id}/commands"),
                &json!({
                    "type": "confirmDelivery",
                    "requestId": "remote-delivery-confirm",
                    "expectedRevision": preview["revision"],
                    "previewToken": preview_token,
                    "actions": [commit_id, push_id]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(confirmed.status(), StatusCode::OK);
        assert_eq!(client.delivery_inspections.load(Ordering::SeqCst), 2);
        let confirmed = response_json(confirmed).await;
        let preview_id = confirmed["previewId"].as_str().unwrap().to_owned();
        let mut delivered = Vec::new();
        for _ in 0..200 {
            delivered = recovered_store
                .sdd_delivery_actions(&preview_id)
                .await
                .unwrap();
            if delivered.len() == 2
                && delivered
                    .iter()
                    .all(|action| matches!(action.status.as_str(), "succeeded" | "sync_pending"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            delivered
                .iter()
                .find(|action| action.action_type == "commit")
                .unwrap()
                .status,
            "succeeded"
        );
        assert_eq!(
            delivered
                .iter()
                .find(|action| action.action_type == "push")
                .unwrap()
                .status,
            "sync_pending"
        );
        let ready_after_partial = recovered_store.sdd_get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(
            (
                ready_after_partial.phase.as_str(),
                ready_after_partial.status.as_str()
            ),
            ("ready", "succeeded")
        );
        assert!(!repository.path().join(".agentum").exists());

        // Reopen the durable database, then explicitly retry only the
        // ambiguous push. Attempt two has a distinct typed idempotency key and
        // reconciles without rerunning the successful commit.
        drop(recovered_app);
        drop(recovered_store);
        let restarted_store = agentum_store::Store::open(&database_path).await.unwrap();
        let restarted_app = test_app(restarted_store.clone());
        let retry = restarted_app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/sdd/runs/{run_id}/commands"),
                &json!({
                    "type": "confirmDelivery",
                    "requestId": "remote-delivery-retry",
                    "expectedRevision": ready_after_partial.aggregate_revision,
                    "previewToken": preview_token,
                    "actions": [push_id]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(retry.status(), StatusCode::OK);
        for _ in 0..200 {
            delivered = restarted_store
                .sdd_delivery_actions(&preview_id)
                .await
                .unwrap();
            if delivered
                .iter()
                .find(|action| action.action_type == "push")
                .is_some_and(|action| action.status == "succeeded")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            delivered
                .iter()
                .find(|action| action.action_type == "push")
                .unwrap()
                .status,
            "succeeded"
        );
        let requests = client
            .delivery_requests
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].attempt, 1);
        assert_eq!(requests[1].attempt, 1);
        assert_eq!(requests[2].attempt, 2);
        assert_ne!(requests[1].request_id, requests[2].request_id);
        assert!(requests.iter().all(|request| {
            request.envelope.actor_id == "human:local-desktop"
                && request.envelope.worktree_identity
                    == sha256(b"fixture remote authoritative worktree")
        }));
        drop(requests);
        let delivered_run = restarted_store.sdd_get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(
            (delivered_run.phase.as_str(), delivered_run.status.as_str()),
            ("ready", "succeeded")
        );
        assert!(!repository.path().join(".agentum").exists());
        assert_eq!(std::fs::read_dir(repository.path()).unwrap().count(), 0);

        unsafe {
            match old_agentum_home {
                Some(value) => std::env::set_var("AGENTUM_HOME", value),
                None => std::env::remove_var("AGENTUM_HOME"),
            }
        }
    }

    #[test]
    fn source_request_is_a_closed_discriminated_union() {
        let valid = json!({
            "requestId": "typed-source",
            "expectedRevision": 0,
            "title": "Example",
            "goal": "Example",
            "provider": "codex",
            "source": { "type": "markdown", "markdown": "# Context" }
        });
        assert!(serde_json::from_value::<CreateSpecBody>(valid).is_ok());
        for invalid_source in [
            json!({ "provider": "description", "type": "jira", "url": "secret" }),
            json!({ "type": "markdown", "markdown": "# Context", "token": "secret" }),
            json!({ "type": "github", "url": "https://github.com/o/r/issues/1", "sourceRevision": "caller-controlled" }),
            json!({ "value": "untyped" }),
        ] {
            let invalid = json!({
                "requestId": "typed-source",
                "expectedRevision": 0,
                "title": "Example",
                "goal": "Example",
                "provider": "codex",
                "source": invalid_source
            });
            assert!(
                serde_json::from_value::<CreateSpecBody>(invalid).is_err(),
                "accepted unsafe source input"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn openspec_preview_is_read_only_and_revision_conflicts_fail_closed() {
        let repository = tempfile::tempdir().unwrap();
        let change = repository.path().join("openspec/changes/refresh-sessions");
        std::fs::create_dir_all(change.join("specs/auth")).unwrap();
        std::fs::write(
            change.join("proposal.md"),
            "# Refresh sessions\n\nKeep active sessions online.\n",
        )
        .unwrap();
        std::fs::write(
            change.join("specs/auth/spec.md"),
            "## ADDED Requirements\n\n### Requirement: Refresh\nTokens MUST refresh atomically.\n\n#### Scenario: Active session\n- THEN the session remains online\n",
        )
        .unwrap();
        let _registration = crate::routes::repos::register_test_repo(
            "repo-openspec-preview",
            repository.path().to_string_lossy(),
        );
        let database = tempfile::tempdir().unwrap();
        let store = agentum_store::Store::open(&database.path().join("preview.sqlite"))
            .await
            .unwrap();
        let app = test_app(store.clone());
        let preview_body = json!({
            "title": "Refresh sessions",
            "source": {
                "type": "openspec",
                "path": "openspec/changes/refresh-sessions"
            }
        });
        let preview = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/sdd/repos/repo-openspec-preview/sources/preview",
                &preview_body,
            ))
            .await
            .unwrap();
        assert_eq!(preview.status(), StatusCode::OK);
        let preview = response_json(preview).await;
        let revision = preview["sourceRevision"].as_str().unwrap();
        assert!(revision.starts_with("sha256:"));
        assert_eq!(preview["taskCount"], 0);
        assert!(!repository.path().join(".agentum").exists());
        assert!(
            store
                .sdd_list_specs("repo-openspec-preview")
                .await
                .unwrap()
                .is_empty()
        );

        std::fs::write(
            change.join("proposal.md"),
            "# Refresh sessions\n\nChanged after preview.\n",
        )
        .unwrap();
        let conflict = app
            .oneshot(json_request(
                "POST",
                "/api/sdd/repos/repo-openspec-preview/sources/preview",
                &json!({
                    "title": "Refresh sessions",
                    "source": {
                        "type": "openspec",
                        "path": "openspec/changes/refresh-sessions",
                        "expectedSourceRevision": revision
                    }
                }),
            ))
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(conflict).await["error"],
            "source_revision_changed"
        );
        assert!(!repository.path().join(".agentum").exists());
        assert!(
            store
                .sdd_list_specs("repo-openspec-preview")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)] // isolates Agentum's delivery temp root
    async fn delivery_and_openspec_preview_confirmation_are_hash_bound_and_durable() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let agentum_home = tempfile::tempdir().unwrap();
        let _environment = TestEnvironmentFixture::set(vec![(
            "AGENTUM_HOME",
            agentum_home.path().as_os_str().to_os_string(),
        )]);
        let authoritative = tempfile::tempdir().unwrap();
        let _repository_registration = crate::routes::repos::register_test_repo(
            "repo-delivery",
            authoritative.path().to_string_lossy(),
        );
        git(authoritative.path(), &["init", "-q"]);
        git(
            authoritative.path(),
            &["config", "user.email", "delivery@example.invalid"],
        );
        git(
            authoritative.path(),
            &["config", "user.name", "Agentum Delivery Test"],
        );
        std::fs::write(authoritative.path().join("README.md"), "fixture\n").unwrap();
        git(authoritative.path(), &["add", "README.md"]);
        git(authoritative.path(), &["commit", "-qm", "fixture"]);
        let base_commit = git_output(authoritative.path(), &["rev-parse", "HEAD"]);

        let spec_id: SpecId = "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV".parse().unwrap();
        let title = "Deliver verified work";
        let root = initialize(authoritative.path(), &spec_id, title, ulid::Ulid::new()).unwrap();
        let spec_content = render_spec(
            &spec_id,
            4,
            title,
            None,
            "# Deliver verified work\n\n## Requirements\n\n- RQ-001 Deliver only after confirmation.\n\n## Acceptance criteria\n\n- AC-001 The exact previewed change is committed.",
        )
        .unwrap();
        let spec_path = root.spec_dir.join("spec.md");
        let spec_hash =
            atomic_write(&spec_path, spec_content.as_bytes(), Some(MISSING_HASH)).unwrap();
        let database = tempfile::tempdir().unwrap();
        let store = agentum_store::Store::open(&database.path().join("delivery.sqlite"))
            .await
            .unwrap();
        let at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let slug = spec_id.directory_name(title);
        sqlx::query(
            "INSERT INTO sdd_specs
             (spec_id, spec_ulid, repo_id, title, slug, profile, control, provider,
              current_revision, aggregate_revision, created_at, updated_at)
             VALUES (?, ?, 'repo-delivery', ?, ?, 'standard', 'guarded', 'codex',
                     4, 1, ?, ?)",
        )
        .bind(spec_id.to_string())
        .bind(spec_id.ulid())
        .bind(title)
        .bind(&slug)
        .bind(&at)
        .bind(&at)
        .execute(store.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sdd_runs
             (run_id, spec_id, repo_id, phase, status, aggregate_revision, base_ref,
              base_commit, branch_name, authoritative_path, workspace_fingerprint,
              policy_json, created_at, updated_at)
             VALUES ('run-delivery', ?, 'repo-delivery', 'ready', 'succeeded', 7, 'HEAD',
                     ?, ?, ?, 'workspace-fingerprint', '{}', ?, ?)",
        )
        .bind(spec_id.to_string())
        .bind(&base_commit)
        .bind(spec_id.branch_name(title))
        .bind(authoritative.path().to_string_lossy())
        .bind(&at)
        .bind(&at)
        .execute(store.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sdd_artifact_revisions
             (artifact_revision_id, run_id, spec_id, kind, revision, spec_revision,
              relative_path, content_hash, submitted_by, created_at)
             VALUES ('artifact-spec', 'run-delivery', ?, 'specification', 1, 4,
                     ?, ?, 'agent:test', ?)",
        )
        .bind(spec_id.to_string())
        .bind(&root.spec_relative_path)
        .bind(&spec_hash)
        .bind(&at)
        .execute(store.pool())
        .await
        .unwrap();

        let app = test_app(store.clone());
        let preview_request = json!({
            "type": "previewDelivery",
            "requestId": "preview-delivery-1",
            "expectedRevision": 7,
            "actions": [{ "type": "commit", "message": "Ship verified work" }]
        });
        let preview = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/sdd/runs/run-delivery/commands",
                &preview_request,
            ))
            .await
            .unwrap();
        assert_eq!(preview.status(), StatusCode::OK);
        let preview = response_json(preview).await;
        assert_eq!(preview["revision"], 8);
        assert_eq!(preview["phase"], "ready");
        let first_token = preview["previewToken"].as_str().unwrap().to_owned();
        let first_action = preview["actions"][0]["id"].as_str().unwrap().to_owned();

        let replay = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/sdd/runs/run-delivery/commands",
                &preview_request,
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(response_json(replay).await["previewToken"], first_token);

        std::fs::write(
            authoritative.path().join("implementation.txt"),
            "verified\n",
        )
        .unwrap();
        let stale_confirm = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/sdd/runs/run-delivery/commands",
                &json!({
                    "type": "confirmDelivery",
                    "requestId": "confirm-stale-preview",
                    "expectedRevision": 8,
                    "previewToken": first_token,
                    "actions": [first_action]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(stale_confirm.status(), StatusCode::PRECONDITION_FAILED);

        let preview = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/sdd/runs/run-delivery/commands",
                &json!({
                    "type": "previewDelivery",
                    "requestId": "preview-delivery-2",
                    "expectedRevision": 8,
                    "actions": [{ "type": "commit", "message": "Ship verified work" }]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(preview.status(), StatusCode::OK);
        let preview = response_json(preview).await;
        let preview_id = preview["previewId"].as_str().unwrap().to_owned();
        let preview_token = preview["previewToken"].as_str().unwrap().to_owned();
        let action_id = preview["actions"][0]["id"].as_str().unwrap().to_owned();
        let confirmed = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/sdd/runs/run-delivery/commands",
                &json!({
                    "type": "confirmDelivery",
                    "requestId": "confirm-delivery-2",
                    "expectedRevision": 9,
                    "previewToken": preview_token,
                    "actions": [action_id]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(confirmed.status(), StatusCode::OK);
        assert_eq!(response_json(confirmed).await["phase"], "ready");

        let mut final_action = None;
        for _ in 0..100 {
            final_action = store
                .sdd_delivery_actions(&preview_id)
                .await
                .unwrap()
                .into_iter()
                .next();
            if final_action
                .as_ref()
                .is_some_and(|action| action.status != "pending" && action.status != "running")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let final_action = final_action.expect("delivery action persisted");
        assert_eq!(final_action.status, "succeeded", "{final_action:?}");
        let run = store.sdd_get_run("run-delivery").await.unwrap().unwrap();
        assert_eq!(
            (run.phase.as_str(), run.status.as_str()),
            ("ready", "succeeded")
        );
        assert!(git_output(authoritative.path(), &["status", "--porcelain"]).is_empty());
        let commit_body = git_output(authoritative.path(), &["show", "-s", "--format=%B"]);
        assert!(commit_body.contains("Agentum-Delivery-Action:"));

        let restored = app
            .clone()
            .oneshot(get_request("/api/sdd/runs/run-delivery"))
            .await
            .unwrap();
        let restored = response_json(restored).await;
        assert_eq!(restored["delivery"]["actions"][0]["status"], "succeeded");

        // OpenSpec export uses the same preview/confirm authorization. If the
        // Agentum artifacts and destination both change after preview, confirm
        // refuses to overwrite either side.
        let current = store.sdd_get_run("run-delivery").await.unwrap().unwrap();
        let export_preview = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/sdd/runs/run-delivery/commands",
                &json!({
                    "type": "previewDelivery",
                    "requestId": "preview-openspec-export",
                    "expectedRevision": current.aggregate_revision,
                    "actions": [{ "type": "openSpecExport" }]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(export_preview.status(), StatusCode::OK);
        let export_preview = response_json(export_preview).await;
        let export_destination = export_preview["actions"][0]["openspecExport"]["destination"]
            .as_str()
            .unwrap();
        let changed_spec = format!("{spec_content}\n- RQ-002 Preserve the export baseline.\n");
        let changed_hash =
            atomic_write(&spec_path, changed_spec.as_bytes(), Some(&spec_hash)).unwrap();
        sqlx::query(
            "UPDATE sdd_artifact_revisions SET content_hash = ?
             WHERE artifact_revision_id = 'artifact-spec'",
        )
        .bind(&changed_hash)
        .execute(store.pool())
        .await
        .unwrap();
        let destination = authoritative.path().join(export_destination);
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("user-owned.txt"), "preserve\n").unwrap();
        let conflict = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/sdd/runs/run-delivery/commands",
                &json!({
                    "type": "confirmDelivery",
                    "requestId": "confirm-openspec-conflict",
                    "expectedRevision": export_preview["revision"],
                    "previewToken": export_preview["previewToken"],
                    "actions": [export_preview["actions"][0]["id"]]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::PRECONDITION_FAILED);
        assert!(
            response_json(conflict).await["message"]
                .as_str()
                .unwrap()
                .contains("both changed")
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("user-owned.txt")).unwrap(),
            "preserve\n"
        );
        let events = app
            .oneshot(get_request("/api/sdd/runs/run-delivery/events?after=0"))
            .await
            .unwrap();
        let events = response_json(events).await.to_string();
        assert!(!events.contains(&preview_token));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)] // serializes the task-specific AGENTUM_HOME seam
    async fn vertical_slice_is_durable_idempotent_and_does_not_dirty_source_checkout() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let repository = tempfile::tempdir().unwrap();
        let agentum_home = tempfile::tempdir().unwrap();
        git(repository.path(), &["init", "-q"]);
        git(
            repository.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(repository.path(), &["config", "user.name", "Test"]);
        std::fs::write(repository.path().join("README.md"), "fixture\n").unwrap();
        git(repository.path(), &["add", "README.md"]);
        git(repository.path(), &["commit", "-qm", "fixture"]);

        let _provider_probe = CodexProbeFixture::install(agentum_home.path());
        let _repository_registration = crate::routes::repos::register_test_repo(
            "repo-fixture",
            repository.path().to_string_lossy(),
        );
        let store = agentum_store::Store::open(&agentum_home.path().join("sdd-test.sqlite"))
            .await
            .unwrap();
        let app = test_app(store.clone());
        for legacy_path in ["/api/sdd/playbooks", "/api/harness"] {
            let retired = app
                .clone()
                .oneshot(get_request(legacy_path))
                .await
                .unwrap();
            assert_eq!(retired.status(), StatusCode::NOT_FOUND, "{legacy_path}");
        }
        let create_body = json!({
            "requestId": "create-demo-spec",
            "expectedRevision": 0,
            "title": "Refresh access tokens",
            "goal": "Refresh access tokens without interrupting active sessions",
            "profile": "standard",
            "control": "guarded",
            "provider": "codex",
            "baseRef": "HEAD",
            "source": {
                "type": "markdown",
                "markdown": "# Source context\n\nRefresh tokens without interrupting active sessions."
            },
            "specMarkdown": "# Refresh access tokens\n\n## Requirements\n\n- RQ-001 Refresh tokens without ending active sessions.\n\n## Acceptance criteria\n\n- AC-001 An active session remains usable during refresh."
        });
        let created = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/sdd/repos/repo-fixture/specs",
                &create_body,
            ))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = response_json(created).await;
        assert_eq!(created["nextAction"], "Spec approval required");
        let source_revision = normalize_markdown_intake(
            "Refresh access tokens",
            "# Source context\n\nRefresh tokens without interrupting active sessions.",
        )
        .unwrap()
        .source_revision;
        assert!(
            store
                .sdd_import_job("repo-fixture", "markdown", &source_revision)
                .await
                .unwrap()
                .is_some()
        );
        let run_id = created["runId"].as_str().unwrap();
        let authoritative = PathBuf::from(created["authoritativePath"].as_str().unwrap());
        assert!(!repository.path().join(".agentum").exists());
        assert!(git_output(repository.path(), &["status", "--porcelain"]).is_empty());
        assert_eq!(
            git_output(&authoritative, &["status", "--porcelain"]),
            "?? .agentum/"
        );
        let entries: Vec<_> = std::fs::read_dir(authoritative.join(".agentum"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries.len(), 2);

        let replay = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/sdd/repos/repo-fixture/specs",
                &create_body,
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(response_json(replay).await["runId"], run_id);

        // A fresh AppState over the same SQLite file models process restart.
        let restarted = test_app(store);
        let restored = restarted
            .clone()
            .oneshot(get_request(format!("/api/sdd/runs/{run_id}")))
            .await
            .unwrap();
        assert_eq!(restored.status(), StatusCode::OK);
        let restored = response_json(restored).await;
        assert_eq!(restored["run"]["status"], "waiting");
        assert_eq!(restored["approval"]["status"], "pending");

        let false_resume = restarted
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/sdd/runs/{run_id}/commands"),
                &json!({
                    "type": "resume",
                    "requestId": "cannot-resume-waiting",
                    "expectedRevision": 1
                }),
            ))
            .await
            .unwrap();
        assert_eq!(false_resume.status(), StatusCode::CONFLICT);
        let false_review = restarted
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/sdd/runs/{run_id}/commands"),
                &json!({
                    "type": "submitArtifact",
                    "requestId": "cannot-skip-to-review",
                    "expectedRevision": 1,
                    "kind": "review",
                    "content": "# Review\n\nLooks good.",
                    "expectedContentHash": "missing",
                    "attemptId": "not-an-attempt"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(false_review.status(), StatusCode::CONFLICT);
        assert!(
            !std::fs::read_dir(spec_dir_path(&authoritative))
                .unwrap()
                .any(|entry| entry.unwrap().file_name() == "review.md")
        );

        let approved = restarted
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/sdd/runs/{run_id}/commands"),
                &json!({
                    "type": "decideApproval",
                    "requestId": "approve-demo-spec",
                    "expectedRevision": 1,
                    "approvalId": restored["approval"]["approvalId"],
                    "digest": restored["approval"]["digest"],
                    "decision": "approve"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(approved.status(), StatusCode::OK);
        let approved = response_json(approved).await;
        assert_eq!(approved["phase"], "design");
        assert_eq!(approved["status"], "queued");

        let spec_dir = std::fs::read_dir(authoritative.join(".agentum/specs"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let spec_path = spec_dir.join("spec.md");
        let edited = std::fs::read_to_string(&spec_path)
            .unwrap()
            .replace("revision: 2", "revision: 3")
            .replace(
                "RQ-001 Refresh tokens without ending active sessions.",
                "RQ-001 Refresh tokens atomically without ending active sessions.",
            );
        std::fs::write(&spec_path, &edited).unwrap();
        let imported = restarted
            .clone()
            .oneshot(get_request(format!("/api/sdd/runs/{run_id}")))
            .await
            .unwrap();
        assert_eq!(imported.status(), StatusCode::OK);
        let imported = response_json(imported).await;
        assert_eq!(imported["spec"]["currentRevision"], 3);
        assert_eq!(imported["run"]["status"], "paused");
        assert!(imported["approval"].is_null());

        let invalid = edited.replace(
            &format!("id: {}", imported["spec"]["specId"].as_str().unwrap()),
            "id: SPC-00000000000000000000000000",
        );
        std::fs::write(&spec_path, &invalid).unwrap();
        let blocked = restarted
            .oneshot(get_request(format!("/api/sdd/runs/{run_id}")))
            .await
            .unwrap();
        assert_eq!(blocked.status(), StatusCode::OK);
        let blocked = response_json(blocked).await;
        assert_eq!(blocked["run"]["status"], "blocked");
        assert!(
            blocked["run"]["blocker"]
                .as_str()
                .unwrap()
                .contains("canonical id")
        );
        assert_eq!(std::fs::read_to_string(&spec_path).unwrap(), invalid);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)] // serializes the task-specific AGENTUM_HOME seam
    async fn list_specs_discovers_and_starts_a_durable_first_run() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let repository = tempfile::tempdir().unwrap();
        let agentum_home = tempfile::tempdir().unwrap();
        git(repository.path(), &["init", "-q"]);
        git(
            repository.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(repository.path(), &["config", "user.name", "Test"]);
        std::fs::write(repository.path().join("README.md"), "fixture\n").unwrap();
        git(repository.path(), &["add", "README.md"]);
        git(repository.path(), &["commit", "-qm", "fixture"]);
        let _provider_probe = CodexProbeFixture::install(agentum_home.path());

        let artifact_set_id = ulid::Ulid::new();
        let mut spec_paths = Vec::new();
        let mut expected_ids = Vec::new();
        let mut first_spec_dir = None;
        let mut first_spec_content = None;
        for (index, title) in [
            "First migrated spec",
            "Second migrated spec",
            "Third migrated spec",
        ]
        .into_iter()
        .enumerate()
        {
            let spec_id = SpecId::new();
            let root = initialize(repository.path(), &spec_id, title, artifact_set_id).unwrap();
            let rendered = render_spec(
                &spec_id,
                1,
                title,
                None,
                "## Requirements\n\n- RQ-001 Preserve imported intent.\n\n## Acceptance criteria\n\n- AC-001 The specification is visible in Run Center.",
            )
            .unwrap();
            atomic_write(
                &root.spec_dir.join("spec.md"),
                rendered.as_bytes(),
                Some(MISSING_HASH),
            )
            .unwrap();
            if index == 0 {
                first_spec_dir = Some(root.spec_dir.clone());
                first_spec_content = Some(rendered.clone());
                let design = "# Historical design\n\nPreserve this material for impact analysis.\n";
                atomic_write(
                    &root.spec_dir.join("design.md"),
                    design.as_bytes(),
                    Some(MISSING_HASH),
                )
                .unwrap();
                let mut plan = serde_json::to_string_pretty(&json!({
                    "schemaVersion": 1,
                    "specId": spec_id,
                    "specRevision": 1,
                    "tasks": [{
                        "id": "T-001",
                        "objective": "Historical planning intent",
                        "dependencies": [],
                        "readScopes": [],
                        "writeScopes": [],
                        "acceptanceCriteria": ["AC-001"],
                        "verification": [],
                        "risk": "low",
                        "parallelSafe": true
                    }]
                }))
                .unwrap();
                plan.push('\n');
                atomic_write(
                    &root.spec_dir.join("plan.json"),
                    plan.as_bytes(),
                    Some(MISSING_HASH),
                )
                .unwrap();
            }
            spec_paths.push(root.spec_dir.join("spec.md"));
            expected_ids.push(spec_id.to_string());
        }
        let _registration = crate::routes::repos::register_test_repo(
            "repo-discovered-specs",
            repository.path().to_string_lossy(),
        );
        let store = agentum_store::Store::open(&agentum_home.path().join("sdd.sqlite"))
            .await
            .unwrap();
        let app = test_app(store.clone());

        let listed = app
            .clone()
            .oneshot(get_request("/api/sdd/repos/repo-discovered-specs/specs"))
            .await
            .unwrap();
        let listed_status = listed.status();
        let listed = response_json(listed).await;
        assert_eq!(listed_status, StatusCode::OK, "{listed}");
        let specs = listed["specs"].as_array().unwrap();
        assert_eq!(specs.len(), expected_ids.len());
        assert!(
            expected_ids
                .iter()
                .all(|expected| specs.iter().any(|spec| spec["specId"] == expected.as_str()))
        );
        assert!(specs.iter().all(|spec| spec["provider"] == "unassigned"));
        assert_eq!(
            store
                .sdd_list_specs("repo-discovered-specs")
                .await
                .unwrap()
                .len(),
            3
        );
        let source_status = git_output(repository.path(), &["status", "--porcelain"]);
        let create_run_body = json!({
            "requestId": "start-discovered",
            "expectedRevision": 1,
            "profile": "standard",
            "control": "guarded",
            "provider": "codex",
            "baseRef": "HEAD",
            "sourceCheckout": "snapshot"
        });
        let created = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/sdd/specs/{}/runs", expected_ids[0]),
                &create_run_body,
            ))
            .await
            .unwrap();
        let created_status = created.status();
        let created = response_json(created).await;
        assert_eq!(created_status, StatusCode::OK, "{created}");
        assert_eq!(created["phase"], "specification");
        assert_eq!(created["status"], "waiting");
        assert_eq!(created["nextAction"], "Spec approval required");
        assert_eq!(created["approval"]["purpose"], "specification");
        assert_eq!(created["approval"]["status"], "pending");
        assert_eq!(
            created["downstreamDisposition"],
            "historical_unapproved_reopen_from_specification"
        );
        assert_eq!(
            created["preservedLaterArtifacts"],
            json!(["design.md", "plan.json"])
        );
        assert_eq!(
            git_output(repository.path(), &["status", "--porcelain"]),
            source_status
        );

        let run_id = created["runId"].as_str().unwrap();
        let authoritative = PathBuf::from(created["authoritativePath"].as_str().unwrap());
        assert!(!authoritative.starts_with(repository.path()));
        let snapshot = store.sdd_snapshot(run_id).await.unwrap().unwrap();
        assert_eq!(snapshot.spec.aggregate_revision, 2);
        assert_eq!(snapshot.spec.profile, "standard");
        assert_eq!(snapshot.spec.control, "guarded");
        assert_eq!(snapshot.spec.provider, "codex");
        assert_eq!(snapshot.run.phase, "specification");
        assert_eq!(snapshot.run.status, "waiting");
        assert_eq!(snapshot.attempts.len(), 1);
        assert_eq!(snapshot.attempts[0].status, "succeeded");
        assert_eq!(snapshot.artifacts.len(), 3);
        assert!(snapshot.tasks.is_empty());
        assert_eq!(snapshot.approval.as_ref().unwrap().purpose, "specification");
        let policy: Value = serde_json::from_str(&snapshot.run.policy_json).unwrap();
        assert_eq!(
            policy["discoveredArtifactDisposition"]["status"],
            "historical_unapproved_reopen_from_specification"
        );

        let authoritative_spec_dir = authoritative
            .join(".agentum/specs")
            .join(first_spec_dir.as_ref().unwrap().file_name().unwrap());
        assert_eq!(
            std::fs::read_to_string(authoritative_spec_dir.join("spec.md")).unwrap(),
            first_spec_content.unwrap()
        );
        let historical_design =
            std::fs::read_to_string(first_spec_dir.as_ref().unwrap().join("design.md")).unwrap();
        assert_eq!(
            std::fs::read_to_string(authoritative_spec_dir.join("design.md")).unwrap(),
            historical_design
        );
        assert_eq!(
            std::fs::read_to_string(authoritative_spec_dir.join("plan.json")).unwrap(),
            std::fs::read_to_string(first_spec_dir.as_ref().unwrap().join("plan.json")).unwrap()
        );

        // Historical later-phase bytes are preserved but are deliberately not
        // part of this specification-only approval digest.
        let canonical: SpecId = expected_ids[0].parse().unwrap();
        let spec_relative = format!(
            ".agentum/specs/{}/spec.md",
            first_spec_dir
                .as_ref()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
        );
        let spec_hash = snapshot
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == "specification")
            .unwrap()
            .content_hash
            .as_str();
        assert_eq!(
            created["approval"]["digest"],
            approval_digest(
                &canonical,
                1,
                &[(spec_relative.as_str(), spec_hash)],
                &policy,
                &snapshot.run.workspace_fingerprint
            )
        );
        std::fs::write(
            first_spec_dir.as_ref().unwrap().join("design.md"),
            "# Changed historical design\n\nThis is still not approved.\n",
        )
        .unwrap();
        let restored = app
            .clone()
            .oneshot(get_request(format!("/api/sdd/runs/{run_id}")))
            .await
            .unwrap();
        assert_eq!(restored.status(), StatusCode::OK);
        let restored = response_json(restored).await;
        assert_eq!(restored["approval"]["purpose"], "specification");
        assert_eq!(
            restored["approval"]["digest"],
            created["approval"]["digest"]
        );

        let replay = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/sdd/specs/{}/runs", expected_ids[0]),
                &create_run_body,
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(response_json(replay).await, created);

        let reused = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/sdd/specs/{}/runs", expected_ids[0]),
                &json!({
                    "requestId": "start-discovered-again",
                    "expectedRevision": 2,
                    "profile": "standard",
                    "control": "guarded",
                    "provider": "codex",
                    "baseRef": "HEAD",
                    "sourceCheckout": "snapshot"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(reused.status(), StatusCode::OK);
        let reused = response_json(reused).await;
        assert_eq!(reused["reused"], true);
        assert_eq!(reused["run"]["runId"], run_id);

        let edited = std::fs::read_to_string(&spec_paths[1]).unwrap().replace(
            "Preserve imported intent",
            "Preserve externally changed intent",
        );
        std::fs::write(&spec_paths[1], edited).unwrap();
        let rejected = app
            .oneshot(get_request("/api/sdd/repos/repo-discovered-specs/specs"))
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            store
                .sdd_list_specs("repo-discovered-specs")
                .await
                .unwrap()
                .len(),
            3
        );
    }

    fn test_app(store: agentum_store::Store) -> Router {
        let (bus, _) = broadcast::channel::<Event>(32);
        let mut state = AppState::new(store, bus);
        state.no_auth = true;
        state.embedded_ui_token = Some(std::sync::Arc::new(TEST_UI_TOKEN.into()));
        crate::router(state)
    }

    #[cfg(unix)]
    fn spec_dir_path(authoritative: &FsPath) -> PathBuf {
        std::fs::read_dir(authoritative.join(".agentum/specs"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
    }

    fn json_request(method: &str, path: &str, body: &Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {TEST_UI_TOKEN}"))
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get_request(path: impl AsRef<str>) -> Request<Body> {
        Request::get(path.as_ref())
            .header("authorization", format!("Bearer {TEST_UI_TOKEN}"))
            .body(Body::empty())
            .unwrap()
    }

    async fn response_json(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn git(repository: &FsPath, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn git_output(repository: &FsPath, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}
