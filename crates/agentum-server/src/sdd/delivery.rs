//! Explicit, preview/confirm delivery for Ready SDD runs.
//!
//! Every subprocess is constructed as a program plus argument vector. No
//! generated shell string is evaluated. The durable store owns authorization,
//! claims, retries, and results; this module only performs a claimed side
//! effect and reports a bounded/redacted outcome.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use agentum_store::sdd::SddRunRecord;
use agentum_store::sdd_delivery::{
    DeliveryActionResult, SddDeliveryActionRecord, SddDeliveryPreviewRecord, SddExternalLinkRecord,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::AppState;

use super::artifacts::{AnchoredDirectory, AnchoredEntryKind, read_bytes};
use super::credentials::{
    JiraApiTokenCredential, JiraCredential, LinearCredential, get_jira_credential,
    get_linear_credential,
};
use super::remote::{
    REMOTE_SDD_SCHEMA_VERSION, RemoteDeliveryActionRequest, RemoteDeliveryActionStatus,
    RemoteLifecyclePlan, RemoteSddTransport,
};
use super::sha256;
use super::sources::OpenSpecExportPreview;

const MAX_ACTIONS: usize = 12;
const MAX_WORKSPACE_BYTES: usize = 64 * 1024 * 1024;
const OUTPUT_LIMIT: usize = 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const TRACKER_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TRACKER_RESPONSE: usize = 2 * 1024 * 1024;
const LINEAR_GRAPHQL: &str = "https://api.linear.app/graphql";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DeliveryActionRequest {
    Commit {
        message: String,
    },
    Push {
        #[serde(default = "default_remote")]
        remote: String,
    },
    PullRequest {
        title: String,
        body: String,
        base: String,
    },
    TrackerComment {
        body: String,
    },
    TrackerStatus {
        status: String,
        /// Provider transition/state identity selected from a prior ambiguous
        /// preview. Jira in particular can expose multiple allowed transitions
        /// with the same destination name, so Agentum never guesses.
        #[serde(default)]
        transition_id: Option<String>,
    },
    TrackerFieldUpdate {
        field_id: String,
        value: TrackerFieldValue,
    },
    Release {
        tag: String,
        name: String,
        notes: String,
        #[serde(default)]
        prerelease: bool,
    },
    OpenSpecExport,
}

fn default_remote() -> String {
    "origin".into()
}

impl DeliveryActionRequest {
    fn kind(&self) -> &'static str {
        match self {
            Self::Commit { .. } => "commit",
            Self::Push { .. } => "push",
            Self::PullRequest { .. } => "pull_request",
            Self::TrackerComment { .. } => "tracker_comment",
            Self::TrackerStatus { .. } => "tracker_status",
            Self::TrackerFieldUpdate { .. } => "tracker_field_update",
            Self::Release { .. } => "release",
            Self::OpenSpecExport => "openspec_export",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedDeliveryAction {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub depends_on: Vec<String>,
    pub intent: DeliveryActionRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openspec_export: Option<OpenSpecExportPreview>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_mutation: Option<TrackerMutationPreview>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrackerMutationPreview {
    pub provider: String,
    pub connection_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_id: Option<String>,
    pub external_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub url: String,
    pub source_revision: String,
    pub live_revision: String,
    pub operation: TrackerMutationOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TrackerMutationOperation {
    Comment {
        marker: String,
    },
    Status {
        target_id: String,
        target_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_id: Option<String>,
        current_status_id: String,
        current_status_name: String,
    },
    FieldUpdate {
        field_id: String,
        field_name: String,
        value: TrackerFieldValue,
        current_value_hash: String,
        target_value_hash: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrackerTransitionChoice {
    pub id: String,
    pub name: String,
    pub target_id: String,
    pub target_name: String,
}

/// Closed typed values accepted by the public delivery contract. Provider JSON
/// is derived from these values only after live edit metadata authorization.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TrackerFieldValue {
    Text { value: String },
    Number { value: serde_json::Number },
    Boolean { value: bool },
    Option { option_id: String },
    User { account_id: String },
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryPreviewEnvelope {
    pub schema_version: u32,
    pub actor_id: String,
    pub repo_id: String,
    pub spec_id: String,
    pub spec_revision: i64,
    pub run_id: String,
    pub run_revision: i64,
    pub base_commit: String,
    pub branch_name: String,
    pub worktree_identity: String,
    pub workspace_fingerprint: String,
    pub workspace_state_hash: String,
    pub artifact_hashes: Vec<DeliveryArtifactHash>,
    pub actions: Vec<PreparedDeliveryAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryArtifactHash {
    pub kind: String,
    pub relative_path: String,
    pub content_hash: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DeliveryError {
    #[error("store: {0}")]
    Store(#[from] agentum_store::StoreError),
    #[error("artifact boundary: {0}")]
    Artifact(#[from] super::artifacts::ArtifactError),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("source conversion: {0}")]
    Source(#[from] super::sources::SourceError),
    #[error("delivery request is invalid: {0}")]
    Invalid(String),
    #[error("delivery conflict: {0}")]
    Conflict(String),
    #[error("delivery precondition failed: {0}")]
    Precondition(String),
    #[error("tracker transition requires an explicit choice")]
    TransitionChoiceRequired {
        provider: String,
        target: String,
        choices: Vec<TrackerTransitionChoice>,
    },
    #[error("delivery command failed: {0}")]
    Command(String),
}

pub(crate) fn prepare_actions(
    requested: Vec<DeliveryActionRequest>,
) -> Result<Vec<PreparedDeliveryAction>, DeliveryError> {
    if requested.is_empty() || requested.len() > MAX_ACTIONS {
        return Err(DeliveryError::Invalid(format!(
            "delivery requires 1 to {MAX_ACTIONS} actions"
        )));
    }
    let mut seen = HashSet::new();
    for action in &requested {
        if !seen.insert(action.kind()) {
            return Err(DeliveryError::Invalid(format!(
                "only one {} action may be previewed at a time",
                action.kind()
            )));
        }
        validate_action(action)?;
    }
    let mut prepared = requested
        .into_iter()
        .map(|intent| PreparedDeliveryAction {
            id: uuid::Uuid::new_v4().to_string(),
            kind: intent.kind().into(),
            depends_on: Vec::new(),
            intent,
            openspec_export: None,
            tracker_mutation: None,
        })
        .collect::<Vec<_>>();
    let ids = prepared
        .iter()
        .map(|action| (action.kind.clone(), action.id.clone()))
        .collect::<HashMap<_, _>>();
    for action in &mut prepared {
        match action.kind.as_str() {
            "push" => add_dependency(&mut action.depends_on, &ids, "commit"),
            "pull_request" => add_dependency(&mut action.depends_on, &ids, "push"),
            "release" => {
                add_dependency(&mut action.depends_on, &ids, "commit");
                add_dependency(&mut action.depends_on, &ids, "push");
            }
            _ => {}
        }
    }
    Ok(prepared)
}

fn add_dependency(target: &mut Vec<String>, ids: &HashMap<String, String>, kind: &str) {
    if let Some(id) = ids.get(kind) {
        target.push(id.clone());
    }
}

fn validate_action(action: &DeliveryActionRequest) -> Result<(), DeliveryError> {
    match action {
        DeliveryActionRequest::Commit { message } => validate_text("commit message", message, 512),
        DeliveryActionRequest::Push { remote } => validate_ref_component("remote", remote),
        DeliveryActionRequest::PullRequest { title, body, base } => {
            validate_text("pull request title", title, 512)?;
            validate_text("pull request body", body, 64 * 1024)?;
            validate_ref_component("pull request base", base)
        }
        DeliveryActionRequest::TrackerComment { body } => {
            validate_text("tracker comment", body, 64 * 1024)
        }
        DeliveryActionRequest::TrackerStatus {
            status,
            transition_id,
        } => {
            validate_text("tracker status", status, 512)?;
            if let Some(transition_id) = transition_id {
                validate_text("tracker transition id", transition_id, 512)?;
            }
            Ok(())
        }
        DeliveryActionRequest::TrackerFieldUpdate { field_id, value } => {
            validate_field_id(field_id)?;
            validate_tracker_field_value(value)
        }
        DeliveryActionRequest::Release {
            tag, name, notes, ..
        } => {
            validate_ref_component("release tag", tag)?;
            validate_text("release name", name, 512)?;
            validate_text("release notes", notes, 128 * 1024)
        }
        DeliveryActionRequest::OpenSpecExport => Ok(()),
    }
}

fn validate_field_id(value: &str) -> Result<(), DeliveryError> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(DeliveryError::Invalid(
            "tracker field id must be 1..=256 ASCII letters, digits, '.', '_' or '-'".into(),
        ));
    }
    Ok(())
}

fn validate_tracker_field_value(value: &TrackerFieldValue) -> Result<(), DeliveryError> {
    match value {
        TrackerFieldValue::Text { value } => validate_text("tracker field text", value, 64 * 1024),
        TrackerFieldValue::Number { .. }
        | TrackerFieldValue::Boolean { .. }
        | TrackerFieldValue::Clear => Ok(()),
        TrackerFieldValue::Option { option_id } => {
            validate_text("tracker field option id", option_id, 512)
        }
        TrackerFieldValue::User { account_id } => {
            validate_text("tracker field account id", account_id, 512)
        }
    }
}

/// Resolve tracker actions against the provider's live state and bind the
/// exact external identity, revision and transition into the immutable preview.
/// Credentials are used only for this live read and are never serialized.
pub(crate) async fn bind_tracker_mutations(
    state: &AppState,
    run: &SddRunRecord,
    actions: &mut [PreparedDeliveryAction],
) -> Result<(), DeliveryError> {
    if !actions.iter().any(|action| {
        matches!(
            action.kind.as_str(),
            "tracker_comment" | "tracker_status" | "tracker_field_update"
        )
    }) {
        return Ok(());
    }
    let link = state
        .store
        .sdd_external_link_for_spec(&run.spec_id)
        .await?
        .ok_or_else(|| {
            DeliveryError::Invalid(
                "tracker delivery requires an imported external work-item reference".into(),
            )
        })?;
    for action in actions.iter_mut().filter(|action| {
        matches!(
            action.kind.as_str(),
            "tracker_comment" | "tracker_status" | "tracker_field_update"
        )
    }) {
        let marker = format!("<!-- agentum-delivery:{} -->", action.id);
        let binding = match &action.intent {
            DeliveryActionRequest::TrackerComment { .. } => {
                bind_tracker_comment(state, run, &link, marker).await?
            }
            DeliveryActionRequest::TrackerStatus {
                status,
                transition_id,
            } => bind_tracker_status(state, run, &link, status, transition_id.as_deref()).await?,
            DeliveryActionRequest::TrackerFieldUpdate { field_id, value } => {
                bind_tracker_field_update(state, &link, field_id, value).await?
            }
            _ => unreachable!("tracker action kind and intent disagree"),
        };
        action.tracker_mutation = Some(binding);
    }
    Ok(())
}

/// Re-read the provider immediately before first confirmation. A tracker edit
/// after preview invalidates the token rather than being silently overwritten.
/// Retries reuse the confirmed binding and reconcile in the executor instead.
pub(crate) async fn validate_tracker_mutations(
    state: &AppState,
    run: &SddRunRecord,
    actions: &[PreparedDeliveryAction],
) -> Result<(), DeliveryError> {
    for action in actions.iter().filter(|action| {
        matches!(
            action.kind.as_str(),
            "tracker_comment" | "tracker_status" | "tracker_field_update"
        )
    }) {
        let expected = action.tracker_mutation.as_ref().ok_or_else(|| {
            DeliveryError::Invalid("tracker action is missing its hash-bound preview".into())
        })?;
        let link = state
            .store
            .sdd_external_link_for_spec(&run.spec_id)
            .await?
            .ok_or_else(|| {
                DeliveryError::Precondition("external work-item link was removed".into())
            })?;
        ensure_link_matches(expected, &link)?;
        let live_revision = tracker_live_revision(state, run, expected).await?;
        if live_revision != expected.live_revision {
            return Err(DeliveryError::Precondition(
                "external work item changed after delivery preview; create a new preview".into(),
            ));
        }
        if let TrackerMutationOperation::FieldUpdate {
            field_id,
            field_name,
            value,
            current_value_hash,
            target_value_hash,
        } = &expected.operation
        {
            let credential = jira_delivery_credential(state, expected).await?;
            let live = jira_field_binding(&credential, expected, field_id, value).await?;
            if &live.field_name != field_name
                || sha256(canonical_json_bytes(&live.current_value)?) != *current_value_hash
                || sha256(canonical_json_bytes(&live.target_compare_value)?) != *target_value_hash
            {
                return Err(DeliveryError::Precondition(
                    "Jira field metadata or value changed after delivery preview".into(),
                ));
            }
        }
        if let TrackerMutationOperation::Status {
            target_id,
            transition_id: Some(transition_id),
            ..
        } = &expected.operation
        {
            if expected.provider == "jira" {
                let credential = jira_delivery_credential(state, expected).await?;
                let snapshot = jira_issue_snapshot(&credential, expected).await?;
                if snapshot.status_id != *target_id
                    && !snapshot
                        .transitions
                        .iter()
                        .any(|choice| choice.id == *transition_id && choice.target_id == *target_id)
                {
                    return Err(DeliveryError::Precondition(
                        "the previewed Jira transition is no longer allowed".into(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn ensure_link_matches(
    expected: &TrackerMutationPreview,
    actual: &SddExternalLinkRecord,
) -> Result<(), DeliveryError> {
    if expected.provider != actual.provider
        || expected.connection_id != actual.connection_id
        || expected.site_id != actual.site_id
        || expected.external_id != actual.external_id
        || expected.key != actual.key
        || expected.url != actual.url
        || expected.source_revision != actual.source_revision
    {
        return Err(DeliveryError::Precondition(
            "external work-item identity changed after delivery preview".into(),
        ));
    }
    Ok(())
}

async fn bind_tracker_comment(
    state: &AppState,
    run: &SddRunRecord,
    link: &SddExternalLinkRecord,
    marker: String,
) -> Result<TrackerMutationPreview, DeliveryError> {
    let live_revision = match link.provider.as_str() {
        "github" => {
            let cwd = integration_adapter_cwd(state, run).await?;
            github_issue_snapshot(&cwd, &link.url).await?.revision
        }
        "linear" => {
            let credential = linear_delivery_credential(state, &link.connection_id).await?;
            linear_issue_snapshot(&credential, &link.external_id)
                .await?
                .revision
        }
        "jira" => {
            require_jira_write_scope(state, &link.connection_id).await?;
            let expected = tracker_preview_shell(
                link,
                String::new(),
                TrackerMutationOperation::Comment {
                    marker: marker.clone(),
                },
            );
            let credential = jira_delivery_credential(state, &expected).await?;
            jira_issue_snapshot(&credential, &expected).await?.revision
        }
        provider => {
            return Err(DeliveryError::Precondition(format!(
                "tracker delivery provider {provider:?} is unsupported"
            )));
        }
    };
    Ok(tracker_preview_shell(
        link,
        live_revision,
        TrackerMutationOperation::Comment { marker },
    ))
}

async fn bind_tracker_status(
    state: &AppState,
    run: &SddRunRecord,
    link: &SddExternalLinkRecord,
    requested: &str,
    selected_transition_id: Option<&str>,
) -> Result<TrackerMutationPreview, DeliveryError> {
    let (revision, operation) = match link.provider.as_str() {
        "github" => {
            if selected_transition_id.is_some() || !matches!(requested, "open" | "closed") {
                return Err(DeliveryError::Invalid(
                    "GitHub tracker status must be open or closed and has no transition id".into(),
                ));
            }
            let cwd = integration_adapter_cwd(state, run).await?;
            let snapshot = github_issue_snapshot(&cwd, &link.url).await?;
            (
                snapshot.revision,
                TrackerMutationOperation::Status {
                    target_id: requested.to_owned(),
                    target_name: requested.to_owned(),
                    transition_id: None,
                    current_status_id: snapshot.status_id,
                    current_status_name: snapshot.status_name,
                },
            )
        }
        "linear" => {
            let credential = linear_delivery_credential(state, &link.connection_id).await?;
            let snapshot = linear_issue_snapshot(&credential, &link.external_id).await?;
            let choice =
                resolve_linear_state(requested, selected_transition_id, &snapshot.transitions)?;
            (
                snapshot.revision,
                TrackerMutationOperation::Status {
                    target_id: choice.target_id,
                    target_name: choice.target_name,
                    transition_id: Some(choice.id),
                    current_status_id: snapshot.status_id,
                    current_status_name: snapshot.status_name,
                },
            )
        }
        "jira" => {
            require_jira_write_scope(state, &link.connection_id).await?;
            let shell = tracker_preview_shell(
                link,
                String::new(),
                TrackerMutationOperation::Comment {
                    marker: String::new(),
                },
            );
            let credential = jira_delivery_credential(state, &shell).await?;
            let snapshot = jira_issue_snapshot(&credential, &shell).await?;
            let already_target =
                status_matches(requested, &snapshot.status_id, &snapshot.status_name);
            let choice = if already_target {
                if selected_transition_id.is_some() {
                    return Err(DeliveryError::Invalid(
                        "a Jira transition id is unnecessary because the issue already has the target status"
                            .into(),
                    ));
                }
                None
            } else {
                Some(resolve_jira_transition(
                    requested,
                    selected_transition_id,
                    &snapshot.transitions,
                )?)
            };
            let (target_id, target_name, transition_id) = choice
                .map(|choice| (choice.target_id, choice.target_name, Some(choice.id)))
                .unwrap_or_else(|| {
                    (
                        snapshot.status_id.clone(),
                        snapshot.status_name.clone(),
                        None,
                    )
                });
            (
                snapshot.revision,
                TrackerMutationOperation::Status {
                    target_id,
                    target_name,
                    transition_id,
                    current_status_id: snapshot.status_id,
                    current_status_name: snapshot.status_name,
                },
            )
        }
        provider => {
            return Err(DeliveryError::Precondition(format!(
                "tracker delivery provider {provider:?} is unsupported"
            )));
        }
    };
    Ok(tracker_preview_shell(link, revision, operation))
}

async fn bind_tracker_field_update(
    state: &AppState,
    link: &SddExternalLinkRecord,
    field_id: &str,
    value: &TrackerFieldValue,
) -> Result<TrackerMutationPreview, DeliveryError> {
    if link.provider != "jira" {
        return Err(DeliveryError::Precondition(
            "typed tracker field updates are currently supported only for Jira".into(),
        ));
    }
    require_jira_write_scope(state, &link.connection_id).await?;
    let shell = tracker_preview_shell(
        link,
        String::new(),
        TrackerMutationOperation::Comment {
            marker: String::new(),
        },
    );
    let credential = jira_delivery_credential(state, &shell).await?;
    let field = jira_field_binding(&credential, &shell, field_id, value).await?;
    Ok(tracker_preview_shell(
        link,
        field.revision,
        TrackerMutationOperation::FieldUpdate {
            field_id: field_id.to_owned(),
            field_name: field.field_name,
            value: value.clone(),
            current_value_hash: sha256(canonical_json_bytes(&field.current_value)?),
            target_value_hash: sha256(canonical_json_bytes(&field.target_compare_value)?),
        },
    ))
}

fn tracker_preview_shell(
    link: &SddExternalLinkRecord,
    live_revision: String,
    operation: TrackerMutationOperation,
) -> TrackerMutationPreview {
    TrackerMutationPreview {
        provider: link.provider.clone(),
        connection_id: link.connection_id.clone(),
        site_id: link.site_id.clone(),
        external_id: link.external_id.clone(),
        key: link.key.clone(),
        url: link.url.clone(),
        source_revision: link.source_revision.clone(),
        live_revision,
        operation,
    }
}

fn resolve_linear_state(
    requested: &str,
    selected_id: Option<&str>,
    choices: &[TrackerTransitionChoice],
) -> Result<TrackerTransitionChoice, DeliveryError> {
    resolve_transition_choice("linear", requested, selected_id, choices)
}

fn resolve_jira_transition(
    requested: &str,
    selected_id: Option<&str>,
    choices: &[TrackerTransitionChoice],
) -> Result<TrackerTransitionChoice, DeliveryError> {
    resolve_transition_choice("jira", requested, selected_id, choices)
}

fn resolve_transition_choice(
    provider: &str,
    requested: &str,
    selected_id: Option<&str>,
    choices: &[TrackerTransitionChoice],
) -> Result<TrackerTransitionChoice, DeliveryError> {
    let matching = choices
        .iter()
        .filter(|choice| {
            status_matches(requested, &choice.target_id, &choice.target_name)
                || choice.name.eq_ignore_ascii_case(requested)
        })
        .cloned()
        .collect::<Vec<_>>();
    if let Some(selected_id) = selected_id {
        return matching
            .into_iter()
            .find(|choice| choice.id == selected_id)
            .ok_or_else(|| {
                DeliveryError::Precondition(format!(
                    "selected {provider} transition is not currently allowed for the requested status"
                ))
            });
    }
    match matching.as_slice() {
        [choice] => Ok(choice.clone()),
        [] => Err(DeliveryError::Precondition(format!(
            "no allowed {provider} transition reaches status {requested:?}"
        ))),
        _ => Err(DeliveryError::TransitionChoiceRequired {
            provider: provider.to_owned(),
            target: requested.to_owned(),
            choices: matching,
        }),
    }
}

fn status_matches(requested: &str, id: &str, name: &str) -> bool {
    id.eq_ignore_ascii_case(requested) || name.eq_ignore_ascii_case(requested)
}

#[derive(Debug)]
struct TrackerSnapshot {
    revision: String,
    status_id: String,
    status_name: String,
    transitions: Vec<TrackerTransitionChoice>,
}

async fn github_issue_snapshot(
    worktree: &Path,
    url: &str,
) -> Result<TrackerSnapshot, DeliveryError> {
    let result = run_gh_owned(
        worktree,
        vec![
            "issue".into(),
            "view".into(),
            url.into(),
            "--json".into(),
            "updatedAt,state".into(),
        ],
    )
    .await
    .map_err(|value| {
        DeliveryError::Command(format!(
            "GitHub issue read failed ({})",
            sha256(value.to_string())
        ))
    })?;
    let value: Value = serde_json::from_slice(&result.stdout)?;
    let revision = required_json_string(&value, "updatedAt", "GitHub issue")?;
    let status_name = required_json_string(&value, "state", "GitHub issue")?.to_ascii_lowercase();
    Ok(TrackerSnapshot {
        revision,
        status_id: status_name.clone(),
        status_name,
        transitions: Vec::new(),
    })
}

async fn linear_delivery_credential(
    state: &AppState,
    connection_id: &str,
) -> Result<LinearCredential, DeliveryError> {
    let vault = state.sdd_credentials.clone();
    let connection_id = connection_id.to_owned();
    tokio::task::spawn_blocking(move || get_linear_credential(vault.as_ref(), Some(&connection_id)))
        .await
        .map_err(|_| DeliveryError::Precondition("Linear credential vault failed".into()))?
        .map_err(|_| DeliveryError::Precondition("Linear credential is unavailable".into()))?
        .ok_or_else(|| DeliveryError::Precondition("Linear credential is not configured".into()))
}

async fn linear_issue_snapshot(
    credential: &LinearCredential,
    external_id: &str,
) -> Result<TrackerSnapshot, DeliveryError> {
    const QUERY: &str = "query($id: String!) { issue(id: $id) { id updatedAt state { id name } team { states { nodes { id name } } } } }";
    let value = linear_graphql(credential.token(), QUERY, json!({ "id": external_id }))
        .await
        .map_err(TrackerRemoteError::into_delivery)?;
    let issue = value
        .pointer("/data/issue")
        .filter(|value| !value.is_null())
        .ok_or_else(|| {
            DeliveryError::Precondition("Linear issue is unavailable to this connection".into())
        })?;
    let revision = required_json_string(issue, "updatedAt", "Linear issue")?;
    let state = issue
        .get("state")
        .ok_or_else(|| DeliveryError::Precondition("Linear issue has no workflow state".into()))?;
    let status_id = required_json_string(state, "id", "Linear workflow state")?;
    let status_name = required_json_string(state, "name", "Linear workflow state")?;
    let transitions = issue
        .pointer("/team/states/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DeliveryError::Precondition("Linear workflow states are unavailable".into())
        })?
        .iter()
        .map(|state| {
            let id = required_json_string(state, "id", "Linear workflow state")?;
            let name = required_json_string(state, "name", "Linear workflow state")?;
            Ok(TrackerTransitionChoice {
                id: id.clone(),
                name: name.clone(),
                target_id: id,
                target_name: name,
            })
        })
        .collect::<Result<Vec<_>, DeliveryError>>()?;
    Ok(TrackerSnapshot {
        revision,
        status_id,
        status_name,
        transitions,
    })
}

async fn require_jira_write_scope(
    state: &AppState,
    connection_id: &str,
) -> Result<(), DeliveryError> {
    let authorized = super::jira::delivery_write_authorized(state, connection_id)
        .await
        .map_err(|error| {
            DeliveryError::Command(format!(
                "Jira delivery authorization check failed ({})",
                sha256(error.to_string())
            ))
        })?;
    if !authorized {
        return Err(DeliveryError::Precondition(
            "Jira connection has no encrypted, revision-matched delivery grant".into(),
        ));
    }
    Ok(())
}

async fn jira_delivery_credential(
    state: &AppState,
    preview: &TrackerMutationPreview,
) -> Result<JiraDeliveryCredential, DeliveryError> {
    let site_id = preview.site_id.as_deref().ok_or_else(|| {
        DeliveryError::Precondition("Jira delivery requires an explicit site".into())
    })?;
    require_jira_write_scope(state, &preview.connection_id).await?;
    let vault = state.sdd_credentials.clone();
    let connection_id = preview.connection_id.clone();
    let credential = tokio::task::spawn_blocking(move || {
        get_jira_credential(vault.as_ref(), Some(&connection_id))
    })
    .await
    .map_err(|_| DeliveryError::Precondition("Jira credential vault failed".into()))?
    .map_err(|_| DeliveryError::Precondition("Jira credential is unavailable".into()))?;
    if let Some(credential) = credential {
        if credential.connection_id != preview.connection_id
            || credential.selected_site_id != site_id
            || credential.selected_site().is_none()
        {
            return Err(DeliveryError::Precondition(
                "Jira connection or selected site does not match the imported work item".into(),
            ));
        }
        let credential = super::jira::ensure_fresh_credential(state, credential)
            .await
            .map_err(|error| {
                DeliveryError::Command(format!(
                    "Jira credential refresh failed ({})",
                    sha256(error.to_string())
                ))
            })?;
        return Ok(JiraDeliveryCredential::Oauth(credential));
    }
    let credential = super::jira::api_token_credential(state, &preview.connection_id)
        .await
        .map_err(|error| {
            DeliveryError::Command(format!(
                "Jira API-token credential check failed ({})",
                sha256(error.to_string())
            ))
        })?
        .ok_or_else(|| DeliveryError::Precondition("Jira credential is not configured".into()))?;
    if credential.connection_id != preview.connection_id || credential.site.id != site_id {
        return Err(DeliveryError::Precondition(
            "Jira connection or selected site does not match the imported work item".into(),
        ));
    }
    Ok(JiraDeliveryCredential::ApiToken(credential))
}

enum JiraDeliveryCredential {
    Oauth(JiraCredential),
    ApiToken(JiraApiTokenCredential),
}

impl JiraDeliveryCredential {
    fn endpoint(&self, site_id: &str, segments: &[&str]) -> Result<reqwest::Url, DeliveryError> {
        match self {
            Self::Oauth(credential) => {
                if credential.selected_site_id != site_id {
                    return Err(DeliveryError::Precondition(
                        "Jira OAuth site changed after preview".into(),
                    ));
                }
                jira_endpoint(site_id, segments)
            }
            Self::ApiToken(credential) => {
                if credential.site.id != site_id {
                    return Err(DeliveryError::Precondition(
                        "Jira API-token site changed after preview".into(),
                    ));
                }
                jira_basic_endpoint(&credential.site.url, segments)
            }
        }
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Self::Oauth(credential) => request.bearer_auth(credential.access_token()),
            Self::ApiToken(credential) => {
                request.basic_auth(credential.email(), Some(credential.api_token()))
            }
        }
    }
}

async fn jira_issue_snapshot(
    credential: &JiraDeliveryCredential,
    preview: &TrackerMutationPreview,
) -> Result<TrackerSnapshot, DeliveryError> {
    let issue = jira_issue_reference(preview)?;
    let site_id = preview.site_id.as_deref().ok_or_else(|| {
        DeliveryError::Precondition("Jira delivery requires an explicit site".into())
    })?;
    let mut issue_url = credential.endpoint(site_id, &["issue", issue])?;
    issue_url
        .query_pairs_mut()
        .append_pair("fields", "status,updated");
    let issue_value = jira_get_json(credential, issue_url)
        .await
        .map_err(TrackerRemoteError::into_delivery)?;
    let fields = issue_value
        .get("fields")
        .ok_or_else(|| DeliveryError::Precondition("Jira issue fields are unavailable".into()))?;
    let status = fields
        .get("status")
        .ok_or_else(|| DeliveryError::Precondition("Jira issue status is unavailable".into()))?;
    let revision = required_json_string(fields, "updated", "Jira issue")?;
    let status_id = required_json_string(status, "id", "Jira issue status")?;
    let status_name = required_json_string(status, "name", "Jira issue status")?;
    let transition_url = credential.endpoint(site_id, &["issue", issue, "transitions"])?;
    let transition_value = jira_get_json(credential, transition_url)
        .await
        .map_err(TrackerRemoteError::into_delivery)?;
    let transitions = parse_jira_transitions(&transition_value)?;
    Ok(TrackerSnapshot {
        revision,
        status_id,
        status_name,
        transitions,
    })
}

#[derive(Debug)]
struct JiraFieldBinding {
    revision: String,
    field_name: String,
    current_value: Value,
    target_value: Value,
    target_compare_value: Value,
}

async fn jira_field_binding(
    credential: &JiraDeliveryCredential,
    preview: &TrackerMutationPreview,
    field_id: &str,
    requested: &TrackerFieldValue,
) -> Result<JiraFieldBinding, DeliveryError> {
    validate_field_id(field_id)?;
    let site_id = preview.site_id.as_deref().ok_or_else(|| {
        DeliveryError::Precondition("Jira delivery requires an explicit site".into())
    })?;
    let issue = jira_issue_reference(preview)?;
    let editmeta = jira_get_json(
        credential,
        credential.endpoint(site_id, &["issue", issue, "editmeta"])?,
    )
    .await
    .map_err(TrackerRemoteError::into_delivery)?;
    let metadata = editmeta
        .get("fields")
        .and_then(|fields| fields.get(field_id))
        .ok_or_else(|| {
            DeliveryError::Precondition(format!(
                "Jira field {field_id:?} is not live-editable for this issue and actor"
            ))
        })?;
    if !metadata
        .get("operations")
        .and_then(Value::as_array)
        .is_some_and(|operations| {
            operations
                .iter()
                .any(|operation| operation.as_str() == Some("set"))
        })
    {
        return Err(DeliveryError::Precondition(format!(
            "Jira field {field_id:?} does not authorize the set operation"
        )));
    }
    let field_name = required_json_string(metadata, "name", "Jira edit field")?;
    let target_value = jira_typed_field_json(field_id, metadata, requested)?;
    let mut issue_url = credential.endpoint(site_id, &["issue", issue])?;
    issue_url
        .query_pairs_mut()
        .append_pair("fields", &format!("updated,{field_id}"));
    let issue_value = jira_get_json(credential, issue_url)
        .await
        .map_err(TrackerRemoteError::into_delivery)?;
    let fields = issue_value
        .get("fields")
        .ok_or_else(|| DeliveryError::Precondition("Jira issue fields are unavailable".into()))?;
    let revision = required_json_string(fields, "updated", "Jira issue")?;
    let current_value = normalize_jira_field_value(
        requested,
        fields.get(field_id).cloned().unwrap_or(Value::Null),
    )?;
    let target_compare_value = normalize_jira_field_value(requested, target_value.clone())?;
    Ok(JiraFieldBinding {
        revision,
        field_name,
        current_value,
        target_value,
        target_compare_value,
    })
}

fn normalize_jira_field_value(
    requested: &TrackerFieldValue,
    provider_value: Value,
) -> Result<Value, DeliveryError> {
    match requested {
        TrackerFieldValue::Text { .. } => match provider_value {
            Value::String(value) => Ok(Value::String(value)),
            Value::Object(_) => {
                fn collect_text(value: &Value, values: &mut Vec<String>) {
                    match value {
                        Value::Object(object) => {
                            if object.get("type").and_then(Value::as_str) == Some("text") {
                                if let Some(text) = object.get("text").and_then(Value::as_str) {
                                    values.push(text.to_owned());
                                }
                            } else {
                                for child in object.values() {
                                    collect_text(child, values);
                                }
                            }
                        }
                        Value::Array(items) => {
                            for item in items {
                                collect_text(item, values);
                            }
                        }
                        _ => {}
                    }
                }
                let mut values = Vec::new();
                collect_text(&provider_value, &mut values);
                Ok(Value::String(values.join("\n")))
            }
            Value::Null => Ok(Value::Null),
            _ => Err(DeliveryError::Precondition(
                "Jira returned an incompatible text field value".into(),
            )),
        },
        TrackerFieldValue::Number { .. } => match provider_value {
            Value::Number(_) | Value::Null => Ok(provider_value),
            _ => Err(DeliveryError::Precondition(
                "Jira returned an incompatible number field value".into(),
            )),
        },
        TrackerFieldValue::Boolean { .. } => match provider_value {
            Value::Bool(_) | Value::Null => Ok(provider_value),
            _ => Err(DeliveryError::Precondition(
                "Jira returned an incompatible boolean field value".into(),
            )),
        },
        TrackerFieldValue::Option { .. } => match provider_value {
            Value::Object(value) => value
                .get("id")
                .and_then(Value::as_str)
                .map(|id| Value::String(id.to_owned()))
                .ok_or_else(|| {
                    DeliveryError::Precondition(
                        "Jira returned an option without an exact id".into(),
                    )
                }),
            Value::Null => Ok(Value::Null),
            _ => Err(DeliveryError::Precondition(
                "Jira returned an incompatible option field value".into(),
            )),
        },
        TrackerFieldValue::User { .. } => match provider_value {
            Value::Object(value) => value
                .get("accountId")
                .and_then(Value::as_str)
                .map(|id| Value::String(id.to_owned()))
                .ok_or_else(|| {
                    DeliveryError::Precondition(
                        "Jira returned a user without an exact account id".into(),
                    )
                }),
            Value::Null => Ok(Value::Null),
            _ => Err(DeliveryError::Precondition(
                "Jira returned an incompatible user field value".into(),
            )),
        },
        TrackerFieldValue::Clear => Ok(provider_value),
    }
}

fn jira_typed_field_json(
    field_id: &str,
    metadata: &Value,
    requested: &TrackerFieldValue,
) -> Result<Value, DeliveryError> {
    let required = metadata
        .get("required")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if matches!(requested, TrackerFieldValue::Clear) {
        return if required {
            Err(DeliveryError::Precondition(format!(
                "required Jira field {field_id:?} cannot be cleared"
            )))
        } else {
            Ok(Value::Null)
        };
    }
    let schema = metadata.get("schema").ok_or_else(|| {
        DeliveryError::Precondition(format!("Jira field {field_id:?} has no type schema"))
    })?;
    let schema_type = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match requested {
        TrackerFieldValue::Text { value } if schema_type == "string" => {
            let textarea = matches!(field_id, "description" | "environment")
                || schema
                    .get("custom")
                    .and_then(Value::as_str)
                    .is_some_and(|custom| custom.to_ascii_lowercase().contains("textarea"));
            if textarea {
                Ok(json!({
                    "type": "doc",
                    "version": 1,
                    "content": [{
                        "type": "paragraph",
                        "content": [{ "type": "text", "text": value }]
                    }]
                }))
            } else {
                Ok(Value::String(value.clone()))
            }
        }
        TrackerFieldValue::Number { value } if schema_type == "number" => {
            Ok(Value::Number(value.clone()))
        }
        TrackerFieldValue::Boolean { value } if schema_type == "boolean" => Ok(Value::Bool(*value)),
        TrackerFieldValue::Option { option_id } if matches!(schema_type, "option" | "priority") => {
            let exact = metadata
                .get("allowedValues")
                .and_then(Value::as_array)
                .and_then(|values| {
                    values.iter().find(|value| {
                        value.get("id").and_then(Value::as_str) == Some(option_id.as_str())
                    })
                });
            if exact.is_none() {
                return Err(DeliveryError::Precondition(format!(
                    "Jira option {option_id:?} is not a live allowed value for field {field_id:?}"
                )));
            }
            Ok(json!({ "id": option_id }))
        }
        TrackerFieldValue::User { account_id } if schema_type == "user" => {
            let exact = metadata
                .get("allowedValues")
                .and_then(Value::as_array)
                .and_then(|values| {
                    values.iter().find(|value| {
                        value.get("accountId").and_then(Value::as_str) == Some(account_id.as_str())
                    })
                });
            if exact.is_none() {
                return Err(DeliveryError::Precondition(format!(
                    "Jira user {account_id:?} is not a live allowed value for field {field_id:?}"
                )));
            }
            Ok(json!({ "accountId": account_id }))
        }
        _ => Err(DeliveryError::Precondition(format!(
            "typed value is incompatible with Jira field {field_id:?} ({schema_type})"
        ))),
    }
}

fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, DeliveryError> {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Array(values) => Value::Array(values.iter().map(sorted).collect()),
            Value::Object(values) => {
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort();
                Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), sorted(&values[key])))
                        .collect(),
                )
            }
            other => other.clone(),
        }
    }
    Ok(serde_json::to_vec(&sorted(value))?)
}

fn parse_jira_transitions(value: &Value) -> Result<Vec<TrackerTransitionChoice>, DeliveryError> {
    value
        .get("transitions")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DeliveryError::Precondition("Jira allowed transitions are unavailable".into())
        })?
        .iter()
        .map(|transition| {
            let id = required_json_string(transition, "id", "Jira transition")?;
            let name = required_json_string(transition, "name", "Jira transition")?;
            let target = transition.get("to").ok_or_else(|| {
                DeliveryError::Precondition("Jira transition has no target status".into())
            })?;
            Ok(TrackerTransitionChoice {
                id,
                name,
                target_id: required_json_string(target, "id", "Jira target status")?,
                target_name: required_json_string(target, "name", "Jira target status")?,
            })
        })
        .collect()
}

fn required_json_string(
    value: &Value,
    field: &str,
    context: &str,
) -> Result<String, DeliveryError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| DeliveryError::Precondition(format!("{context} has no {field}")))
}

async fn tracker_live_revision(
    state: &AppState,
    run: &SddRunRecord,
    preview: &TrackerMutationPreview,
) -> Result<String, DeliveryError> {
    match preview.provider.as_str() {
        "github" => {
            let cwd = integration_adapter_cwd(state, run).await?;
            Ok(github_issue_snapshot(&cwd, &preview.url).await?.revision)
        }
        "linear" => {
            let credential = linear_delivery_credential(state, &preview.connection_id).await?;
            Ok(linear_issue_snapshot(&credential, &preview.external_id)
                .await?
                .revision)
        }
        "jira" => {
            let credential = jira_delivery_credential(state, preview).await?;
            Ok(jira_issue_snapshot(&credential, preview).await?.revision)
        }
        provider => Err(DeliveryError::Precondition(format!(
            "tracker delivery provider {provider:?} is unsupported"
        ))),
    }
}

fn jira_issue_reference(preview: &TrackerMutationPreview) -> Result<&str, DeliveryError> {
    let value = preview
        .key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&preview.external_id);
    if value.trim().is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value.contains('/')
        || matches!(value, "." | "..")
    {
        return Err(DeliveryError::Invalid(
            "Jira external issue identity is unsafe".into(),
        ));
    }
    Ok(value)
}

fn jira_endpoint(site_id: &str, segments: &[&str]) -> Result<reqwest::Url, DeliveryError> {
    if site_id.trim().is_empty()
        || site_id.len() > 256
        || site_id.chars().any(char::is_control)
        || site_id.contains('/')
        || matches!(site_id, "." | "..")
    {
        return Err(DeliveryError::Invalid(
            "Jira site identity is unsafe".into(),
        ));
    }
    let mut url = reqwest::Url::parse("https://api.atlassian.com/")
        .map_err(|error| DeliveryError::Invalid(error.to_string()))?;
    url.path_segments_mut()
        .map_err(|_| DeliveryError::Invalid("Jira API URL cannot contain path segments".into()))?
        .extend(["ex", "jira", site_id, "rest", "api", "3"])
        .extend(segments.iter().copied());
    Ok(url)
}

fn jira_basic_endpoint(site_url: &str, segments: &[&str]) -> Result<reqwest::Url, DeliveryError> {
    let mut url = reqwest::Url::parse(site_url)
        .map_err(|_| DeliveryError::Invalid("Jira site URL is invalid".into()))?;
    if url.scheme() != "https"
        || !url
            .host_str()
            .is_some_and(|host| host.ends_with(".atlassian.net"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(DeliveryError::Invalid(
            "Jira API-token site URL is outside Atlassian Cloud".into(),
        ));
    }
    let mut path = url
        .path_segments_mut()
        .map_err(|_| DeliveryError::Invalid("Jira site URL cannot contain path segments".into()))?;
    path.clear()
        .extend(["rest", "api", "3"])
        .extend(segments.iter().copied());
    drop(path);
    Ok(url)
}

async fn jira_get_json(
    credential: &JiraDeliveryCredential,
    url: reqwest::Url,
) -> Result<Value, TrackerRemoteError> {
    let request = tracker_client()?
        .get(url)
        .header("Accept", "application/json");
    let response = credential
        .authorize(request)
        .send()
        .await
        .map_err(|error| TrackerRemoteError::ambiguous("Jira read failed", error.to_string()))?;
    tracker_json_response(response, "Jira read failed").await
}

async fn jira_post_json(
    credential: &JiraDeliveryCredential,
    url: reqwest::Url,
    body: &Value,
) -> Result<Value, TrackerRemoteError> {
    let request = tracker_client()?
        .post(url)
        .header("Accept", "application/json")
        .json(body);
    let response = credential
        .authorize(request)
        .send()
        .await
        .map_err(|error| {
            TrackerRemoteError::ambiguous("Jira mutation failed", error.to_string())
        })?;
    tracker_json_response(response, "Jira mutation failed").await
}

async fn jira_put_json(
    credential: &JiraDeliveryCredential,
    url: reqwest::Url,
    body: &Value,
) -> Result<Value, TrackerRemoteError> {
    let request = tracker_client()?
        .put(url)
        .header("Accept", "application/json")
        .json(body);
    let response = credential
        .authorize(request)
        .send()
        .await
        .map_err(|error| {
            TrackerRemoteError::ambiguous("Jira mutation failed", error.to_string())
        })?;
    tracker_json_response(response, "Jira mutation failed").await
}

async fn linear_graphql(
    token: &str,
    query: &str,
    variables: Value,
) -> Result<Value, TrackerRemoteError> {
    let response = tracker_client()?
        .post(LINEAR_GRAPHQL)
        .header("Authorization", token)
        .header("Accept", "application/json")
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .await
        .map_err(|error| {
            TrackerRemoteError::ambiguous("Linear request failed", error.to_string())
        })?;
    let value = tracker_json_response(response, "Linear request failed").await?;
    if value
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|errors| !errors.is_empty())
    {
        return Err(TrackerRemoteError::definite(
            "Linear GraphQL request was rejected",
            value.to_string(),
        ));
    }
    Ok(value)
}

fn tracker_client() -> Result<reqwest::Client, TrackerRemoteError> {
    reqwest::Client::builder()
        .timeout(TRACKER_TIMEOUT)
        .build()
        .map_err(|error| TrackerRemoteError::definite("tracker client failed", error.to_string()))
}

async fn tracker_json_response(
    response: reqwest::Response,
    summary: &'static str,
) -> Result<Value, TrackerRemoteError> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_TRACKER_RESPONSE as u64)
    {
        return Err(TrackerRemoteError::definite(
            "tracker response exceeded its bound",
            status.to_string(),
        ));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            TrackerRemoteError::ambiguous("tracker response read failed", error.to_string())
        })?;
        if bytes.len().saturating_add(chunk.len()) > MAX_TRACKER_RESPONSE {
            return Err(TrackerRemoteError::definite(
                "tracker response exceeded its bound",
                status.to_string(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let detail = format!("{status}:{}", sha256(&bytes));
        return Err(
            if status.is_server_error()
                || status == reqwest::StatusCode::REQUEST_TIMEOUT
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            {
                TrackerRemoteError::ambiguous(summary, detail)
            } else {
                TrackerRemoteError::definite(summary, detail)
            },
        );
    }
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        TrackerRemoteError::definite("tracker response was malformed", error.to_string())
    })
}

#[derive(Debug)]
struct TrackerRemoteError {
    summary: &'static str,
    detail_hash: String,
    ambiguous: bool,
}

impl TrackerRemoteError {
    fn definite(summary: &'static str, detail: impl AsRef<[u8]>) -> Self {
        Self {
            summary,
            detail_hash: sha256(detail),
            ambiguous: false,
        }
    }

    fn ambiguous(summary: &'static str, detail: impl AsRef<[u8]>) -> Self {
        Self {
            summary,
            detail_hash: sha256(detail),
            ambiguous: true,
        }
    }

    fn into_delivery(self) -> DeliveryError {
        DeliveryError::Command(format!("{} ({})", self.summary, self.detail_hash))
    }

    fn into_action_failure(self) -> ActionFailure {
        let value = json!({ "summary": self.summary, "errorHash": self.detail_hash });
        if self.ambiguous {
            ActionFailure::Ambiguous(value)
        } else {
            ActionFailure::Definite(value)
        }
    }
}

pub(crate) async fn bind_openspec_exports(
    state: &AppState,
    run: &SddRunRecord,
    actions: &mut [PreparedDeliveryAction],
) -> Result<(), DeliveryError> {
    if let Some(action) = actions
        .iter_mut()
        .find(|action| action.kind == "openspec_export")
    {
        let preview = current_openspec_export(state, run).await?;
        if state.store.sdd_remote_run(&run.run_id).await?.is_none() {
            ensure_export_destination_absent(Path::new(&run.authoritative_path), &preview)?;
        }
        action.openspec_export = Some(preview);
    }
    Ok(())
}

pub(crate) async fn validate_openspec_exports(
    state: &AppState,
    run: &SddRunRecord,
    actions: &[PreparedDeliveryAction],
) -> Result<(), DeliveryError> {
    let Some(action) = actions
        .iter()
        .find(|action| action.kind == "openspec_export")
    else {
        return Ok(());
    };
    let expected = action.openspec_export.as_ref().ok_or_else(|| {
        DeliveryError::Invalid("OpenSpec export preview payload is missing".into())
    })?;
    let remote = state.store.sdd_remote_run(&run.run_id).await?.is_some();
    let destination_exists = if remote {
        // The host-side typed inspection performed by preview/confirm owns
        // destination existence for a remote worktree.
        false
    } else {
        export_destination_exists(Path::new(&run.authoritative_path), expected)?
    };
    let current = current_openspec_export(state, run).await?;
    let source_changed = &current != expected;
    if source_changed && destination_exists {
        return Err(DeliveryError::Precondition(
            "OpenSpec source and Agentum artifacts both changed after the export baseline; refusing to overwrite"
                .into(),
        ));
    }
    if destination_exists {
        return Err(DeliveryError::Precondition(
            "OpenSpec export destination already exists; one-shot export never overwrites".into(),
        ));
    }
    if source_changed {
        return Err(DeliveryError::Precondition(
            "Agentum artifacts changed after the OpenSpec export preview".into(),
        ));
    }
    Ok(())
}

async fn current_openspec_export(
    state: &AppState,
    run: &SddRunRecord,
) -> Result<OpenSpecExportPreview, DeliveryError> {
    let artifacts = state.store.sdd_artifacts(&run.run_id).await?;
    if state.store.sdd_remote_run(&run.run_id).await?.is_some() {
        return current_remote_openspec_export(state, run, &artifacts).await;
    }
    let specification = artifacts
        .iter()
        .find(|artifact| artifact.kind == "specification")
        .ok_or_else(|| DeliveryError::Invalid("OpenSpec export requires spec.md".into()))?;
    let (spec_directory_name, spec_file_name) =
        export_artifact_location(&specification.relative_path)?;
    if spec_file_name != "spec.md" {
        return Err(DeliveryError::Invalid(
            "OpenSpec export specification path is invalid".into(),
        ));
    }
    let worktree = AnchoredDirectory::open(Path::new(&run.authoritative_path))?;
    let artifact_directory = worktree
        .open_child(".agentum")?
        .open_child("specs")?
        .open_child(spec_directory_name)?;
    let read = |kind: &str| -> Result<Option<String>, DeliveryError> {
        let Some(artifact) = artifacts.iter().find(|artifact| artifact.kind == kind) else {
            return Ok(None);
        };
        let (directory_name, file_name) = export_artifact_location(&artifact.relative_path)?;
        let expected_file_name = match kind {
            "specification" => "spec.md",
            "design" => "design.md",
            "plan" => "plan.json",
            _ => {
                return Err(DeliveryError::Invalid(format!(
                    "unsupported OpenSpec source artifact: {kind}"
                )));
            }
        };
        if directory_name != spec_directory_name || file_name != expected_file_name {
            return Err(DeliveryError::Invalid(format!(
                "{kind} has an invalid OpenSpec export path"
            )));
        }
        let (bytes, hash) = artifact_directory.read_file(file_name)?;
        if hash != artifact.content_hash {
            return Err(DeliveryError::Invalid(format!(
                "{kind} changed outside Agentum before OpenSpec export"
            )));
        }
        let content = String::from_utf8(bytes).map_err(|_| {
            DeliveryError::Invalid(format!("{kind} is not UTF-8 before OpenSpec export"))
        })?;
        Ok(Some(content))
    };
    let spec = read("specification")?
        .ok_or_else(|| DeliveryError::Invalid("OpenSpec export requires spec.md".into()))?;
    let design = read("design")?;
    let plan = read("plan")?;
    let rebound_directory = worktree
        .open_child(".agentum")?
        .open_child("specs")?
        .open_child(spec_directory_name)?;
    if !artifact_directory.same_identity(&rebound_directory)? {
        return Err(DeliveryError::Precondition(
            "Agentum artifact directory was replaced during OpenSpec export preview".into(),
        ));
    }
    Ok(super::sources::preview_openspec_export(
        &spec,
        design.as_deref(),
        plan.as_deref(),
    )?)
}

async fn current_remote_openspec_export(
    state: &AppState,
    run: &SddRunRecord,
    artifacts: &[agentum_store::sdd::SddArtifactRecord],
) -> Result<OpenSpecExportPreview, DeliveryError> {
    let payloads = state
        .store
        .sdd_remote_artifact_payloads(&run.run_id)
        .await?
        .into_iter()
        .map(|payload| (payload.artifact_revision_id.clone(), payload))
        .collect::<HashMap<_, _>>();
    let specification = artifacts
        .iter()
        .find(|artifact| artifact.kind == "specification")
        .ok_or_else(|| DeliveryError::Invalid("OpenSpec export requires spec.md".into()))?;
    let (spec_directory_name, spec_file_name) =
        export_artifact_location(&specification.relative_path)?;
    if spec_file_name != "spec.md" {
        return Err(DeliveryError::Invalid(
            "OpenSpec export specification path is invalid".into(),
        ));
    }
    let read = |kind: &str| -> Result<Option<String>, DeliveryError> {
        let Some(artifact) = artifacts.iter().find(|artifact| artifact.kind == kind) else {
            return Ok(None);
        };
        let (directory_name, file_name) = export_artifact_location(&artifact.relative_path)?;
        let expected_file_name = match kind {
            "specification" => "spec.md",
            "design" => "design.md",
            "plan" => "plan.json",
            _ => {
                return Err(DeliveryError::Invalid(format!(
                    "unsupported OpenSpec source artifact: {kind}"
                )));
            }
        };
        if directory_name != spec_directory_name || file_name != expected_file_name {
            return Err(DeliveryError::Invalid(format!(
                "{kind} has an invalid OpenSpec export path"
            )));
        }
        let payload = payloads
            .get(&artifact.artifact_revision_id)
            .ok_or_else(|| DeliveryError::Invalid(format!("remote {kind} payload is missing")))?;
        let actual = sha256(payload.content.as_bytes());
        if actual != payload.content_sha256 || actual != artifact.content_hash {
            return Err(DeliveryError::Precondition(format!(
                "remote {kind} payload changed after projection"
            )));
        }
        Ok(Some(payload.content.clone()))
    };
    let spec = read("specification")?
        .ok_or_else(|| DeliveryError::Invalid("OpenSpec export requires spec.md".into()))?;
    let design = read("design")?;
    let plan = read("plan")?;
    Ok(super::sources::preview_openspec_export(
        &spec,
        design.as_deref(),
        plan.as_deref(),
    )?)
}

fn export_artifact_location(relative_path: &str) -> Result<(&str, &str), DeliveryError> {
    agentum_core::sdd::validate_relative_path(relative_path)
        .map_err(|error| DeliveryError::Invalid(error.to_string()))?;
    let parts = relative_path.split('/').collect::<Vec<_>>();
    if parts.len() != 4
        || parts[0] != ".agentum"
        || parts[1] != "specs"
        || !parts[2].starts_with("spc-")
        || parts[2].len() > 80
        || !parts[2]
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(DeliveryError::Invalid(
            "OpenSpec source artifact path is outside the canonical artifact directory".into(),
        ));
    }
    Ok((parts[2], parts[3]))
}

fn ensure_export_destination_absent(
    worktree: &Path,
    preview: &OpenSpecExportPreview,
) -> Result<(), DeliveryError> {
    export_destination_name(preview)?;
    if export_destination_exists(worktree, preview)? {
        return Err(DeliveryError::Conflict(
            "OpenSpec export destination already exists; one-shot export never overwrites".into(),
        ));
    }
    Ok(())
}

fn export_destination_name(preview: &OpenSpecExportPreview) -> Result<&str, DeliveryError> {
    export_destination_name_from_path(&preview.destination)
}

fn export_destination_name_from_path(destination: &str) -> Result<&str, DeliveryError> {
    agentum_core::sdd::validate_relative_path(destination)
        .map_err(|error| DeliveryError::Invalid(error.to_string()))?;
    let parts = destination.split('/').collect::<Vec<_>>();
    let valid_name = parts.get(2).is_some_and(|name| {
        name.starts_with("agentum-")
            && name.len() <= 128
            && name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    });
    if parts.len() != 3 || parts[0] != "openspec" || parts[1] != "changes" || !valid_name {
        return Err(DeliveryError::Invalid(
            "OpenSpec export destination is outside the Agentum one-shot namespace".into(),
        ));
    }
    Ok(parts[2])
}

pub(crate) fn openspec_destination_exists(
    worktree: &Path,
    destination: &str,
) -> Result<bool, DeliveryError> {
    let destination_name = export_destination_name_from_path(destination)?;
    let worktree = AnchoredDirectory::open(worktree)?;
    let Some(openspec) = worktree.open_child_optional("openspec")? else {
        return Ok(false);
    };
    let Some(changes) = openspec.open_child_optional("changes")? else {
        return Ok(false);
    };
    Ok(changes.open_child_optional(destination_name)?.is_some())
}

fn open_export_destination(
    worktree: &AnchoredDirectory,
    preview: &OpenSpecExportPreview,
) -> Result<Option<AnchoredDirectory>, DeliveryError> {
    let destination_name = export_destination_name(preview)?;
    let Some(openspec) = worktree.open_child_optional("openspec")? else {
        return Ok(None);
    };
    let Some(changes) = openspec.open_child_optional("changes")? else {
        return Ok(None);
    };
    Ok(changes.open_child_optional(destination_name)?)
}

fn export_destination_exists(
    worktree: &Path,
    preview: &OpenSpecExportPreview,
) -> Result<bool, DeliveryError> {
    let worktree = AnchoredDirectory::open(worktree)?;
    Ok(open_export_destination(&worktree, preview)?.is_some())
}

fn validate_text(label: &str, value: &str, max: usize) -> Result<(), DeliveryError> {
    if value.trim().is_empty() || value.len() > max || value.contains('\0') {
        return Err(DeliveryError::Invalid(format!(
            "{label} must be non-empty, NUL-free, and at most {max} bytes"
        )));
    }
    Ok(())
}

fn validate_ref_component(label: &str, value: &str) -> Result<(), DeliveryError> {
    let valid = !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && !value.contains("..")
        && !value.ends_with('.')
        && !value.ends_with('/')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'));
    if valid {
        Ok(())
    } else {
        Err(DeliveryError::Invalid(format!("{label} is unsafe")))
    }
}

pub(crate) async fn workspace_state_hash(worktree: &Path) -> Result<String, DeliveryError> {
    let worktree = worktree.canonicalize()?;
    if !worktree.is_dir() {
        return Err(DeliveryError::Invalid(
            "authoritative worktree is not a directory".into(),
        ));
    }
    let head = run_checked(
        &worktree,
        "git",
        &["rev-parse".into(), "HEAD".into()],
        &[],
        Duration::from_secs(15),
        128 * 1024,
    )
    .await?;
    let diff = run_checked(
        &worktree,
        "git",
        &[
            "diff".into(),
            "--binary".into(),
            "--full-index".into(),
            "--no-ext-diff".into(),
            "HEAD".into(),
            "--".into(),
        ],
        &[],
        Duration::from_secs(30),
        MAX_WORKSPACE_BYTES,
    )
    .await?;
    let untracked = run_checked(
        &worktree,
        "git",
        &[
            "ls-files".into(),
            "--others".into(),
            "--exclude-standard".into(),
            "-z".into(),
        ],
        &[],
        Duration::from_secs(30),
        MAX_WORKSPACE_BYTES,
    )
    .await?;
    let mut files = Vec::new();
    let mut total = diff.stdout.len().saturating_add(untracked.stdout.len());
    for raw in untracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
    {
        let relative = std::str::from_utf8(raw)
            .map_err(|_| DeliveryError::Invalid("untracked path is not UTF-8".into()))?;
        agentum_core::sdd::validate_relative_path(relative)
            .map_err(|error| DeliveryError::Invalid(error.to_string()))?;
        let path = worktree.join(relative);
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(DeliveryError::Invalid(format!(
                "unsupported untracked entry: {relative}"
            )));
        }
        let (bytes, hash) = read_bytes(&path)?;
        total = total.saturating_add(bytes.len());
        if total > MAX_WORKSPACE_BYTES {
            return Err(DeliveryError::Invalid(format!(
                "workspace delivery snapshot exceeds {} bytes",
                MAX_WORKSPACE_BYTES
            )));
        }
        files.push(json!({"path": relative, "hash": hash, "size": bytes.len()}));
    }
    files.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    Ok(sha256(serde_json::to_vec(&json!({
        "head": String::from_utf8_lossy(&head.stdout).trim(),
        "diffHash": sha256(&diff.stdout),
        "untracked": files
    }))?))
}

pub(crate) fn preview_digest(envelope: &DeliveryPreviewEnvelope) -> Result<String, DeliveryError> {
    Ok(sha256(serde_json::to_vec(envelope)?))
}

pub(crate) fn preview_token(preview_id: &str, digest: &str) -> String {
    format!("{preview_id}.{digest}")
}

pub(crate) fn validate_preview_token(
    preview: &SddDeliveryPreviewRecord,
    token: &str,
) -> Result<(), DeliveryError> {
    if token != preview_token(&preview.preview_id, &preview.digest)
        || sha256(token) != preview.token_hash
    {
        return Err(DeliveryError::Invalid(
            "delivery preview token is invalid".into(),
        ));
    }
    Ok(())
}

fn active_workers() -> &'static Mutex<HashSet<String>> {
    static ACTIVE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
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

pub(crate) fn spawn(state: AppState, preview_id: String) -> bool {
    let inserted = active_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(preview_id.clone());
    if !inserted {
        return false;
    }
    tokio::spawn(async move {
        let _guard = WorkerGuard(preview_id.clone());
        if let Err(error) = drive(&state, &preview_id).await {
            tracing::error!(preview_id, %error, "SDD delivery worker stopped");
        }
    });
    true
}

async fn drive(state: &AppState, preview_id: &str) -> Result<(), DeliveryError> {
    let preview = state
        .store
        .sdd_delivery_preview(preview_id)
        .await?
        .ok_or_else(|| DeliveryError::Invalid("delivery preview disappeared".into()))?;
    if preview.status != "confirmed" {
        return Ok(());
    }
    let envelope: DeliveryPreviewEnvelope = serde_json::from_str(&preview.actions_json)?;
    let run = state
        .store
        .sdd_get_run(&preview.run_id)
        .await?
        .ok_or_else(|| DeliveryError::Invalid("delivery run disappeared".into()))?;
    loop {
        let actions = state.store.sdd_delivery_actions(preview_id).await?;
        let mut progressed = false;
        for action in actions.iter().filter(|action| action.status == "pending") {
            let Some(claimed) = state
                .store
                .sdd_claim_delivery_action(preview_id, &action.action_id)
                .await?
            else {
                continue;
            };
            progressed = true;
            let outcome = execute_action(state, &run, &envelope, &claimed).await;
            let (status, result) = match outcome {
                Ok(result) => ("succeeded", result),
                Err(ActionFailure::Definite(result)) => ("failed", result),
                Err(ActionFailure::Ambiguous(result)) => ("sync_pending", result),
            };
            state
                .store
                .sdd_record_delivery_action_result(DeliveryActionResult {
                    preview_id,
                    action_id: &claimed.action_id,
                    status,
                    result_json: &result.to_string(),
                })
                .await?;
        }
        if !progressed {
            return Ok(());
        }
    }
}

#[derive(Debug)]
enum ActionFailure {
    Definite(Value),
    Ambiguous(Value),
}

/// Typed result returned by the host-side delivery executor. The desktop
/// coordinator persists this result without having to interpret command
/// output, and a retry can reconcile the same stable action identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "result", rename_all = "snake_case")]
pub(crate) enum RepositoryDeliveryOutcome {
    Succeeded(Value),
    Failed(Value),
    SyncPending(Value),
}

pub(crate) fn is_repository_delivery_action(action: &PreparedDeliveryAction) -> bool {
    matches!(
        action.intent,
        DeliveryActionRequest::Commit { .. }
            | DeliveryActionRequest::Push { .. }
            | DeliveryActionRequest::PullRequest { .. }
            | DeliveryActionRequest::Release { .. }
            | DeliveryActionRequest::OpenSpecExport
    )
}

/// Execute one repository-owned action in the supplied authoritative
/// worktree. Remote workers call this with their registered worktree; desktop
/// callers must never substitute a local path for a remote run.
pub(crate) async fn execute_repository_delivery_action(
    worktree: &Path,
    branch_name: &str,
    envelope: &DeliveryPreviewEnvelope,
    prepared: &PreparedDeliveryAction,
) -> RepositoryDeliveryOutcome {
    let record = SddDeliveryActionRecord {
        preview_id: String::new(),
        action_id: prepared.id.clone(),
        action_type: prepared.kind.clone(),
        intent_json: String::new(),
        status: "running".into(),
        result_json: None,
        attempts: 1,
        updated_at: String::new(),
    };
    let outcome = match &prepared.intent {
        DeliveryActionRequest::Commit { message } => {
            execute_commit(worktree, envelope, &record, message).await
        }
        DeliveryActionRequest::Push { remote } => {
            execute_push(worktree, branch_name, &record, remote).await
        }
        DeliveryActionRequest::PullRequest { title, body, base } => {
            execute_pull_request(worktree, branch_name, &record, title, body, base).await
        }
        DeliveryActionRequest::Release {
            tag,
            name,
            notes,
            prerelease,
        } => {
            execute_release(
                worktree,
                branch_name,
                &record,
                tag,
                name,
                notes,
                *prerelease,
            )
            .await
        }
        DeliveryActionRequest::OpenSpecExport => prepared
            .openspec_export
            .as_ref()
            .ok_or_else(|| definite("OpenSpec export action is missing its hash-bound preview"))
            .and_then(|export| publish_openspec_export(worktree, export)),
        DeliveryActionRequest::TrackerComment { .. }
        | DeliveryActionRequest::TrackerStatus { .. }
        | DeliveryActionRequest::TrackerFieldUpdate { .. } => {
            return RepositoryDeliveryOutcome::Failed(json!({
                "summary": "tracker actions are not repository delivery actions"
            }));
        }
    };
    match outcome {
        Ok(result) => RepositoryDeliveryOutcome::Succeeded(result),
        Err(ActionFailure::Definite(result)) => RepositoryDeliveryOutcome::Failed(result),
        Err(ActionFailure::Ambiguous(result)) => RepositoryDeliveryOutcome::SyncPending(result),
    }
}

async fn execute_action(
    state: &AppState,
    run: &SddRunRecord,
    envelope: &DeliveryPreviewEnvelope,
    action: &SddDeliveryActionRecord,
) -> Result<Value, ActionFailure> {
    let prepared: PreparedDeliveryAction = serde_json::from_str(&action.intent_json).map_err(|e| {
        ActionFailure::Definite(json!({"summary": "stored action is malformed", "errorHash": sha256(e.to_string())}))
    })?;
    let remote = state
        .store
        .sdd_remote_run(&run.run_id)
        .await
        .map_err(|error| definite_hashed("remote delivery lookup failed", &error.to_string()))?;
    if let Some(remote) = remote.as_ref() {
        if is_repository_delivery_action(&prepared) {
            return execute_remote_repository_action(
                state, run, envelope, action, &prepared, remote,
            )
            .await;
        }
    }
    let tracker_mutation = prepared.tracker_mutation.clone();
    let adapter_cwd;
    let worktree = if remote.is_some() {
        // Tracker APIs remain desktop integrations, but their executable cwd
        // is an Agentum-owned neutral directory. A remote authoritative URI is
        // never interpreted as a local path and no local repository fallback
        // exists for repository delivery actions.
        adapter_cwd = delivery_adapter_cwd()?;
        adapter_cwd.as_path()
    } else {
        Path::new(&run.authoritative_path)
    };
    match prepared.intent {
        DeliveryActionRequest::Commit { message } => {
            execute_commit(worktree, envelope, action, &message).await
        }
        DeliveryActionRequest::Push { remote } => {
            execute_push(worktree, &run.branch_name, action, &remote).await
        }
        DeliveryActionRequest::PullRequest { title, body, base } => {
            execute_pull_request(worktree, &run.branch_name, action, &title, &body, &base).await
        }
        DeliveryActionRequest::TrackerComment { body } => {
            let binding = tracker_mutation
                .as_ref()
                .ok_or_else(|| definite("tracker action is missing its hash-bound preview"))?;
            execute_tracker_comment_provider(state, worktree, action, binding, &body).await
        }
        DeliveryActionRequest::TrackerStatus { .. } => {
            let binding = tracker_mutation
                .as_ref()
                .ok_or_else(|| definite("tracker action is missing its hash-bound preview"))?;
            execute_tracker_status_provider(state, worktree, action, binding).await
        }
        DeliveryActionRequest::TrackerFieldUpdate { .. } => {
            let binding = tracker_mutation
                .as_ref()
                .ok_or_else(|| definite("tracker action is missing its hash-bound preview"))?;
            execute_tracker_field_update(state, action, binding).await
        }
        DeliveryActionRequest::Release {
            tag,
            name,
            notes,
            prerelease,
        } => {
            execute_release(
                worktree,
                &run.branch_name,
                action,
                &tag,
                &name,
                &notes,
                prerelease,
            )
            .await
        }
        DeliveryActionRequest::OpenSpecExport => {
            let export = prepared.openspec_export.as_ref().ok_or_else(|| {
                definite("OpenSpec export action is missing its hash-bound preview")
            })?;
            let current = current_openspec_export(state, run).await.map_err(|error| {
                definite_hashed("OpenSpec export validation failed", &error.to_string())
            })?;
            if &current != export {
                return Err(definite(
                    "Agentum artifacts changed after the OpenSpec export preview",
                ));
            }
            publish_openspec_export(worktree, export)
        }
    }
}

fn delivery_adapter_cwd() -> Result<PathBuf, ActionFailure> {
    let path = agentum_store::paths::data_dir()
        .map_err(|error| definite_hashed("delivery adapter path unavailable", &error.to_string()))?
        .join("delivery-adapters");
    std::fs::create_dir_all(&path).map_err(|error| {
        definite_hashed("delivery adapter directory unavailable", &error.to_string())
    })?;
    Ok(path)
}

async fn integration_adapter_cwd(
    state: &AppState,
    run: &SddRunRecord,
) -> Result<PathBuf, DeliveryError> {
    if state.store.sdd_remote_run(&run.run_id).await?.is_none() {
        return Ok(PathBuf::from(&run.authoritative_path));
    }
    let path = agentum_store::paths::data_dir()
        .map_err(|error| DeliveryError::Precondition(error.to_string()))?
        .join("delivery-adapters");
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

async fn execute_remote_repository_action(
    state: &AppState,
    run: &SddRunRecord,
    envelope: &DeliveryPreviewEnvelope,
    action: &SddDeliveryActionRecord,
    prepared: &PreparedDeliveryAction,
    projection: &agentum_store::sdd_remote_projection::SddRemoteRunRecord,
) -> Result<Value, ActionFailure> {
    let plan: RemoteLifecyclePlan =
        serde_json::from_str(&projection.plan_json).map_err(|error| {
            definite_hashed("remote delivery plan is malformed", &error.to_string())
        })?;
    if plan.run_id != run.run_id
        || plan.spec_id != run.spec_id
        || plan.spec_revision != envelope.spec_revision
        || plan.base_commit != run.base_commit
        || plan.host_id != projection.host_id
        || plan.repository_identity_sha256 != projection.repository_identity_sha256
        || plan.artifact_set_id != projection.artifact_set_id
        || envelope.actor_id.trim().is_empty()
    {
        return Err(definite(
            "remote delivery does not match the approved lifecycle plan",
        ));
    }
    let host_id = uuid::Uuid::parse_str(&projection.host_id)
        .map_err(|error| definite_hashed("remote delivery host is invalid", &error.to_string()))?;
    let host = state
        .store
        .get_host(host_id)
        .await
        .map_err(|error| definite_hashed("remote delivery host lookup failed", &error.to_string()))?
        .ok_or_else(|| definite("remote delivery host is unavailable"))?;
    let client = super::remote_lifecycle::client_for_host(host).map_err(|error| {
        definite_hashed(
            "remote delivery transport is unavailable",
            &error.to_string(),
        )
    })?;
    let preview_digest = preview_digest(envelope)
        .map_err(|error| definite_hashed("remote delivery digest failed", &error.to_string()))?;
    let material = serde_json::to_vec(&json!({
        "runId": run.run_id,
        "previewDigest": preview_digest,
        "actionId": action.action_id,
        "attempt": action.attempts,
    }))
    .map_err(|error| definite_hashed("remote delivery identity failed", &error.to_string()))?;
    let request = RemoteDeliveryActionRequest {
        schema_version: REMOTE_SDD_SCHEMA_VERSION,
        request_id: format!("delivery-action-{}", &sha256(material)[..32]),
        host_id: plan.host_id,
        run_id: plan.run_id,
        spec_id: plan.spec_id,
        spec_revision: plan.spec_revision,
        repository_identity_sha256: plan.repository_identity_sha256,
        artifact_set_id: plan.artifact_set_id,
        base_commit: plan.base_commit,
        approval_digest: plan.approval_digest,
        preview_digest,
        envelope: envelope.clone(),
        action: prepared.clone(),
        attempt: action.attempts,
        timeout_ms: plan.timeout_ms,
        output_limit: plan.output_limit,
    };
    let validation_request = request.clone();
    let result = RemoteSddTransport::execute_delivery_action(client.as_ref(), request)
        .await
        .map_err(|error| {
            ActionFailure::Ambiguous(json!({
                "summary": "remote delivery outcome could not be confirmed",
                "errorHash": sha256(error.to_string()),
                "localFallback": false
            }))
        })?;
    super::remote::validate_delivery_action_result(&validation_request, &result).map_err(
        |error| {
            ActionFailure::Ambiguous(json!({
                "summary": "remote delivery result failed validation",
                "errorHash": sha256(error.to_string()),
                "localFallback": false
            }))
        },
    )?;
    match result.status {
        RemoteDeliveryActionStatus::Succeeded => Ok(result.result),
        RemoteDeliveryActionStatus::Failed => Err(ActionFailure::Definite(result.result)),
        RemoteDeliveryActionStatus::SyncPending => Err(ActionFailure::Ambiguous(result.result)),
    }
}

fn publish_openspec_export(
    worktree: &Path,
    export: &OpenSpecExportPreview,
) -> Result<Value, ActionFailure> {
    publish_openspec_export_with_hook(worktree, export, || {})
}

fn publish_openspec_export_with_hook<F>(
    worktree: &Path,
    export: &OpenSpecExportPreview,
    after_parent_open: F,
) -> Result<Value, ActionFailure>
where
    F: FnOnce(),
{
    validate_export_preview(export).map_err(|error| {
        definite_hashed("OpenSpec export preview is invalid", &error.to_string())
    })?;
    let worktree = AnchoredDirectory::open(worktree)
        .map_err(|error| definite_hashed("OpenSpec worktree is unsafe", &error.to_string()))?;
    if export_destination_matches_in(&worktree, export).map_err(|error| {
        definite_hashed("OpenSpec destination inspection failed", &error.to_string())
    })? {
        return Ok(json!({
            "summary": "OpenSpec export already matches",
            "destination": export.destination,
            "sourceRevision": export.source_revision
        }));
    }
    if open_export_destination(&worktree, export)
        .map_err(|error| definite_hashed("OpenSpec export refused", &error.to_string()))?
        .is_some()
    {
        return Err(definite(
            "OpenSpec export destination already exists; one-shot export never overwrites",
        ));
    }
    let destination_name = export_destination_name(export)
        .map_err(|error| definite_hashed("OpenSpec export refused", &error.to_string()))?;
    let (openspec, _) = worktree
        .ensure_child("openspec")
        .map_err(|error| definite_hashed("OpenSpec export parent is unsafe", &error.to_string()))?;
    let (changes, _) = openspec
        .ensure_child("changes")
        .map_err(|error| definite_hashed("OpenSpec export parent is unsafe", &error.to_string()))?;
    if changes
        .open_child_optional(destination_name)
        .map_err(|error| definite_hashed("OpenSpec export refused", &error.to_string()))?
        .is_some()
    {
        return Err(definite(
            "OpenSpec export destination already exists; one-shot export never overwrites",
        ));
    }

    after_parent_open();
    let staging_name = format!(".agentum-export-{}", uuid::Uuid::new_v4());
    let mut staging = Some(
        changes
            .create_child_exclusive(&staging_name)
            .map_err(|error| {
                definite_hashed("OpenSpec staging creation failed", &error.to_string())
            })?,
    );
    let publication = (|| -> Result<(), DeliveryError> {
        {
            let staging = staging
                .as_ref()
                .expect("staging handle is present until publication");
            for file in &export.files {
                write_export_file(staging, &file.relative_path, file.content.as_bytes())?;
            }
            staging.sync()?;
        }
        // On Windows, the no-follow directory handle intentionally denies
        // delete sharing. Close it before reopening the same child with DELETE
        // access for SetFileInformationByHandle publication.
        drop(staging.take());

        // Re-resolve the publication parent beneath the still-held worktree
        // handle. A renamed parent or replacement symlink/junction blocks the
        // commit, while all staged bytes remain confined to the original
        // descriptor and are removed below.
        let rebound_changes = worktree.open_child("openspec")?.open_child("changes")?;
        if !changes.same_identity(&rebound_changes)? {
            return Err(DeliveryError::Precondition(
                "OpenSpec export parent was replaced during publication".into(),
            ));
        }
        changes.publish_child_directory_noreplace(&staging_name, destination_name)?;
        Ok(())
    })();
    if let Err(error) = publication {
        drop(staging.take());
        let _ = changes.remove_child_tree(&staging_name);
        return Err(definite_hashed(
            "OpenSpec export publication failed",
            &error.to_string(),
        ));
    }
    Ok(json!({
        "summary": "OpenSpec export published",
        "destination": export.destination,
        "sourceRevision": export.source_revision,
        "files": export.files.iter().map(|file| json!({
            "path": file.relative_path,
            "contentHash": file.content_hash
        })).collect::<Vec<_>>()
    }))
}

fn validate_export_preview(export: &OpenSpecExportPreview) -> Result<(), DeliveryError> {
    export_destination_name(export)?;
    if export.files.is_empty() || export.files.len() > 256 {
        return Err(DeliveryError::Invalid(
            "OpenSpec export must contain 1..=256 files".into(),
        ));
    }
    let mut paths = std::collections::HashSet::new();
    for file in &export.files {
        export_file_parts(&file.relative_path)?;
        if !paths.insert(file.relative_path.as_str()) {
            return Err(DeliveryError::Invalid(format!(
                "OpenSpec export repeats {}",
                file.relative_path
            )));
        }
        if sha256(file.content.as_bytes()) != file.content_hash {
            return Err(DeliveryError::Invalid(format!(
                "OpenSpec preview hash mismatch for {}",
                file.relative_path
            )));
        }
    }
    Ok(())
}

fn export_file_parts(relative_path: &str) -> Result<Vec<&str>, DeliveryError> {
    agentum_core::sdd::validate_relative_path(relative_path)
        .map_err(|error| DeliveryError::Invalid(error.to_string()))?;
    let parts = relative_path.split('/').collect::<Vec<_>>();
    if parts.is_empty() || parts.len() > 16 || parts.iter().any(|part| part.is_empty()) {
        return Err(DeliveryError::Invalid(format!(
            "OpenSpec export path is unsafe: {relative_path}"
        )));
    }
    Ok(parts)
}

fn write_export_file(
    staging: &AnchoredDirectory,
    relative_path: &str,
    content: &[u8],
) -> Result<(), DeliveryError> {
    let parts = export_file_parts(relative_path)?;
    let mut parent = staging.try_clone()?;
    for component in &parts[..parts.len() - 1] {
        parent = parent.ensure_child(component)?.0;
    }
    parent.atomic_write_missing(parts[parts.len() - 1], content)?;
    Ok(())
}

#[cfg(test)]
fn export_destination_matches(
    worktree: &Path,
    export: &OpenSpecExportPreview,
) -> Result<bool, DeliveryError> {
    let worktree = AnchoredDirectory::open(worktree)?;
    export_destination_matches_in(&worktree, export)
}

fn export_destination_matches_in(
    worktree: &AnchoredDirectory,
    export: &OpenSpecExportPreview,
) -> Result<bool, DeliveryError> {
    let Some(destination) = open_export_destination(worktree, export)? else {
        return Ok(false);
    };
    let expected = export
        .files
        .iter()
        .map(|file| (file.relative_path.as_str(), file.content_hash.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut actual = std::collections::BTreeMap::new();
    let mut pending = vec![(destination, String::new(), 0_usize)];
    let mut directories = 0usize;
    while let Some((directory, parent_relative, depth)) = pending.pop() {
        directories += 1;
        if depth > 16 || actual.len() > 256 || directories > 512 {
            return Err(DeliveryError::Invalid(
                "OpenSpec export destination exceeds safe limits".into(),
            ));
        }
        let mut entries = directory.entries()?;
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        for entry in entries {
            let relative = if parent_relative.is_empty() {
                entry.name.clone()
            } else {
                format!("{parent_relative}/{}", entry.name)
            };
            match entry.kind {
                AnchoredEntryKind::Directory => {
                    pending.push((directory.open_child(&entry.name)?, relative, depth + 1))
                }
                AnchoredEntryKind::File => {
                    let (_, hash) = directory.read_file(&entry.name)?;
                    actual.insert(relative, hash);
                    if actual.len() > 256 {
                        return Err(DeliveryError::Invalid(
                            "OpenSpec export destination exceeds safe limits".into(),
                        ));
                    }
                }
            }
        }
    }
    Ok(actual.len() == expected.len()
        && actual.iter().all(|(path, hash)| {
            expected
                .get(path.as_str())
                .is_some_and(|expected_hash| expected_hash == &hash.as_str())
        }))
}

fn definite(message: &str) -> ActionFailure {
    ActionFailure::Definite(json!({"summary": message}))
}

fn ambiguous(message: &str) -> ActionFailure {
    ActionFailure::Ambiguous(json!({"summary": message}))
}

fn delivery_action_failure(error: DeliveryError) -> ActionFailure {
    definite_hashed("tracker delivery failed", &error.to_string())
}

async fn execute_commit(
    worktree: &Path,
    envelope: &DeliveryPreviewEnvelope,
    action: &SddDeliveryActionRecord,
    message: &str,
) -> Result<Value, ActionFailure> {
    let head = git_text(worktree, &["rev-parse", "HEAD"])
        .await
        .map_err(ActionFailure::Definite)?;
    if let Ok(body) = git_text(worktree, &["show", "-s", "--format=%B", &head]).await {
        if body.contains(&format!("Agentum-Delivery-Action: {}", action.action_id)) {
            run_git(worktree, &["read-tree", "HEAD"], &[])
                .await
                .map_err(ActionFailure::Definite)?;
            return Ok(json!({"summary": "commit already exists", "commit": head}));
        }
    }
    let current_hash = workspace_state_hash(worktree)
        .await
        .map_err(|error| definite_hashed("workspace snapshot failed", &error.to_string()))?;
    if current_hash != envelope.workspace_state_hash {
        return Err(definite("workspace changed after delivery preview"));
    }
    let temp_root = agentum_store::paths::data_dir()
        .map_err(|error| definite_hashed("delivery temp path unavailable", &error.to_string()))?
        .join("delivery-tmp");
    std::fs::create_dir_all(&temp_root)
        .map_err(|error| definite_hashed("delivery temp directory failed", &error.to_string()))?;
    let temp = tempfile::Builder::new()
        .prefix("index-")
        .tempdir_in(&temp_root)
        .map_err(|error| definite_hashed("delivery temp index failed", &error.to_string()))?;
    let index = temp.path().join("index");
    let index_env = vec![(
        "GIT_INDEX_FILE".to_owned(),
        index.to_string_lossy().into_owned(),
    )];
    run_git(worktree, &["read-tree", "HEAD"], &index_env)
        .await
        .map_err(ActionFailure::Definite)?;
    run_git(worktree, &["add", "-A", "--", "."], &index_env)
        .await
        .map_err(ActionFailure::Definite)?;
    let tree = run_git(worktree, &["write-tree"], &index_env)
        .await
        .map_err(ActionFailure::Definite)?;
    let tree = String::from_utf8_lossy(&tree.stdout).trim().to_owned();
    let head_tree = git_text(worktree, &["rev-parse", "HEAD^{tree}"])
        .await
        .map_err(ActionFailure::Definite)?;
    if tree == head_tree {
        return Err(definite("delivery commit has no changes"));
    }
    let stable_hash = workspace_state_hash(worktree)
        .await
        .map_err(|error| definite_hashed("workspace snapshot failed", &error.to_string()))?;
    if stable_hash != envelope.workspace_state_hash {
        return Err(definite(
            "workspace changed while the delivery commit was prepared",
        ));
    }
    let trailer = format!(
        "Agentum-Spec: {}\nAgentum-Delivery-Action: {}",
        envelope.spec_id, action.action_id
    );
    let commit = run_git_owned(
        worktree,
        vec![
            "commit-tree".into(),
            tree,
            "-p".into(),
            head.clone(),
            "-m".into(),
            message.into(),
            "-m".into(),
            trailer,
        ],
        &[],
    )
    .await
    .map_err(ActionFailure::Definite)?;
    let commit = String::from_utf8_lossy(&commit.stdout).trim().to_owned();
    run_git_owned(
        worktree,
        vec!["update-ref".into(), "HEAD".into(), commit.clone(), head],
        &[],
    )
    .await
    .map_err(ActionFailure::Definite)?;
    run_git_owned(worktree, vec!["read-tree".into(), commit.clone()], &[])
        .await
        .map_err(ActionFailure::Definite)?;
    Ok(json!({"summary": "commit created", "commit": commit}))
}

async fn execute_push(
    worktree: &Path,
    branch: &str,
    action: &SddDeliveryActionRecord,
    remote: &str,
) -> Result<Value, ActionFailure> {
    let head = git_text(worktree, &["rev-parse", "HEAD"])
        .await
        .map_err(ActionFailure::Definite)?;
    if remote_head(worktree, remote, branch).await.as_deref() == Some(head.as_str()) {
        return Ok(json!({"summary": "remote branch already matches", "commit": head}));
    }
    let refspec = format!("HEAD:refs/heads/{branch}");
    let outcome = run_git_owned(
        worktree,
        vec!["push".into(), "--porcelain".into(), remote.into(), refspec],
        &[],
    )
    .await;
    if remote_head(worktree, remote, branch).await.as_deref() == Some(head.as_str()) {
        return Ok(json!({
            "summary": "branch pushed",
            "commit": head,
            "actionId": action.action_id
        }));
    }
    match outcome {
        Ok(result) => Err(ActionFailure::Ambiguous(command_result(
            "push outcome could not be confirmed",
            &result,
        ))),
        Err(result) => Err(ActionFailure::Ambiguous(result)),
    }
}

async fn remote_head(worktree: &Path, remote: &str, branch: &str) -> Option<String> {
    let reference = format!("refs/heads/{branch}");
    let result = run_git_owned(
        worktree,
        vec![
            "ls-remote".into(),
            "--heads".into(),
            remote.into(),
            reference,
        ],
        &[],
    )
    .await
    .ok()?;
    let line = String::from_utf8_lossy(&result.stdout);
    line.split_whitespace().next().map(str::to_owned)
}

async fn execute_pull_request(
    worktree: &Path,
    branch: &str,
    action: &SddDeliveryActionRecord,
    title: &str,
    body: &str,
    base: &str,
) -> Result<Value, ActionFailure> {
    let marker = format!("<!-- agentum-delivery:{} -->", action.action_id);
    let expected_body = format!("{body}\n\n{marker}");
    if let Some(existing) = find_pull_request(worktree, branch).await {
        if existing.get("baseRefName").and_then(Value::as_str) == Some(base)
            && existing.get("headRefName").and_then(Value::as_str) == Some(branch)
            && existing.get("title").and_then(Value::as_str) == Some(title)
            && existing.get("body").and_then(Value::as_str) == Some(expected_body.as_str())
        {
            return Ok(json!({
                "summary": "pull request already exists",
                "url": existing.get("url").and_then(Value::as_str),
                "actionId": action.action_id
            }));
        }
        return Err(definite(
            "an existing pull request has different base/head intent",
        ));
    }
    let result = run_gh_owned(
        worktree,
        vec![
            "pr".into(),
            "create".into(),
            "--title".into(),
            title.into(),
            "--body".into(),
            expected_body.clone(),
            "--base".into(),
            base.into(),
            "--head".into(),
            branch.into(),
        ],
    )
    .await;
    if let Some(existing) = find_pull_request(worktree, branch).await {
        if existing.get("baseRefName").and_then(Value::as_str) == Some(base)
            && existing.get("headRefName").and_then(Value::as_str) == Some(branch)
            && existing.get("title").and_then(Value::as_str) == Some(title)
            && existing.get("body").and_then(Value::as_str) == Some(expected_body.as_str())
        {
            return Ok(json!({
                "summary": "pull request created",
                "url": existing.get("url").and_then(Value::as_str),
                "actionId": action.action_id
            }));
        }
        return Err(definite("pull request was created with unexpected content"));
    }
    Err(ActionFailure::Ambiguous(match result {
        Ok(outcome) => command_result("pull request outcome could not be confirmed", &outcome),
        Err(value) => value,
    }))
}

async fn find_pull_request(worktree: &Path, branch: &str) -> Option<Value> {
    let result = run_gh_owned(
        worktree,
        vec![
            "pr".into(),
            "view".into(),
            branch.into(),
            "--json".into(),
            "url,title,body,baseRefName,headRefName,state".into(),
        ],
    )
    .await
    .ok()?;
    serde_json::from_slice(&result.stdout).ok()
}

async fn execute_tracker_comment_provider(
    state: &AppState,
    worktree: &Path,
    action: &SddDeliveryActionRecord,
    binding: &TrackerMutationPreview,
    body: &str,
) -> Result<Value, ActionFailure> {
    let TrackerMutationOperation::Comment { marker } = &binding.operation else {
        return Err(definite("tracker comment preview has the wrong operation"));
    };
    let expected_marker = format!("<!-- agentum-delivery:{} -->", action.action_id);
    if marker != &expected_marker {
        return Err(definite("tracker comment idempotency marker is invalid"));
    }
    match binding.provider.as_str() {
        "github" => execute_tracker_comment(worktree, action, &binding.url, body).await,
        "linear" => execute_linear_comment(state, action, binding, body, marker).await,
        "jira" => execute_jira_comment(state, action, binding, body, marker).await,
        _ => Err(definite("tracker provider is unsupported")),
    }
}

async fn execute_tracker_status_provider(
    state: &AppState,
    worktree: &Path,
    action: &SddDeliveryActionRecord,
    binding: &TrackerMutationPreview,
) -> Result<Value, ActionFailure> {
    let TrackerMutationOperation::Status {
        target_id,
        target_name,
        transition_id,
        ..
    } = &binding.operation
    else {
        return Err(definite("tracker status preview has the wrong operation"));
    };
    match binding.provider.as_str() {
        "github" => {
            if transition_id.is_some() || !matches!(target_id.as_str(), "open" | "closed") {
                return Err(definite("GitHub tracker status preview is invalid"));
            }
            execute_tracker_status(worktree, action, &binding.url, target_id).await
        }
        "linear" => execute_linear_status(state, action, binding, target_id, target_name).await,
        "jira" => {
            execute_jira_status(
                state,
                action,
                binding,
                target_id,
                target_name,
                transition_id.as_deref(),
            )
            .await
        }
        _ => Err(definite("tracker provider is unsupported")),
    }
}

async fn execute_tracker_field_update(
    state: &AppState,
    action: &SddDeliveryActionRecord,
    binding: &TrackerMutationPreview,
) -> Result<Value, ActionFailure> {
    if binding.provider != "jira" {
        return Err(definite(
            "typed tracker field preview has an unsupported provider",
        ));
    }
    let TrackerMutationOperation::FieldUpdate {
        field_id,
        field_name,
        value,
        current_value_hash,
        target_value_hash,
    } = &binding.operation
    else {
        return Err(definite("tracker field preview has the wrong operation"));
    };
    let credential = jira_delivery_credential(state, binding)
        .await
        .map_err(delivery_action_failure)?;
    let before = jira_field_binding(&credential, binding, field_id, value)
        .await
        .map_err(delivery_action_failure)?;
    let before_hash =
        sha256(canonical_json_bytes(&before.current_value).map_err(delivery_action_failure)?);
    let derived_target_hash = sha256(
        canonical_json_bytes(&before.target_compare_value).map_err(delivery_action_failure)?,
    );
    if derived_target_hash != *target_value_hash || before.field_name != *field_name {
        return Err(definite(
            "Jira field metadata changed after delivery preview",
        ));
    }
    if before_hash == *target_value_hash {
        return Ok(json!({
            "summary": "Jira field already matches",
            "url": binding.url,
            "fieldId": field_id,
            "fieldName": field_name,
            "actionId": action.action_id
        }));
    }
    if before_hash != *current_value_hash {
        return Err(definite("Jira field changed after delivery confirmation"));
    }
    let site_id = binding
        .site_id
        .as_deref()
        .ok_or_else(|| definite("Jira preview has no site"))?;
    let issue = jira_issue_reference(binding).map_err(delivery_action_failure)?;
    let url = credential
        .endpoint(site_id, &["issue", issue])
        .map_err(delivery_action_failure)?;
    let mutation = jira_put_json(
        &credential,
        url,
        &json!({ "fields": { (field_id): before.target_value } }),
    )
    .await;
    match jira_field_binding(&credential, binding, field_id, value).await {
        Ok(after)
            if canonical_json_bytes(&after.current_value)
                .map(|value| sha256(value) == *target_value_hash)
                .unwrap_or(false) =>
        {
            Ok(json!({
                "summary": "Jira field updated",
                "url": binding.url,
                "fieldId": field_id,
                "fieldName": field_name,
                "actionId": action.action_id
            }))
        }
        Ok(_) => Err(mutation
            .err()
            .map(TrackerRemoteError::into_action_failure)
            .unwrap_or_else(|| ambiguous("Jira field update outcome could not be confirmed"))),
        Err(error) => Err(ActionFailure::Ambiguous(json!({
            "summary": "Jira field update outcome could not be reread",
            "errorHash": sha256(error.to_string())
        }))),
    }
}

async fn execute_linear_comment(
    state: &AppState,
    action: &SddDeliveryActionRecord,
    binding: &TrackerMutationPreview,
    body: &str,
    marker: &str,
) -> Result<Value, ActionFailure> {
    let credential = linear_delivery_credential(state, &binding.connection_id)
        .await
        .map_err(delivery_action_failure)?;
    match linear_issue_has_comment(&credential, &binding.external_id, marker).await {
        Ok(true) => {
            return Ok(json!({
                "summary": "Linear comment already exists",
                "url": binding.url,
                "actionId": action.action_id
            }));
        }
        Ok(false) => {}
        Err(error) => return Err(error.into_action_failure()),
    }
    const MUTATION: &str = "mutation($issueId: String!, $body: String!) { commentCreate(input: { issueId: $issueId, body: $body }) { success comment { id } } }";
    let mutation = linear_graphql(
        credential.token(),
        MUTATION,
        json!({
            "issueId": binding.external_id,
            "body": format!("{body}\n\n{marker}")
        }),
    )
    .await
    .and_then(|value| {
        if value
            .pointer("/data/commentCreate/success")
            .and_then(Value::as_bool)
            == Some(true)
        {
            Ok(value)
        } else {
            Err(TrackerRemoteError::definite(
                "Linear comment mutation was rejected",
                value.to_string(),
            ))
        }
    });
    match linear_issue_has_comment(&credential, &binding.external_id, marker).await {
        Ok(true) => Ok(json!({
            "summary": "Linear comment added",
            "url": binding.url,
            "actionId": action.action_id
        })),
        Ok(false) => Err(mutation
            .err()
            .map(TrackerRemoteError::into_action_failure)
            .unwrap_or_else(|| ambiguous("Linear comment outcome could not be confirmed"))),
        Err(error) => Err(ActionFailure::Ambiguous(json!({
            "summary": "Linear comment outcome could not be reread",
            "errorHash": error.detail_hash
        }))),
    }
}

async fn execute_linear_status(
    state: &AppState,
    action: &SddDeliveryActionRecord,
    binding: &TrackerMutationPreview,
    target_id: &str,
    target_name: &str,
) -> Result<Value, ActionFailure> {
    let credential = linear_delivery_credential(state, &binding.connection_id)
        .await
        .map_err(delivery_action_failure)?;
    let before = linear_issue_snapshot(&credential, &binding.external_id)
        .await
        .map_err(delivery_action_failure)?;
    if before.status_id == target_id {
        return Ok(json!({
            "summary": "Linear status already matches",
            "url": binding.url,
            "status": target_name,
            "actionId": action.action_id
        }));
    }
    if !before
        .transitions
        .iter()
        .any(|choice| choice.target_id == target_id)
    {
        return Err(definite(
            "previewed Linear workflow state is no longer available",
        ));
    }
    const MUTATION: &str = "mutation($id: String!, $stateId: String!) { issueUpdate(id: $id, input: { stateId: $stateId }) { success issue { id state { id name } } } }";
    let mutation = linear_graphql(
        credential.token(),
        MUTATION,
        json!({ "id": binding.external_id, "stateId": target_id }),
    )
    .await
    .and_then(|value| {
        if value
            .pointer("/data/issueUpdate/success")
            .and_then(Value::as_bool)
            == Some(true)
        {
            Ok(value)
        } else {
            Err(TrackerRemoteError::definite(
                "Linear status mutation was rejected",
                value.to_string(),
            ))
        }
    });
    match linear_issue_snapshot(&credential, &binding.external_id).await {
        Ok(after) if after.status_id == target_id => Ok(json!({
            "summary": "Linear status updated",
            "url": binding.url,
            "status": target_name,
            "actionId": action.action_id
        })),
        Ok(_) => Err(mutation
            .err()
            .map(TrackerRemoteError::into_action_failure)
            .unwrap_or_else(|| ambiguous("Linear status outcome could not be confirmed"))),
        Err(error) => Err(ActionFailure::Ambiguous(json!({
            "summary": "Linear status outcome could not be reread",
            "errorHash": sha256(error.to_string())
        }))),
    }
}

async fn linear_issue_has_comment(
    credential: &LinearCredential,
    issue_id: &str,
    marker: &str,
) -> Result<bool, TrackerRemoteError> {
    const QUERY: &str = "query($id: String!, $after: String) { issue(id: $id) { comments(first: 100, after: $after) { nodes { body } pageInfo { hasNextPage endCursor } } } }";
    let mut after: Option<String> = None;
    for _ in 0..100 {
        let value = linear_graphql(
            credential.token(),
            QUERY,
            json!({ "id": issue_id, "after": after }),
        )
        .await?;
        let comments = value.pointer("/data/issue/comments").ok_or_else(|| {
            TrackerRemoteError::definite(
                "Linear comments response was malformed",
                value.to_string(),
            )
        })?;
        if comments
            .get("nodes")
            .and_then(Value::as_array)
            .is_some_and(|nodes| {
                nodes.iter().any(|comment| {
                    comment
                        .get("body")
                        .and_then(Value::as_str)
                        .is_some_and(|body| body.contains(marker))
                })
            })
        {
            return Ok(true);
        }
        if comments
            .pointer("/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            != Some(true)
        {
            return Ok(false);
        }
        after = comments
            .pointer("/pageInfo/endCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if after.is_none() {
            return Err(TrackerRemoteError::definite(
                "Linear comments pagination was malformed",
                value.to_string(),
            ));
        }
    }
    Err(TrackerRemoteError::definite(
        "Linear comment reconciliation exceeded its bound",
        issue_id,
    ))
}

async fn execute_jira_comment(
    state: &AppState,
    action: &SddDeliveryActionRecord,
    binding: &TrackerMutationPreview,
    body: &str,
    marker: &str,
) -> Result<Value, ActionFailure> {
    let credential = jira_delivery_credential(state, binding)
        .await
        .map_err(delivery_action_failure)?;
    match jira_issue_has_comment(&credential, binding, marker).await {
        Ok(true) => {
            return Ok(json!({
                "summary": "Jira comment already exists",
                "url": binding.url,
                "actionId": action.action_id
            }));
        }
        Ok(false) => {}
        Err(error) => return Err(error.into_action_failure()),
    }
    let site_id = binding
        .site_id
        .as_deref()
        .ok_or_else(|| definite("Jira preview has no site"))?;
    let issue = jira_issue_reference(binding).map_err(delivery_action_failure)?;
    let url = credential
        .endpoint(site_id, &["issue", issue, "comment"])
        .map_err(delivery_action_failure)?;
    let mutation = jira_post_json(
        &credential,
        url,
        &json!({
            "body": {
                "type": "doc",
                "version": 1,
                "content": [{
                    "type": "paragraph",
                    "content": [{
                        "type": "text",
                        "text": format!("{body}\n\n{marker}")
                    }]
                }]
            }
        }),
    )
    .await;
    match jira_issue_has_comment(&credential, binding, marker).await {
        Ok(true) => Ok(json!({
            "summary": "Jira comment added",
            "url": binding.url,
            "actionId": action.action_id
        })),
        Ok(false) => Err(mutation
            .err()
            .map(TrackerRemoteError::into_action_failure)
            .unwrap_or_else(|| ambiguous("Jira comment outcome could not be confirmed"))),
        Err(error) => Err(ActionFailure::Ambiguous(json!({
            "summary": "Jira comment outcome could not be reread",
            "errorHash": error.detail_hash
        }))),
    }
}

async fn execute_jira_status(
    state: &AppState,
    action: &SddDeliveryActionRecord,
    binding: &TrackerMutationPreview,
    target_id: &str,
    target_name: &str,
    transition_id: Option<&str>,
) -> Result<Value, ActionFailure> {
    let credential = jira_delivery_credential(state, binding)
        .await
        .map_err(delivery_action_failure)?;
    let before = jira_issue_snapshot(&credential, binding)
        .await
        .map_err(delivery_action_failure)?;
    if before.status_id == target_id {
        return Ok(json!({
            "summary": "Jira status already matches",
            "url": binding.url,
            "status": target_name,
            "actionId": action.action_id
        }));
    }
    let transition_id =
        transition_id.ok_or_else(|| definite("Jira status preview has no selected transition"))?;
    if !before
        .transitions
        .iter()
        .any(|choice| choice.id == transition_id && choice.target_id == target_id)
    {
        return Err(definite("previewed Jira transition is no longer allowed"));
    }
    let site_id = binding
        .site_id
        .as_deref()
        .ok_or_else(|| definite("Jira preview has no site"))?;
    let issue = jira_issue_reference(binding).map_err(delivery_action_failure)?;
    let url = credential
        .endpoint(site_id, &["issue", issue, "transitions"])
        .map_err(delivery_action_failure)?;
    let mutation = jira_post_json(
        &credential,
        url,
        &json!({ "transition": { "id": transition_id } }),
    )
    .await;
    match jira_issue_snapshot(&credential, binding).await {
        Ok(after) if after.status_id == target_id => Ok(json!({
            "summary": "Jira status updated",
            "url": binding.url,
            "status": target_name,
            "transitionId": transition_id,
            "actionId": action.action_id
        })),
        Ok(_) => Err(mutation
            .err()
            .map(TrackerRemoteError::into_action_failure)
            .unwrap_or_else(|| ambiguous("Jira status outcome could not be confirmed"))),
        Err(error) => Err(ActionFailure::Ambiguous(json!({
            "summary": "Jira status outcome could not be reread",
            "errorHash": sha256(error.to_string())
        }))),
    }
}

async fn jira_issue_has_comment(
    credential: &JiraDeliveryCredential,
    binding: &TrackerMutationPreview,
    marker: &str,
) -> Result<bool, TrackerRemoteError> {
    let site_id = binding
        .site_id
        .as_deref()
        .ok_or_else(|| TrackerRemoteError::definite("Jira preview has no site", "missing"))?;
    let issue = jira_issue_reference(binding).map_err(|error| {
        TrackerRemoteError::definite("Jira issue identity is invalid", error.to_string())
    })?;
    let mut start_at = 0_u64;
    for _ in 0..100 {
        let mut url = credential
            .endpoint(site_id, &["issue", issue, "comment"])
            .map_err(|error| {
                TrackerRemoteError::definite("Jira comment endpoint is invalid", error.to_string())
            })?;
        url.query_pairs_mut()
            .append_pair("startAt", &start_at.to_string())
            .append_pair("maxResults", "100")
            .append_pair("orderBy", "-created");
        let value = jira_get_json(credential, url).await?;
        let comments = value
            .get("comments")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                TrackerRemoteError::definite(
                    "Jira comments response was malformed",
                    value.to_string(),
                )
            })?;
        if comments.iter().any(|comment| {
            comment
                .get("body")
                .is_some_and(|body| body.to_string().contains(marker))
        }) {
            return Ok(true);
        }
        let total = value.get("total").and_then(Value::as_u64).unwrap_or(0);
        start_at = start_at.saturating_add(comments.len() as u64);
        if start_at >= total {
            return Ok(false);
        }
        if comments.is_empty() {
            return Err(TrackerRemoteError::definite(
                "Jira comments pagination did not advance",
                value.to_string(),
            ));
        }
    }
    Err(TrackerRemoteError::definite(
        "Jira comment reconciliation exceeded its bound",
        issue,
    ))
}

async fn execute_tracker_comment(
    worktree: &Path,
    action: &SddDeliveryActionRecord,
    url: &str,
    body: &str,
) -> Result<Value, ActionFailure> {
    let marker = format!("<!-- agentum-delivery:{} -->", action.action_id);
    if issue_has_comment(worktree, url, &marker).await {
        return Ok(json!({"summary": "tracker comment already exists", "url": url}));
    }
    let result = run_gh_owned(
        worktree,
        vec![
            "issue".into(),
            "comment".into(),
            url.into(),
            "--body".into(),
            format!("{body}\n\n{marker}"),
        ],
    )
    .await;
    if issue_has_comment(worktree, url, &marker).await {
        return Ok(json!({"summary": "tracker comment added", "url": url}));
    }
    Err(ActionFailure::Ambiguous(match result {
        Ok(outcome) => command_result("tracker comment outcome could not be confirmed", &outcome),
        Err(value) => value,
    }))
}

async fn issue_has_comment(worktree: &Path, url: &str, marker: &str) -> bool {
    let result = run_gh_owned(
        worktree,
        vec![
            "issue".into(),
            "view".into(),
            url.into(),
            "--json".into(),
            "comments".into(),
        ],
    )
    .await;
    let Ok(result) = result else { return false };
    let Ok(value) = serde_json::from_slice::<Value>(&result.stdout) else {
        return false;
    };
    value["comments"].as_array().is_some_and(|comments| {
        comments.iter().any(|item| {
            item["body"]
                .as_str()
                .is_some_and(|body| body.contains(marker))
        })
    })
}

async fn execute_tracker_status(
    worktree: &Path,
    action: &SddDeliveryActionRecord,
    url: &str,
    target: &str,
) -> Result<Value, ActionFailure> {
    if issue_state(worktree, url).await.as_deref() == Some(target) {
        return Ok(
            json!({"summary": "tracker status already matches", "url": url, "status": target}),
        );
    }
    let operation = if target == "closed" {
        "close"
    } else {
        "reopen"
    };
    let result = run_gh_owned(worktree, vec!["issue".into(), operation.into(), url.into()]).await;
    if issue_state(worktree, url).await.as_deref() == Some(target) {
        return Ok(json!({
            "summary": "tracker status updated",
            "url": url,
            "status": target,
            "actionId": action.action_id
        }));
    }
    Err(ActionFailure::Ambiguous(match result {
        Ok(outcome) => command_result("tracker status outcome could not be confirmed", &outcome),
        Err(value) => value,
    }))
}

async fn issue_state(worktree: &Path, url: &str) -> Option<String> {
    let result = run_gh_owned(
        worktree,
        vec![
            "issue".into(),
            "view".into(),
            url.into(),
            "--json".into(),
            "state".into(),
        ],
    )
    .await
    .ok()?;
    let value: Value = serde_json::from_slice(&result.stdout).ok()?;
    value["state"]
        .as_str()
        .map(|state| state.to_ascii_lowercase())
}

async fn execute_release(
    worktree: &Path,
    branch: &str,
    action: &SddDeliveryActionRecord,
    tag: &str,
    name: &str,
    notes: &str,
    prerelease: bool,
) -> Result<Value, ActionFailure> {
    let marker = format!("<!-- agentum-delivery:{} -->", action.action_id);
    let expected_notes = format!("{notes}\n\n{marker}");
    if let Some(existing) = find_release(worktree, tag).await {
        if release_matches(&existing, branch, name, &expected_notes, prerelease) {
            return Ok(json!({
                "summary": "release already exists",
                "url": existing.get("url").and_then(Value::as_str),
                "tag": tag
            }));
        }
        return Err(definite(
            "an existing release has different previewed intent",
        ));
    }
    let mut args = vec![
        "release".into(),
        "create".into(),
        tag.into(),
        "--title".into(),
        name.into(),
        "--notes".into(),
        expected_notes.clone(),
        "--target".into(),
        branch.into(),
    ];
    if prerelease {
        args.push("--prerelease".into());
    }
    let result = run_gh_owned(worktree, args).await;
    if let Some(existing) = find_release(worktree, tag).await {
        if release_matches(&existing, branch, name, &expected_notes, prerelease) {
            return Ok(json!({
                "summary": "release created",
                "url": existing.get("url").and_then(Value::as_str),
                "tag": tag,
                "actionId": action.action_id
            }));
        }
        return Err(definite("release was created with unexpected content"));
    }
    Err(ActionFailure::Ambiguous(match result {
        Ok(outcome) => command_result("release outcome could not be confirmed", &outcome),
        Err(value) => value,
    }))
}

async fn find_release(worktree: &Path, tag: &str) -> Option<Value> {
    let result = run_gh_owned(
        worktree,
        vec![
            "release".into(),
            "view".into(),
            tag.into(),
            "--json".into(),
            "tagName,name,body,isDraft,isPrerelease,targetCommitish,url".into(),
        ],
    )
    .await
    .ok()?;
    serde_json::from_slice(&result.stdout).ok()
}

fn release_matches(
    release: &Value,
    branch: &str,
    name: &str,
    notes: &str,
    prerelease: bool,
) -> bool {
    release.get("name").and_then(Value::as_str) == Some(name)
        && release.get("body").and_then(Value::as_str) == Some(notes)
        && release.get("isDraft").and_then(Value::as_bool) == Some(false)
        && release.get("isPrerelease").and_then(Value::as_bool) == Some(prerelease)
        && release.get("targetCommitish").and_then(Value::as_str) == Some(branch)
}

fn definite_hashed(summary: &str, detail: &str) -> ActionFailure {
    ActionFailure::Definite(json!({"summary": summary, "errorHash": sha256(detail)}))
}

async fn git_text(worktree: &Path, args: &[&str]) -> Result<String, Value> {
    let result = run_git(worktree, args, &[]).await?;
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_owned())
}

async fn run_git(
    worktree: &Path,
    args: &[&str],
    extra_env: &[(String, String)],
) -> Result<CommandOutcome, Value> {
    run_git_owned(
        worktree,
        args.iter().map(|value| (*value).to_owned()).collect(),
        extra_env,
    )
    .await
}

async fn run_git_owned(
    worktree: &Path,
    args: Vec<String>,
    extra_env: &[(String, String)],
) -> Result<CommandOutcome, Value> {
    match run_command(
        worktree,
        "git",
        &args,
        extra_env,
        COMMAND_TIMEOUT,
        OUTPUT_LIMIT,
    )
    .await
    {
        Ok(result) if result.success => Ok(result),
        Ok(result) => Err(command_result("git command failed", &result)),
        Err(error) => Err(
            json!({"summary": "git command could not run", "errorHash": sha256(error.to_string())}),
        ),
    }
}

async fn run_gh_owned(worktree: &Path, args: Vec<String>) -> Result<CommandOutcome, Value> {
    match run_command(worktree, "gh", &args, &[], COMMAND_TIMEOUT, OUTPUT_LIMIT).await {
        Ok(result) if result.success => Ok(result),
        Ok(result) => Err(command_result("GitHub command failed", &result)),
        Err(error) => Err(
            json!({"summary": "GitHub command could not run", "errorHash": sha256(error.to_string())}),
        ),
    }
}

fn command_result(summary: &str, outcome: &CommandOutcome) -> Value {
    let mut combined = outcome.stdout.clone();
    combined.push(0xff);
    combined.extend_from_slice(&outcome.stderr);
    json!({
        "summary": summary,
        "exitCode": outcome.exit_code,
        "outputHash": sha256(combined),
        "capturedBytes": outcome.stdout.len().saturating_add(outcome.stderr.len())
    })
}

#[derive(Debug)]
struct CommandOutcome {
    success: bool,
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn run_checked(
    cwd: &Path,
    program: &str,
    args: &[String],
    env: &[(String, String)],
    timeout: Duration,
    output_limit: usize,
) -> Result<CommandOutcome, DeliveryError> {
    let result = run_command(cwd, program, args, env, timeout, output_limit).await?;
    if result.success {
        Ok(result)
    } else {
        Err(DeliveryError::Command(format!(
            "{program} exited {:?} (output hash {})",
            result.exit_code,
            sha256([result.stdout, result.stderr].concat())
        )))
    }
}

async fn run_command(
    cwd: &Path,
    program: &str,
    args: &[String],
    extra_env: &[(String, String)],
    timeout: Duration,
    output_limit: usize,
) -> Result<CommandOutcome, std::io::Error> {
    let executable = which::which(program).map_err(std::io::Error::other)?;
    let mut command = tokio::process::Command::new(executable);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for key in [
        "PATH",
        "HOME",
        "XDG_CONFIG_HOME",
        "SSH_AUTH_SOCK",
        "GH_TOKEN",
        "GITHUB_TOKEN",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    let pid = child.id();
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let per_stream = output_limit / 2;
    let stdout_task = tokio::spawn(read_bounded(stdout, per_stream));
    let stderr_task = tokio::spawn(read_bounded(stderr, per_stream));
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            super::providers::terminate_process_tree(&mut child, pid).await;
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "delivery command timed out",
            ));
        }
    };
    let stdout = stdout_task.await.map_err(std::io::Error::other)??;
    let stderr = stderr_task.await.map_err(std::io::Error::other)??;
    Ok(CommandOutcome {
        success: status.success(),
        exit_code: status.code(),
        stdout,
        stderr,
    })
}

async fn read_bounded(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > limit {
        bytes.truncate(limit);
    }
    Ok(bytes)
}

/// Interrupted actions are not blindly replayed. The user sees an ambiguous
/// result and explicitly retries through the same confirmed preview.
pub(crate) async fn recover_interrupted(state: &AppState) -> Result<u64, DeliveryError> {
    Ok(state.store.sdd_recover_interrupted_delivery().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_dependencies_are_typed_and_deterministic() {
        let actions = prepare_actions(vec![
            DeliveryActionRequest::Commit {
                message: "Release work".into(),
            },
            DeliveryActionRequest::Push {
                remote: "origin".into(),
            },
            DeliveryActionRequest::PullRequest {
                title: "Ready change".into(),
                body: "Verified".into(),
                base: "main".into(),
            },
        ])
        .unwrap();
        assert_eq!(actions[1].depends_on, vec![actions[0].id.clone()]);
        assert_eq!(actions[2].depends_on, vec![actions[1].id.clone()]);
    }

    #[test]
    fn unsafe_ref_like_values_are_rejected() {
        assert!(
            prepare_actions(vec![DeliveryActionRequest::Push {
                remote: "--upload-pack=evil".into(),
            }])
            .is_err()
        );
        assert!(
            prepare_actions(vec![DeliveryActionRequest::Release {
                tag: "../escape".into(),
                name: "bad".into(),
                notes: "bad".into(),
                prerelease: false,
            }])
            .is_err()
        );
    }

    #[tokio::test]
    async fn workspace_hash_changes_with_untracked_content() {
        let dir = tempfile::tempdir().unwrap();
        run_command(
            dir.path(),
            "git",
            &["init".into()],
            &[],
            Duration::from_secs(10),
            64 * 1024,
        )
        .await
        .unwrap();
        std::fs::write(dir.path().join("tracked"), "one").unwrap();
        run_command(
            dir.path(),
            "git",
            &[
                "-c".into(),
                "user.name=Agentum Test".into(),
                "-c".into(),
                "user.email=test@invalid".into(),
                "add".into(),
                "tracked".into(),
            ],
            &[],
            Duration::from_secs(10),
            64 * 1024,
        )
        .await
        .unwrap();
        run_command(
            dir.path(),
            "git",
            &[
                "-c".into(),
                "user.name=Agentum Test".into(),
                "-c".into(),
                "user.email=test@invalid".into(),
                "commit".into(),
                "-m".into(),
                "initial".into(),
            ],
            &[],
            Duration::from_secs(10),
            64 * 1024,
        )
        .await
        .unwrap();
        let before = workspace_state_hash(dir.path()).await.unwrap();
        std::fs::write(dir.path().join("new-file"), "two").unwrap();
        let after = workspace_state_hash(dir.path()).await.unwrap();
        assert_ne!(before, after);
    }

    #[tokio::test]
    async fn push_reconciles_a_local_remote_without_duplicate_effects() {
        let source = tempfile::tempdir().unwrap();
        let remote_root = tempfile::tempdir().unwrap();
        let remote = remote_root.path().join("remote.git");
        std::process::Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&remote)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(source.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Agentum Test"])
            .current_dir(source.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@invalid"])
            .current_dir(source.path())
            .status()
            .unwrap();
        std::fs::write(source.path().join("README.md"), "ready\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(source.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-qm", "ready"])
            .current_dir(source.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(&remote)
            .current_dir(source.path())
            .status()
            .unwrap();
        let action = SddDeliveryActionRecord {
            preview_id: "preview".into(),
            action_id: "action".into(),
            action_type: "push".into(),
            intent_json: "{}".into(),
            status: "running".into(),
            result_json: None,
            attempts: 1,
            updated_at: "now".into(),
        };
        let first = execute_push(source.path(), "main", &action, "origin")
            .await
            .unwrap();
        assert_eq!(first["summary"], "branch pushed");
        let second = execute_push(source.path(), "main", &action, "origin")
            .await
            .unwrap();
        assert_eq!(second["summary"], "remote branch already matches");
    }

    #[test]
    fn release_reconciliation_requires_exact_previewed_intent() {
        let release = json!({
            "name": "Agentum 1.2.0",
            "body": "Notes\n\n<!-- agentum-delivery:a -->",
            "isDraft": false,
            "isPrerelease": false,
            "targetCommitish": "main"
        });
        assert!(release_matches(
            &release,
            "main",
            "Agentum 1.2.0",
            "Notes\n\n<!-- agentum-delivery:a -->",
            false
        ));
        assert!(!release_matches(
            &release,
            "main",
            "Agentum 1.2.1",
            "Notes\n\n<!-- agentum-delivery:a -->",
            false
        ));
    }

    #[test]
    fn jira_transition_ambiguity_requires_an_exact_live_choice() {
        let choices = vec![
            TrackerTransitionChoice {
                id: "21".into(),
                name: "Complete normally".into(),
                target_id: "done".into(),
                target_name: "Done".into(),
            },
            TrackerTransitionChoice {
                id: "42".into(),
                name: "Complete with review".into(),
                target_id: "done".into(),
                target_name: "Done".into(),
            },
        ];
        let error = resolve_jira_transition("Done", None, &choices).unwrap_err();
        match error {
            DeliveryError::TransitionChoiceRequired {
                provider,
                target,
                choices: offered,
            } => {
                assert_eq!(provider, "jira");
                assert_eq!(target, "Done");
                assert_eq!(offered, choices);
            }
            other => panic!("expected explicit transition choice, got {other}"),
        }
        assert_eq!(
            resolve_jira_transition("Done", Some("42"), &choices)
                .unwrap()
                .id,
            "42"
        );
        assert!(resolve_jira_transition("Done", Some("missing"), &choices).is_err());
    }

    #[test]
    fn jira_field_values_are_closed_and_live_metadata_authorized() {
        let option_metadata = json!({
            "name": "Priority",
            "required": false,
            "operations": ["set"],
            "schema": { "type": "priority" },
            "allowedValues": [
                { "id": "1", "name": "Highest" },
                { "id": "3", "name": "Medium" }
            ]
        });
        assert_eq!(
            jira_typed_field_json(
                "priority",
                &option_metadata,
                &TrackerFieldValue::Option {
                    option_id: "3".into()
                }
            )
            .unwrap(),
            json!({ "id": "3" })
        );
        assert!(
            jira_typed_field_json(
                "priority",
                &option_metadata,
                &TrackerFieldValue::Option {
                    option_id: "9".into()
                }
            )
            .is_err()
        );
        assert_eq!(
            normalize_jira_field_value(
                &TrackerFieldValue::Option {
                    option_id: "3".into()
                },
                json!({
                    "id": "3",
                    "name": "Medium",
                    "self": "https://example.atlassian.net/rest/api/3/priority/3"
                })
            )
            .unwrap(),
            Value::String("3".into())
        );
        let description_metadata = json!({
            "name": "Description",
            "required": false,
            "operations": ["set"],
            "schema": { "type": "string" }
        });
        let adf = jira_typed_field_json(
            "description",
            &description_metadata,
            &TrackerFieldValue::Text {
                value: "Verified".into(),
            },
        )
        .unwrap();
        assert_eq!(adf["type"], "doc");
        assert_eq!(adf["content"][0]["content"][0]["text"], "Verified");

        let arbitrary = json!({
            "type": "trackerFieldUpdate",
            "fieldId": "priority",
            "value": { "type": "raw", "value": { "id": "3" } }
        });
        assert!(serde_json::from_value::<DeliveryActionRequest>(arbitrary).is_err());
    }

    #[test]
    fn canonical_field_hash_ignores_object_member_order() {
        let left = json!({ "b": 2, "a": { "d": 4, "c": 3 } });
        let right = json!({ "a": { "c": 3, "d": 4 }, "b": 2 });
        assert_eq!(
            sha256(canonical_json_bytes(&left).unwrap()),
            sha256(canonical_json_bytes(&right).unwrap())
        );
    }

    fn export_preview() -> OpenSpecExportPreview {
        let proposal = "# Proposal\n".to_owned();
        let delta = "## ADDED Requirements\n".to_owned();
        OpenSpecExportPreview {
            destination: "openspec/changes/agentum-01k12345-example".into(),
            source_revision: "sha256:source".into(),
            files: vec![
                super::super::sources::OpenSpecExportFile {
                    relative_path: "proposal.md".into(),
                    content_hash: sha256(proposal.as_bytes()),
                    content: proposal,
                },
                super::super::sources::OpenSpecExportFile {
                    relative_path: "specs/example/spec.md".into(),
                    content_hash: sha256(delta.as_bytes()),
                    content: delta,
                },
            ],
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn openspec_export_is_atomic_idempotent_and_never_overwrites() {
        let worktree = tempfile::tempdir().unwrap();
        let export = export_preview();
        let published = publish_openspec_export(worktree.path(), &export).unwrap();
        assert_eq!(published["summary"], "OpenSpec export published");
        assert!(export_destination_matches(worktree.path(), &export).unwrap());
        let replay = publish_openspec_export(worktree.path(), &export).unwrap();
        assert_eq!(replay["summary"], "OpenSpec export already matches");

        let conflict_root = tempfile::tempdir().unwrap();
        let destination = conflict_root.path().join(&export.destination);
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("user-owned.txt"), "preserve\n").unwrap();
        assert!(publish_openspec_export(conflict_root.path(), &export).is_err());
        assert_eq!(
            std::fs::read_to_string(destination.join("user-owned.txt")).unwrap(),
            "preserve\n"
        );

        let racing_root = tempfile::tempdir().unwrap();
        let racing_destination = racing_root.path().join(&export.destination);
        let raced = publish_openspec_export_with_hook(racing_root.path(), &export, || {
            std::fs::create_dir(&racing_destination).unwrap();
            std::fs::write(racing_destination.join("racing-owner.txt"), "preserve\n").unwrap();
        });
        assert!(raced.is_err());
        assert_eq!(
            std::fs::read_to_string(racing_destination.join("racing-owner.txt")).unwrap(),
            "preserve\n"
        );
        assert!(
            std::fs::read_dir(racing_destination.parent().unwrap())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".agentum-export-"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn openspec_export_refuses_a_symlink_destination() {
        use std::os::unix::fs::symlink;
        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let destination = worktree
            .path()
            .join("openspec/changes/agentum-01k12345-example");
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        symlink(outside.path(), &destination).unwrap();
        assert!(publish_openspec_export(worktree.path(), &export_preview()).is_err());
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn openspec_export_fails_closed_when_open_parent_is_swapped_to_symlink() {
        use std::os::unix::fs::symlink;

        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(worktree.path().join("openspec/changes")).unwrap();
        std::fs::create_dir_all(outside.path().join("changes")).unwrap();
        let openspec = worktree.path().join("openspec");
        let original = worktree.path().join("openspec-original");

        let result = publish_openspec_export_with_hook(worktree.path(), &export_preview(), || {
            std::fs::rename(&openspec, &original).unwrap();
            symlink(outside.path(), &openspec).unwrap();
        });
        assert!(result.is_err());
        assert!(
            std::fs::read_dir(outside.path().join("changes"))
                .unwrap()
                .next()
                .is_none()
        );
        assert!(
            std::fs::read_dir(original.join("changes"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".agentum-export-"))
        );
        assert!(!original.join("changes/agentum-01k12345-example").exists());

        std::fs::remove_file(&openspec).unwrap();
        std::fs::rename(&original, &openspec).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn openspec_export_rejects_windows_reparse_or_non_directory_parent() {
        use std::os::windows::fs::symlink_dir;

        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(worktree.path().join("openspec/changes")).unwrap();
        std::fs::create_dir_all(outside.path().join("changes")).unwrap();
        let openspec = worktree.path().join("openspec");
        let original = worktree.path().join("openspec-original");
        std::fs::rename(&openspec, &original).unwrap();
        let directory_link = symlink_dir(outside.path(), &openspec).is_ok();
        if !directory_link {
            std::fs::write(&openspec, "unsafe replacement").unwrap();
        }
        assert!(publish_openspec_export(worktree.path(), &export_preview()).is_err());
        assert!(
            std::fs::read_dir(outside.path().join("changes"))
                .unwrap()
                .next()
                .is_none()
        );
        if directory_link {
            std::fs::remove_dir(&openspec).unwrap();
        } else {
            std::fs::remove_file(&openspec).unwrap();
        }
        std::fs::rename(&original, &openspec).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn openspec_export_windows_handle_blocks_or_detects_parent_swap() {
        use std::cell::Cell;
        use std::os::windows::fs::symlink_dir;

        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(worktree.path().join("openspec/changes")).unwrap();
        std::fs::create_dir_all(outside.path().join("changes")).unwrap();
        let openspec = worktree.path().join("openspec");
        let original = worktree.path().join("openspec-original");
        let swapped = Cell::new(false);
        let swap_denied = Cell::new(false);
        let directory_link = Cell::new(false);

        let result = publish_openspec_export_with_hook(worktree.path(), &export_preview(), || {
            if std::fs::rename(&openspec, &original).is_err() {
                // cap-primitives opens Windows directories without
                // FILE_SHARE_DELETE, so the kernel normally blocks this race.
                swap_denied.set(true);
                return;
            }
            swapped.set(true);
            if symlink_dir(outside.path(), &openspec).is_ok() {
                directory_link.set(true);
            } else {
                std::fs::write(&openspec, "unsafe replacement").unwrap();
            }
        });
        assert!(swap_denied.get() || (swapped.get() && result.is_err()));
        assert!(
            std::fs::read_dir(outside.path().join("changes"))
                .unwrap()
                .next()
                .is_none()
        );
        if swapped.get() {
            assert!(
                std::fs::read_dir(original.join("changes"))
                    .unwrap()
                    .all(|entry| !entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".agentum-export-"))
            );
            if directory_link.get() {
                std::fs::remove_dir(&openspec).unwrap();
            } else {
                std::fs::remove_file(&openspec).unwrap();
            }
            std::fs::rename(&original, &openspec).unwrap();
        }
    }
}
