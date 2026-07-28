//! Provider-neutral contracts for Agentum's specification-driven workflow.
//!
//! Runtime state belongs to Agentum, never to an interactive terminal or a
//! mutable repository index. These types are shared by persistence, HTTP, and
//! desktop clients so phase/status terminology cannot drift between surfaces.

use std::fmt;
use std::path::{Component, Path};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

pub const ARTIFACT_FORMAT: &str = "agentum-sdd";
pub const SCHEMA_VERSION: u32 = 1;

/// Stable public identity. The cosmetic directory slug is deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SpecId(String);

impl SpecId {
    pub fn new() -> Self {
        Self(format!("SPC-{}", Ulid::new()))
    }

    pub fn ulid(&self) -> &str {
        &self.0[4..]
    }

    pub fn directory_name(&self, title: &str) -> String {
        format!(
            "spc-{}-{}",
            self.ulid().to_ascii_lowercase(),
            slugify(title)
        )
    }

    pub fn branch_name(&self, title: &str) -> String {
        format!(
            "agentum/spc-{}-{}",
            self.ulid().to_ascii_lowercase(),
            slugify(title)
        )
    }
}

impl Default for SpecId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SpecId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<SpecId> for String {
    fn from(value: SpecId) -> Self {
        value.0
    }
}

impl TryFrom<String> for SpecId {
    type Error = SddContractError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl FromStr for SpecId {
    type Err = SddContractError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some(raw) = value.strip_prefix("SPC-") else {
            return Err(SddContractError::InvalidSpecId);
        };
        if raw.len() != 26 || raw != raw.to_ascii_uppercase() || raw.parse::<Ulid>().is_err() {
            return Err(SddContractError::InvalidSpecId);
        }
        Ok(Self(value.to_owned()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SddContractError {
    #[error("spec id must be SPC- followed by a 26-character uppercase ULID")]
    InvalidSpecId,
    #[error("artifact path must be non-empty, relative, UTF-8, and traversal-free")]
    UnsafeRelativePath,
}

pub fn slugify(value: &str) -> String {
    let mut out = String::with_capacity(value.len().min(64));
    let mut separator = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            if separator && !out.is_empty() {
                out.push('-');
            }
            separator = false;
            if out.len() < 48 {
                out.push(ch);
            }
        } else {
            separator = true;
        }
    }
    out.trim_matches('-')
        .to_owned()
        .chars()
        .take(48)
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
        .pipe(|slug| if slug.is_empty() { "spec".into() } else { slug })
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

pub fn validate_relative_path(value: &str) -> Result<(), SddContractError> {
    if value.is_empty() || Path::new(value).is_absolute() {
        return Err(SddContractError::UnsafeRelativePath);
    }
    for component in Path::new(value).components() {
        match component {
            Component::Normal(part) if part.to_str().is_some() => {}
            _ => return Err(SddContractError::UnsafeRelativePath),
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Specification,
    Design,
    Planning,
    Implementation,
    Verification,
    Review,
    Ready,
    Delivery,
    Completed,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Specification => "specification",
            Self::Design => "design",
            Self::Planning => "planning",
            Self::Implementation => "implementation",
            Self::Verification => "verification",
            Self::Review => "review",
            Self::Ready => "ready",
            Self::Delivery => "delivery",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Idle,
    Queued,
    Running,
    Waiting,
    RetryScheduled,
    Pausing,
    Paused,
    Blocked,
    Canceling,
    Canceled,
    Failed,
    Succeeded,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::RetryScheduled => "retry_scheduled",
            Self::Pausing => "pausing",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::Canceling => "canceling",
            Self::Canceled => "canceled",
            Self::Failed => "failed",
            Self::Succeeded => "succeeded",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowProfile {
    Standard,
    HighRisk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowControl {
    Guarded,
    Interactive,
    Autopilot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Specification,
    Design,
    Plan,
    Decisions,
    Review,
}

impl ArtifactKind {
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Specification => "spec.md",
            Self::Design => "design.md",
            Self::Plan => "plan.json",
            Self::Decisions => "decisions.md",
            Self::Review => "review.md",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactManifest {
    pub format: String,
    pub schema_version: u32,
    pub artifact_set_id: Ulid,
}

impl ArtifactManifest {
    pub fn new() -> Self {
        Self {
            format: ARTIFACT_FORMAT.into(),
            schema_version: SCHEMA_VERSION,
            artifact_set_id: Ulid::new(),
        }
    }

    pub fn validate(&self) -> bool {
        self.format == ARTIFACT_FORMAT && self.schema_version == SCHEMA_VERSION
    }
}

impl Default for ArtifactManifest {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalReference {
    pub provider: String,
    pub connection_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_id: Option<String>,
    pub external_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub url: String,
    pub source_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub env_allowlist: Vec<String>,
    pub timeout_ms: u64,
    pub output_limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserWaitUntil {
    Load,
    DomContentLoaded,
    NetworkIdle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserViewport {
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_device_scale_milli")]
    pub device_scale_milli: u32,
}

const fn default_device_scale_milli() -> u32 {
    1_000
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum BrowserCheckAssertion {
    PageLoaded { id: String, expected_status: u16 },
    TextPresent { id: String, text: String },
    SelectorVisible { id: String, selector: String },
    UrlContains { id: String, value: String },
}

impl BrowserCheckAssertion {
    pub fn id(&self) -> &str {
        match self {
            Self::PageLoaded { id, .. }
            | Self::TextPresent { id, .. }
            | Self::SelectorVisible { id, .. }
            | Self::UrlContains { id, .. } => id,
        }
    }
}

/// Immutable browser-verification intent. Execution and captured evidence are
/// runtime state; only this bounded, provider-neutral check belongs in plan.json.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserCheck {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    pub wait_until: BrowserWaitUntil,
    pub viewport: BrowserViewport,
    pub timeout_ms: u64,
    pub assertions: Vec<BrowserCheckAssertion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanArtifact {
    pub schema_version: u32,
    pub spec_id: SpecId,
    pub spec_revision: i64,
    pub tasks: Vec<PlanTask>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanTask {
    pub id: String,
    pub objective: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub read_scopes: Vec<String>,
    #[serde(default)]
    pub write_scopes: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub verification: Vec<CommandSpec>,
    #[serde(default)]
    pub browser_checks: Vec<BrowserCheck>,
    pub risk: String,
    pub parallel_safe: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_id_round_trips_and_slug_is_not_identity() {
        let id = SpecId::new();
        assert_eq!(id.to_string().len(), 30);
        assert_eq!(id.to_string().parse::<SpecId>().unwrap(), id);
        assert!(
            id.directory_name("Refresh Access Tokens!")
                .ends_with("-refresh-access-tokens")
        );
        assert_eq!(id, id.to_string().parse().unwrap());
    }

    #[test]
    fn rejects_lowercase_or_non_ulid_identity() {
        assert!("spc-01ARZ3NDEKTSV4RRFFQ69G5FAV".parse::<SpecId>().is_err());
        assert!("SPC-not-an-ulid-not-an-ulid!!".parse::<SpecId>().is_err());
    }

    #[test]
    fn artifact_paths_are_relative_and_traversal_free() {
        assert!(validate_relative_path("src/auth/token.rs").is_ok());
        for bad in ["", "/etc/passwd", "../secret", "a/../../b", "./spec.md"] {
            assert!(validate_relative_path(bad).is_err(), "accepted {bad:?}");
        }
    }
}
