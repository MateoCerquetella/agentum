//! SDD-specific provider parity contract. Interactive terminal adapters are
//! intentionally not reused: they own a TTY, while this transport accepts a
//! bounded prompt and returns a typed artifact or disposable-worktree diff.

use agentum_core::sdd::CommandSpec;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tokio::io::AsyncReadExt;

pub const SPEC_BEGIN: &str = "AGENTUM_SPEC_BEGIN";
pub const SPEC_END: &str = "AGENTUM_SPEC_END";
pub const DESIGN_BEGIN: &str = "AGENTUM_DESIGN_BEGIN";
pub const DESIGN_END: &str = "AGENTUM_DESIGN_END";
pub const PLAN_BEGIN: &str = "AGENTUM_PLAN_BEGIN";
pub const PLAN_END: &str = "AGENTUM_PLAN_END";
pub const DIFF_BEGIN: &str = "AGENTUM_DIFF_BEGIN";
pub const DIFF_END: &str = "AGENTUM_DIFF_END";
pub const REVIEW_BEGIN: &str = "AGENTUM_REVIEW_BEGIN";
pub const REVIEW_END: &str = "AGENTUM_REVIEW_END";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderTransport {
    StructuredJson,
    JsonEvents,
    Acp,
    HarvestArtifact,
    HarvestDiff,
}

/// The source Agentum is allowed to harvest after a provider exits. Keeping
/// this separate from the provider's wire transport prevents an adapter from
/// silently falling back to ambient stdout when its manifest promised a fixed
/// staging artifact (or vice versa).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderResultTransport {
    Stdout,
    StagingArtifact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCancellation {
    ProcessGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderIsolation {
    OsSandboxDisposableWorktree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderOperation {
    Authoring,
    Design,
    Planning,
    ImplementationDiff,
    Review,
}

impl ProviderOperation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Authoring => "authoring",
            Self::Design => "design",
            Self::Planning => "planning",
            Self::ImplementationDiff => "implementation_diff",
            Self::Review => "independent_review",
        }
    }

    fn max_turns(self) -> &'static str {
        match self {
            Self::Authoring => "8",
            Self::Design | Self::Planning => "10",
            Self::ImplementationDiff => "16",
            Self::Review => "12",
        }
    }
}

const REQUIRED_OPERATIONS: [ProviderOperation; 5] = [
    ProviderOperation::Authoring,
    ProviderOperation::Design,
    ProviderOperation::Planning,
    ProviderOperation::ImplementationDiff,
    ProviderOperation::Review,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderDescriptor {
    pub id: String,
    pub version_probe: Vec<String>,
    pub capabilities: Vec<ProviderOperation>,
    pub transport: ProviderTransport,
    pub result_transport: ProviderResultTransport,
    pub cancellation: ProviderCancellation,
    pub isolation: ProviderIsolation,
    pub timeout_ms: u64,
    pub output_limit: usize,
}

impl ProviderDescriptor {
    pub fn supports(&self, operation: ProviderOperation) -> bool {
        self.capabilities.contains(&operation)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapability {
    #[serde(flatten)]
    pub descriptor: ProviderDescriptor,
    pub available: bool,
    pub version: Option<String>,
    pub reason: Option<String>,
}

pub const WINDOWS_LOCAL_SDD_REASON_CODE: &str = "windows_agentum_sandbox_unavailable";
pub const WINDOWS_LOCAL_SDD_REASON: &str = "Windows local SDD is disabled because Agentum does not provide a restricted-token/AppContainer filesystem sandbox; provider-native sandbox flags and process-tree cancellation are not isolation.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderExecutionBoundary {
    LocalSandboxed,
    RemoteClientOnly,
    Unavailable,
}

/// Agentum's platform-owned provider execution boundary. Provider-declared
/// flags and process-tree cancellation are intentionally absent from this
/// decision: neither can constrain filesystem access by the provider process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderIsolationCapability {
    pub available: bool,
    pub boundary: ProviderExecutionBoundary,
    pub mechanism: Option<&'static str>,
    pub reason_code: Option<&'static str>,
    pub reason: Option<&'static str>,
}

pub trait SddProviderAdapter: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;
    fn phase_command(
        &self,
        operation: ProviderOperation,
        cwd: &str,
        prompt: &str,
        staging_path: &str,
        sandbox_dir: &str,
    ) -> CommandSpec;

    fn authoring_command(
        &self,
        cwd: &str,
        prompt: &str,
        staging_path: &str,
        sandbox_dir: &str,
    ) -> CommandSpec {
        self.phase_command(
            ProviderOperation::Authoring,
            cwd,
            prompt,
            staging_path,
            sandbox_dir,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BundledProvider {
    id: &'static str,
}

impl BundledProvider {
    pub fn get(id: &str) -> Option<Self> {
        BUNDLED_IDS
            .iter()
            .copied()
            .find(|candidate| {
                *candidate == id
                    || (matches!(id, "cursor" | "cursor-agent") && *candidate == "agent")
            })
            .map(|id| Self { id })
    }
}

pub const BUNDLED_IDS: &[&str] = &[
    "claude", "codex", "agent", "gemini", "hermes", "opencode", "aider",
];

const CURSOR_EXECUTABLE_CANDIDATES: [&str; 2] = ["agent", "cursor-agent"];

#[derive(Debug, Clone, Copy)]
struct ProviderProjectInputPolicy {
    directories: &'static [&'static str],
    root_files: &'static [&'static str],
    recursive_rule_files: &'static [&'static str],
}

/// Provider-owned files that may be loaded implicitly before the typed SDD
/// prompt. These are hidden from an attempt, while ordinary repository source
/// remains readable. User-level configuration is separately hidden by the
/// sandbox HOME and only the provider's credential leaf is mounted back.
fn provider_project_input_policy(provider_id: &str) -> ProviderProjectInputPolicy {
    match provider_id {
        "claude" => ProviderProjectInputPolicy {
            directories: &[".claude"],
            root_files: &[".mcp.json"],
            recursive_rule_files: &["CLAUDE.md", "CLAUDE.local.md"],
        },
        "codex" => ProviderProjectInputPolicy {
            directories: &[".codex"],
            root_files: &[],
            recursive_rule_files: &["AGENTS.md", "AGENTS.override.md"],
        },
        "agent" => ProviderProjectInputPolicy {
            directories: &[".cursor"],
            root_files: &[
                ".cursorrules",
                ".cursorignore",
                ".cursorindexingignore",
                "mcp.json",
            ],
            recursive_rule_files: &["AGENTS.md", "CLAUDE.md"],
        },
        "gemini" => ProviderProjectInputPolicy {
            directories: &[".gemini"],
            root_files: &[".env", ".geminiignore"],
            recursive_rule_files: &["GEMINI.md", ".agentum-provider-context-disabled"],
        },
        "hermes" => ProviderProjectInputPolicy {
            directories: &[".hermes", ".claude", ".cursor"],
            root_files: &[".hermes.md", ".cursorrules"],
            recursive_rule_files: &["HERMES.md", "AGENTS.md", "CLAUDE.md", "SOUL.md"],
        },
        "opencode" => ProviderProjectInputPolicy {
            directories: &[".opencode", ".claude"],
            root_files: &["opencode.json", "opencode.jsonc", ".env"],
            recursive_rule_files: &["AGENTS.md", "CLAUDE.md"],
        },
        // Aider searches its documented defaults in addition to explicitly
        // supplied model/env paths, so the project defaults are masked too.
        // It has no implicit conventions filename: conventions are opt-in
        // through --read or config, and config is pinned to an empty file.
        "aider" => ProviderProjectInputPolicy {
            directories: &[],
            root_files: &[
                ".aider.conf.yml",
                ".aiderignore",
                ".aider.model.settings.yml",
                ".aider.model.metadata.json",
                ".env",
            ],
            recursive_rule_files: &[],
        },
        // Custom adapters pass the same conformance boundary. Agentum cannot
        // infer their underlying CLI, so hide the union of supported provider
        // conventions instead of silently exposing one bundled provider's
        // project configuration through a wrapper executable.
        _ => ProviderProjectInputPolicy {
            directories: &[
                ".claude",
                ".codex",
                ".cursor",
                ".gemini",
                ".hermes",
                ".opencode",
            ],
            root_files: &[
                ".mcp.json",
                ".cursorrules",
                ".cursorignore",
                ".cursorindexingignore",
                "mcp.json",
                ".env",
                ".geminiignore",
                ".hermes.md",
                "opencode.json",
                "opencode.jsonc",
                ".aider.conf.yml",
                ".aiderignore",
                ".aider.model.settings.yml",
                ".aider.model.metadata.json",
            ],
            recursive_rule_files: &[
                "AGENTS.md",
                "AGENTS.override.md",
                "CLAUDE.md",
                "CLAUDE.local.md",
                "GEMINI.md",
                "HERMES.md",
                "SOUL.md",
            ],
        },
    }
}

fn resolve_bundled_executable_with(
    provider_id: &str,
    mut locate: impl FnMut(&str) -> Option<PathBuf>,
) -> Option<(String, PathBuf)> {
    if provider_id == "agent" {
        return CURSOR_EXECUTABLE_CANDIDATES
            .iter()
            .find_map(|candidate| locate(candidate).map(|path| ((*candidate).into(), path)));
    }
    locate(provider_id).map(|path| (provider_id.into(), path))
}

fn resolve_bundled_executable(provider_id: &str) -> Option<(String, PathBuf)> {
    resolve_bundled_executable_with(provider_id, |candidate| which::which(candidate).ok())
}

fn bundled_program(provider_id: &str) -> String {
    resolve_bundled_executable(provider_id)
        .map(|(program, _)| program)
        .unwrap_or_else(|| provider_id.into())
}

impl SddProviderAdapter for BundledProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        let transport = match self.id {
            "codex" | "agent" | "gemini" | "opencode" => ProviderTransport::JsonEvents,
            "claude" => ProviderTransport::StructuredJson,
            "aider" => ProviderTransport::HarvestDiff,
            _ => ProviderTransport::HarvestArtifact,
        };
        ProviderDescriptor {
            id: self.id.into(),
            version_probe: vec!["--version".into()],
            capabilities: REQUIRED_OPERATIONS.to_vec(),
            transport,
            result_transport: if self.id == "codex" {
                ProviderResultTransport::StagingArtifact
            } else {
                ProviderResultTransport::Stdout
            },
            cancellation: ProviderCancellation::ProcessGroup,
            isolation: ProviderIsolation::OsSandboxDisposableWorktree,
            timeout_ms: 20 * 60 * 1000,
            output_limit: 2 * 1024 * 1024,
        }
    }

    fn phase_command(
        &self,
        operation: ProviderOperation,
        cwd: &str,
        prompt: &str,
        staging_path: &str,
        sandbox_dir: &str,
    ) -> CommandSpec {
        let phase_prompt = format!(
            "Agentum SDD operation: {}. This invocation must return only the requested typed envelope.\n\n{prompt}",
            operation.name()
        );
        let cursor_program = (self.id == "agent").then(|| bundled_program(self.id));
        let aider_config = format!("{sandbox_dir}/aider.conf.yml");
        let aider_env = format!("{sandbox_dir}/aider.env");
        let aider_ignore = format!("{sandbox_dir}/aiderignore");
        let aider_model_settings = format!("{sandbox_dir}/aider.model.settings.yml");
        let aider_model_metadata = format!("{sandbox_dir}/aider.model.metadata.json");
        let (program, args) = match self.id {
            "claude" => (
                "claude",
                vec![
                    "-p",
                    &phase_prompt,
                    "--safe-mode",
                    "--no-session-persistence",
                    "--no-chrome",
                    "--tools",
                    "Read,Glob,Grep",
                    "--output-format",
                    "json",
                    "--permission-mode",
                    "plan",
                ],
            ),
            "codex" => (
                "codex",
                vec![
                    "exec",
                    "--json",
                    "--sandbox",
                    "read-only",
                    "--skip-git-repo-check",
                    "--ephemeral",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--color",
                    "never",
                    "--output-last-message",
                    staging_path,
                    &phase_prompt,
                ],
            ),
            "agent" => (
                cursor_program.as_deref().unwrap_or("agent"),
                vec![
                    "-p",
                    &phase_prompt,
                    "--mode",
                    "plan",
                    "--sandbox",
                    "enabled",
                    "--trust",
                    "--skip-worktree-setup",
                    "--output-format",
                    "json",
                ],
            ),
            "gemini" => (
                "gemini",
                vec![
                    "-p",
                    &phase_prompt,
                    "--output-format",
                    "json",
                    "--approval-mode",
                    "plan",
                    "--sandbox",
                    "--skip-trust",
                ],
            ),
            "hermes" => (
                "hermes",
                vec![
                    "chat",
                    "--safe-mode",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--quiet",
                    "--source",
                    "tool",
                    "--max-turns",
                    operation.max_turns(),
                    "--query",
                    &phase_prompt,
                ],
            ),
            "opencode" => (
                "opencode",
                vec!["--pure", "run", "--format", "json", &phase_prompt],
            ),
            "aider" => (
                "aider",
                vec![
                    "--message",
                    &phase_prompt,
                    "--yes-always",
                    "--dry-run",
                    "--no-git",
                    "--no-auto-commits",
                    "--no-gitignore",
                    "--no-auto-lint",
                    "--no-check-update",
                    "--no-analytics",
                    "--config",
                    &aider_config,
                    "--env-file",
                    &aider_env,
                    "--aiderignore",
                    &aider_ignore,
                    "--model-settings-file",
                    &aider_model_settings,
                    "--model-metadata-file",
                    &aider_model_metadata,
                ],
            ),
            _ => unreachable!("BundledProvider is constructed from BUNDLED_IDS"),
        };
        let descriptor = self.descriptor();
        let mut env_allowlist = vec!["PATH".into(), "HOME".into(), "USERPROFILE".into()];
        let provider_env: &[&str] = match self.id {
            "claude" => &["ANTHROPIC_API_KEY"],
            "codex" => &["OPENAI_API_KEY", "CODEX_ACCESS_TOKEN"],
            "agent" => &["CURSOR_API_KEY"],
            "gemini" => &["GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_CLOUD_PROJECT"],
            "hermes" => &["HERMES_API_KEY", "OPENROUTER_API_KEY", "DEEPSEEK_API_KEY"],
            "opencode" | "aider" => &[
                "ANTHROPIC_API_KEY",
                "OPENAI_API_KEY",
                "GEMINI_API_KEY",
                "OPENROUTER_API_KEY",
                "DEEPSEEK_API_KEY",
            ],
            _ => &[],
        };
        env_allowlist.extend(provider_env.iter().map(|key| (*key).to_owned()));
        CommandSpec {
            program: program.into(),
            args: args.into_iter().map(str::to_owned).collect(),
            cwd: cwd.into(),
            env_allowlist,
            timeout_ms: descriptor.timeout_ms,
            output_limit: descriptor.output_limit,
        }
    }
}

pub const CUSTOM_PROVIDER_SCHEMA_VERSION: u32 = 1;
pub const CUSTOM_PROVIDER_RECEIPT_SCHEMA_VERSION: u32 = 2;
pub const CUSTOM_PROVIDER_CONFORMANCE_SUITE: &str = "standard_guarded_v2";
const MAX_CUSTOM_PROVIDER_MANIFEST_BYTES: usize = 128 * 1024;
const MAX_CUSTOM_PROVIDER_ARGS: usize = 128;
const MAX_CUSTOM_PROVIDER_ENV: usize = 32;
const MAX_CUSTOM_PROVIDER_OUTPUT: usize = 8 * 1024 * 1024;

/// Operator-owned custom provider configuration. It lives below Agentum's
/// configuration directory and is never read from or written to a customer
/// repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomProviderManifest {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub program: String,
    pub args: Vec<String>,
    pub version_probe: Vec<String>,
    pub capabilities: Vec<ProviderOperation>,
    pub transport: ProviderTransport,
    pub result_transport: ProviderResultTransport,
    pub isolation: ProviderIsolation,
    pub cancellation: ProviderCancellation,
    pub timeout_ms: u64,
    pub output_limit: usize,
    #[serde(default)]
    pub env_allowlist: Vec<String>,
}

/// Agentum-owned receipt emitted only after the exact manifest passes the
/// complete provider conformance suite. The digest makes approval immutable:
/// changing one byte in the manifest requires conformance to run again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomProviderConformanceReceipt {
    pub schema_version: u32,
    pub provider_id: String,
    pub provider_version: String,
    pub suite: String,
    pub manifest_sha256: String,
    pub fixture_sha256: String,
    pub source_revision: String,
    pub completed_at_unix: i64,
    pub cases: Vec<CustomProviderConformanceEvidence>,
    pub signing_key_sha256: String,
    pub signature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomProviderConformanceCase {
    Authoring,
    GuardedApproval,
    Design,
    Planning,
    ImplementationDiff,
    Verification,
    IndependentReview,
    MalformedOutput,
    Cancellation,
    RestartRecovery,
    ReadyNoDelivery,
}

const REQUIRED_CONFORMANCE_CASES: [CustomProviderConformanceCase; 11] = [
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
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomProviderConformanceEvidence {
    pub case: CustomProviderConformanceCase,
    /// Digest of the redacted case transcript and immutable fixture inputs.
    pub evidence_sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CustomProviderError {
    #[error("custom provider reference must be custom:<id>")]
    InvalidReference,
    #[error("custom provider configuration is unsafe: {0}")]
    UnsafeConfiguration(String),
    #[error("custom provider manifest is invalid: {0}")]
    InvalidManifest(String),
    #[error("custom provider is not approved for the required conformance suite: {0}")]
    ConformanceRequired(String),
    #[error("custom provider configuration could not be read: {0}")]
    Read(String),
    #[error("custom provider JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("custom provider approval could not use secure credential storage")]
    Vault(#[from] super::credentials::VaultError),
    #[error("custom provider approval signature is invalid")]
    InvalidSignature,
}

#[derive(Debug, Clone)]
pub struct CustomProviderAdapter {
    descriptor: ProviderDescriptor,
    program: String,
    args: Vec<String>,
    env_allowlist: Vec<String>,
    pub version: String,
    pub manifest_sha256: String,
}

impl SddProviderAdapter for CustomProviderAdapter {
    fn descriptor(&self) -> ProviderDescriptor {
        self.descriptor.clone()
    }

    fn phase_command(
        &self,
        operation: ProviderOperation,
        cwd: &str,
        prompt: &str,
        staging_path: &str,
        sandbox_dir: &str,
    ) -> CommandSpec {
        let replace = |template: &str| {
            template
                .replace("{operation}", operation.name())
                .replace("{prompt}", prompt)
                .replace("{staging}", staging_path)
                .replace("{cwd}", cwd)
                .replace("{sandbox}", sandbox_dir)
        };
        CommandSpec {
            program: self.program.clone(),
            args: self.args.iter().map(|argument| replace(argument)).collect(),
            cwd: cwd.into(),
            env_allowlist: self.env_allowlist.clone(),
            timeout_ms: self.descriptor.timeout_ms,
            output_limit: self.descriptor.output_limit,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ProviderAdapter {
    Bundled(BundledProvider),
    Custom(CustomProviderAdapter),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderApprovalBinding {
    pub descriptor: ProviderDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
}

impl ProviderAdapter {
    pub fn approval_binding(&self) -> ProviderApprovalBinding {
        match self {
            Self::Bundled(provider) => ProviderApprovalBinding {
                descriptor: provider.descriptor(),
                adapter_version: None,
                manifest_sha256: None,
            },
            Self::Custom(provider) => ProviderApprovalBinding {
                descriptor: provider.descriptor(),
                adapter_version: Some(provider.version.clone()),
                manifest_sha256: Some(provider.manifest_sha256.clone()),
            },
        }
    }
}

impl SddProviderAdapter for ProviderAdapter {
    fn descriptor(&self) -> ProviderDescriptor {
        match self {
            Self::Bundled(provider) => provider.descriptor(),
            Self::Custom(provider) => provider.descriptor(),
        }
    }

    fn phase_command(
        &self,
        operation: ProviderOperation,
        cwd: &str,
        prompt: &str,
        staging_path: &str,
        sandbox_dir: &str,
    ) -> CommandSpec {
        match self {
            Self::Bundled(provider) => {
                provider.phase_command(operation, cwd, prompt, staging_path, sandbox_dir)
            }
            Self::Custom(provider) => {
                provider.phase_command(operation, cwd, prompt, staging_path, sandbox_dir)
            }
        }
    }
}

/// Resolve a bundled provider or an approved custom provider. Custom lookup is
/// deliberately rooted in Agentum's configuration directory.
pub fn resolve_provider(reference: &str) -> Result<ProviderAdapter, CustomProviderError> {
    if let Some(provider) = BundledProvider::get(reference) {
        return Ok(ProviderAdapter::Bundled(provider));
    }
    load_custom_provider(reference).map(ProviderAdapter::Custom)
}

pub fn custom_provider_directory() -> Result<PathBuf, CustomProviderError> {
    agentum_store::paths::config_dir()
        .map(|root| root.join("sdd-providers"))
        .map_err(|error| CustomProviderError::Read(error.to_string()))
}

pub fn load_custom_provider(reference: &str) -> Result<CustomProviderAdapter, CustomProviderError> {
    let vault = default_conformance_vault();
    load_custom_provider_from_directory_with_vault(
        &custom_provider_directory()?,
        reference,
        vault.as_ref(),
    )
}

/// Directory-taking variant used by the conformance runner and tests. It has
/// the same no-follow reads and name containment checks as the production
/// loader.
pub fn load_custom_provider_from_directory(
    directory: &Path,
    reference: &str,
) -> Result<CustomProviderAdapter, CustomProviderError> {
    let vault = default_conformance_vault();
    load_custom_provider_from_directory_with_vault(directory, reference, vault.as_ref())
}

/// Explicit-vault loader used by the conformance runner and security tests.
/// Production callers use the platform vault selected above; accepting the
/// vault as a dependency here keeps tests from touching a developer keyring.
pub(crate) fn load_custom_provider_from_directory_with_vault(
    directory: &Path,
    reference: &str,
    vault: &dyn super::credentials::SddCredentialVault,
) -> Result<CustomProviderAdapter, CustomProviderError> {
    let id = parse_custom_reference(reference)?;
    let manifest_path = directory.join(format!("{id}.json"));
    let receipt_path = directory.join(format!("{id}.approval.json"));
    let manifest_bytes = read_custom_configuration(&manifest_path)?;
    let adapter = validate_custom_provider_manifest(&manifest_bytes, id)?;
    let receipt_bytes = read_custom_configuration(&receipt_path).map_err(|error| {
        CustomProviderError::ConformanceRequired(format!("{} ({error})", receipt_path.display()))
    })?;
    let receipt: CustomProviderConformanceReceipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|error| CustomProviderError::ConformanceRequired(error.to_string()))?;
    validate_conformance_receipt(&receipt, &adapter, vault)?;
    Ok(adapter)
}

fn default_conformance_vault() -> std::sync::Arc<dyn super::credentials::SddCredentialVault> {
    if std::env::var_os("AGENTUM_SDD_VAULT_MASTER_KEY").is_some() {
        super::credentials::headless_vault_or_unavailable()
    } else {
        std::sync::Arc::new(super::credentials::OsCredentialVault::new())
    }
}

/// Parse and validate a manifest without approving it. Conformance tooling
/// uses this function before executing its deterministic lifecycle cases.
pub fn validate_custom_provider_manifest(
    bytes: &[u8],
    expected_id: &str,
) -> Result<CustomProviderAdapter, CustomProviderError> {
    if bytes.len() > MAX_CUSTOM_PROVIDER_MANIFEST_BYTES {
        return Err(CustomProviderError::InvalidManifest(format!(
            "manifest exceeds {MAX_CUSTOM_PROVIDER_MANIFEST_BYTES} bytes"
        )));
    }
    let manifest: CustomProviderManifest = serde_json::from_slice(bytes)?;
    validate_manifest_fields(&manifest, expected_id)?;
    let manifest_sha256 = super::sha256(bytes);
    let mut env_allowlist = vec!["PATH".into(), "HOME".into(), "USERPROFILE".into()];
    env_allowlist.extend(manifest.env_allowlist.iter().cloned());
    env_allowlist.sort();
    env_allowlist.dedup();
    let adapter = CustomProviderAdapter {
        descriptor: ProviderDescriptor {
            id: format!("custom:{}", manifest.id),
            version_probe: manifest.version_probe,
            capabilities: manifest.capabilities,
            transport: manifest.transport,
            result_transport: manifest.result_transport,
            cancellation: manifest.cancellation,
            isolation: manifest.isolation,
            timeout_ms: manifest.timeout_ms,
            output_limit: manifest.output_limit,
        },
        program: manifest.program,
        args: manifest.args,
        env_allowlist,
        version: manifest.version,
        manifest_sha256,
    };
    validate_provider_contract(&adapter)
        .map_err(|error| CustomProviderError::InvalidManifest(error.to_string()))?;
    Ok(adapter)
}

/// Credential-free conformance gate shared by bundled and custom adapters.
/// It verifies the complete Standard + Guarded operation surface and the
/// direct-argv, bounded execution contract without contacting a model.
pub fn validate_provider_contract(adapter: &dyn SddProviderAdapter) -> Result<(), ProviderError> {
    const CWD: &str = "/agentum/conformance/authoritative";
    const STAGING: &str = "/agentum/conformance/authoritative/.agentum/staging/result";
    const SANDBOX: &str = "/agentum/conformance/provider";
    const PROMPT_SENTINEL: &str = "AGENTUM_CONFORMANCE_PROMPT_7d8f7a";
    let descriptor = adapter.descriptor();
    if descriptor.id.trim().is_empty()
        || descriptor.version_probe.is_empty()
        || descriptor.timeout_ms == 0
        || descriptor.output_limit == 0
    {
        return Err(ProviderError::InvalidCommand(
            "descriptor has an empty identity, version probe, or bound".into(),
        ));
    }
    let declared: HashSet<_> = descriptor.capabilities.iter().copied().collect();
    if declared.len() != descriptor.capabilities.len()
        || REQUIRED_OPERATIONS
            .iter()
            .any(|operation| !declared.contains(operation))
    {
        return Err(ProviderError::InvalidCommand(
            "descriptor does not declare every lifecycle capability exactly once".into(),
        ));
    }
    for operation in REQUIRED_OPERATIONS {
        let command = adapter.phase_command(operation, CWD, PROMPT_SENTINEL, STAGING, SANDBOX);
        validate_command_contract(&command, &descriptor, CWD)?;
        if is_shell_program(&command.program) {
            return Err(ProviderError::InvalidCommand(
                "provider command resolves to a shell".into(),
            ));
        }
        if !command
            .args
            .iter()
            .any(|argument| argument.contains(operation.name()))
            || !command
                .args
                .iter()
                .any(|argument| argument.contains(PROMPT_SENTINEL))
        {
            return Err(ProviderError::InvalidCommand(format!(
                "{} command does not bind its operation and prompt",
                operation.name()
            )));
        }
        if descriptor.result_transport == ProviderResultTransport::StagingArtifact
            && !command
                .args
                .iter()
                .any(|argument| argument.contains(STAGING))
        {
            return Err(ProviderError::InvalidCommand(
                "staging result transport does not bind the staging path".into(),
            ));
        }
        if command.args.iter().any(|argument| {
            matches!(
                argument.as_str(),
                "--yolo"
                    | "--force"
                    | "--auto"
                    | "--dangerously-skip-permissions"
                    | "danger-full-access"
                    | "workspace-write"
            )
        }) {
            return Err(ProviderError::InvalidCommand(
                "provider command requests an unsafe execution mode".into(),
            ));
        }
    }
    Ok(())
}

/// Assemble a receipt from evidence emitted by the conformance runner. This
/// function does not invent passing evidence: every mandatory case must be
/// supplied exactly once with a valid transcript digest.
pub(crate) fn signed_conformance_receipt_for(
    adapter: &CustomProviderAdapter,
    cases: Vec<CustomProviderConformanceEvidence>,
    fixture_sha256: &str,
    source_revision: &str,
    vault: &dyn super::credentials::SddCredentialVault,
) -> Result<CustomProviderConformanceReceipt, CustomProviderError> {
    validate_conformance_cases(&cases)?;
    if !valid_sha256(fixture_sha256)
        || source_revision.is_empty()
        || source_revision.len() > 128
        || source_revision.chars().any(char::is_control)
    {
        return Err(CustomProviderError::ConformanceRequired(
            "fixture or source revision binding is invalid".into(),
        ));
    }
    let key_pair = conformance_signing_key(vault, true)?;
    let mut receipt = CustomProviderConformanceReceipt {
        schema_version: CUSTOM_PROVIDER_RECEIPT_SCHEMA_VERSION,
        provider_id: adapter
            .descriptor
            .id
            .strip_prefix("custom:")
            .unwrap_or(&adapter.descriptor.id)
            .into(),
        provider_version: adapter.version.clone(),
        suite: CUSTOM_PROVIDER_CONFORMANCE_SUITE.into(),
        manifest_sha256: adapter.manifest_sha256.clone(),
        fixture_sha256: fixture_sha256.into(),
        source_revision: source_revision.into(),
        completed_at_unix: time::OffsetDateTime::now_utc().unix_timestamp(),
        cases,
        signing_key_sha256: super::sha256(key_pair.public_key().as_ref()),
        signature: String::new(),
    };
    let signature = key_pair.sign(&conformance_receipt_payload(&receipt)?);
    receipt.signature = URL_SAFE_NO_PAD.encode(signature.as_ref());
    Ok(receipt)
}

fn parse_custom_reference(reference: &str) -> Result<&str, CustomProviderError> {
    let id = reference
        .strip_prefix("custom:")
        .ok_or(CustomProviderError::InvalidReference)?;
    if !valid_custom_id(id) {
        return Err(CustomProviderError::InvalidReference);
    }
    Ok(id)
}

fn valid_custom_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (byte == b'-' && index > 0 && index + 1 < id.len())
        })
}

fn read_custom_configuration(path: &Path) -> Result<Vec<u8>, CustomProviderError> {
    let (bytes, _) = super::artifacts::read_bytes(path)
        .map_err(|error| CustomProviderError::Read(error.to_string()))?;
    if bytes.len() > MAX_CUSTOM_PROVIDER_MANIFEST_BYTES {
        return Err(CustomProviderError::UnsafeConfiguration(format!(
            "{} exceeds {MAX_CUSTOM_PROVIDER_MANIFEST_BYTES} bytes",
            path.display()
        )));
    }
    Ok(bytes)
}

fn validate_manifest_fields(
    manifest: &CustomProviderManifest,
    expected_id: &str,
) -> Result<(), CustomProviderError> {
    let invalid = |message: &str| CustomProviderError::InvalidManifest(message.into());
    if manifest.schema_version != CUSTOM_PROVIDER_SCHEMA_VERSION {
        return Err(invalid("unsupported schemaVersion"));
    }
    if !valid_custom_id(&manifest.id) || manifest.id != expected_id {
        return Err(invalid("id is invalid or does not match its file name"));
    }
    if manifest.version.trim().is_empty()
        || manifest.version.len() > 128
        || manifest.version.chars().any(char::is_control)
    {
        return Err(invalid("version must be 1..128 characters"));
    }
    validate_program(&manifest.program)?;
    if manifest.args.is_empty() || manifest.args.len() > MAX_CUSTOM_PROVIDER_ARGS {
        return Err(invalid("args must contain 1..128 direct argv entries"));
    }
    for argument in &manifest.args {
        validate_template(argument)?;
    }
    let joined = manifest.args.join("\u{0}");
    if !joined.contains("{operation}") || !joined.contains("{prompt}") {
        return Err(invalid("args must bind {operation} and {prompt}"));
    }
    if manifest.result_transport == ProviderResultTransport::StagingArtifact
        && !joined.contains("{staging}")
    {
        return Err(invalid(
            "staging_artifact result transport requires a {staging} argument",
        ));
    }
    if manifest.version_probe.is_empty() || manifest.version_probe.len() > 32 {
        return Err(invalid(
            "versionProbe must contain 1..32 direct argv entries",
        ));
    }
    if manifest
        .version_probe
        .iter()
        .any(|argument| argument.is_empty() || argument.len() > 4096 || argument.contains('\0'))
    {
        return Err(invalid("versionProbe contains an invalid argv entry"));
    }
    let capabilities: HashSet<_> = manifest.capabilities.iter().copied().collect();
    if capabilities.len() != manifest.capabilities.len()
        || REQUIRED_OPERATIONS
            .iter()
            .any(|operation| !capabilities.contains(operation))
    {
        return Err(invalid(
            "capabilities must declare each required lifecycle operation exactly once",
        ));
    }
    if !(1_000..=3_600_000).contains(&manifest.timeout_ms) {
        return Err(invalid("timeoutMs must be between 1000 and 3600000"));
    }
    if !(1_024..=MAX_CUSTOM_PROVIDER_OUTPUT).contains(&manifest.output_limit) {
        return Err(invalid("outputLimit must be between 1024 and 8388608"));
    }
    if manifest.env_allowlist.len() > MAX_CUSTOM_PROVIDER_ENV {
        return Err(invalid("envAllowlist contains too many entries"));
    }
    let mut seen = HashSet::new();
    for key in &manifest.env_allowlist {
        if !valid_environment_name(key) || !seen.insert(key) || forbidden_environment_name(key) {
            return Err(invalid("envAllowlist contains an unsafe or duplicate name"));
        }
    }
    Ok(())
}

fn validate_program(program: &str) -> Result<(), CustomProviderError> {
    let invalid = |message: &str| CustomProviderError::InvalidManifest(message.into());
    if program.is_empty() || program.len() > 4096 || program.contains('\0') {
        return Err(invalid(
            "program must be a bounded executable name or absolute path",
        ));
    }
    let path = Path::new(program);
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) || (path.components().count() > 1 && !path.is_absolute())
    {
        return Err(invalid("program path must be a bare name or absolute path"));
    }
    if is_shell_program(program) {
        return Err(invalid("shell programs are not valid provider transports"));
    }
    Ok(())
}

fn is_shell_program(program: &str) -> bool {
    let name = Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "sh" | "bash" | "zsh" | "fish" | "cmd" | "cmd.exe" | "powershell" | "pwsh"
    )
}

fn validate_template(template: &str) -> Result<(), CustomProviderError> {
    if template.is_empty() || template.len() > 16 * 1024 || template.contains('\0') {
        return Err(CustomProviderError::InvalidManifest(
            "args contains an invalid argv template".into(),
        ));
    }
    let mut cursor = 0;
    while cursor < template.len() {
        let rest = &template[cursor..];
        let open = rest.find('{');
        let close = rest.find('}');
        match (open, close) {
            (None, None) => break,
            (None, Some(_)) => {
                return Err(CustomProviderError::InvalidManifest(
                    "args contains an unmatched closing brace".into(),
                ));
            }
            (Some(open), Some(close)) if close < open => {
                return Err(CustomProviderError::InvalidManifest(
                    "args contains an unmatched closing brace".into(),
                ));
            }
            (Some(open), _) => {
                let after_open = cursor + open + 1;
                let Some(relative_close) = template[after_open..].find('}') else {
                    return Err(CustomProviderError::InvalidManifest(
                        "args contains an unterminated placeholder".into(),
                    ));
                };
                let close = after_open + relative_close;
                let placeholder = &template[after_open..close];
                if placeholder.contains('{')
                    || !matches!(
                        placeholder,
                        "operation" | "prompt" | "staging" | "cwd" | "sandbox"
                    )
                {
                    return Err(CustomProviderError::InvalidManifest(format!(
                        "unknown argv placeholder {{{placeholder}}}"
                    )));
                }
                cursor = close + 1;
            }
        }
    }
    Ok(())
}

fn valid_environment_name(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_uppercase() || (index > 0 && byte.is_ascii_digit())
        })
}

fn forbidden_environment_name(key: &str) -> bool {
    matches!(
        key,
        "BASH_ENV"
            | "ENV"
            | "_"
            | "IFS"
            | "SHELLOPTS"
            | "LD_PRELOAD"
            | "LD_LIBRARY_PATH"
            | "DYLD_INSERT_LIBRARIES"
            | "DYLD_LIBRARY_PATH"
            | "NODE_OPTIONS"
            | "PYTHONPATH"
            | "PYTHONINSPECT"
            | "PYTHONSTARTUP"
            | "PERL5OPT"
            | "RUBYOPT"
            | "GIT_CONFIG"
            | "GIT_CONFIG_SYSTEM"
            | "GIT_CONFIG_GLOBAL"
            | "GIT_DIR"
            | "GIT_WORK_TREE"
            | "GIT_EXEC_PATH"
            | "GIT_ASKPASS"
            | "SSH_ASKPASS"
            | "RUSTC_WRAPPER"
    )
}

fn validate_conformance_receipt(
    receipt: &CustomProviderConformanceReceipt,
    adapter: &CustomProviderAdapter,
    vault: &dyn super::credentials::SddCredentialVault,
) -> Result<(), CustomProviderError> {
    let expected_id = adapter
        .descriptor
        .id
        .strip_prefix("custom:")
        .unwrap_or(&adapter.descriptor.id);
    if receipt.schema_version != CUSTOM_PROVIDER_RECEIPT_SCHEMA_VERSION
        || receipt.provider_id != expected_id
        || receipt.provider_version != adapter.version
        || receipt.suite != CUSTOM_PROVIDER_CONFORMANCE_SUITE
        || receipt.manifest_sha256 != adapter.manifest_sha256
        || !valid_sha256(&receipt.fixture_sha256)
        || receipt.source_revision.is_empty()
        || receipt.source_revision.len() > 128
        || receipt.source_revision.chars().any(char::is_control)
        || receipt.completed_at_unix <= 0
        || !valid_sha256(&receipt.signing_key_sha256)
        || validate_conformance_cases(&receipt.cases).is_err()
    {
        return Err(CustomProviderError::ConformanceRequired(
            "receipt does not bind this exact manifest and suite".into(),
        ));
    }
    let key_pair = conformance_signing_key(vault, false)?;
    if super::sha256(key_pair.public_key().as_ref()) != receipt.signing_key_sha256 {
        return Err(CustomProviderError::InvalidSignature);
    }
    let signature = URL_SAFE_NO_PAD
        .decode(receipt.signature.as_bytes())
        .map_err(|_| CustomProviderError::InvalidSignature)?;
    UnparsedPublicKey::new(&ring::signature::ED25519, key_pair.public_key().as_ref())
        .verify(&conformance_receipt_payload(receipt)?, &signature)
        .map_err(|_| CustomProviderError::InvalidSignature)?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConformanceReceiptPayload<'a> {
    schema_version: u32,
    provider_id: &'a str,
    provider_version: &'a str,
    suite: &'a str,
    manifest_sha256: &'a str,
    fixture_sha256: &'a str,
    source_revision: &'a str,
    completed_at_unix: i64,
    cases: &'a [CustomProviderConformanceEvidence],
    signing_key_sha256: &'a str,
}

fn conformance_receipt_payload(
    receipt: &CustomProviderConformanceReceipt,
) -> Result<Vec<u8>, CustomProviderError> {
    Ok(serde_json::to_vec(&ConformanceReceiptPayload {
        schema_version: receipt.schema_version,
        provider_id: &receipt.provider_id,
        provider_version: &receipt.provider_version,
        suite: &receipt.suite,
        manifest_sha256: &receipt.manifest_sha256,
        fixture_sha256: &receipt.fixture_sha256,
        source_revision: &receipt.source_revision,
        completed_at_unix: receipt.completed_at_unix,
        cases: &receipt.cases,
        signing_key_sha256: &receipt.signing_key_sha256,
    })?)
}

fn conformance_signing_key(
    vault: &dyn super::credentials::SddCredentialVault,
    create: bool,
) -> Result<Ed25519KeyPair, CustomProviderError> {
    let bytes = match super::credentials::get_provider_conformance_signing_key(vault)? {
        Some(secret) => secret.expose().to_vec(),
        None if create => {
            let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
                .map_err(|_| CustomProviderError::InvalidSignature)?;
            super::credentials::put_provider_conformance_signing_key(vault, document.as_ref())?;
            document.as_ref().to_vec()
        }
        None => return Err(CustomProviderError::InvalidSignature),
    };
    Ed25519KeyPair::from_pkcs8(&bytes).map_err(|_| CustomProviderError::InvalidSignature)
}

fn validate_conformance_cases(
    cases: &[CustomProviderConformanceEvidence],
) -> Result<(), CustomProviderError> {
    let declared: HashSet<_> = cases.iter().map(|evidence| evidence.case).collect();
    let evidence_digests: HashSet<_> = cases
        .iter()
        .map(|evidence| evidence.evidence_sha256.as_str())
        .collect();
    if cases.len() != REQUIRED_CONFORMANCE_CASES.len()
        || declared.len() != cases.len()
        || evidence_digests.len() != cases.len()
        || REQUIRED_CONFORMANCE_CASES
            .iter()
            .any(|case| !declared.contains(case))
        || cases
            .iter()
            .any(|evidence| !valid_sha256(&evidence.evidence_sha256))
    {
        return Err(CustomProviderError::ConformanceRequired(
            "all required conformance cases need unique evidence digests".into(),
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

pub fn authoring_prompt(title: &str, goal: &str) -> String {
    format!(
        "You are the specification author for an Agentum-owned workflow. Read the repository in your current working directory. Do not edit files, run implementation, change Git state, configure a provider, commit, push, or contact a tracker. Author a neutral Markdown specification for the title {title:?} and goal {goal:?}. It must contain concrete Requirements and Acceptance criteria, with unique stable identifiers beginning RQ-001 and AC-001. Return only the Markdown body (no YAML/frontmatter) between literal lines {SPEC_BEGIN} and {SPEC_END}."
    )
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider could not start: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("provider timed out after {0} ms")]
    Timeout(u64),
    #[error("provider run was canceled")]
    Canceled,
    #[error("provider output exceeded {0} bytes")]
    OutputLimit(usize),
    #[error("provider exited unsuccessfully: {0}")]
    Failed(String),
    #[error("provider did not return a marked specification artifact")]
    MalformedOutput,
    #[error("provider staging artifact was unsafe: {0}")]
    UnsafeStaging(String),
    #[error("provider isolation is unavailable: {0}")]
    IsolationUnavailable(String),
    #[error("provider isolation setup failed: {0}")]
    IsolationSetup(String),
    #[error("provider does not support the requested operation: {0}")]
    UnsupportedOperation(&'static str),
    #[error("provider returned an invalid command contract: {0}")]
    InvalidCommand(String),
}

pub fn provider_isolation_capability() -> ProviderIsolationCapability {
    provider_isolation_capability_for_platform()
}

pub fn isolation_available() -> bool {
    provider_isolation_capability().available
}

#[cfg(target_os = "linux")]
fn provider_isolation_capability_for_platform() -> ProviderIsolationCapability {
    if which::which("bwrap").is_ok() {
        ProviderIsolationCapability {
            available: true,
            boundary: ProviderExecutionBoundary::LocalSandboxed,
            mechanism: Some("bubblewrap"),
            reason_code: None,
            reason: None,
        }
    } else {
        ProviderIsolationCapability {
            available: false,
            boundary: ProviderExecutionBoundary::Unavailable,
            mechanism: None,
            reason_code: Some("bubblewrap_unavailable"),
            reason: Some("Bubblewrap is required for Agentum provider execution on Linux."),
        }
    }
}

#[cfg(target_os = "macos")]
fn provider_isolation_capability_for_platform() -> ProviderIsolationCapability {
    if Path::new("/usr/bin/sandbox-exec").is_file() {
        ProviderIsolationCapability {
            available: true,
            boundary: ProviderExecutionBoundary::LocalSandboxed,
            mechanism: Some("macos_seatbelt"),
            reason_code: None,
            reason: None,
        }
    } else {
        ProviderIsolationCapability {
            available: false,
            boundary: ProviderExecutionBoundary::Unavailable,
            mechanism: None,
            reason_code: Some("macos_seatbelt_unavailable"),
            reason: Some("The macOS Seatbelt launcher is required for Agentum provider execution."),
        }
    }
}

#[cfg(target_os = "windows")]
fn provider_isolation_capability_for_platform() -> ProviderIsolationCapability {
    ProviderIsolationCapability {
        available: false,
        boundary: ProviderExecutionBoundary::RemoteClientOnly,
        mechanism: None,
        reason_code: Some(WINDOWS_LOCAL_SDD_REASON_CODE),
        reason: Some(WINDOWS_LOCAL_SDD_REASON),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn provider_isolation_capability_for_platform() -> ProviderIsolationCapability {
    ProviderIsolationCapability {
        available: false,
        boundary: ProviderExecutionBoundary::Unavailable,
        mechanism: None,
        reason_code: Some("unsupported_platform"),
        reason: Some("Agentum does not provide a provider filesystem sandbox for this platform."),
    }
}

fn require_provider_isolation() -> Result<(), ProviderError> {
    let capability = provider_isolation_capability();
    if capability.available {
        Ok(())
    } else {
        Err(ProviderError::IsolationUnavailable(
            capability
                .reason
                .unwrap_or("Agentum provider isolation is unavailable.")
                .to_owned(),
        ))
    }
}

pub async fn probe_provider(provider: BundledProvider) -> ProviderCapability {
    let descriptor = provider.descriptor();
    let isolation = provider_isolation_capability();
    if !isolation.available {
        return ProviderCapability {
            descriptor,
            available: false,
            version: None,
            reason: isolation.reason.map(str::to_owned),
        };
    }
    let executable = match resolve_bundled_executable(&descriptor.id) {
        Some((_, path)) => path,
        None => {
            let reason = if descriptor.id == "agent" {
                "neither agent nor cursor-agent is installed or on PATH".into()
            } else {
                format!(
                    "{} executable is not installed or is not on PATH",
                    descriptor.id
                )
            };
            return ProviderCapability {
                descriptor,
                available: false,
                version: None,
                reason: Some(reason),
            };
        }
    };
    let mut command = tokio::process::Command::new(&executable);
    command
        .args(&descriptor.version_probe)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    for key in ["PATH", "HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output =
        match tokio::time::timeout(std::time::Duration::from_secs(5), command.output()).await {
            Ok(Ok(output)) if output.status.success() => output,
            Ok(Ok(_)) => {
                return ProviderCapability {
                    descriptor,
                    available: false,
                    version: None,
                    reason: Some("provider version probe exited unsuccessfully".into()),
                };
            }
            Ok(Err(_)) => {
                return ProviderCapability {
                    descriptor,
                    available: false,
                    version: None,
                    reason: Some("provider version probe could not start".into()),
                };
            }
            Err(_) => {
                return ProviderCapability {
                    descriptor,
                    available: false,
                    version: None,
                    reason: Some("provider version probe timed out".into()),
                };
            }
        };
    let raw = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    let version = String::from_utf8_lossy(&raw[..raw.len().min(512)])
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let parsed = parse_version(&version);
    let minimum = minimum_version(&descriptor.id);
    if parsed.is_none_or(|found| found < minimum) {
        return ProviderCapability {
            descriptor,
            available: false,
            version: (!version.is_empty()).then_some(version),
            reason: Some(format!(
                "provider version is unsupported; minimum is {}.{}.{}",
                minimum[0], minimum[1], minimum[2]
            )),
        };
    }
    if let Err(reason) = probe_authentication(&descriptor.id, &executable).await {
        return ProviderCapability {
            descriptor,
            available: false,
            version: Some(version),
            reason: Some(reason),
        };
    }
    ProviderCapability {
        descriptor,
        available: true,
        version: Some(version),
        reason: None,
    }
}

/// Probe an approved custom provider without attempting model work. The
/// executable must report the version bound by its manifest; authentication is
/// provider-specific and is exercised by the conformance run itself.
pub async fn probe_custom_provider(
    reference: &str,
) -> Result<ProviderCapability, CustomProviderError> {
    let adapter = load_custom_provider(reference)?;
    let descriptor = adapter.descriptor();
    let isolation = provider_isolation_capability();
    if !isolation.available {
        return Ok(ProviderCapability {
            descriptor,
            available: false,
            version: None,
            reason: isolation.reason.map(str::to_owned),
        });
    }
    let executable = match which::which(&adapter.program) {
        Ok(path) => path,
        Err(_) => {
            return Ok(ProviderCapability {
                descriptor,
                available: false,
                version: None,
                reason: Some("custom provider executable is not installed or not on PATH".into()),
            });
        }
    };
    let mut command = tokio::process::Command::new(executable);
    command
        .args(&descriptor.version_probe)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    for key in &adapter.env_allowlist {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output =
        match tokio::time::timeout(std::time::Duration::from_secs(5), command.output()).await {
            Ok(Ok(output)) if output.status.success() => output,
            Ok(Ok(_)) => {
                return Ok(ProviderCapability {
                    descriptor,
                    available: false,
                    version: None,
                    reason: Some("custom provider version probe exited unsuccessfully".into()),
                });
            }
            Ok(Err(_)) => {
                return Ok(ProviderCapability {
                    descriptor,
                    available: false,
                    version: None,
                    reason: Some("custom provider version probe could not start".into()),
                });
            }
            Err(_) => {
                return Ok(ProviderCapability {
                    descriptor,
                    available: false,
                    version: None,
                    reason: Some("custom provider version probe timed out".into()),
                });
            }
        };
    if output.stdout.len().saturating_add(output.stderr.len()) > 64 * 1024 {
        return Ok(ProviderCapability {
            descriptor,
            available: false,
            version: None,
            reason: Some("custom provider version output exceeded 65536 bytes".into()),
        });
    }
    let mut bytes = output.stdout;
    bytes.extend_from_slice(&output.stderr);
    let reported = String::from_utf8_lossy(&bytes);
    if !reported.contains(&adapter.version) {
        return Ok(ProviderCapability {
            descriptor,
            available: false,
            version: None,
            reason: Some("custom provider version does not match its approved manifest".into()),
        });
    }
    Ok(ProviderCapability {
        descriptor,
        available: true,
        version: Some(adapter.version),
        reason: None,
    })
}

async fn probe_authentication(provider_id: &str, executable: &Path) -> Result<(), String> {
    let environment_authenticated = provider_auth_environment(provider_id)
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()));
    if environment_authenticated {
        return Ok(());
    }
    if provider_id == "gemini" {
        let account_root = platform_account_root()
            .ok_or_else(|| "provider authentication could not be verified".to_owned())?;
        let authenticated = [
            account_root.join(".gemini/oauth_creds.json"),
            account_root.join(".gemini/google_accounts.json"),
        ]
        .iter()
        .any(|path| regular_nonempty_file(path));
        return authenticated
            .then_some(())
            .ok_or_else(|| "provider is installed but is not authenticated".to_owned());
    }
    if provider_id == "aider" {
        return Err("provider is installed but no supported model credential is configured".into());
    }

    let args: &[&str] = match provider_id {
        "claude" => &["auth", "status", "--json"],
        "codex" => &["login", "status"],
        "agent" => &["status", "--format", "json"],
        "hermes" => &["auth", "list"],
        "opencode" => &["--pure", "auth", "list"],
        _ => return Err("provider authentication probe is not implemented".into()),
    };
    let mut command = tokio::process::Command::new(executable);
    command
        .args(args)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    for key in ["PATH", "HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), command.output())
        .await
        .map_err(|_| "provider authentication probe timed out".to_owned())?
        .map_err(|_| "provider authentication probe could not start".to_owned())?;
    if !output.status.success()
        || output.stdout.len().saturating_add(output.stderr.len()) > 64 * 1024
    {
        return Err("provider authentication could not be verified".into());
    }
    let mut bytes = output.stdout;
    bytes.extend_from_slice(&output.stderr);
    let authenticated = authentication_output_is_valid(provider_id, &bytes);
    authenticated
        .then_some(())
        .ok_or_else(|| "provider is installed but is not authenticated".to_owned())
}

fn authentication_output_is_valid(provider_id: &str, bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let lower = text.to_ascii_lowercase();
    match provider_id {
        "claude" => serde_json::from_slice::<serde_json::Value>(bytes)
            .ok()
            .and_then(|value| value.get("loggedIn").and_then(serde_json::Value::as_bool))
            .unwrap_or(false),
        "codex" => lower.contains("logged in"),
        "agent" => serde_json::from_slice::<serde_json::Value>(bytes)
            .ok()
            .and_then(|value| {
                value
                    .get("isAuthenticated")
                    .and_then(serde_json::Value::as_bool)
            })
            .unwrap_or(false),
        "hermes" | "opencode" => lower.contains(" credentials") && !lower.contains("0 credentials"),
        _ => false,
    }
}

fn provider_auth_environment(provider_id: &str) -> &'static [&'static str] {
    match provider_id {
        "claude" => &["ANTHROPIC_API_KEY"],
        "codex" => &["OPENAI_API_KEY", "CODEX_ACCESS_TOKEN"],
        "agent" => &["CURSOR_API_KEY"],
        "gemini" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        "hermes" => &["HERMES_API_KEY", "OPENROUTER_API_KEY"],
        "opencode" | "aider" => &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "OPENROUTER_API_KEY",
            "DEEPSEEK_API_KEY",
        ],
        _ => &[],
    }
}

fn platform_account_root() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn regular_nonempty_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() > 2
    })
}

fn minimum_version(id: &str) -> [u64; 3] {
    match id {
        "claude" => [2, 1, 220],
        "codex" => [0, 145, 0],
        "agent" => [2026, 7, 23],
        "gemini" => [0, 49, 0],
        "hermes" => [0, 18, 2],
        "opencode" => [1, 17, 13],
        "aider" => [0, 86, 2],
        _ => [u64::MAX, u64::MAX, u64::MAX],
    }
}

fn parse_version(value: &str) -> Option<[u64; 3]> {
    for token in value.split_whitespace() {
        let token = token.trim_start_matches(|value: char| !value.is_ascii_digit());
        let mut components = token.split('.');
        let parse = |component: &str| {
            let digits: String = component.chars().take_while(char::is_ascii_digit).collect();
            (!digits.is_empty())
                .then(|| digits.parse::<u64>().ok())
                .flatten()
        };
        let (Some(major), Some(minor), Some(patch)) = (
            components.next().and_then(parse),
            components.next().and_then(parse),
            components.next().and_then(parse),
        ) else {
            continue;
        };
        return Some([major, minor, patch]);
    }
    None
}

/// Run an SDD provider without a shell or ambient environment. The child owns
/// a process group so timeout cancellation terminates descendants as well.
pub async fn run_authoring(
    run_id: &str,
    adapter: &dyn SddProviderAdapter,
    cwd: &str,
    prompt: &str,
    staging_path: &str,
) -> Result<String, ProviderError> {
    run_artifact(
        run_id,
        adapter,
        ProviderOperation::Authoring,
        cwd,
        prompt,
        staging_path,
        SPEC_BEGIN,
        SPEC_END,
    )
    .await
}

/// Execute a provider and accept only content inside the phase's explicit
/// envelope. JSON/JSONL transports are traversed structurally; harvest-only
/// adapters may use the fixed staging artifact. Unmarked prose is rejected.
#[allow(clippy::too_many_arguments)]
pub async fn run_artifact(
    execution_id: &str,
    adapter: &dyn SddProviderAdapter,
    operation: ProviderOperation,
    cwd: &str,
    prompt: &str,
    staging_path: &str,
    begin_marker: &str,
    end_marker: &str,
) -> Result<String, ProviderError> {
    // This gate deliberately precedes descriptor lookup, staging-directory
    // creation, adapter command construction, executable lookup, and spawn.
    // On unsupported platforms provider-native flags never become a fallback
    // isolation boundary.
    require_provider_isolation()?;
    let descriptor = adapter.descriptor();
    if !descriptor.supports(operation) {
        return Err(ProviderError::UnsupportedOperation(operation.name()));
    }
    let sandbox = prepare_sandbox_files(execution_id)?;
    let inner = adapter.phase_command(
        operation,
        cwd,
        prompt,
        staging_path,
        &sandbox.path().to_string_lossy(),
    );
    validate_command_contract(&inner, &descriptor, cwd)?;
    let spec = isolate_command(inner, &descriptor.id, staging_path, sandbox.path())?;
    let mut command = tokio::process::Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for key in &spec.env_allowlist {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    let pid = child.id();
    let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    active_runs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(execution_id.to_owned(), cancel_tx);
    let _active_guard = ActiveRunGuard(execution_id.to_owned());
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let output_limit = spec.output_limit;
    let (limit_tx, limit_rx) = tokio::sync::mpsc::unbounded_channel();
    let stdout_task = tokio::spawn(read_bounded(stdout, output_limit, limit_tx.clone()));
    let stderr_task = tokio::spawn(read_bounded(stderr, output_limit.min(128 * 1024), limit_tx));
    let timeout = tokio::time::sleep(std::time::Duration::from_millis(spec.timeout_ms));
    tokio::pin!(timeout);
    let status = tokio::select! {
        status = child.wait() => status?,
        _ = &mut timeout => {
            terminate_process_tree(&mut child, pid).await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(ProviderError::Timeout(spec.timeout_ms));
        }
        changed = cancel_rx.changed() => {
            if changed.is_ok() && *cancel_rx.borrow() {
                terminate_process_tree(&mut child, pid).await;
                stdout_task.abort();
                stderr_task.abort();
                return Err(ProviderError::Canceled);
            }
            terminate_process_tree(&mut child, pid).await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(ProviderError::Canceled);
        }
        exceeded = wait_for_output_limit(limit_rx) => {
            terminate_process_tree(&mut child, pid).await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(ProviderError::OutputLimit(exceeded));
        }
    };
    let stdout = stdout_task
        .await
        .map_err(|error| ProviderError::Failed(error.to_string()))??;
    let stderr = stderr_task
        .await
        .map_err(|error| ProviderError::Failed(error.to_string()))??;
    if stdout.len().saturating_add(stderr.len()) > output_limit {
        return Err(ProviderError::OutputLimit(output_limit));
    }
    if !status.success() {
        // Provider stderr can echo prompts, credentials, or account metadata.
        // Keep it bounded in memory above, but never return it through the API.
        return Err(ProviderError::Failed(status.to_string()));
    }
    let candidates = match descriptor.result_transport {
        ProviderResultTransport::Stdout => {
            vec![String::from_utf8(stdout).map_err(|_| ProviderError::MalformedOutput)?]
        }
        ProviderResultTransport::StagingArtifact => {
            match super::artifacts::read_text(std::path::Path::new(staging_path)) {
                Ok((staging, _)) => {
                    if staging.len() > output_limit {
                        return Err(ProviderError::OutputLimit(output_limit));
                    }
                    vec![staging]
                }
                Err(_) if std::fs::symlink_metadata(staging_path).is_ok() => {
                    return Err(ProviderError::UnsafeStaging(staging_path.into()));
                }
                Err(_) => Vec::new(),
            }
        }
    };
    let artifact = candidates
        .iter()
        .find_map(|candidate| extract_artifact(candidate, begin_marker, end_marker))
        .ok_or(ProviderError::MalformedOutput)?;
    Ok(finalize_artifact(operation, artifact))
}

fn finalize_artifact(operation: ProviderOperation, mut artifact: String) -> String {
    // Envelope extraction deliberately trims presentation whitespace for
    // Markdown and JSON artifacts. A unified diff, however, is a line-based
    // protocol and `git apply` rejects a final patch line without its newline.
    // Restore only that structural delimiter; this does not alter diff intent.
    if operation == ProviderOperation::ImplementationDiff && !artifact.ends_with('\n') {
        artifact.push('\n');
    }
    artifact
}

fn validate_command_contract(
    command: &CommandSpec,
    descriptor: &ProviderDescriptor,
    expected_cwd: &str,
) -> Result<(), ProviderError> {
    if command.program.trim().is_empty() || command.program.contains('\0') {
        return Err(ProviderError::InvalidCommand(
            "program must be a non-empty executable".into(),
        ));
    }
    if command.cwd != expected_cwd || !Path::new(&command.cwd).is_absolute() {
        return Err(ProviderError::InvalidCommand(
            "cwd must be the absolute disposable worktree path".into(),
        ));
    }
    if command.timeout_ms == 0 || command.timeout_ms > descriptor.timeout_ms {
        return Err(ProviderError::InvalidCommand(
            "timeout exceeds the declared provider limit".into(),
        ));
    }
    if command.output_limit == 0 || command.output_limit > descriptor.output_limit {
        return Err(ProviderError::InvalidCommand(
            "output limit exceeds the declared provider limit".into(),
        ));
    }
    if command.args.len() > MAX_CUSTOM_PROVIDER_ARGS
        || command.args.iter().any(|argument| argument.contains('\0'))
    {
        return Err(ProviderError::InvalidCommand(
            "argv is not bounded or contains NUL".into(),
        ));
    }
    let mut seen = HashSet::new();
    if command.env_allowlist.iter().any(|key| {
        !valid_environment_name(key) || forbidden_environment_name(key) || !seen.insert(key)
    }) {
        return Err(ProviderError::InvalidCommand(
            "environment allowlist contains an unsafe or duplicate name".into(),
        ));
    }
    Ok(())
}

async fn wait_for_output_limit(mut receiver: tokio::sync::mpsc::UnboundedReceiver<usize>) -> usize {
    match receiver.recv().await {
        Some(limit) => limit,
        None => std::future::pending().await,
    }
}

fn prepare_sandbox_files(run_id: &str) -> Result<tempfile::TempDir, ProviderError> {
    let root = agentum_store::paths::data_dir()
        .map_err(|error| ProviderError::IsolationSetup(error.to_string()))?
        .join("provider-sandboxes");
    std::fs::create_dir_all(&root)
        .map_err(|error| ProviderError::IsolationSetup(error.to_string()))?;
    let directory = tempfile::Builder::new()
        .prefix(&format!("run-{run_id}."))
        .tempdir_in(&root)
        .map_err(|error| ProviderError::IsolationSetup(error.to_string()))?;
    populate_sandbox_directory(directory.path())?;
    Ok(directory)
}

fn populate_sandbox_directory(directory: &Path) -> Result<(), ProviderError> {
    for name in [
        "aider.conf.yml",
        "aider.env",
        "aiderignore",
        "aider.model.settings.yml",
        "project-input-empty-file",
    ] {
        super::artifacts::atomic_write(&directory.join(name), b"", Some("missing"))
            .map_err(|error| ProviderError::IsolationSetup(error.to_string()))?;
    }
    for (name, contents) in [
        ("aider.model.metadata.json", b"{}\n".as_slice()),
        (
            "gemini-system-settings.json",
            br#"{"context":{"fileName":[".agentum-provider-context-disabled"]}}
"#,
        ),
        ("opencode-config.json", b"{}\n".as_slice()),
    ] {
        super::artifacts::atomic_write(&directory.join(name), contents, Some("missing"))
            .map_err(|error| ProviderError::IsolationSetup(error.to_string()))?;
    }
    for name in [
        "runtime",
        "project-input-empty-directory",
        "opencode-config",
    ] {
        std::fs::create_dir(directory.join(name))
            .map_err(|error| ProviderError::IsolationSetup(error.to_string()))?;
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn provider_project_input_masks(
    provider_id: &str,
    cwd: &Path,
) -> Result<Vec<(PathBuf, super::artifacts::AnchoredEntryKind)>, ProviderError> {
    let policy = provider_project_input_policy(provider_id);
    let root = super::artifacts::AnchoredDirectory::open(cwd)
        .map_err(|error| ProviderError::IsolationSetup(error.to_string()))?;
    let mut masks = root
        .find_named_descendants(policy.directories, policy.recursive_rule_files)
        .map_err(|error| ProviderError::IsolationSetup(error.to_string()))?
        .into_iter()
        .map(|entry| (cwd.join(entry.relative_path), entry.kind))
        .collect::<Vec<_>>();
    for name in policy.root_files {
        if let Some(kind) = root
            .child_kind_optional(name)
            .map_err(|error| ProviderError::IsolationSetup(error.to_string()))?
        {
            masks.push((cwd.join(name), kind));
        }
    }
    masks.sort_by(|left, right| left.0.cmp(&right.0));
    masks.dedup_by(|left, right| left.0 == right.0);
    Ok(masks)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn provider_fixed_environment(provider_id: &str, sandbox_dir: &Path) -> Vec<(String, String)> {
    match provider_id {
        "gemini" => vec![(
            "GEMINI_CLI_SYSTEM_SETTINGS_PATH".into(),
            sandbox_dir
                .join("gemini-system-settings.json")
                .to_string_lossy()
                .into_owned(),
        )],
        "opencode" => vec![
            (
                "OPENCODE_CONFIG".into(),
                sandbox_dir
                    .join("opencode-config.json")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "OPENCODE_CONFIG_DIR".into(),
                sandbox_dir
                    .join("opencode-config")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("OPENCODE_CONFIG_CONTENT".into(), "{}".into()),
            ("OPENCODE_DISABLE_CLAUDE_CODE".into(), "1".into()),
            ("OPENCODE_DISABLE_DEFAULT_PLUGINS".into(), "1".into()),
        ],
        _ => Vec::new(),
    }
}

#[cfg(target_os = "linux")]
fn isolate_command(
    inner: CommandSpec,
    provider_id: &str,
    staging_path: &str,
    sandbox_dir: &Path,
) -> Result<CommandSpec, ProviderError> {
    let bwrap = which::which("bwrap").map_err(|_| {
        ProviderError::IsolationUnavailable(
            "bubblewrap is required for provider execution on Linux".into(),
        )
    })?;
    let executable = which::which(&inner.program).map_err(|_| {
        ProviderError::IsolationUnavailable(format!(
            "{} executable is not installed",
            inner.program
        ))
    })?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            ProviderError::IsolationUnavailable("an absolute HOME is required".into())
        })?;
    let runtime_search_path = provider_runtime_layout(provider_id, &executable, &home)
        .map(|(_, search_path)| search_path);
    let cwd = Path::new(&inner.cwd);
    let staging = Path::new(staging_path);
    let staging_dir = staging.parent().ok_or_else(|| {
        ProviderError::IsolationSetup("staging artifact has no parent directory".into())
    })?;
    if !cwd.is_absolute()
        || !staging_dir.is_absolute()
        || !sandbox_dir.is_absolute()
        || !staging.starts_with(cwd)
    {
        return Err(ProviderError::IsolationSetup(
            "provider paths must be absolute and staging must be inside the attempt".into(),
        ));
    }
    let project_input_masks = provider_project_input_masks(provider_id, cwd)?;

    let mut args = vec![
        "--die-with-parent".into(),
        "--new-session".into(),
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
        "--ro-bind".into(),
        "/".into(),
        "/".into(),
        // Host temporary files and runtime sockets are ambient account
        // context, not provider input. Mask all of /run (which also masks the
        // /var/run symlink) so D-Bus, keyrings, Docker, SSH agents, and desktop
        // Unix sockets cannot be reached. Provider cloud networking remains
        // available because the network namespace is intentionally shared.
        "--tmpfs".into(),
        "/tmp".into(),
        "--tmpfs".into(),
        "/var/tmp".into(),
        "--remount-ro".into(),
        "/var/tmp".into(),
        "--tmpfs".into(),
        "/run".into(),
        "--tmpfs".into(),
        home.to_string_lossy().into_owned(),
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
    ];

    let mut mounts = provider_runtime_mounts(provider_id, &executable, &home);
    mounts.extend(provider_credential_mounts(provider_id, &home));
    mounts.push((sandbox_dir.to_path_buf(), sandbox_dir.to_path_buf(), true));
    mounts.push((cwd.to_path_buf(), cwd.to_path_buf(), true));
    mounts.push((staging_dir.to_path_buf(), staging_dir.to_path_buf(), false));
    mounts.push((
        sandbox_dir.join("runtime"),
        sandbox_dir.join("runtime"),
        false,
    ));
    // Some distributions point /etc/resolv.conf into /run. Restore that one
    // regular file after masking /run; never restore a directory or socket.
    if let Ok(resolver) = Path::new("/etc/resolv.conf").canonicalize()
        && resolver.starts_with("/run")
        && resolver.is_file()
    {
        mounts.push((resolver.clone(), resolver, true));
    }
    let mut mounted = std::collections::HashSet::new();
    for (source, target, read_only) in mounts {
        if !source.exists() || !mounted.insert(target.clone()) {
            continue;
        }
        add_destination_parents(&mut args, &target);
        args.push(if read_only { "--ro-bind" } else { "--bind" }.into());
        args.push(source.to_string_lossy().into_owned());
        args.push(target.to_string_lossy().into_owned());
    }
    let empty_file = sandbox_dir.join("project-input-empty-file");
    let empty_directory = sandbox_dir.join("project-input-empty-directory");
    for (target, kind) in project_input_masks {
        let source = match kind {
            super::artifacts::AnchoredEntryKind::File => &empty_file,
            super::artifacts::AnchoredEntryKind::Directory => &empty_directory,
        };
        if !source.exists() {
            return Err(ProviderError::IsolationSetup(format!(
                "provider input mask is missing: {}",
                source.display()
            )));
        }
        args.push("--ro-bind".into());
        args.push(source.to_string_lossy().into_owned());
        args.push(target.to_string_lossy().into_owned());
    }
    for (name, value) in provider_fixed_environment(provider_id, sandbox_dir) {
        args.push("--setenv".into());
        args.push(name);
        args.push(value);
    }
    args.extend([
        "--setenv".into(),
        "HOME".into(),
        home.to_string_lossy().into_owned(),
        "--setenv".into(),
        "XDG_CONFIG_HOME".into(),
        home.join(".config").to_string_lossy().into_owned(),
        "--setenv".into(),
        "XDG_DATA_HOME".into(),
        home.join(".local/share").to_string_lossy().into_owned(),
        "--setenv".into(),
        "XDG_CACHE_HOME".into(),
        sandbox_dir
            .join("runtime/cache")
            .to_string_lossy()
            .into_owned(),
        "--setenv".into(),
        "TMPDIR".into(),
        sandbox_dir.join("runtime").to_string_lossy().into_owned(),
        "--setenv".into(),
        "PATH".into(),
        sandbox_provider_path(runtime_search_path.as_deref()),
        "--chdir".into(),
        inner.cwd.clone(),
        "--".into(),
        inner.program.clone(),
    ]);
    args.extend(inner.args);
    Ok(CommandSpec {
        program: bwrap.to_string_lossy().into_owned(),
        args,
        cwd: inner.cwd,
        env_allowlist: inner.env_allowlist,
        timeout_ms: inner.timeout_ms,
        output_limit: inner.output_limit,
    })
}

#[cfg(target_os = "macos")]
fn isolate_command(
    inner: CommandSpec,
    provider_id: &str,
    staging_path: &str,
    sandbox_dir: &Path,
) -> Result<CommandSpec, ProviderError> {
    let sandbox_exec = Path::new("/usr/bin/sandbox-exec");
    if !sandbox_exec.is_file() {
        return Err(ProviderError::IsolationUnavailable(
            "sandbox-exec is required for provider execution on macOS".into(),
        ));
    }
    let executable = which::which(&inner.program).map_err(|_| {
        ProviderError::IsolationUnavailable(format!(
            "{} executable is not installed",
            inner.program
        ))
    })?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            ProviderError::IsolationUnavailable("an absolute HOME is required".into())
        })?;
    let cwd = Path::new(&inner.cwd);
    let staging = Path::new(staging_path);
    let staging_dir = staging.parent().ok_or_else(|| {
        ProviderError::IsolationSetup("staging artifact has no parent directory".into())
    })?;
    if !cwd.is_absolute()
        || !staging_dir.is_absolute()
        || !sandbox_dir.is_absolute()
        || !staging.starts_with(cwd)
    {
        return Err(ProviderError::IsolationSetup(
            "provider paths must be absolute and staging must be inside the attempt".into(),
        ));
    }
    let project_input_masks = provider_project_input_masks(provider_id, cwd)?;

    let mut readable = vec![
        PathBuf::from("/System"),
        PathBuf::from("/usr"),
        PathBuf::from("/bin"),
        PathBuf::from("/sbin"),
        PathBuf::from("/Library"),
        PathBuf::from("/opt/homebrew"),
        PathBuf::from("/usr/local"),
        cwd.to_path_buf(),
        sandbox_dir.to_path_buf(),
        executable.clone(),
    ];
    readable.extend(
        provider_runtime_mounts(provider_id, &executable, &home)
            .into_iter()
            .map(|(source, _, _)| source),
    );
    readable.extend(
        provider_credential_mounts(provider_id, &home)
            .into_iter()
            .map(|(source, _, _)| source),
    );
    readable.retain(|path| path.exists());
    readable.sort();
    readable.dedup();

    let mut profile = String::from(
        "(version 1)\n(deny default)\n(allow process-exec)\n(allow process-fork)\n(allow signal (target same-sandbox))\n(allow sysctl-read)\n(allow network-outbound)\n(allow mach-lookup (global-name \"com.apple.system.opendirectoryd.libinfo\") (global-name \"com.apple.cfprefsd.agent\"))\n(allow file-read-metadata)\n",
    );
    for path in readable {
        profile.push_str(&format!(
            "(allow file-read-data (subpath {}))\n",
            seatbelt_literal(&path)?
        ));
    }
    for (path, kind) in project_input_masks {
        let filter = match kind {
            super::artifacts::AnchoredEntryKind::File => "literal",
            super::artifacts::AnchoredEntryKind::Directory => "subpath",
        };
        profile.push_str(&format!(
            "(deny file-read* ({filter} {}))\n",
            seatbelt_literal(&path)?
        ));
    }
    for path in [sandbox_dir.join("runtime"), staging_dir.to_path_buf()] {
        profile.push_str(&format!(
            "(allow file-write* (subpath {}))\n",
            seatbelt_literal(&path)?
        ));
    }
    profile.push_str("(allow file-write-data (literal \"/dev/null\"))\n");

    let mut args = vec![
        "-p".into(),
        profile,
        "--".into(),
        "/usr/bin/env".into(),
        format!("TMPDIR={}", sandbox_dir.join("runtime").display()),
        format!(
            "XDG_CACHE_HOME={}",
            sandbox_dir.join("runtime/cache").display()
        ),
    ];
    args.extend(
        provider_fixed_environment(provider_id, sandbox_dir)
            .into_iter()
            .map(|(name, value)| format!("{name}={value}")),
    );
    args.push(inner.program.clone());
    args.extend(inner.args);
    Ok(CommandSpec {
        program: sandbox_exec.to_string_lossy().into_owned(),
        args,
        cwd: inner.cwd,
        env_allowlist: inner.env_allowlist,
        timeout_ms: inner.timeout_ms,
        output_limit: inner.output_limit,
    })
}

#[cfg(target_os = "macos")]
fn seatbelt_literal(path: &Path) -> Result<String, ProviderError> {
    let value = path.to_string_lossy();
    if value.contains(['\0', '\n', '\r']) {
        return Err(ProviderError::IsolationSetup(
            "sandbox path contains control characters".into(),
        ));
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

#[cfg(target_os = "windows")]
fn isolate_command(
    _inner: CommandSpec,
    _provider_id: &str,
    _staging_path: &str,
    _sandbox_dir: &Path,
) -> Result<CommandSpec, ProviderError> {
    Err(ProviderError::IsolationUnavailable(
        WINDOWS_LOCAL_SDD_REASON.into(),
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn isolate_command(
    _inner: CommandSpec,
    _provider_id: &str,
    _staging_path: &str,
    _sandbox_dir: &Path,
) -> Result<CommandSpec, ProviderError> {
    Err(ProviderError::IsolationUnavailable(
        "Agentum does not provide a provider filesystem sandbox for this platform.".into(),
    ))
}

#[cfg(target_os = "linux")]
fn add_destination_parents(args: &mut Vec<String>, target: &Path) {
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn provider_runtime_mounts(
    provider_id: &str,
    executable: &Path,
    home: &Path,
) -> Vec<(PathBuf, PathBuf, bool)> {
    let mut values = Vec::new();
    if executable.starts_with(home) {
        values.push((executable.to_path_buf(), executable.to_path_buf(), true));
    }
    if let Some((runtime, _)) = provider_runtime_layout(provider_id, executable, home) {
        values.push((runtime.clone(), runtime, true));
    }
    values
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn provider_runtime_layout(
    provider_id: &str,
    executable: &Path,
    home: &Path,
) -> Option<(PathBuf, PathBuf)> {
    let canonical = executable
        .canonicalize()
        .unwrap_or_else(|_| executable.to_path_buf());
    let named_ancestor = |name: &str| {
        canonical
            .ancestors()
            .find(|path| path.file_name().is_some_and(|value| value == name))
            .map(Path::to_path_buf)
    };
    let layout = match provider_id {
        "claude" | "codex" | "gemini" => {
            named_ancestor("installation").map(|runtime| (runtime.clone(), runtime.join("bin")))
        }
        "agent" | "opencode" => canonical
            .parent()
            .map(|runtime| (runtime.to_path_buf(), runtime.to_path_buf())),
        "aider" => {
            named_ancestor("aider-chat").map(|runtime| (runtime.clone(), runtime.join("bin")))
        }
        "hermes" => {
            let runtime = home.join(".hermes/hermes-agent");
            Some((runtime.clone(), runtime.join("bin")))
        }
        _ => None,
    };
    layout.or_else(|| {
        canonical
            .parent()
            .map(|runtime| (runtime.to_path_buf(), runtime.to_path_buf()))
    })
}

#[cfg(target_os = "linux")]
fn sandbox_provider_path(runtime_search_path: Option<&Path>) -> String {
    let mut paths = Vec::new();
    if let Some(path) = runtime_search_path.filter(|path| path.is_dir()) {
        paths.push(path.to_string_lossy().into_owned());
    }
    if let Some(host_path) = std::env::var_os("PATH") {
        paths.push(host_path.to_string_lossy().into_owned());
    }
    if paths.is_empty() {
        "/usr/local/bin:/usr/bin:/bin".into()
    } else {
        paths.join(":")
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn provider_credential_mounts(provider_id: &str, home: &Path) -> Vec<(PathBuf, PathBuf, bool)> {
    let relative: &[&str] = match provider_id {
        "claude" => &[".claude/.credentials.json"],
        "codex" => &[".codex/auth.json"],
        "agent" => &[".config/cursor/auth.json", ".cursor/auth.json"],
        "gemini" => &[".gemini/oauth_creds.json", ".gemini/google_accounts.json"],
        // Hermes stores OAuth/API credentials separately from behavioral
        // config. Mount only the credential leaf; config.yaml and .env are
        // untrusted ambient customization and stay behind the empty HOME.
        "hermes" => &[".hermes/auth.json"],
        "opencode" => &[".local/share/opencode/auth.json"],
        _ => &[],
    };
    relative
        .iter()
        .map(|path| {
            let path = home.join(path);
            (path.clone(), path, true)
        })
        .collect()
}

async fn read_bounded(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
    limit_tx: tokio::sync::mpsc::UnboundedSender<usize>,
) -> Result<Vec<u8>, ProviderError> {
    let mut output = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut output)
        .await?;
    if output.len() > limit {
        let _ = limit_tx.send(limit);
        return Err(ProviderError::OutputLimit(limit));
    }
    Ok(output)
}

fn active_runs() -> &'static Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>> {
    static ACTIVE: OnceLock<Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>>> =
        OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

struct ActiveRunGuard(String);

impl Drop for ActiveRunGuard {
    fn drop(&mut self) {
        active_runs()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.0);
    }
}

/// Request cancellation of a live provider. The provider runner owns the child
/// handle and performs the platform-specific process-tree termination.
pub fn cancel_run(run_id: &str) -> bool {
    let active = active_runs()
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

pub(crate) async fn terminate_process_tree(child: &mut tokio::process::Child, pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        // SAFETY: the child was placed in a fresh process group whose id is the
        // child pid. A negative target cannot reach an unrelated process group.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    if let Some(pid) = pid {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

fn extract_artifact(raw: &str, begin_marker: &str, end_marker: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(marked) = marked_value(&value, begin_marker, end_marker) {
            return Some(marked);
        }
    }
    for line in raw.lines() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(marked) = marked_value(&value, begin_marker, end_marker) {
                return Some(marked);
            }
        }
    }
    marked_string(raw, begin_marker, end_marker)
}

fn marked_value(value: &serde_json::Value, begin_marker: &str, end_marker: &str) -> Option<String> {
    match value {
        serde_json::Value::String(value) => marked_string(value, begin_marker, end_marker),
        serde_json::Value::Array(values) => values
            .iter()
            .find_map(|value| marked_value(value, begin_marker, end_marker)),
        serde_json::Value::Object(values) => values
            .values()
            .find_map(|value| marked_value(value, begin_marker, end_marker)),
        _ => None,
    }
}

fn marked_string(value: &str, begin_marker: &str, end_marker: &str) -> Option<String> {
    if value.match_indices(begin_marker).count() != 1
        || value.match_indices(end_marker).count() != 1
    {
        return None;
    }
    let (_, rest) = value.split_once(begin_marker)?;
    let (body, _) = rest.split_once(end_marker)?;
    let body = body.trim();
    (!body.is_empty()).then(|| body.to_owned())
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    use std::sync::Arc;
    #[cfg(target_os = "windows")]
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_local_sdd_boundary_is_explicit_and_never_provider_native() {
        let capability = provider_isolation_capability();
        assert!(!capability.available);
        assert_eq!(
            capability.boundary,
            ProviderExecutionBoundary::RemoteClientOnly
        );
        assert_eq!(capability.mechanism, None);
        assert_eq!(capability.reason_code, Some(WINDOWS_LOCAL_SDD_REASON_CODE));
        assert_eq!(capability.reason, Some(WINDOWS_LOCAL_SDD_REASON));
        assert!(WINDOWS_LOCAL_SDD_REASON.contains("restricted-token/AppContainer"));
        assert!(WINDOWS_LOCAL_SDD_REASON.contains("provider-native sandbox flags"));
        assert!(WINDOWS_LOCAL_SDD_REASON.contains("process-tree cancellation"));
    }

    #[cfg(target_os = "windows")]
    #[tokio::test(flavor = "current_thread")]
    async fn windows_local_sdd_boundary_rejects_all_bundled_probes_before_version_or_auth() {
        for id in BUNDLED_IDS {
            let capability = probe_provider(BundledProvider::get(id).unwrap()).await;
            assert!(!capability.available, "{id} must remain disabled");
            assert_eq!(capability.version, None, "{id} must not be probed");
            assert_eq!(capability.reason.as_deref(), Some(WINDOWS_LOCAL_SDD_REASON));
        }
    }

    #[cfg(target_os = "windows")]
    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)]
    async fn windows_local_sdd_boundary_rejects_before_adapter_or_sandbox_filesystem_work() {
        struct CountingAdapter {
            descriptor_calls: Arc<AtomicUsize>,
            phase_calls: Arc<AtomicUsize>,
        }

        impl SddProviderAdapter for CountingAdapter {
            fn descriptor(&self) -> ProviderDescriptor {
                self.descriptor_calls.fetch_add(1, Ordering::SeqCst);
                ProviderDescriptor {
                    id: "windows-boundary-test".into(),
                    version_probe: vec!["--version".into()],
                    capabilities: REQUIRED_OPERATIONS.to_vec(),
                    transport: ProviderTransport::HarvestArtifact,
                    result_transport: ProviderResultTransport::Stdout,
                    cancellation: ProviderCancellation::ProcessGroup,
                    isolation: ProviderIsolation::OsSandboxDisposableWorktree,
                    timeout_ms: 1_000,
                    output_limit: 1_024,
                }
            }

            fn phase_command(
                &self,
                _operation: ProviderOperation,
                cwd: &str,
                _prompt: &str,
                _staging_path: &str,
                _sandbox_dir: &str,
            ) -> CommandSpec {
                self.phase_calls.fetch_add(1, Ordering::SeqCst);
                CommandSpec {
                    program: "provider-native-sandbox-flag-is-not-enough.exe".into(),
                    args: vec!["--sandbox".into()],
                    cwd: cwd.into(),
                    env_allowlist: Vec::new(),
                    timeout_ms: 1_000,
                    output_limit: 1_024,
                }
            }
        }

        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let agentum_home = tempfile::tempdir().unwrap();
        let attempt = tempfile::tempdir().unwrap();
        let descriptor_calls = Arc::new(AtomicUsize::new(0));
        let phase_calls = Arc::new(AtomicUsize::new(0));
        let adapter = CountingAdapter {
            descriptor_calls: descriptor_calls.clone(),
            phase_calls: phase_calls.clone(),
        };
        let old = std::env::var_os("AGENTUM_HOME");
        unsafe { std::env::set_var("AGENTUM_HOME", agentum_home.path()) };
        let result = run_artifact(
            "windows-no-local-sdd",
            &adapter,
            ProviderOperation::Authoring,
            &attempt.path().to_string_lossy(),
            "prompt",
            &attempt.path().join("spec-output.md").to_string_lossy(),
            SPEC_BEGIN,
            SPEC_END,
        )
        .await;
        match old {
            Some(value) => unsafe { std::env::set_var("AGENTUM_HOME", value) },
            None => unsafe { std::env::remove_var("AGENTUM_HOME") },
        }

        assert!(matches!(
            result,
            Err(ProviderError::IsolationUnavailable(ref reason))
                if reason == WINDOWS_LOCAL_SDD_REASON
        ));
        assert_eq!(descriptor_calls.load(Ordering::SeqCst), 0);
        assert_eq!(phase_calls.load(Ordering::SeqCst), 0);
        assert!(!agentum_home.path().join("data/provider-sandboxes").exists());
    }

    fn custom_manifest(id: &str, result_transport: ProviderResultTransport) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": CUSTOM_PROVIDER_SCHEMA_VERSION,
            "id": id,
            "version": "1.2.3",
            "program": "fixture-provider",
            "args": [
                "--operation={operation}",
                "--prompt={prompt}",
                "--staging={staging}"
            ],
            "versionProbe": ["--version"],
            "capabilities": [
                "authoring",
                "design",
                "planning",
                "implementation_diff",
                "review"
            ],
            "transport": "json_events",
            "resultTransport": result_transport,
            "isolation": "os_sandbox_disposable_worktree",
            "cancellation": "process_group",
            "timeoutMs": 30_000,
            "outputLimit": 65_536,
            "envAllowlist": ["FIXTURE_PROVIDER_TOKEN"]
        }))
        .unwrap()
    }

    fn conformance_evidence(
        adapter: &CustomProviderAdapter,
    ) -> Vec<CustomProviderConformanceEvidence> {
        REQUIRED_CONFORMANCE_CASES
            .iter()
            .copied()
            .map(|case| CustomProviderConformanceEvidence {
                case,
                evidence_sha256: super::super::sha256(format!(
                    "{}:{case:?}",
                    adapter.manifest_sha256
                )),
            })
            .collect()
    }

    fn signed_test_receipt(
        adapter: &CustomProviderAdapter,
        cases: Vec<CustomProviderConformanceEvidence>,
        vault: &super::super::credentials::MemoryCredentialVault,
    ) -> Result<CustomProviderConformanceReceipt, CustomProviderError> {
        signed_conformance_receipt_for(
            adapter,
            cases,
            &super::super::sha256(b"provider-conformance-fixture"),
            "test-source-revision",
            vault,
        )
    }

    #[test]
    fn every_required_provider_has_a_bounded_non_shell_command() {
        for id in BUNDLED_IDS {
            let adapter = BundledProvider::get(id).unwrap();
            validate_provider_contract(&adapter).unwrap();
            let command = adapter.authoring_command(
                "/worktree",
                "author",
                "/staging/spec.md",
                "/agentum/provider",
            );
            assert_ne!(command.program, "bash");
            assert_ne!(command.program, "sh");
            assert!(!command.args.is_empty());
            assert!(command.timeout_ms > 0);
            assert!(command.output_limit > 0);
            assert_eq!(
                adapter.descriptor().isolation,
                ProviderIsolation::OsSandboxDisposableWorktree
            );
            assert_eq!(
                adapter.descriptor().cancellation,
                ProviderCancellation::ProcessGroup
            );
            assert!(
                REQUIRED_OPERATIONS
                    .iter()
                    .all(|operation| adapter.descriptor().supports(*operation))
            );
        }
    }

    #[test]
    fn custom_manifest_is_strict_bounded_and_expands_direct_argv() {
        let bytes = custom_manifest("fixture", ProviderResultTransport::Stdout);
        let adapter = validate_custom_provider_manifest(&bytes, "fixture").unwrap();
        assert_eq!(adapter.descriptor().id, "custom:fixture");
        assert_eq!(adapter.version, "1.2.3");
        let command = adapter.phase_command(
            ProviderOperation::Review,
            "/attempt",
            "review prompt",
            "/attempt/staging/result",
            "/sandbox",
        );
        assert_eq!(command.program, "fixture-provider");
        assert!(
            command
                .args
                .iter()
                .any(|value| value == "--operation=independent_review")
        );
        assert!(
            command
                .args
                .iter()
                .any(|value| value == "--prompt=review prompt")
        );
        assert!(
            command
                .args
                .iter()
                .any(|value| value == "--staging=/attempt/staging/result")
        );
        assert!(
            command
                .env_allowlist
                .iter()
                .any(|value| value == "FIXTURE_PROVIDER_TOKEN")
        );
        assert!(
            !command
                .env_allowlist
                .iter()
                .any(|value| value == "BASH_ENV")
        );
    }

    #[test]
    fn custom_manifest_rejects_unknown_fields_shells_unsafe_env_and_partial_capabilities() {
        let mut value: serde_json::Value =
            serde_json::from_slice(&custom_manifest("fixture", ProviderResultTransport::Stdout))
                .unwrap();
        value["unknown"] = serde_json::json!(true);
        assert!(
            validate_custom_provider_manifest(&serde_json::to_vec(&value).unwrap(), "fixture")
                .is_err()
        );

        value.as_object_mut().unwrap().remove("unknown");
        value["program"] = serde_json::json!("bash");
        assert!(matches!(
            validate_custom_provider_manifest(&serde_json::to_vec(&value).unwrap(), "fixture"),
            Err(CustomProviderError::InvalidManifest(_))
        ));

        value["program"] = serde_json::json!("fixture-provider");
        value["envAllowlist"] = serde_json::json!(["LD_PRELOAD"]);
        assert!(matches!(
            validate_custom_provider_manifest(&serde_json::to_vec(&value).unwrap(), "fixture"),
            Err(CustomProviderError::InvalidManifest(_))
        ));

        value["envAllowlist"] = serde_json::json!([]);
        value["capabilities"] = serde_json::json!(["authoring", "design"]);
        assert!(matches!(
            validate_custom_provider_manifest(&serde_json::to_vec(&value).unwrap(), "fixture"),
            Err(CustomProviderError::InvalidManifest(_))
        ));
    }

    #[test]
    fn custom_provider_approval_is_hash_bound_and_survives_reload() {
        let directory = tempfile::tempdir().unwrap();
        let vault = super::super::credentials::MemoryCredentialVault::default();
        let bytes = custom_manifest("fixture", ProviderResultTransport::Stdout);
        let adapter = validate_custom_provider_manifest(&bytes, "fixture").unwrap();
        let receipt =
            signed_test_receipt(&adapter, conformance_evidence(&adapter), &vault).unwrap();
        let receipt_path = directory.path().join("fixture.approval.json");
        std::fs::write(directory.path().join("fixture.json"), &bytes).unwrap();
        std::fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();

        let first = load_custom_provider_from_directory_with_vault(
            directory.path(),
            "custom:fixture",
            &vault,
        )
        .expect("approved manifest loads");
        let after_restart = load_custom_provider_from_directory_with_vault(
            directory.path(),
            "custom:fixture",
            &vault,
        )
        .expect("approval and provider contract are durable");
        assert_eq!(first.descriptor(), after_restart.descriptor());
        assert_eq!(first.manifest_sha256, after_restart.manifest_sha256);

        let other_installation = super::super::credentials::MemoryCredentialVault::default();
        assert!(matches!(
            load_custom_provider_from_directory_with_vault(
                directory.path(),
                "custom:fixture",
                &other_installation
            ),
            Err(CustomProviderError::InvalidSignature)
        ));

        let mut tampered = receipt.clone();
        tampered.cases[0].evidence_sha256 = super::super::sha256(b"forged evidence");
        std::fs::write(&receipt_path, serde_json::to_vec(&tampered).unwrap()).unwrap();
        assert!(matches!(
            load_custom_provider_from_directory_with_vault(
                directory.path(),
                "custom:fixture",
                &vault
            ),
            Err(CustomProviderError::InvalidSignature)
        ));
        std::fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();

        let mut changed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        changed["version"] = serde_json::json!("1.2.4");
        std::fs::write(
            directory.path().join("fixture.json"),
            serde_json::to_vec(&changed).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            load_custom_provider_from_directory_with_vault(
                directory.path(),
                "custom:fixture",
                &vault
            ),
            Err(CustomProviderError::ConformanceRequired(_))
        ));
        assert!(matches!(
            load_custom_provider_from_directory_with_vault(
                directory.path(),
                "custom:../fixture",
                &vault
            ),
            Err(CustomProviderError::InvalidReference)
        ));
    }

    #[test]
    fn custom_provider_receipt_requires_evidence_for_the_complete_suite() {
        let vault = super::super::credentials::MemoryCredentialVault::default();
        let bytes = custom_manifest("fixture", ProviderResultTransport::Stdout);
        let adapter = validate_custom_provider_manifest(&bytes, "fixture").unwrap();
        let mut cases = conformance_evidence(&adapter);
        cases.pop();
        assert!(matches!(
            signed_test_receipt(&adapter, cases, &vault),
            Err(CustomProviderError::ConformanceRequired(_))
        ));
        let mut cases = conformance_evidence(&adapter);
        cases[0].evidence_sha256 = "not-a-hash".into();
        assert!(matches!(
            signed_test_receipt(&adapter, cases, &vault),
            Err(CustomProviderError::ConformanceRequired(_))
        ));
        let mut cases = conformance_evidence(&adapter);
        cases[1].evidence_sha256 = cases[0].evidence_sha256.clone();
        assert!(matches!(
            signed_test_receipt(&adapter, cases, &vault),
            Err(CustomProviderError::ConformanceRequired(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn custom_provider_loader_does_not_follow_manifest_links() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            outside.path(),
            custom_manifest("fixture", ProviderResultTransport::Stdout),
        )
        .unwrap();
        symlink(outside.path(), directory.path().join("fixture.json")).unwrap();
        let vault = super::super::credentials::MemoryCredentialVault::default();
        assert!(matches!(
            load_custom_provider_from_directory_with_vault(
                directory.path(),
                "custom:fixture",
                &vault
            ),
            Err(CustomProviderError::Read(_))
        ));
    }

    #[test]
    fn authoring_adapters_use_their_supported_read_only_or_dry_run_modes() {
        let command = |id| {
            BundledProvider::get(id)
                .unwrap()
                .authoring_command(
                    "/worktree",
                    "author",
                    "/staging/spec.md",
                    "/agentum/provider",
                )
                .args
        };
        assert!(
            command("codex")
                .windows(2)
                .any(|pair| pair == ["--sandbox", "read-only"])
        );
        assert!(command("claude").iter().any(|arg| arg == "--safe-mode"));
        assert!(
            command("claude")
                .iter()
                .any(|arg| arg == "--no-session-persistence")
        );
        assert!(
            command("gemini")
                .windows(2)
                .any(|pair| pair == ["--approval-mode", "plan"])
        );
        assert!(command("hermes").iter().any(|arg| arg == "--safe-mode"));
        assert!(
            command("hermes")
                .iter()
                .any(|arg| arg == "--ignore-user-config")
        );
        assert!(command("hermes").iter().any(|arg| arg == "--ignore-rules"));
        assert!(command("aider").iter().any(|arg| arg == "--dry-run"));
        assert!(command("opencode").starts_with(&["--pure".into(), "run".into()]));
        let aider = command("aider");
        for path in [
            "/agentum/provider/aider.conf.yml",
            "/agentum/provider/aider.env",
            "/agentum/provider/aiderignore",
            "/agentum/provider/aider.model.settings.yml",
            "/agentum/provider/aider.model.metadata.json",
        ] {
            assert!(aider.iter().any(|argument| argument == path));
        }
        assert!(
            command("agent")
                .windows(2)
                .any(|pair| pair == ["--mode", "plan"])
        );
    }

    #[test]
    fn bundled_project_input_roster_covers_each_documented_ambient_source() {
        let claude = provider_project_input_policy("claude");
        assert!(claude.directories.contains(&".claude"));
        assert!(claude.root_files.contains(&".mcp.json"));
        assert!(claude.recursive_rule_files.contains(&"CLAUDE.md"));

        let codex = provider_project_input_policy("codex");
        assert!(codex.directories.contains(&".codex"));
        assert!(codex.recursive_rule_files.contains(&"AGENTS.md"));

        let cursor = provider_project_input_policy("agent");
        assert!(cursor.directories.contains(&".cursor"));
        for input in [
            ".cursorrules",
            ".cursorignore",
            ".cursorindexingignore",
            "mcp.json",
        ] {
            assert!(cursor.root_files.contains(&input));
        }
        for input in ["AGENTS.md", "CLAUDE.md"] {
            assert!(cursor.recursive_rule_files.contains(&input));
        }

        let gemini = provider_project_input_policy("gemini");
        assert!(gemini.directories.contains(&".gemini"));
        for input in [".env", ".geminiignore"] {
            assert!(gemini.root_files.contains(&input));
        }
        assert!(gemini.recursive_rule_files.contains(&"GEMINI.md"));

        let hermes = provider_project_input_policy("hermes");
        for directory in [".hermes", ".claude", ".cursor"] {
            assert!(hermes.directories.contains(&directory));
        }
        for input in ["HERMES.md", "AGENTS.md", "CLAUDE.md", "SOUL.md"] {
            assert!(hermes.recursive_rule_files.contains(&input));
        }

        let opencode = provider_project_input_policy("opencode");
        for directory in [".opencode", ".claude"] {
            assert!(opencode.directories.contains(&directory));
        }
        for input in ["opencode.json", "opencode.jsonc", ".env"] {
            assert!(opencode.root_files.contains(&input));
        }
        for input in ["AGENTS.md", "CLAUDE.md"] {
            assert!(opencode.recursive_rule_files.contains(&input));
        }

        let aider = provider_project_input_policy("aider");
        for input in [
            ".aider.conf.yml",
            ".aiderignore",
            ".aider.model.settings.yml",
            ".aider.model.metadata.json",
            ".env",
        ] {
            assert!(aider.root_files.contains(&input));
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn project_input_masks_handle_absent_file_and_directory_forms_without_mutation() {
        use super::super::artifacts::AnchoredEntryKind;

        let root = tempfile::tempdir().unwrap();
        let attempt = root.path().join("attempt");
        std::fs::create_dir_all(attempt.join("nested/CLAUDE.md")).unwrap();
        std::fs::create_dir(attempt.join("mcp.json")).unwrap();
        std::fs::write(attempt.join(".cursor"), b"directory-name-as-file\n").unwrap();
        std::fs::write(attempt.join("AGENTS.md"), b"poisoned rule\n").unwrap();

        let masks = provider_project_input_masks("agent", &attempt).unwrap();
        let mask = |relative: &str| {
            masks
                .iter()
                .find(|(path, _)| path == &attempt.join(relative))
                .map(|(_, kind)| *kind)
        };
        assert_eq!(mask(".cursor"), Some(AnchoredEntryKind::File));
        assert_eq!(mask("AGENTS.md"), Some(AnchoredEntryKind::File));
        assert_eq!(mask("mcp.json"), Some(AnchoredEntryKind::Directory));
        assert_eq!(mask("nested/CLAUDE.md"), Some(AnchoredEntryKind::Directory));
        assert_eq!(mask(".cursorrules"), None, "absent inputs need no mount");

        assert_eq!(
            std::fs::read(attempt.join(".cursor")).unwrap(),
            b"directory-name-as-file\n"
        );
        assert_eq!(
            std::fs::read(attempt.join("AGENTS.md")).unwrap(),
            b"poisoned rule\n"
        );
        assert!(attempt.join("mcp.json").is_dir());
        assert!(attempt.join("nested/CLAUDE.md").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn project_input_links_fail_closed_without_touching_the_target() {
        use std::os::unix::fs::symlink;

        for relative in ["AGENTS.md", ".cursor"] {
            let root = tempfile::tempdir().unwrap();
            let attempt = root.path().join("attempt");
            let outside = root.path().join("outside");
            std::fs::create_dir(&attempt).unwrap();
            std::fs::write(&outside, b"must remain private\n").unwrap();
            symlink(&outside, attempt.join(relative)).unwrap();

            let error = provider_project_input_masks("agent", &attempt).unwrap_err();
            assert!(error.to_string().contains("provider-input link"));
            assert_eq!(std::fs::read(&outside).unwrap(), b"must remain private\n");
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn hermes_mounts_only_its_auth_leaf() {
        let home = Path::new("/account");
        let mounts = provider_credential_mounts("hermes", home);
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].0, home.join(".hermes/auth.json"));
        assert!(
            !mounts.iter().any(|(source, _, _)| {
                source.ends_with("config.yaml") || source.ends_with(".env")
            })
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn gemini_and_opencode_receive_only_agentum_owned_configuration() {
        let sandbox = Path::new("/agentum/provider");
        let gemini = provider_fixed_environment("gemini", sandbox);
        assert_eq!(
            gemini,
            vec![(
                "GEMINI_CLI_SYSTEM_SETTINGS_PATH".into(),
                "/agentum/provider/gemini-system-settings.json".into()
            )]
        );

        let opencode = provider_fixed_environment("opencode", sandbox);
        assert!(opencode.contains(&(
            "OPENCODE_CONFIG".into(),
            "/agentum/provider/opencode-config.json".into()
        )));
        assert!(opencode.contains(&(
            "OPENCODE_CONFIG_DIR".into(),
            "/agentum/provider/opencode-config".into()
        )));
        assert!(opencode.contains(&("OPENCODE_CONFIG_CONTENT".into(), "{}".into())));
        assert!(opencode.contains(&("OPENCODE_DISABLE_CLAUDE_CODE".into(), "1".into())));
        assert!(opencode.contains(&("OPENCODE_DISABLE_DEFAULT_PLUGINS".into(), "1".into())));
    }

    #[test]
    fn cursor_executable_aliases_resolve_deterministically_to_agent_provider() {
        for reference in ["agent", "cursor", "cursor-agent"] {
            assert_eq!(
                BundledProvider::get(reference).unwrap().descriptor().id,
                "agent"
            );
        }

        let fallback = resolve_bundled_executable_with("agent", |candidate| {
            (candidate == "cursor-agent").then(|| PathBuf::from("/bin/cursor-agent"))
        })
        .unwrap();
        assert_eq!(fallback.0, "cursor-agent");

        let preferred = resolve_bundled_executable_with("agent", |candidate| {
            Some(PathBuf::from(format!("/bin/{candidate}")))
        })
        .unwrap();
        assert_eq!(preferred.0, "agent");
        assert_eq!(preferred.1, PathBuf::from("/bin/agent"));
    }

    #[test]
    fn every_phase_is_explicit_and_remains_read_only() {
        let operations = [
            ProviderOperation::Authoring,
            ProviderOperation::Design,
            ProviderOperation::Planning,
            ProviderOperation::ImplementationDiff,
            ProviderOperation::Review,
        ];
        for id in BUNDLED_IDS {
            let adapter = BundledProvider::get(id).unwrap();
            for operation in operations {
                let command = adapter.phase_command(
                    operation,
                    "/worktree",
                    "same prompt",
                    "/worktree/.agentum/staging/result",
                    "/agentum/provider",
                );
                assert!(
                    command
                        .args
                        .iter()
                        .any(|argument| argument.contains(operation.name())),
                    "{id} did not bind operation {}",
                    operation.name()
                );
                assert!(!command.args.iter().any(|argument| {
                    matches!(
                        argument.as_str(),
                        "--yolo"
                            | "--force"
                            | "--auto"
                            | "--dangerously-skip-permissions"
                            | "danger-full-access"
                            | "workspace-write"
                    )
                }));
            }
        }
    }

    #[test]
    fn adapters_receive_only_provider_specific_credentials() {
        let command = |id| {
            BundledProvider::get(id)
                .unwrap()
                .authoring_command(
                    "/worktree",
                    "author",
                    "/worktree/.agentum/staging/spec.md",
                    "/agentum/provider",
                )
                .env_allowlist
        };
        let claude = command("claude");
        assert!(claude.iter().any(|key| key == "ANTHROPIC_API_KEY"));
        assert!(!claude.iter().any(|key| key == "OPENAI_API_KEY"));
        let codex = command("codex");
        assert!(codex.iter().any(|key| key == "OPENAI_API_KEY"));
        assert!(!codex.iter().any(|key| key == "ANTHROPIC_API_KEY"));
        let cursor = command("agent");
        assert!(cursor.iter().any(|key| key == "CURSOR_API_KEY"));
        assert!(!cursor.iter().any(|key| key == "OPENAI_API_KEY"));
    }

    #[test]
    fn authentication_probe_parsers_fail_closed_without_exposing_identity() {
        assert!(authentication_output_is_valid(
            "claude",
            br#"{"loggedIn":true,"email":"private@example.test"}"#
        ));
        assert!(!authentication_output_is_valid(
            "claude",
            br#"{"loggedIn":false}"#
        ));
        assert!(authentication_output_is_valid(
            "agent",
            br#"{"isAuthenticated":true}"#
        ));
        assert!(!authentication_output_is_valid(
            "agent",
            br#"{"isAuthenticated":false}"#
        ));
        assert!(authentication_output_is_valid(
            "codex",
            b"Logged in using account"
        ));
        assert!(!authentication_output_is_valid(
            "opencode",
            b"0 credentials"
        ));
        assert!(authentication_output_is_valid(
            "hermes",
            b"provider (1 credentials)"
        ));
    }

    #[test]
    fn cursor_alias_uses_the_current_agent_adapter() {
        assert_eq!(
            BundledProvider::get("cursor").unwrap().descriptor().id,
            "agent"
        );
    }

    #[test]
    fn version_parser_accepts_bundled_formats_and_rejects_unknown_output() {
        assert_eq!(parse_version("2.1.220 (Claude Code)"), Some([2, 1, 220]));
        assert_eq!(parse_version("codex-cli 0.145.0"), Some([0, 145, 0]));
        assert_eq!(
            parse_version("Hermes Agent v0.18.2 (2026.7.7.2)"),
            Some([0, 18, 2])
        );
        assert_eq!(parse_version("2026.07.23-e383d2b"), Some([2026, 7, 23]));
        assert_eq!(parse_version("unknown"), None);
    }

    #[test]
    fn harvests_plain_json_and_jsonl_artifacts_but_not_unmarked_prose() {
        let body = "# S\n- RQ-001 R\n- AC-001 A";
        assert_eq!(
            extract_artifact(
                &format!("noise\n{SPEC_BEGIN}\n{body}\n{SPEC_END}\n"),
                SPEC_BEGIN,
                SPEC_END,
            )
            .as_deref(),
            Some(body)
        );
        let json = serde_json::json!({ "result": format!("{SPEC_BEGIN}\n{body}\n{SPEC_END}") });
        assert_eq!(
            extract_artifact(&json.to_string(), SPEC_BEGIN, SPEC_END).as_deref(),
            Some(body)
        );
        let jsonl = format!("{{\"event\":\"start\"}}\n{}", json);
        assert_eq!(
            extract_artifact(&jsonl, SPEC_BEGIN, SPEC_END).as_deref(),
            Some(body)
        );
        assert!(extract_artifact(body, SPEC_BEGIN, SPEC_END).is_none());
        assert!(
            extract_artifact(
                &format!("{SPEC_BEGIN}\n{body}\n{SPEC_END}\n{SPEC_BEGIN}\nsecond\n{SPEC_END}"),
                SPEC_BEGIN,
                SPEC_END,
            )
            .is_none()
        );
    }

    #[test]
    fn implementation_diff_harvest_restores_its_required_terminal_newline() {
        assert_eq!(
            finalize_artifact(ProviderOperation::ImplementationDiff, "diff line".into()),
            "diff line\n"
        );
        assert_eq!(
            finalize_artifact(ProviderOperation::Design, "design".into()),
            "design"
        );
    }

    #[test]
    fn installed_provider_help_accepts_the_bundled_argv_contract() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            (
                "claude",
                &["--help"],
                &[
                    "--safe-mode",
                    "--permission-mode",
                    "--no-session-persistence",
                    "--tools",
                    "--output-format",
                ],
            ),
            (
                "codex",
                &["exec", "--help"],
                &[
                    "--sandbox",
                    "--output-last-message",
                    "--ephemeral",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--color",
                ],
            ),
            (
                "agent",
                &["--help"],
                &[
                    "--mode",
                    "--output-format",
                    "--sandbox",
                    "--trust",
                    "--skip-worktree-setup",
                ],
            ),
            (
                "gemini",
                &["--help"],
                &[
                    "--approval-mode",
                    "--output-format",
                    "--sandbox",
                    "--skip-trust",
                ],
            ),
            (
                "hermes",
                &["chat", "--help"],
                &[
                    "--safe-mode",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--query",
                    "--max-turns",
                ],
            ),
            ("opencode", &["run", "--help"], &["--pure", "--format"]),
            (
                "aider",
                &["--help"],
                &[
                    "--config",
                    "--env-file",
                    "--aiderignore",
                    "--model-settings-file",
                    "--model-metadata-file",
                    "--dry-run",
                    "--no-git",
                    "--no-auto-commits",
                ],
            ),
        ];
        for (program, help_args, required) in cases {
            let Ok(executable) = which::which(program) else {
                continue;
            };
            let version = std::process::Command::new(&executable)
                .arg("--version")
                .output()
                .unwrap();
            assert!(version.status.success(), "{program} --version failed");
            assert!(
                !version.stdout.is_empty() || !version.stderr.is_empty(),
                "{program} returned no version"
            );
            let help = std::process::Command::new(&executable)
                .args(*help_args)
                .output()
                .unwrap();
            assert!(help.status.success(), "{program} help failed");
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&help.stdout),
                String::from_utf8_lossy(&help.stderr)
            );
            for flag in *required {
                assert!(
                    text.contains(flag),
                    "{program} help does not advertise {flag}"
                );
            }
        }
    }

    /// Release-only live gate. Ordinary contributors are not expected to have
    /// seven authenticated provider CLIs, but a release machine must run this
    /// exact ignored test explicitly and treat any unavailable adapter as a
    /// blocker.
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "release gate: requires every bundled provider and live authentication"]
    async fn release_gate_all_bundled_providers_are_installed_supported_and_authenticated() {
        let mut failures = Vec::new();
        for id in BUNDLED_IDS {
            let capability = probe_provider(BundledProvider::get(id).unwrap()).await;
            if !capability.available {
                failures.push(format!(
                    "{id}: {}",
                    capability
                        .reason
                        .as_deref()
                        .unwrap_or("unspecified provider failure")
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "release capability probes failed: {}",
            failures.join("; ")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bubblewrap_hides_poisoned_project_rules_and_preserves_the_originals() {
        if which::which("bwrap").is_err() || which::which("python3").is_err() {
            return;
        }
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let attempt = root.path().join("attempt");
        let staging = attempt.join(".agentum/staging/result");
        let sandbox = root.path().join("sandbox");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(staging.parent().unwrap()).unwrap();
        std::fs::create_dir_all(attempt.join(".cursor/rules")).unwrap();
        std::fs::create_dir_all(attempt.join("nested/CLAUDE.md")).unwrap();
        std::fs::create_dir(attempt.join("mcp.json")).unwrap();
        std::fs::create_dir(&sandbox).unwrap();
        populate_sandbox_directory(&sandbox).unwrap();
        std::fs::write(attempt.join("AGENTS.md"), b"POISON_AGENTUM_RULE\n").unwrap();
        std::fs::write(attempt.join(".cursorignore"), b"POISON_CURSOR_IGNORE\n").unwrap();
        std::fs::write(
            attempt.join(".cursor/rules/poison.mdc"),
            b"POISON_CURSOR_RULE\n",
        )
        .unwrap();
        std::fs::write(
            attempt.join("nested/CLAUDE.md/poison"),
            b"POISON_CLAUDE_RULE\n",
        )
        .unwrap();
        std::fs::write(attempt.join("mcp.json/poison"), b"POISON_MCP\n").unwrap();

        let old_home = std::env::var_os("HOME");
        let old_path = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("PATH", "/usr/local/bin:/usr/bin:/bin");
        }
        let code = r#"
from pathlib import Path
import sys
assert Path("AGENTS.md").read_bytes() == b""
assert Path(".cursorignore").read_bytes() == b""
assert list(Path(".cursor").iterdir()) == []
assert list(Path("nested/CLAUDE.md").iterdir()) == []
assert list(Path("mcp.json").iterdir()) == []
try:
    Path("AGENTS.md").write_text("provider mutation")
except OSError:
    pass
else:
    raise AssertionError("masked input was writable")
Path(sys.argv[1]).write_text("typed staging output\n")
"#;
        let isolated = isolate_command(
            CommandSpec {
                program: "python3".into(),
                args: vec![
                    "-c".into(),
                    code.into(),
                    staging.to_string_lossy().into_owned(),
                ],
                cwd: attempt.to_string_lossy().into_owned(),
                env_allowlist: vec!["PATH".into(), "HOME".into()],
                timeout_ms: 10_000,
                output_limit: 64 * 1024,
            },
            "agent",
            &staging.to_string_lossy(),
            &sandbox,
        )
        .unwrap();

        for (source, target) in [
            (
                sandbox.join("project-input-empty-file"),
                attempt.join("AGENTS.md"),
            ),
            (
                sandbox.join("project-input-empty-file"),
                attempt.join(".cursorignore"),
            ),
            (
                sandbox.join("project-input-empty-directory"),
                attempt.join(".cursor"),
            ),
            (
                sandbox.join("project-input-empty-directory"),
                attempt.join("nested/CLAUDE.md"),
            ),
            (
                sandbox.join("project-input-empty-directory"),
                attempt.join("mcp.json"),
            ),
        ] {
            assert!(isolated.args.windows(3).any(|arguments| {
                arguments[0] == "--ro-bind"
                    && arguments[1] == source.to_string_lossy()
                    && arguments[2] == target.to_string_lossy()
            }));
        }

        let output = std::process::Command::new(&isolated.program)
            .args(&isolated.args)
            .current_dir(&isolated.cwd)
            .env_clear()
            .output()
            .unwrap();
        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match old_path {
            Some(value) => unsafe { std::env::set_var("PATH", value) },
            None => unsafe { std::env::remove_var("PATH") },
        }

        assert_eq!(
            std::fs::read(attempt.join("AGENTS.md")).unwrap(),
            b"POISON_AGENTUM_RULE\n"
        );
        assert_eq!(
            std::fs::read(attempt.join(".cursorignore")).unwrap(),
            b"POISON_CURSOR_IGNORE\n"
        );
        assert_eq!(
            std::fs::read(attempt.join(".cursor/rules/poison.mdc")).unwrap(),
            b"POISON_CURSOR_RULE\n"
        );
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("Operation not permitted")
                || stderr.contains("Creating new namespace failed")
                || stderr.contains("No permissions to create new namespace")
            {
                return;
            }
            panic!("bubblewrap fixture failed: {stderr}");
        }
        assert_eq!(
            std::fs::read_to_string(staging).unwrap(),
            "typed staging output\n"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn symlinked_node_runtime_resolves_inside_bubblewrap() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        if which::which("bwrap").is_err() {
            return;
        }
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home/account");
        let installation = home.join(".local/share/fnm/node-versions/v22/installation");
        let package_bin = installation.join("lib/node_modules/@openai/codex/bin");
        std::fs::create_dir_all(&package_bin).unwrap();
        std::fs::create_dir_all(installation.join("bin")).unwrap();
        let script = package_bin.join("codex.js");
        std::fs::write(
            &script,
            b"#!/bin/sh\n[ ! -e \"$1\" ] || exit 91\n[ \"$2\" = NONE ] || [ ! -e \"$2\" ] || exit 92\n[ -s \"$3\" ] || exit 93\nprintf 'AGENTUM_SPEC_BEGIN\\n# Runtime fixture\\n- RQ-001 Requirement\\n- AC-001 Criterion\\nAGENTUM_SPEC_END\\n'\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();
        symlink(
            "../lib/node_modules/@openai/codex/bin/codex.js",
            installation.join("bin/codex"),
        )
        .unwrap();

        let aliases = home.join(".local/share/fnm/aliases");
        std::fs::create_dir_all(&aliases).unwrap();
        symlink(&installation, aliases.join("default")).unwrap();
        let multishells = root.path().join("run/user/1000/fnm_multishells");
        std::fs::create_dir_all(&multishells).unwrap();
        let launcher = multishells.join("session");
        symlink(aliases.join("default"), &launcher).unwrap();

        let credential = home.join(".codex/auth.json");
        std::fs::create_dir_all(credential.parent().unwrap()).unwrap();
        std::fs::write(&credential, b"{\"fixture\":true}\n").unwrap();
        let tmp_sentinel = tempfile::Builder::new()
            .prefix("agentum-provider-host-sentinel-")
            .tempfile_in("/tmp")
            .unwrap();
        let run_user = PathBuf::from(format!("/run/user/{}", unsafe { libc::geteuid() }));
        let run_sentinel = tempfile::Builder::new()
            .prefix("agentum-provider-host-sentinel-")
            .tempfile_in(&run_user)
            .ok();

        let attempt = root.path().join("attempt");
        let staging_dir = attempt.join(".agentum/staging");
        let staging = staging_dir.join("spec-output.md");
        let sandbox = root.path().join("sandbox");
        std::fs::create_dir_all(&staging_dir).unwrap();
        std::fs::create_dir_all(sandbox.join("runtime")).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_path = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var(
                "PATH",
                format!("{}:/usr/bin:/bin", launcher.join("bin").display()),
            );
        }

        let isolated = isolate_command(
            CommandSpec {
                program: "codex".into(),
                args: vec![
                    tmp_sentinel.path().to_string_lossy().into_owned(),
                    run_sentinel
                        .as_ref()
                        .map(|sentinel| sentinel.path().to_string_lossy().into_owned())
                        .unwrap_or_else(|| "NONE".into()),
                    credential.to_string_lossy().into_owned(),
                ],
                cwd: attempt.to_string_lossy().into_owned(),
                env_allowlist: vec!["PATH".into(), "HOME".into()],
                timeout_ms: 10_000,
                output_limit: 64 * 1024,
            },
            "codex",
            &staging.to_string_lossy(),
            &sandbox,
        )
        .unwrap();
        for masked in ["/tmp", "/var/tmp", "/run"] {
            assert!(
                isolated
                    .args
                    .windows(2)
                    .any(|values| values == ["--tmpfs", masked]),
                "{masked} must be masked in the provider sandbox"
            );
        }
        let output = std::process::Command::new(&isolated.program)
            .args(&isolated.args)
            .current_dir(&isolated.cwd)
            .env_clear()
            .output()
            .unwrap();

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match old_path {
            Some(value) => unsafe { std::env::set_var("PATH", value) },
            None => unsafe { std::env::remove_var("PATH") },
        }

        assert!(
            output.status.success(),
            "bubblewrap could not resolve the symlinked runtime: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("AGENTUM_SPEC_BEGIN"));
        assert!(tmp_sentinel.path().exists());
        if let Some(sentinel) = run_sentinel {
            assert!(sentinel.path().exists());
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)]
    async fn runner_harvests_only_the_declared_result_transport() {
        #[derive(Clone, Copy)]
        enum Behavior {
            MarkedStdout,
            MarkedStaging,
        }
        struct SourceAdapter {
            result_transport: ProviderResultTransport,
            behavior: Behavior,
        }
        impl SddProviderAdapter for SourceAdapter {
            fn descriptor(&self) -> ProviderDescriptor {
                ProviderDescriptor {
                    id: "result-source-test".into(),
                    version_probe: vec!["--version".into()],
                    capabilities: REQUIRED_OPERATIONS.to_vec(),
                    transport: ProviderTransport::HarvestArtifact,
                    result_transport: self.result_transport,
                    cancellation: ProviderCancellation::ProcessGroup,
                    isolation: ProviderIsolation::OsSandboxDisposableWorktree,
                    timeout_ms: 10_000,
                    output_limit: 64 * 1024,
                }
            }

            fn phase_command(
                &self,
                operation: ProviderOperation,
                cwd: &str,
                prompt: &str,
                staging_path: &str,
                _sandbox_dir: &str,
            ) -> CommandSpec {
                let marked = "AGENTUM_SPEC_BEGIN\\n# Fixture\\n- RQ-001 Requirement\\n- AC-001 Criterion\\nAGENTUM_SPEC_END\\n".to_owned();
                let script = match self.behavior {
                    Behavior::MarkedStdout => "printf '%b' \"$1\"; printf 'unmarked' > \"$2\"",
                    Behavior::MarkedStaging => "printf '%b' \"$1\" > \"$2\"; printf 'unmarked'",
                };
                CommandSpec {
                    program: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        script.into(),
                        format!("provider-{}-{prompt}", operation.name()),
                        marked,
                        staging_path.into(),
                    ],
                    cwd: cwd.into(),
                    env_allowlist: vec!["PATH".into(), "HOME".into()],
                    timeout_ms: 10_000,
                    output_limit: 64 * 1024,
                }
            }
        }

        if !isolation_available() {
            return;
        }
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let attempt = tempfile::tempdir().unwrap();
        let staging_dir = attempt.path().join(".agentum/staging");
        std::fs::create_dir_all(&staging_dir).unwrap();
        let old = std::env::var_os("AGENTUM_HOME");
        unsafe { std::env::set_var("AGENTUM_HOME", home.path()) };

        for (index, (result_transport, behavior, expected_success)) in [
            (
                ProviderResultTransport::Stdout,
                Behavior::MarkedStdout,
                true,
            ),
            (
                ProviderResultTransport::StagingArtifact,
                Behavior::MarkedStaging,
                true,
            ),
            (
                ProviderResultTransport::Stdout,
                Behavior::MarkedStaging,
                false,
            ),
            (
                ProviderResultTransport::StagingArtifact,
                Behavior::MarkedStdout,
                false,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let staging = staging_dir.join(format!("result-{index}.md"));
            let result = run_authoring(
                &format!("source-contract-{index}"),
                &SourceAdapter {
                    result_transport,
                    behavior,
                },
                &attempt.path().to_string_lossy(),
                "prompt",
                &staging.to_string_lossy(),
            )
            .await;
            assert_eq!(result.is_ok(), expected_success, "case {index}: {result:?}");
            if !expected_success {
                assert!(matches!(result, Err(ProviderError::MalformedOutput)));
            }
        }

        match old {
            Some(value) => unsafe { std::env::set_var("AGENTUM_HOME", value) },
            None => unsafe { std::env::remove_var("AGENTUM_HOME") },
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)]
    async fn os_sandbox_blocks_account_escape_and_allows_only_staging_output() {
        struct EscapeAttempt {
            outside: String,
        }
        impl SddProviderAdapter for EscapeAttempt {
            fn descriptor(&self) -> ProviderDescriptor {
                ProviderDescriptor {
                    id: "escape-test".into(),
                    version_probe: vec![],
                    capabilities: REQUIRED_OPERATIONS.to_vec(),
                    transport: ProviderTransport::HarvestArtifact,
                    result_transport: ProviderResultTransport::StagingArtifact,
                    cancellation: ProviderCancellation::ProcessGroup,
                    isolation: ProviderIsolation::OsSandboxDisposableWorktree,
                    timeout_ms: 10_000,
                    output_limit: 64 * 1024,
                }
            }

            fn phase_command(
                &self,
                _operation: ProviderOperation,
                cwd: &str,
                _prompt: &str,
                staging_path: &str,
                _sandbox_dir: &str,
            ) -> CommandSpec {
                let script = "if touch \"$1\"; then exit 97; fi; \
                    printf 'AGENTUM_SPEC_BEGIN\\n# S\\n- RQ-001 R\\n- AC-001 A\\nAGENTUM_SPEC_END\\n' > \"$2\"";
                CommandSpec {
                    program: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        script.into(),
                        "agentum-sandbox-test".into(),
                        self.outside.clone(),
                        staging_path.into(),
                    ],
                    cwd: cwd.into(),
                    env_allowlist: vec!["PATH".into(), "HOME".into()],
                    timeout_ms: 10_000,
                    output_limit: 64 * 1024,
                }
            }
        }

        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let attempt = tempfile::tempdir().unwrap();
        let staging_dir = attempt.path().join(".agentum/staging");
        std::fs::create_dir_all(&staging_dir).unwrap();
        let staging = staging_dir.join("spec-output.md");
        let old = std::env::var_os("AGENTUM_HOME");
        unsafe { std::env::set_var("AGENTUM_HOME", home.path()) };
        let outside = format!(
            "/var/tmp/agentum-sdd-sandbox-escape-{}",
            uuid::Uuid::new_v4()
        );
        let adapter = EscapeAttempt {
            outside: outside.clone(),
        };
        let result = run_authoring(
            "escape-test",
            &adapter,
            &attempt.path().to_string_lossy(),
            "ignored",
            &staging.to_string_lossy(),
        )
        .await;
        match old {
            Some(value) => unsafe { std::env::set_var("AGENTUM_HOME", value) },
            None => unsafe { std::env::remove_var("AGENTUM_HOME") },
        }
        assert!(result.is_ok(), "sandboxed provider failed: {result:?}");
        assert!(!Path::new(&outside).exists());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)]
    async fn cancellation_terminates_a_live_provider_group() {
        struct Sleeping;
        impl SddProviderAdapter for Sleeping {
            fn descriptor(&self) -> ProviderDescriptor {
                ProviderDescriptor {
                    id: "test-sleep".into(),
                    version_probe: vec![],
                    capabilities: REQUIRED_OPERATIONS.to_vec(),
                    transport: ProviderTransport::HarvestArtifact,
                    result_transport: ProviderResultTransport::Stdout,
                    cancellation: ProviderCancellation::ProcessGroup,
                    isolation: ProviderIsolation::OsSandboxDisposableWorktree,
                    timeout_ms: 30_000,
                    output_limit: 1024,
                }
            }

            fn phase_command(
                &self,
                _operation: ProviderOperation,
                cwd: &str,
                _prompt: &str,
                _staging_path: &str,
                _sandbox_dir: &str,
            ) -> CommandSpec {
                CommandSpec {
                    program: "sleep".into(),
                    args: vec!["30".into()],
                    cwd: cwd.into(),
                    env_allowlist: vec!["PATH".into()],
                    timeout_ms: 30_000,
                    output_limit: 1024,
                }
            }
        }

        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let attempt = tempfile::tempdir().unwrap();
        let old = std::env::var_os("AGENTUM_HOME");
        unsafe { std::env::set_var("AGENTUM_HOME", home.path()) };
        let staging = attempt.path().join("spec-output.md");
        let run = tokio::spawn(async move {
            run_authoring(
                "run-cancel",
                &Sleeping,
                &attempt.path().to_string_lossy(),
                "ignored",
                &staging.to_string_lossy(),
            )
            .await
        });
        for _ in 0..100 {
            if cancel_run("run-cancel") {
                break;
            }
            tokio::task::yield_now().await;
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("cancellation must not wait for provider timeout")
            .unwrap();
        assert!(matches!(result, Err(ProviderError::Canceled)));
        assert!(!cancel_run("run-cancel"));
        match old {
            Some(value) => unsafe { std::env::set_var("AGENTUM_HOME", value) },
            None => unsafe { std::env::remove_var("AGENTUM_HOME") },
        }
    }
}
