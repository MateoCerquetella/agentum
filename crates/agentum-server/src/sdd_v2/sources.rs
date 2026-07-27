//! Provider-neutral source normalization for Agentum-owned specifications.
//!
//! Source material is read-only input.  Adapters return bounded, deterministic
//! Markdown and typed planning intent; they never write provider configuration
//! or mutate an external work item.  Network-backed adapters live at the route
//! boundary, while the conventional OpenSpec converter stays independent of
//! the OpenSpec CLI.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use agentum_core::sdd::{ExternalReference, PlanArtifact, PlanTask, SpecId};
use serde::{Deserialize, Serialize};

use super::artifacts;
use super::sha256;

const MAX_SOURCE_FILE_BYTES: usize = 512 * 1024;
const MAX_SOURCE_TOTAL_BYTES: usize = 2 * 1024 * 1024;
const MAX_SOURCE_FILES: usize = 256;
const MAX_SOURCE_DIRECTORIES: usize = 512;
const MAX_SOURCE_DEPTH: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDiagnostic {
    pub severity: SourceDiagnosticSeverity,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceDiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedSource {
    pub kind: String,
    pub title: String,
    pub markdown: String,
    pub source_revision: String,
    pub source_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_reference: Option<ExternalReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design: Option<String>,
    #[serde(default)]
    pub tasks: Vec<ImportedTask>,
    #[serde(default)]
    pub diagnostics: Vec<SourceDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedTask {
    pub objective: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSpecExportPreview {
    pub destination: String,
    pub source_revision: String,
    pub files: Vec<OpenSpecExportFile>,
    #[serde(default)]
    pub diagnostics: Vec<SourceDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSpecExportFile {
    pub relative_path: String,
    pub content: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItemSource<'a> {
    pub provider: &'a str,
    pub connection_id: &'a str,
    pub site_id: Option<&'a str>,
    pub external_id: &'a str,
    pub key: Option<&'a str>,
    pub url: &'a str,
    pub source_revision: &'a str,
    pub title: &'a str,
    pub body: &'a str,
}

impl NormalizedSource {
    /// Bind imported task intent to the canonical identity allocated by
    /// Agentum. OpenSpec checklists do not carry safe file scopes or typed
    /// commands, so the conservative conversion is serial and scope-free.
    pub fn plan_tasks(&self, _spec_id: &SpecId) -> Vec<PlanTask> {
        // Identity is bound by the containing PlanArtifact.
        let mut previous: Option<String> = None;
        self.tasks
            .iter()
            .enumerate()
            .map(|(index, task)| {
                let id = format!("T-{:03}", index + 1);
                let dependencies = previous.iter().cloned().collect();
                previous = Some(id.clone());
                PlanTask {
                    id,
                    objective: task.objective.clone(),
                    dependencies,
                    read_scopes: Vec::new(),
                    write_scopes: Vec::new(),
                    acceptance_criteria: task.acceptance_criteria.clone(),
                    verification: Vec::new(),
                    browser_checks: Vec::new(),
                    risk: "unknown".into(),
                    parallel_safe: false,
                }
            })
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("source reference is unsafe: {0}")]
    UnsafeReference(String),
    #[error("source is unsupported: {0}")]
    Unsupported(String),
    #[error("source is malformed: {0}")]
    Malformed(String),
    #[error("source is too large: {0}")]
    TooLarge(String),
    #[error("source changed while it was being read: {0}")]
    Changed(String),
    #[error("source I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("artifact boundary rejected source input: {0}")]
    Artifact(#[from] artifacts::ArtifactError),
}

#[derive(Debug, Clone)]
struct SourceFile {
    relative: String,
    content: String,
    hash: String,
}

#[derive(Debug, Clone)]
struct Requirement {
    id: String,
    capability: String,
    operation: String,
    name: String,
    statement: String,
    scenarios: Vec<Scenario>,
}

#[derive(Debug, Clone)]
struct Scenario {
    id: String,
    name: String,
    body: String,
}

#[derive(Debug, Clone)]
struct ExportRequirement {
    id: String,
    capability: String,
    operation: String,
    name: String,
    statement: String,
    scenarios: Vec<ExportScenario>,
}

#[derive(Debug, Clone)]
struct ExportScenario {
    name: String,
    body: String,
    conventional_body: bool,
}

/// Normalize pasted Markdown without interpreting it as executable commands or
/// repository paths. A real authoring provider still converts this context to
/// the canonical Agentum requirement/acceptance-criteria structure.
pub fn normalize_markdown_intake(
    title: &str,
    markdown: &str,
) -> Result<NormalizedSource, SourceError> {
    let title = title.trim();
    if title.is_empty() || title.len() > 512 {
        return Err(SourceError::Malformed(
            "Markdown title must contain 1..=512 bytes".into(),
        ));
    }
    reject_controls("Markdown title", title)?;
    if markdown.len() > MAX_SOURCE_TOTAL_BYTES {
        return Err(SourceError::TooLarge("Markdown intake".into()));
    }
    let markdown = normalize_markdown(markdown)?;
    let revision = format!("sha256:{}", sha256(markdown.as_bytes()));
    Ok(NormalizedSource {
        kind: "markdown".into(),
        title: title.chars().take(160).collect(),
        markdown,
        source_revision: revision,
        source_path: "inline:markdown".into(),
        external_reference: None,
        design: None,
        tasks: Vec::new(),
        diagnostics: Vec::new(),
    })
}

/// Build a deterministic, one-shot conventional OpenSpec export preview. This
/// function is intentionally pure: the caller must bind the returned source
/// revision and diagnostics into an expiring preview token before using the
/// separate no-overwrite publication operation.
pub fn preview_openspec_export(
    spec_content: &str,
    design: Option<&str>,
    plan: Option<&str>,
) -> Result<OpenSpecExportPreview, SourceError> {
    let (header, body) = artifacts::parse_spec(spec_content)?;
    let short_id = &header.id.ulid()[..8];
    let slug = export_slug(&header.title);
    let destination = format!(
        "openspec/changes/agentum-{}-{}",
        short_id.to_ascii_lowercase(),
        slug
    );
    let requirement_lines = stable_lines(body, "RQ-");
    let criteria = stable_lines(body, "AC-");
    if requirement_lines.is_empty() || criteria.is_empty() {
        return Err(SourceError::Malformed(
            "Agentum spec has no exportable RQ/AC lines".into(),
        ));
    }

    let mut diagnostics = Vec::new();
    let mapping_is_explicit = criteria.iter().all(|(_, criterion)| {
        requirement_lines
            .iter()
            .any(|(id, _)| criterion_references_requirement(criterion, id))
    });
    if !mapping_is_explicit {
        diagnostics.push(SourceDiagnostic {
            severity: SourceDiagnosticSeverity::Warning,
            code: "openspec_lossy_acceptance_mapping".into(),
            message: "One or more Agentum acceptance criteria did not name an RQ identifier; export associated them deterministically by order. Review the delta before using it.".into(),
            path: None,
        });
    }

    let proposal = format!(
        "# {}\n\n## Why\n\nExported explicitly from Agentum specification {} revision {}.\n\n## What changes\n\n{}\n",
        header.title,
        header.id,
        header.revision,
        body.trim()
    );
    let requirements = export_requirements(&requirement_lines, &criteria, &slug);
    let mut files = vec![export_file("proposal.md", normalize_markdown(&proposal)?)];
    for (capability, delta) in render_export_deltas(&requirements) {
        files.push(export_file(
            &format!("specs/{capability}/spec.md"),
            normalize_markdown(&delta)?,
        ));
    }
    if let Some(design) = design {
        files.push(export_file("design.md", normalize_markdown(design)?));
    }
    if let Some(plan) = plan {
        let parsed: PlanArtifact = serde_json::from_str(plan)
            .map_err(|error| SourceError::Malformed(format!("invalid plan.json: {error}")))?;
        if parsed.spec_id != header.id || parsed.spec_revision != header.revision {
            return Err(SourceError::Malformed(
                "plan identity or revision does not match spec.md".into(),
            ));
        }
        let mut tasks = String::from("# Tasks\n\n");
        for task in &parsed.tasks {
            tasks.push_str(&format!("- [ ] {}\n", task.objective.trim()));
        }
        if parsed.tasks.iter().any(|task| {
            !task.verification.is_empty()
                || !task.read_scopes.is_empty()
                || !task.write_scopes.is_empty()
        }) {
            diagnostics.push(SourceDiagnostic {
                severity: SourceDiagnosticSeverity::Warning,
                code: "openspec_lossy_typed_plan".into(),
                message: "OpenSpec tasks.md cannot preserve typed CommandSpec verification or read/write scopes; those remain authoritative in Agentum plan.json.".into(),
                path: Some("tasks.md".into()),
            });
        }
        files.push(export_file("tasks.md", normalize_markdown(&tasks)?));
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let mut revision_payload = Vec::new();
    revision_payload.extend_from_slice(spec_content.as_bytes());
    if let Some(design) = design {
        revision_payload.extend_from_slice(design.as_bytes());
    }
    if let Some(plan) = plan {
        revision_payload.extend_from_slice(plan.as_bytes());
    }
    Ok(OpenSpecExportPreview {
        destination,
        source_revision: format!("sha256:{}", sha256(revision_payload)),
        files,
        diagnostics,
    })
}

fn export_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in value.to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            separator = false;
            slug.push(character);
        } else {
            separator = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug
        .trim_matches('-')
        .to_owned()
        .chars()
        .take(48)
        .collect::<String>();
    if slug.is_empty() {
        "spec".to_owned()
    } else {
        slug
    }
}

fn stable_lines(body: &str, prefix: &str) -> Vec<(String, String)> {
    body.lines()
        .filter_map(|line| {
            let line = line.trim_start_matches(|character: char| {
                character.is_whitespace() || matches!(character, '-' | '*')
            });
            let token = line.split_whitespace().next()?;
            if !token.starts_with(prefix)
                || !token[prefix.len()..]
                    .chars()
                    .all(|character| character.is_ascii_digit())
            {
                return None;
            }
            let statement = line[token.len()..]
                .trim_start_matches([':', '-', ' '])
                .trim();
            (!statement.is_empty()).then(|| (token.to_owned(), statement.to_owned()))
        })
        .collect()
}

fn export_requirements(
    requirements: &[(String, String)],
    criteria: &[(String, String)],
    fallback_capability: &str,
) -> Vec<ExportRequirement> {
    requirements
        .iter()
        .enumerate()
        .map(|(index, (id, raw_statement))| {
            let (capability, operation, name, statement) =
                imported_requirement_metadata(raw_statement).unwrap_or_else(|| {
                    (
                        fallback_capability.to_owned(),
                        "ADDED".to_owned(),
                        id.clone(),
                        raw_statement.clone(),
                    )
                });
            let mut associated = criteria
                .iter()
                .filter(|(_, criterion)| criterion_references_requirement(criterion, id))
                .collect::<Vec<_>>();
            if associated.is_empty() {
                if let Some(criterion) = criteria.get(index).or_else(|| criteria.first()) {
                    associated.push(criterion);
                }
            }
            let scenarios = associated
                .into_iter()
                .map(|(criterion_id, criterion)| {
                    imported_scenario_metadata(criterion, id, &capability).unwrap_or_else(|| {
                        ExportScenario {
                            name: criterion_id.clone(),
                            body: criterion.clone(),
                            conventional_body: false,
                        }
                    })
                })
                .collect();
            ExportRequirement {
                id: id.clone(),
                capability,
                operation,
                name,
                statement,
                scenarios,
            }
        })
        .collect()
}

/// Imported OpenSpec requirements carry lossless, human-readable provenance in
/// the Agentum RQ line. Recognizing that exact shape lets a later explicit
/// export preserve the original capability, operation, and names. Native
/// Agentum requirements use the deterministic ADDED fallback instead.
fn imported_requirement_metadata(statement: &str) -> Option<(String, String, String, String)> {
    let statement = statement.strip_prefix('[')?;
    let (metadata, statement) = statement.split_once("] ")?;
    let (capability, operation) = metadata.split_once(" / ")?;
    let operation = operation.to_ascii_uppercase();
    if !valid_change_name(capability)
        || !matches!(operation.as_str(), "ADDED" | "MODIFIED" | "REMOVED")
    {
        return None;
    }
    let (name, statement) = bold_name(statement)?;
    Some((
        capability.to_owned(),
        operation,
        name.to_owned(),
        statement.to_owned(),
    ))
}

fn imported_scenario_metadata(
    criterion: &str,
    requirement_id: &str,
    capability: &str,
) -> Option<ExportScenario> {
    let criterion = criterion.strip_prefix('[')?;
    let (metadata, criterion) = criterion.split_once("] ")?;
    let (candidate_requirement, candidate_capability) = metadata.split_once(" / ")?;
    if candidate_requirement != requirement_marker(requirement_id)
        || candidate_capability != capability
    {
        return None;
    }
    let (name, body) = bold_name(criterion)?;
    Some(ExportScenario {
        name: name.to_owned(),
        body: body.to_owned(),
        conventional_body: true,
    })
}

fn criterion_references_requirement(criterion: &str, requirement_id: &str) -> bool {
    criterion.contains(requirement_id) || criterion.contains(&requirement_marker(requirement_id))
}

fn requirement_marker(requirement_id: &str) -> String {
    requirement_id
        .strip_prefix("RQ-")
        .map(|number| format!("requirement-{number}"))
        .unwrap_or_else(|| requirement_id.to_owned())
}

fn bold_name(value: &str) -> Option<(&str, &str)> {
    let value = value.strip_prefix("**")?;
    let (name, body) = value.split_once(":**")?;
    let name = name.trim();
    let body = body.trim();
    (!name.is_empty() && !body.is_empty()).then_some((name, body))
}

fn render_export_deltas(requirements: &[ExportRequirement]) -> BTreeMap<String, String> {
    let mut grouped: BTreeMap<String, Vec<&ExportRequirement>> = BTreeMap::new();
    for requirement in requirements {
        grouped
            .entry(requirement.capability.clone())
            .or_default()
            .push(requirement);
    }
    grouped
        .into_iter()
        .map(|(capability, requirements)| {
            let mut output = String::new();
            for operation in ["ADDED", "MODIFIED", "REMOVED"] {
                let selected = requirements
                    .iter()
                    .filter(|requirement| requirement.operation == operation)
                    .copied()
                    .collect::<Vec<_>>();
                if selected.is_empty() {
                    continue;
                }
                output.push_str(&format!("## {operation} Requirements\n\n"));
                for requirement in selected {
                    output.push_str(&format!(
                        "### Requirement: {}\n{}\n\n",
                        requirement.name, requirement.statement
                    ));
                    for scenario in &requirement.scenarios {
                        let name = if scenario.name.is_empty() {
                            &requirement.id
                        } else {
                            &scenario.name
                        };
                        output.push_str(&format!("#### Scenario: {name}\n"));
                        if scenario.conventional_body {
                            output.push_str(&scenario.body);
                            output.push_str("\n\n");
                        } else {
                            output.push_str(&format!("- THEN {}\n\n", scenario.body));
                        }
                    }
                }
            }
            (capability, output)
        })
        .collect()
}

fn export_file(relative_path: &str, content: String) -> OpenSpecExportFile {
    OpenSpecExportFile {
        relative_path: relative_path.into(),
        content_hash: sha256(content.as_bytes()),
        content,
    }
}

/// Normalize a read-only work item after its provider adapter has fetched the
/// authoritative revision. Mutating comments, fields, or status is deliberately
/// outside this operation and remains unavailable until Deliver.
pub fn normalize_work_item(input: WorkItemSource<'_>) -> Result<NormalizedSource, SourceError> {
    if !matches!(input.provider, "github" | "linear" | "jira") {
        return Err(SourceError::Unsupported(format!(
            "unknown work-item provider {:?}",
            input.provider
        )));
    }
    for (label, value, maximum) in [
        ("connectionId", input.connection_id, 256usize),
        ("externalId", input.external_id, 256),
        ("sourceRevision", input.source_revision, 512),
        ("title", input.title, 512),
    ] {
        if value.trim().is_empty() || value.len() > maximum {
            return Err(SourceError::Malformed(format!(
                "{label} must contain 1..={maximum} bytes"
            )));
        }
        reject_controls(label, value)?;
    }
    if input.body.len() > MAX_SOURCE_TOTAL_BYTES {
        return Err(SourceError::TooLarge("work-item body".into()));
    }
    reject_controls("work-item body", input.body)?;
    let url = input.url.trim();
    if !url.starts_with("https://") || url.len() > 4096 || url.chars().any(char::is_whitespace) {
        return Err(SourceError::Malformed(
            "work-item URL must be a bounded HTTPS URL".into(),
        ));
    }
    if let Some(site_id) = input.site_id {
        reject_bounded_optional("siteId", site_id, 256)?;
    }
    if let Some(key) = input.key {
        reject_bounded_optional("key", key, 256)?;
    }

    let title = input.title.trim().chars().take(160).collect::<String>();
    let body = if input.body.trim().is_empty() {
        "No description was supplied by the external work item.\n".into()
    } else {
        normalize_markdown(input.body)?
    };
    let markdown = format!(
        "# {title}\n\n## Imported work-item context\n\n{}\n## Source provenance\n\n- Provider: {}\n- External ID: {}\n- Source revision: {}\n- URL: {}\n",
        body.trim(),
        input.provider,
        input.external_id,
        input.source_revision,
        url
    );
    Ok(NormalizedSource {
        kind: input.provider.into(),
        title,
        markdown,
        source_revision: input.source_revision.into(),
        source_path: url.into(),
        external_reference: Some(ExternalReference {
            provider: input.provider.into(),
            connection_id: input.connection_id.into(),
            site_id: input.site_id.map(str::to_owned),
            external_id: input.external_id.into(),
            key: input.key.map(str::to_owned),
            url: url.into(),
            source_revision: input.source_revision.into(),
        }),
        design: None,
        tasks: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn reject_bounded_optional(label: &str, value: &str, maximum: usize) -> Result<(), SourceError> {
    if value.trim().is_empty() || value.len() > maximum {
        return Err(SourceError::Malformed(format!(
            "{label} must contain 1..={maximum} bytes when supplied"
        )));
    }
    reject_controls(label, value)
}

/// Import one conventional `spec-driven` OpenSpec change from either the
/// active or archived changes tree. The supplied reference is repository
/// relative and must name the change directory itself.
pub fn import_openspec(
    repository: &Path,
    reference: &str,
) -> Result<NormalizedSource, SourceError> {
    import_openspec_with_hook(repository, reference, || {})
}

fn import_openspec_with_hook<F>(
    repository: &Path,
    reference: &str,
    after_open: F,
) -> Result<NormalizedSource, SourceError>
where
    F: FnOnce(),
{
    let relative = validate_openspec_reference(reference)?;
    let repository = artifacts::AnchoredDirectory::open(repository)?;
    let (openspec, root) = open_openspec_change(&repository, &relative)?;
    after_open();
    let files = collect_change_files(&root)?;
    let schema_dependency = validate_schema(&openspec, &files)?;
    validate_known_files(&files)?;

    let proposal = required_file(&files, "proposal.md")?;
    let title = first_heading(&proposal.content)
        .unwrap_or_else(|| change_name(&relative).replace('-', " "));
    let skip_specs = files
        .get(".openspec.yaml")
        .is_some_and(|file| yaml_bool(&file.content, "skip_specs") == Some(true));
    let spec_files: Vec<&SourceFile> = files
        .values()
        .filter(|file| is_delta_spec(&file.relative))
        .collect();
    if spec_files.is_empty() && !skip_specs {
        return Err(SourceError::Malformed(
            "a spec-driven change requires at least one specs/<capability>/spec.md delta".into(),
        ));
    }

    let mut requirements = Vec::new();
    let mut requirement_index = 1usize;
    let mut scenario_index = 1usize;
    for file in spec_files {
        let capability = file
            .relative
            .strip_prefix("specs/")
            .and_then(|value| value.strip_suffix("/spec.md"))
            .unwrap_or("unknown");
        let parsed = parse_delta_spec(
            capability,
            &file.content,
            &mut requirement_index,
            &mut scenario_index,
        )?;
        requirements.extend(parsed);
    }

    let mut diagnostics = Vec::new();
    if skip_specs {
        diagnostics.push(SourceDiagnostic {
            severity: SourceDiagnosticSeverity::Warning,
            code: "openspec_skip_specs".into(),
            message: "The change explicitly skips delta specs; proposal intent will be authored into Agentum requirements before approval.".into(),
            path: Some(".openspec.yaml".into()),
        });
    }
    if files.contains_key("README.md") {
        diagnostics.push(SourceDiagnostic {
            severity: SourceDiagnosticSeverity::Info,
            code: "openspec_readme_context".into(),
            message: "README.md was preserved as source context and does not become a separate Agentum artifact.".into(),
            path: Some("README.md".into()),
        });
    }

    let markdown = render_imported_spec(&proposal.content, &requirements, files.get("README.md"));
    let design = files
        .get("design.md")
        .map(|file| normalize_markdown(&file.content))
        .transpose()?;
    let tasks = files
        .get("tasks.md")
        .map(|file| parse_tasks(&file.content))
        .transpose()?
        .unwrap_or_default();
    if files.contains_key("tasks.md") && tasks.is_empty() {
        return Err(SourceError::Malformed(
            "tasks.md contains no Markdown checklist items".into(),
        ));
    }

    // A held handle prevents an attacker from redirecting any read, while the
    // final identity check also makes a parent replacement fail closed instead
    // of silently importing a now-detached directory snapshot.
    let (_, rebound_root) = open_openspec_change(&repository, &relative)?;
    if !root.same_identity(&rebound_root)? {
        return Err(SourceError::Changed(
            "OpenSpec change directory was replaced during import".into(),
        ));
    }

    let source_revision = source_revision(&files, schema_dependency.as_deref());
    Ok(NormalizedSource {
        kind: "openspec".into(),
        title: title.trim().chars().take(160).collect(),
        markdown,
        source_revision,
        source_path: relative.to_string_lossy().replace('\\', "/"),
        external_reference: None,
        design,
        tasks,
        diagnostics,
    })
}

fn open_openspec_change(
    repository: &artifacts::AnchoredDirectory,
    relative: &Path,
) -> Result<(artifacts::AnchoredDirectory, artifacts::AnchoredDirectory), SourceError> {
    let parts = relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => value
                .to_str()
                .ok_or_else(|| SourceError::UnsafeReference(relative.display().to_string())),
            _ => Err(SourceError::UnsafeReference(relative.display().to_string())),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let openspec = repository.open_child("openspec")?;
    let changes = openspec.open_child("changes")?;
    let root = if parts.len() == 3 {
        changes.open_child(parts[2])?
    } else {
        changes.open_child("archive")?.open_child(parts[3])?
    };
    Ok((openspec, root))
}

fn validate_openspec_reference(reference: &str) -> Result<PathBuf, SourceError> {
    let normalized = reference.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err(SourceError::UnsafeReference(
            "OpenSpec change path is required".into(),
        ));
    }
    let path = Path::new(&normalized);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(SourceError::UnsafeReference(normalized));
    }
    let parts: Vec<_> = path
        .components()
        .filter_map(|part| match part {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect();
    let active = parts.len() == 3
        && parts[0] == "openspec"
        && parts[1] == "changes"
        && parts[2] != "archive";
    let archived = parts.len() == 4
        && parts[0] == "openspec"
        && parts[1] == "changes"
        && parts[2] == "archive";
    if (!active && !archived)
        || !parts.last().is_some_and(|name| valid_change_name(name))
        || (archived && !valid_archived_name(parts[3]))
    {
        return Err(SourceError::UnsafeReference(normalized));
    }
    Ok(path.to_path_buf())
}

fn valid_change_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn valid_archived_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() > 11
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && bytes[10] == b'-'
        && valid_change_name(&value[11..])
}

fn change_name(reference: &Path) -> &str {
    reference
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("imported-change")
}

fn collect_change_files(
    change_root: &artifacts::AnchoredDirectory,
) -> Result<BTreeMap<String, SourceFile>, SourceError> {
    let first = collect_change_files_once(change_root)?;
    let second = collect_change_files_once(change_root)?;
    let first_snapshot: Vec<_> = first
        .iter()
        .map(|(path, file)| (path.as_str(), file.hash.as_str()))
        .collect();
    let second_snapshot: Vec<_> = second
        .iter()
        .map(|(path, file)| (path.as_str(), file.hash.as_str()))
        .collect();
    if first_snapshot != second_snapshot {
        return Err(SourceError::Changed(
            "OpenSpec change files changed during the import snapshot".into(),
        ));
    }
    Ok(first)
}

fn collect_change_files_once(
    change_root: &artifacts::AnchoredDirectory,
) -> Result<BTreeMap<String, SourceFile>, SourceError> {
    struct PendingDirectory {
        directory: artifacts::AnchoredDirectory,
        relative: String,
        depth: usize,
    }

    let mut pending = vec![PendingDirectory {
        directory: change_root.try_clone()?,
        relative: String::new(),
        depth: 0,
    }];
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    let mut directories = 0usize;
    while let Some(current) = pending.pop() {
        directories += 1;
        if directories > MAX_SOURCE_DIRECTORIES {
            return Err(SourceError::TooLarge(format!(
                "more than {MAX_SOURCE_DIRECTORIES} directories"
            )));
        }
        let mut entries = current.directory.entries()?;
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        for entry in entries {
            let depth = current.depth + 1;
            let relative = if current.relative.is_empty() {
                entry.name.clone()
            } else {
                format!("{}/{}", current.relative, entry.name)
            };
            if depth > MAX_SOURCE_DEPTH {
                return Err(SourceError::TooLarge(format!(
                    "source path exceeds {MAX_SOURCE_DEPTH} components: {relative}"
                )));
            }
            match entry.kind {
                artifacts::AnchoredEntryKind::Directory => {
                    pending.push(PendingDirectory {
                        directory: current.directory.open_child(&entry.name)?,
                        relative,
                        depth,
                    });
                }
                artifacts::AnchoredEntryKind::File => {
                    if files.len() >= MAX_SOURCE_FILES {
                        return Err(SourceError::TooLarge(format!(
                            "more than {MAX_SOURCE_FILES} files"
                        )));
                    }
                    let (bytes, hash) = current.directory.read_file(&entry.name)?;
                    if bytes.len() > MAX_SOURCE_FILE_BYTES {
                        return Err(SourceError::TooLarge(relative));
                    }
                    total = total.saturating_add(bytes.len());
                    if total > MAX_SOURCE_TOTAL_BYTES {
                        return Err(SourceError::TooLarge(format!(
                            "change exceeds {MAX_SOURCE_TOTAL_BYTES} bytes"
                        )));
                    }
                    let content = String::from_utf8(bytes)
                        .map_err(|_| SourceError::Malformed(format!("{relative} is not UTF-8")))?;
                    reject_controls(&relative, &content)?;
                    files.insert(
                        relative.clone(),
                        SourceFile {
                            relative,
                            content,
                            hash,
                        },
                    );
                }
            }
        }
    }
    Ok(files)
}

fn reject_controls(path: &str, value: &str) -> Result<(), SourceError> {
    if value.chars().any(|character| {
        character == '\0' || (character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    }) {
        return Err(SourceError::Malformed(format!(
            "{path} contains unsupported control characters"
        )));
    }
    Ok(())
}

fn validate_schema(
    openspec: &artifacts::AnchoredDirectory,
    files: &BTreeMap<String, SourceFile>,
) -> Result<Option<String>, SourceError> {
    let change_schema = files
        .get(".openspec.yaml")
        .and_then(|file| yaml_scalar(&file.content, "schema"));
    let (project_schema, schema_dependency) = if change_schema.is_none() {
        let first = openspec.read_file_optional("config.yaml")?;
        let second = openspec.read_file_optional("config.yaml")?;
        let first_snapshot = first
            .as_ref()
            .map(|(bytes, hash)| (bytes.as_slice(), hash.as_str()));
        let second_snapshot = second
            .as_ref()
            .map(|(bytes, hash)| (bytes.as_slice(), hash.as_str()));
        if first_snapshot != second_snapshot {
            return Err(SourceError::Changed(
                "OpenSpec project schema changed during import".into(),
            ));
        }
        match first {
            Some((bytes, hash)) => {
                let content = String::from_utf8(bytes).map_err(|_| {
                    SourceError::Malformed("openspec/config.yaml is not UTF-8".into())
                })?;
                reject_controls("openspec/config.yaml", &content)?;
                (yaml_scalar(&content, "schema"), Some(hash))
            }
            None => (None, None),
        }
    } else {
        (None, None)
    };
    let schema = change_schema.or(project_schema);
    if schema
        .as_deref()
        .is_some_and(|value| value != "spec-driven")
    {
        return Err(SourceError::Unsupported(format!(
            "custom OpenSpec schema {:?}; only spec-driven can be converted without guessing",
            schema.unwrap_or_default()
        )));
    }
    Ok(schema_dependency)
}

fn yaml_scalar(value: &str, key: &str) -> Option<String> {
    value.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return None;
        }
        let (candidate, raw) = line.split_once(':')?;
        (candidate.trim() == key).then(|| {
            raw.trim()
                .trim_matches(|character| matches!(character, '\'' | '"'))
                .to_owned()
        })
    })
}

fn yaml_bool(value: &str, key: &str) -> Option<bool> {
    match yaml_scalar(value, key)?.to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn validate_known_files(files: &BTreeMap<String, SourceFile>) -> Result<(), SourceError> {
    let unknown: Vec<_> = files
        .keys()
        .filter(|path| {
            !matches!(
                path.as_str(),
                ".openspec.yaml" | "README.md" | "proposal.md" | "design.md" | "tasks.md"
            ) && !is_delta_spec(path)
        })
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Err(SourceError::Unsupported(format!(
            "unknown OpenSpec change files: {}",
            unknown.join(", ")
        )));
    }
    Ok(())
}

fn is_delta_spec(path: &str) -> bool {
    let components: Vec<_> = path.split('/').collect();
    components.len() == 3
        && components[0] == "specs"
        && valid_change_name(components[1])
        && components[2] == "spec.md"
}

fn required_file<'a>(
    files: &'a BTreeMap<String, SourceFile>,
    path: &str,
) -> Result<&'a SourceFile, SourceError> {
    files
        .get(path)
        .filter(|file| !file.content.trim().is_empty())
        .ok_or_else(|| SourceError::Malformed(format!("missing or empty {path}")))
}

fn first_heading(markdown: &str) -> Option<String> {
    markdown.lines().find_map(|line| {
        line.strip_prefix("# ")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn parse_delta_spec(
    capability: &str,
    markdown: &str,
    requirement_index: &mut usize,
    scenario_index: &mut usize,
) -> Result<Vec<Requirement>, SourceError> {
    let lines: Vec<_> = markdown.lines().collect();
    let mut operation = String::new();
    let mut requirements = Vec::new();
    let mut cursor = 0usize;
    while cursor < lines.len() {
        let line = lines[cursor].trim();
        if let Some(section) = line
            .strip_prefix("## ")
            .and_then(|value| value.strip_suffix(" Requirements"))
        {
            let candidate = section.trim().to_ascii_uppercase();
            if matches!(candidate.as_str(), "ADDED" | "MODIFIED" | "REMOVED") {
                operation = candidate;
            }
            cursor += 1;
            continue;
        }
        let Some(name) = line.strip_prefix("### Requirement:").map(str::trim) else {
            cursor += 1;
            continue;
        };
        if operation.is_empty() || name.is_empty() {
            return Err(SourceError::Malformed(format!(
                "specs/{capability}/spec.md has a requirement outside ADDED/MODIFIED/REMOVED"
            )));
        }
        let id = format!("RQ-{:03}", *requirement_index);
        *requirement_index += 1;
        cursor += 1;
        let mut statement = Vec::new();
        let mut scenarios = Vec::new();
        while cursor < lines.len() {
            let current = lines[cursor].trim();
            if current.starts_with("### Requirement:") || current.starts_with("## ") {
                break;
            }
            if let Some(scenario_name) = current.strip_prefix("#### Scenario:").map(str::trim) {
                if scenario_name.is_empty() {
                    return Err(SourceError::Malformed(format!(
                        "specs/{capability}/spec.md has an unnamed scenario"
                    )));
                }
                cursor += 1;
                let mut body = Vec::new();
                while cursor < lines.len() {
                    let scenario_line = lines[cursor].trim();
                    if scenario_line.starts_with("#### Scenario:")
                        || scenario_line.starts_with("### Requirement:")
                        || scenario_line.starts_with("## ")
                    {
                        break;
                    }
                    if !scenario_line.is_empty() {
                        body.push(scenario_line);
                    }
                    cursor += 1;
                }
                if body.is_empty() {
                    return Err(SourceError::Malformed(format!(
                        "scenario {scenario_name:?} in specs/{capability}/spec.md is empty"
                    )));
                }
                let scenario_id = format!("AC-{:03}", *scenario_index);
                *scenario_index += 1;
                scenarios.push(Scenario {
                    id: scenario_id,
                    name: scenario_name.to_owned(),
                    body: body.join(" "),
                });
                continue;
            }
            if !current.is_empty() {
                statement.push(current);
            }
            cursor += 1;
        }
        if statement.is_empty() || scenarios.is_empty() {
            return Err(SourceError::Malformed(format!(
                "requirement {name:?} in specs/{capability}/spec.md needs a statement and scenario"
            )));
        }
        requirements.push(Requirement {
            id,
            capability: capability.into(),
            operation: operation.clone(),
            name: name.into(),
            statement: statement.join(" "),
            scenarios,
        });
    }
    if requirements.is_empty() {
        return Err(SourceError::Malformed(format!(
            "specs/{capability}/spec.md has no conventional requirements"
        )));
    }
    Ok(requirements)
}

fn render_imported_spec(
    proposal: &str,
    requirements: &[Requirement],
    readme: Option<&SourceFile>,
) -> String {
    let mut output = String::from("# Imported OpenSpec change\n\n## Goal and scope\n\n");
    output.push_str(proposal.trim());
    output.push_str("\n\n## Requirements\n\n");
    if requirements.is_empty() {
        output.push_str("- RQ-001 Preserve the proposal intent as testable behavior without expanding its scope.\n");
    } else {
        for requirement in requirements {
            output.push_str(&format!(
                "- {} [{} / {}] **{}:** {}\n",
                requirement.id,
                requirement.capability,
                requirement.operation,
                requirement.name,
                requirement.statement
            ));
        }
    }
    output.push_str("\n## Acceptance criteria\n\n");
    if requirements.is_empty() {
        output.push_str("- AC-001 The approved Agentum spec represents the proposal's complete observable intent.\n");
    } else {
        for requirement in requirements {
            for scenario in &requirement.scenarios {
                output.push_str(&format!(
                    "- {} [{} / {}] **{}:** {}\n",
                    scenario.id,
                    requirement_marker(&requirement.id),
                    requirement.capability,
                    scenario.name,
                    scenario.body
                ));
            }
        }
    }
    if let Some(readme) = readme {
        output.push_str("\n## Additional source context\n\n");
        output.push_str(readme.content.trim());
        output.push('\n');
    }
    normalize_markdown(&output).expect("generated Markdown has no forbidden controls")
}

fn normalize_markdown(value: &str) -> Result<String, SourceError> {
    reject_controls("Markdown", value)?;
    let mut normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    normalized = normalized.trim().to_owned();
    if normalized.is_empty() {
        return Err(SourceError::Malformed("empty Markdown artifact".into()));
    }
    normalized.push('\n');
    Ok(normalized)
}

fn parse_tasks(markdown: &str) -> Result<Vec<ImportedTask>, SourceError> {
    reject_controls("tasks.md", markdown)?;
    let mut tasks = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed
            .strip_prefix("- [ ] ")
            .or_else(|| trimmed.strip_prefix("- [x] "))
            .or_else(|| trimmed.strip_prefix("- [X] "))
            .or_else(|| trimmed.strip_prefix("* [ ] "))
            .or_else(|| trimmed.strip_prefix("* [x] "))
            .or_else(|| trimmed.strip_prefix("* [X] "))
        else {
            continue;
        };
        let objective = rest.trim();
        if objective.is_empty() {
            return Err(SourceError::Malformed(
                "tasks.md has an empty checklist item".into(),
            ));
        }
        let acceptance_criteria = stable_references(objective, "AC-");
        tasks.push(ImportedTask {
            objective: objective.chars().take(500).collect(),
            acceptance_criteria,
        });
    }
    Ok(tasks)
}

fn stable_references(value: &str, prefix: &str) -> Vec<String> {
    let upper = value.to_ascii_uppercase();
    let mut references = BTreeSet::new();
    for token in
        upper.split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
    {
        let Some(number) = token.strip_prefix(prefix) else {
            continue;
        };
        if !number.is_empty() && number.chars().all(|character| character.is_ascii_digit()) {
            if let Ok(number) = number.parse::<u32>() {
                references.insert(format!("{prefix}{number:03}"));
            }
        }
    }
    references.into_iter().collect()
}

fn source_revision(
    files: &BTreeMap<String, SourceFile>,
    schema_dependency: Option<&str>,
) -> String {
    let mut payload = Vec::new();
    for file in files.values() {
        payload.extend_from_slice(file.relative.as_bytes());
        payload.push(0);
        payload.extend_from_slice(file.hash.as_bytes());
        payload.push(b'\n');
    }
    if let Some(hash) = schema_dependency {
        payload.extend_from_slice(b"openspec/config.yaml\0");
        payload.extend_from_slice(hash.as_bytes());
        payload.push(b'\n');
    }
    format!("sha256:{}", sha256(payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFICIAL_FIXTURE_REFERENCE: &str =
        "openspec/changes/archive/2025-10-14-update-cli-init-enter-selection";

    fn official_fixture() -> tempfile::TempDir {
        let repository = tempfile::tempdir().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/openspec/official/changes/archive")
            .join("2025-10-14-update-cli-init-enter-selection");
        let destination = repository.path().join(OFFICIAL_FIXTURE_REFERENCE);
        for relative in ["proposal.md", "specs/cli-init/spec.md", "tasks.md"] {
            let target = destination.join(relative);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::copy(fixture.join(relative), target).unwrap();
        }
        repository
    }

    fn fixture() -> tempfile::TempDir {
        let repository = tempfile::tempdir().unwrap();
        let change = repository
            .path()
            .join("openspec/changes/add-session-refresh");
        std::fs::create_dir_all(change.join("specs/auth")).unwrap();
        std::fs::write(
            change.join("proposal.md"),
            "# Refresh active sessions\n\nKeep active sessions online while refreshing tokens.\n",
        )
        .unwrap();
        std::fs::write(
            change.join("specs/auth/spec.md"),
            "## ADDED Requirements\n\n### Requirement: Atomic refresh\nThe system MUST replace an access token atomically.\n\n#### Scenario: Active session\n- GIVEN an active session\n- WHEN its token refreshes\n- THEN the session remains usable\n",
        )
        .unwrap();
        std::fs::write(change.join("design.md"), "# Design\n\nUse a swap.\n").unwrap();
        std::fs::write(
            change.join("tasks.md"),
            "# Tasks\n\n- [ ] Add atomic swap (AC-001)\n- [ ] Verify active sessions\n",
        )
        .unwrap();
        repository
    }

    #[test]
    fn imports_conventional_change_without_cli_or_generated_configuration() {
        let repository = fixture();
        let imported =
            import_openspec(repository.path(), "openspec/changes/add-session-refresh").unwrap();
        assert_eq!(imported.kind, "openspec");
        assert_eq!(imported.title, "Refresh active sessions");
        assert!(imported.markdown.contains("RQ-001"));
        assert!(imported.markdown.contains("AC-001"));
        assert_eq!(
            imported.design.as_deref(),
            Some("# Design\n\nUse a swap.\n")
        );
        assert_eq!(imported.tasks.len(), 2);
        assert_eq!(imported.tasks[0].acceptance_criteria, ["AC-001"]);
        assert!(imported.source_revision.starts_with("sha256:"));
        assert!(!repository.path().join(".agentum").exists());
        assert!(!repository.path().join(".claude").exists());
    }

    #[test]
    fn openspec_official_golden_import_export_round_trip_is_deterministic() {
        let repository = official_fixture();
        let first = import_openspec(repository.path(), OFFICIAL_FIXTURE_REFERENCE).unwrap();
        let second = import_openspec(repository.path(), OFFICIAL_FIXTURE_REFERENCE).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.source_revision,
            "sha256:b936f2bdfd80a75d6b9f966c848682dc458ca8f89f74c7634f20f681a32a6ea4"
        );
        assert_eq!(first.tasks.len(), 5);
        assert!(
            first
                .markdown
                .contains("[cli-init / MODIFIED] **Interactive Mode:**")
        );
        assert!(first.markdown.contains("**Displaying interactive menu:**"));

        let id: SpecId = "SPC-01K123456789ABCDEFGHJKMNPQ".parse().unwrap();
        let spec = artifacts::render_spec(&id, 1, &first.title, None, &first.markdown).unwrap();
        let tasks = first.plan_tasks(&id);
        let plan = serde_json::to_string(&PlanArtifact {
            schema_version: 1,
            spec_id: id,
            spec_revision: 1,
            tasks,
        })
        .unwrap();
        let export = preview_openspec_export(&spec, first.design.as_deref(), Some(&plan)).unwrap();
        assert_eq!(
            export,
            preview_openspec_export(&spec, first.design.as_deref(), Some(&plan)).unwrap()
        );
        let delta = export
            .files
            .iter()
            .find(|file| file.relative_path == "specs/cli-init/spec.md")
            .expect("round-trip preserves the official capability path");
        assert!(delta.content.contains("## MODIFIED Requirements"));
        assert!(delta.content.contains("### Requirement: Interactive Mode"));
        assert!(
            delta
                .content
                .contains("#### Scenario: Displaying interactive menu")
        );
        assert!(!delta.content.contains("## ADDED Requirements"));

        let round_trip_repository = tempfile::tempdir().unwrap();
        let destination = round_trip_repository.path().join(&export.destination);
        for file in &export.files {
            let target = destination.join(&file.relative_path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, &file.content).unwrap();
        }
        let round_trip =
            import_openspec(round_trip_repository.path(), &export.destination).unwrap();
        assert_eq!(round_trip.tasks, first.tasks);
        assert!(
            round_trip
                .markdown
                .contains("[cli-init / MODIFIED] **Interactive Mode:**")
        );
        assert!(
            round_trip
                .markdown
                .contains("**Displaying interactive menu:**")
        );
        assert_eq!(
            round_trip,
            import_openspec(round_trip_repository.path(), &export.destination).unwrap()
        );
    }

    #[test]
    fn openspec_archived_change_and_skip_specs_are_explicitly_supported() {
        let repository = tempfile::tempdir().unwrap();
        let change = repository
            .path()
            .join("openspec/changes/archive/2026-07-26-doc-only");
        std::fs::create_dir_all(&change).unwrap();
        std::fs::write(change.join("proposal.md"), "# Docs\n\nClarify usage.\n").unwrap();
        std::fs::write(
            change.join(".openspec.yaml"),
            "schema: spec-driven\nskip_specs: true\n",
        )
        .unwrap();
        let imported = import_openspec(
            repository.path(),
            "openspec/changes/archive/2026-07-26-doc-only",
        )
        .unwrap();
        assert!(imported.markdown.contains("RQ-001"));
        assert!(imported.markdown.contains("AC-001"));
        assert_eq!(imported.diagnostics[0].code, "openspec_skip_specs");
    }

    #[test]
    fn openspec_rejects_custom_schema_unknown_files_and_unsafe_paths() {
        let repository = fixture();
        let change = repository
            .path()
            .join("openspec/changes/add-session-refresh");
        std::fs::write(change.join(".openspec.yaml"), "schema: custom-flow\n").unwrap();
        assert!(matches!(
            import_openspec(repository.path(), "openspec/changes/add-session-refresh"),
            Err(SourceError::Unsupported(_))
        ));
        std::fs::write(change.join(".openspec.yaml"), "schema: spec-driven\n").unwrap();
        std::fs::write(change.join("unknown.bin"), b"unknown").unwrap();
        assert!(matches!(
            import_openspec(repository.path(), "openspec/changes/add-session-refresh"),
            Err(SourceError::Unsupported(_))
        ));
        assert!(matches!(
            import_openspec(repository.path(), "../outside"),
            Err(SourceError::UnsafeReference(_))
        ));
        assert!(matches!(
            import_openspec(repository.path(), "/tmp/change"),
            Err(SourceError::UnsafeReference(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_change_components_and_files() {
        let repository = fixture();
        let change = repository
            .path()
            .join("openspec/changes/add-session-refresh");
        let outside = repository.path().join("outside.md");
        std::fs::write(&outside, "outside").unwrap();
        std::os::unix::fs::symlink(&outside, change.join("README.md")).unwrap();
        assert!(matches!(
            import_openspec(repository.path(), "openspec/changes/add-session-refresh"),
            Err(SourceError::UnsafeReference(_))
                | Err(SourceError::Artifact(artifacts::ArtifactError::UnsafeRoot(
                    _
                )))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn openspec_import_fails_closed_when_open_parent_is_swapped_to_symlink() {
        use std::os::unix::fs::symlink;

        let repository = fixture();
        let outside = tempfile::tempdir().unwrap();
        let outside_change = outside
            .path()
            .join("changes/add-session-refresh/specs/auth");
        std::fs::create_dir_all(&outside_change).unwrap();
        std::fs::write(
            outside
                .path()
                .join("changes/add-session-refresh/proposal.md"),
            "# ATTACKER CONTENT\n",
        )
        .unwrap();
        std::fs::write(
            outside_change.join("spec.md"),
            "## ADDED Requirements\n\n### Requirement: Outside\nMUST NOT be imported.\n",
        )
        .unwrap();

        let openspec = repository.path().join("openspec");
        let original = repository.path().join("openspec-original");
        let result = import_openspec_with_hook(
            repository.path(),
            "openspec/changes/add-session-refresh",
            || {
                std::fs::rename(&openspec, &original).unwrap();
                symlink(outside.path(), &openspec).unwrap();
            },
        );
        assert!(matches!(
            result,
            Err(SourceError::Changed(_))
                | Err(SourceError::UnsafeReference(_))
                | Err(SourceError::Artifact(artifacts::ArtifactError::UnsafeRoot(
                    _
                )))
        ));

        std::fs::remove_file(&openspec).unwrap();
        std::fs::rename(&original, &openspec).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn openspec_import_rejects_windows_reparse_or_non_directory_parent() {
        use std::os::windows::fs::symlink_dir;

        let repository = fixture();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("changes/add-session-refresh")).unwrap();
        let openspec = repository.path().join("openspec");
        let original = repository.path().join("openspec-original");
        std::fs::rename(&openspec, &original).unwrap();
        if symlink_dir(outside.path(), &openspec).is_err() {
            std::fs::write(&openspec, "unsafe replacement").unwrap();
        }
        assert!(
            import_openspec(repository.path(), "openspec/changes/add-session-refresh").is_err()
        );
        std::fs::remove_file(&openspec).unwrap();
        std::fs::rename(&original, &openspec).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn openspec_import_windows_handle_blocks_or_detects_parent_swap() {
        use std::cell::Cell;
        use std::os::windows::fs::symlink_dir;

        let repository = fixture();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("changes/add-session-refresh")).unwrap();
        let openspec = repository.path().join("openspec");
        let original = repository.path().join("openspec-original");
        let swapped = Cell::new(false);
        let swap_denied = Cell::new(false);
        let result = import_openspec_with_hook(
            repository.path(),
            "openspec/changes/add-session-refresh",
            || {
                if std::fs::rename(&openspec, &original).is_err() {
                    // cap-primitives deliberately opens Windows directories
                    // without FILE_SHARE_DELETE, so the kernel normally blocks
                    // this race before an entry can be replaced.
                    swap_denied.set(true);
                    return;
                }
                swapped.set(true);
                if symlink_dir(outside.path(), &openspec).is_err() {
                    std::fs::write(&openspec, "unsafe replacement").unwrap();
                }
            },
        );
        assert!(swap_denied.get() || (swapped.get() && result.is_err()));
        if swapped.get() {
            std::fs::remove_file(&openspec).unwrap();
            std::fs::rename(&original, &openspec).unwrap();
        }
    }

    #[test]
    fn conversion_is_deterministic_and_plan_is_serial() {
        let repository = fixture();
        let first =
            import_openspec(repository.path(), "openspec/changes/add-session-refresh").unwrap();
        let second =
            import_openspec(repository.path(), "openspec/changes/add-session-refresh").unwrap();
        assert_eq!(first, second);
        let tasks = first.plan_tasks(&SpecId::new());
        assert!(tasks[0].dependencies.is_empty());
        assert_eq!(tasks[1].dependencies, ["T-001"]);
        assert!(tasks.iter().all(|task| !task.parallel_safe));
    }

    #[test]
    fn openspec_project_schema_and_directory_limits_are_bound_into_the_snapshot() {
        let repository = fixture();
        let config = repository.path().join("openspec/config.yaml");
        std::fs::write(&config, "schema: spec-driven\ncontext: first\n").unwrap();
        let first =
            import_openspec(repository.path(), "openspec/changes/add-session-refresh").unwrap();
        std::fs::write(&config, "schema: spec-driven\ncontext: second\n").unwrap();
        let second =
            import_openspec(repository.path(), "openspec/changes/add-session-refresh").unwrap();
        assert_eq!(first.markdown, second.markdown);
        assert_ne!(first.source_revision, second.source_revision);

        let mut deep = repository
            .path()
            .join("openspec/changes/add-session-refresh");
        for _ in 0..=MAX_SOURCE_DEPTH {
            deep.push("nested");
        }
        std::fs::create_dir_all(deep).unwrap();
        assert!(matches!(
            import_openspec(repository.path(), "openspec/changes/add-session-refresh"),
            Err(SourceError::TooLarge(_))
        ));
    }

    #[test]
    fn openspec_path_and_file_size_limits_fail_closed() {
        let repository = fixture();
        let oversized_name = "a".repeat(129);
        assert!(matches!(
            import_openspec(
                repository.path(),
                &format!("openspec/changes/{oversized_name}")
            ),
            Err(SourceError::UnsafeReference(_))
        ));

        let proposal = repository
            .path()
            .join("openspec/changes/add-session-refresh/proposal.md");
        std::fs::write(proposal, "x".repeat(MAX_SOURCE_FILE_BYTES + 1)).unwrap();
        assert!(matches!(
            import_openspec(repository.path(), "openspec/changes/add-session-refresh"),
            Err(SourceError::TooLarge(_))
        ));
    }

    #[test]
    fn work_item_normalization_binds_generic_provenance_without_mutation_intent() {
        let imported = normalize_work_item(WorkItemSource {
            provider: "github",
            connection_id: "gh:default",
            site_id: None,
            external_id: "42",
            key: Some("owner/repo#42"),
            url: "https://github.com/owner/repo/issues/42",
            source_revision: "2026-07-26T14:00:00Z",
            title: "Refresh active sessions",
            body: "Keep active sessions usable during token refresh.",
        })
        .unwrap();
        let reference = imported.external_reference.unwrap();
        assert_eq!(reference.provider, "github");
        assert_eq!(reference.external_id, "42");
        assert_eq!(reference.source_revision, "2026-07-26T14:00:00Z");
        assert!(imported.markdown.contains("Imported work-item context"));
        assert!(!imported.markdown.contains("comment"));
        assert!(!imported.markdown.contains("transition"));
    }

    #[test]
    fn work_item_normalization_rejects_ambient_or_unbound_inputs() {
        let base = WorkItemSource {
            provider: "jira",
            connection_id: "jira:site-1",
            site_id: Some("site-1"),
            external_id: "10001",
            key: Some("ENG-1"),
            url: "https://example.atlassian.net/browse/ENG-1",
            source_revision: "version:3",
            title: "Issue",
            body: "Body",
        };
        assert!(normalize_work_item(base.clone()).is_ok());
        assert!(matches!(
            normalize_work_item(WorkItemSource {
                connection_id: "",
                ..base.clone()
            }),
            Err(SourceError::Malformed(_))
        ));
        assert!(matches!(
            normalize_work_item(WorkItemSource {
                url: "http://example.invalid/ENG-1",
                ..base.clone()
            }),
            Err(SourceError::Malformed(_))
        ));
        assert!(matches!(
            normalize_work_item(WorkItemSource {
                provider: "unknown",
                ..base
            }),
            Err(SourceError::Unsupported(_))
        ));
    }

    #[test]
    fn markdown_intake_is_bounded_normalized_and_hash_bound() {
        let first =
            normalize_markdown_intake("Refresh", "# Goal\r\n\r\nKeep sessions.\r\n").unwrap();
        let second = normalize_markdown_intake("Refresh", "# Goal\n\nKeep sessions.\n").unwrap();
        assert_eq!(first, second);
        assert_eq!(first.markdown, "# Goal\n\nKeep sessions.\n");
        assert!(first.source_revision.starts_with("sha256:"));
        assert!(matches!(
            normalize_markdown_intake("Refresh", "\0"),
            Err(SourceError::Malformed(_))
        ));
    }

    #[test]
    fn openspec_export_is_deterministic_scoped_and_diagnostic_when_lossy() {
        let id: SpecId = "SPC-01K123456789ABCDEFGHJKMNPQ".parse().unwrap();
        let spec = artifacts::render_spec(
            &id,
            3,
            "Refresh active sessions",
            None,
            "# Refresh\n\n## Requirements\n\n- RQ-001 Replace access tokens atomically.\n\n## Acceptance criteria\n\n- AC-001 Active sessions remain usable.",
        )
        .unwrap();
        let plan = serde_json::to_string(&PlanArtifact {
            schema_version: 1,
            spec_id: id,
            spec_revision: 3,
            tasks: vec![PlanTask {
                id: "T-001".into(),
                objective: "Implement the atomic replacement".into(),
                dependencies: Vec::new(),
                read_scopes: vec!["src/session.rs".into()],
                write_scopes: vec!["src/session.rs".into()],
                acceptance_criteria: vec!["AC-001".into()],
                verification: Vec::new(),
                browser_checks: Vec::new(),
                risk: "medium".into(),
                parallel_safe: false,
            }],
        })
        .unwrap();
        let first = preview_openspec_export(&spec, Some("# Design\n\nAtomic swap.\n"), Some(&plan))
            .unwrap();
        let second =
            preview_openspec_export(&spec, Some("# Design\n\nAtomic swap.\n"), Some(&plan))
                .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.destination,
            "openspec/changes/agentum-01k12345-refresh-active-sessions"
        );
        assert_eq!(
            first
                .files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>(),
            [
                "design.md",
                "proposal.md",
                "specs/refresh-active-sessions/spec.md",
                "tasks.md"
            ]
        );
        assert!(
            first
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "openspec_lossy_acceptance_mapping")
        );
        assert!(
            first
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "openspec_lossy_typed_plan")
        );
        assert!(first.source_revision.starts_with("sha256:"));
    }

    #[test]
    fn openspec_export_rejects_plan_from_another_revision() {
        let id: SpecId = "SPC-01K123456789ABCDEFGHJKMNPQ".parse().unwrap();
        let spec = artifacts::render_spec(
            &id,
            2,
            "Example",
            None,
            "# Example\n\n## Requirements\n\n- RQ-001 Work.\n\n## Acceptance criteria\n\n- AC-001 The work succeeds.",
        )
        .unwrap();
        let plan = serde_json::to_string(&PlanArtifact {
            schema_version: 1,
            spec_id: id,
            spec_revision: 1,
            tasks: Vec::new(),
        })
        .unwrap();
        assert!(matches!(
            preview_openspec_export(&spec, None, Some(&plan)),
            Err(SourceError::Malformed(_))
        ));
    }
}
