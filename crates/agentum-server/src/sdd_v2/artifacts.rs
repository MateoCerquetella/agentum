use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use super::sha256;
use agentum_core::sdd::{ArtifactManifest, SCHEMA_VERSION, SpecId};
use serde::Serialize;
use ulid::Ulid;

pub const MISSING_HASH: &str = "missing";

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("unsafe artifact root: {0}")]
    UnsafeRoot(String),
    #[error("artifact root is not owned by Agentum: {0}")]
    UnownedRoot(String),
    #[error("artifact content changed: expected {expected}, current {current}")]
    ContentChanged { expected: String, current: String },
    #[error("invalid specification: {0}")]
    InvalidSpec(String),
    #[error("artifact is not UTF-8 text: {0}")]
    InvalidText(String),
    #[error("artifact collision: {0}")]
    Collision(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct ArtifactRoot {
    pub root: PathBuf,
    pub spec_dir: PathBuf,
    pub spec_relative_path: String,
    pub manifest: ArtifactManifest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSpecHeader {
    pub schema: u32,
    pub id: SpecId,
    pub revision: i64,
    pub title: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredSpecArtifact {
    pub header: ParsedSpecHeader,
    pub directory_name: String,
    pub relative_path: String,
    pub content: String,
    pub content_hash: String,
    pub later_artifacts: Vec<DiscoveredLaterArtifact>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredLaterArtifact {
    pub kind: String,
    pub file_name: String,
    pub relative_path: String,
    pub content: String,
    pub content_hash: String,
}

#[derive(Debug, Clone)]
pub struct DiscoveredArtifactSet {
    pub manifest: ArtifactManifest,
    pub specs: Vec<DiscoveredSpecArtifact>,
}

/// A directory capability whose children are always resolved relative to one
/// held, no-follow handle.  Keeping this small wrapper in the artifact
/// boundary lets import/export workflows compose several filesystem
/// operations without reopening ambient paths between validation and use.
pub(crate) struct AnchoredDirectory {
    directory: File,
    display_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnchoredEntryKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnchoredEntry {
    pub name: String,
    pub kind: AnchoredEntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) struct AnchoredNamedDescendant {
    pub relative_path: PathBuf,
    pub kind: AnchoredEntryKind,
}

impl AnchoredDirectory {
    pub(crate) fn open(path: &Path) -> Result<Self, ArtifactError> {
        Ok(Self {
            directory: open_directory_chain_portable(path)?,
            display_path: path.to_path_buf(),
        })
    }

    pub(crate) fn open_child(&self, name: &str) -> Result<Self, ArtifactError> {
        validate_child_name(name, &self.display_path)?;
        let display_path = self.display_path.join(name);
        Ok(Self {
            directory: open_child_directory(&self.directory, name, &display_path)?,
            display_path,
        })
    }

    pub(crate) fn try_clone(&self) -> Result<Self, ArtifactError> {
        Ok(Self {
            directory: self.directory.try_clone()?,
            display_path: self.display_path.clone(),
        })
    }

    pub(crate) fn open_child_optional(&self, name: &str) -> Result<Option<Self>, ArtifactError> {
        validate_child_name(name, &self.display_path)?;
        let display_path = self.display_path.join(name);
        Ok(
            open_child_directory_optional(&self.directory, name, &display_path)?.map(|directory| {
                Self {
                    directory,
                    display_path,
                }
            }),
        )
    }

    pub(crate) fn ensure_child(&self, name: &str) -> Result<(Self, bool), ArtifactError> {
        validate_child_name(name, &self.display_path)?;
        let display_path = self.display_path.join(name);
        let (directory, created) = ensure_child_directory_at(&self.directory, name, &display_path)?;
        Ok((
            Self {
                directory,
                display_path,
            },
            created,
        ))
    }

    pub(crate) fn create_child_exclusive(&self, name: &str) -> Result<Self, ArtifactError> {
        validate_child_name(name, &self.display_path)?;
        let display_path = self.display_path.join(name);
        Ok(Self {
            directory: create_child_directory_exclusive(&self.directory, name, &display_path)?,
            display_path,
        })
    }

    pub(crate) fn entries(&self) -> Result<Vec<AnchoredEntry>, ArtifactError> {
        directory_entries(&self.directory, &self.display_path)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) fn child_kind_optional(
        &self,
        name: &str,
    ) -> Result<Option<AnchoredEntryKind>, ArtifactError> {
        validate_child_name(name, &self.display_path)?;
        let entries = cap_primitives::fs::read_base_dir(&self.directory).map_err(|error| {
            ArtifactError::UnsafeRoot(format!("{} ({error})", self.display_path.display()))
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                ArtifactError::UnsafeRoot(format!("{} ({error})", self.display_path.display()))
            })?;
            if entry.file_name() != std::ffi::OsStr::new(name) {
                continue;
            }
            let file_type = entry.file_type().map_err(|error| {
                ArtifactError::UnsafeRoot(format!("{} ({error})", self.display_path.display()))
            })?;
            if file_type.is_symlink() {
                return Err(ArtifactError::UnsafeRoot(format!(
                    "{} is a provider-input link",
                    self.display_path.join(name).display()
                )));
            }
            if file_type.is_file() {
                return Ok(Some(AnchoredEntryKind::File));
            }
            if file_type.is_dir() {
                return Ok(Some(AnchoredEntryKind::Directory));
            }
            return Err(ArtifactError::UnsafeRoot(format!(
                "{} is an unsupported provider input",
                self.display_path.join(name).display()
            )));
        }
        Ok(None)
    }

    /// Finds provider-owned project inputs without following links or reopening
    /// the directory through an ambient path. A link with a reserved name is a
    /// hard error: treating it as either a file or directory would let its
    /// target change between policy construction and sandbox launch.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) fn find_named_descendants(
        &self,
        directory_names: &[&str],
        file_names: &[&str],
    ) -> Result<Vec<AnchoredNamedDescendant>, ArtifactError> {
        const MAX_SCAN_DEPTH: usize = 64;
        const MAX_SCANNED_ENTRIES: usize = 250_000;

        fn visit(
            directory: &AnchoredDirectory,
            relative: &Path,
            directory_names: &[&str],
            file_names: &[&str],
            depth: usize,
            scanned: &mut usize,
            found: &mut Vec<AnchoredNamedDescendant>,
        ) -> Result<(), ArtifactError> {
            if depth > MAX_SCAN_DEPTH {
                return Err(ArtifactError::UnsafeRoot(format!(
                    "{} exceeds the provider-input scan depth",
                    directory.display_path.display()
                )));
            }
            let entries =
                cap_primitives::fs::read_base_dir(&directory.directory).map_err(|error| {
                    ArtifactError::UnsafeRoot(format!(
                        "{} ({error})",
                        directory.display_path.display()
                    ))
                })?;
            for entry in entries {
                *scanned += 1;
                if *scanned > MAX_SCANNED_ENTRIES {
                    return Err(ArtifactError::UnsafeRoot(format!(
                        "{} exceeds the provider-input scan limit",
                        directory.display_path.display()
                    )));
                }
                let entry = entry.map_err(|error| {
                    ArtifactError::UnsafeRoot(format!(
                        "{} ({error})",
                        directory.display_path.display()
                    ))
                })?;
                let name = entry.file_name().into_string().map_err(|_| {
                    ArtifactError::UnsafeRoot(format!(
                        "{} contains a non-UTF-8 name",
                        directory.display_path.display()
                    ))
                })?;
                if name.is_empty() || matches!(name.as_str(), "." | "..") {
                    return Err(ArtifactError::UnsafeRoot(
                        directory.display_path.display().to_string(),
                    ));
                }
                let file_type = entry.file_type().map_err(|error| {
                    ArtifactError::UnsafeRoot(format!(
                        "{} ({error})",
                        directory.display_path.display()
                    ))
                })?;
                let named_directory = directory_names.contains(&name.as_str());
                let named_file = file_names.contains(&name.as_str());
                let child_relative = relative.join(&name);

                if file_type.is_symlink() {
                    if named_directory || named_file {
                        return Err(ArtifactError::UnsafeRoot(format!(
                            "{} is a provider-input link",
                            directory.display_path.join(&name).display()
                        )));
                    }
                    continue;
                }

                let kind = if file_type.is_file() {
                    AnchoredEntryKind::File
                } else if file_type.is_dir() {
                    AnchoredEntryKind::Directory
                } else {
                    if named_directory || named_file {
                        return Err(ArtifactError::UnsafeRoot(format!(
                            "{} is an unsupported provider input",
                            directory.display_path.join(&name).display()
                        )));
                    }
                    continue;
                };

                if named_directory || named_file {
                    found.push(AnchoredNamedDescendant {
                        relative_path: child_relative.clone(),
                        kind,
                    });
                    // A matched directory is hidden wholesale. Descending into
                    // it would only add redundant mounts and expand race surface.
                    if kind == AnchoredEntryKind::Directory {
                        continue;
                    }
                }
                if kind == AnchoredEntryKind::Directory && name != ".git" {
                    let child = directory.open_child(&name)?;
                    visit(
                        &child,
                        &child_relative,
                        directory_names,
                        file_names,
                        depth + 1,
                        scanned,
                        found,
                    )?;
                }
            }
            Ok(())
        }

        let mut found = Vec::new();
        let mut scanned = 0;
        visit(
            self,
            Path::new(""),
            directory_names,
            file_names,
            0,
            &mut scanned,
            &mut found,
        )?;
        found.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(found)
    }

    pub(crate) fn read_file(&self, name: &str) -> Result<(Vec<u8>, String), ArtifactError> {
        validate_child_name(name, &self.display_path)?;
        let bytes = read_file_at(&self.directory, name, &self.display_path.join(name))?;
        let hash = sha256(&bytes);
        Ok((bytes, hash))
    }

    pub(crate) fn read_file_optional(
        &self,
        name: &str,
    ) -> Result<Option<(Vec<u8>, String)>, ArtifactError> {
        match self.read_file(name) {
            Ok(file) => Ok(Some(file)),
            Err(ArtifactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn atomic_write_missing(
        &self,
        name: &str,
        content: &[u8],
    ) -> Result<String, ArtifactError> {
        validate_child_name(name, &self.display_path)?;
        atomic_write_in_directory(
            &self.directory,
            name,
            &self.display_path.join(name),
            content,
            Some(MISSING_HASH),
        )
    }

    pub(crate) fn same_identity(&self, other: &Self) -> Result<bool, ArtifactError> {
        same_directory_identity(&self.directory, &other.directory)
    }

    pub(crate) fn sync(&self) -> Result<(), ArtifactError> {
        #[cfg(unix)]
        {
            self.directory.sync_all()?;
            Ok(())
        }
        #[cfg(windows)]
        {
            // Windows directory handles opened by the capability layer are
            // intentionally read-only and cannot be passed to
            // FlushFileBuffers. Every staged file is flushed before its
            // handle-relative rename; SetFileInformationByHandle then journals
            // the no-replace directory publication.
            Ok(())
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            Err(ArtifactError::UnsafeRoot(format!(
                "safe directory syncing is unsupported on this operating system: {}",
                self.display_path.display()
            )))
        }
    }

    /// Atomically publish one already-created child directory under a new
    /// child name. Existing destinations are never replaced.
    pub(crate) fn publish_child_directory_noreplace(
        &self,
        staging_name: &str,
        destination_name: &str,
    ) -> Result<(), ArtifactError> {
        validate_child_name(staging_name, &self.display_path)?;
        validate_child_name(destination_name, &self.display_path)?;
        rename_child_directory_noreplace(
            &self.directory,
            staging_name,
            destination_name,
            &self.display_path,
        )?;
        self.sync()
    }

    /// Best-effort rollback primitive for a private staging tree. Every
    /// traversal and removal stays relative to held handles. Unexpected links
    /// or special entries are rejected and preserved as recovery evidence.
    pub(crate) fn remove_child_tree(&self, name: &str) -> Result<(), ArtifactError> {
        validate_child_name(name, &self.display_path)?;
        let child = self.open_child(name)?;
        for entry in child.entries()? {
            match entry.kind {
                AnchoredEntryKind::File => {
                    cap_primitives::fs::remove_file(&child.directory, Path::new(&entry.name))?;
                }
                AnchoredEntryKind::Directory => child.remove_child_tree(&entry.name)?,
            }
        }
        child.sync()?;
        drop(child);
        cap_primitives::fs::remove_dir(&self.directory, Path::new(name))?;
        self.sync()
    }
}

fn validate_child_name(name: &str, display_path: &Path) -> Result<(), ArtifactError> {
    let mut components = Path::new(name).components();
    if name.is_empty()
        || !matches!(components.next(), Some(Component::Normal(component)) if component == name)
        || components.next().is_some()
    {
        return Err(ArtifactError::UnsafeRoot(
            display_path.join(name).display().to_string(),
        ));
    }
    Ok(())
}

/// Read the repository-owned manifest without creating or repairing anything.
/// A present `.agentum` directory without a valid manifest is never claimed.
pub fn discover_manifest(worktree: &Path) -> Result<Option<ArtifactManifest>, ArtifactError> {
    let worktree_directory = open_directory_chain_portable(worktree)?;
    let root_path = worktree.join(".agentum");
    let Some(root_directory) =
        open_child_directory_optional(&worktree_directory, ".agentum", &root_path)?
    else {
        return Ok(None);
    };
    let manifest_path = root_path.join("manifest.json");
    let manifest_bytes = read_file_at(&root_directory, "manifest.json", &manifest_path).map_err(
        |error| match error {
            ArtifactError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ArtifactError::UnownedRoot("existing .agentum has no manifest.json".into())
            }
            other => other,
        },
    )?;
    let manifest: ArtifactManifest = serde_json::from_slice(&manifest_bytes)?;
    validate_manifest(manifest).map(Some)
}

/// Scan the complete repository-owned artifact set through held no-follow
/// directory handles. The result is all-or-nothing: one malformed directory,
/// link, identity mismatch, or optional artifact aborts reconciliation before
/// any database mutation occurs.
pub fn discover_specs(worktree: &Path) -> Result<Option<DiscoveredArtifactSet>, ArtifactError> {
    let worktree_directory = open_directory_chain_portable(worktree)?;
    let root_path = worktree.join(".agentum");
    let Some(root_directory) =
        open_child_directory_optional(&worktree_directory, ".agentum", &root_path)?
    else {
        return Ok(None);
    };
    let manifest_path = root_path.join("manifest.json");
    let manifest = validate_manifest(serde_json::from_slice(&read_file_at(
        &root_directory,
        "manifest.json",
        &manifest_path,
    )?)?)?;
    let specs_path = root_path.join("specs");
    let specs_directory = open_child_directory(&root_directory, "specs", &specs_path)?;
    let mut names = directory_entry_names(&specs_directory, &specs_path)?;
    names.sort();
    let mut seen_ids = HashSet::new();
    let mut specs = Vec::with_capacity(names.len());
    for directory_name in names {
        if !valid_spec_directory_name(&directory_name) {
            return Err(ArtifactError::InvalidSpec(format!(
                "invalid entry beneath .agentum/specs: {directory_name}"
            )));
        }
        let spec_path = specs_path.join(&directory_name);
        let spec_directory = open_child_directory(&specs_directory, &directory_name, &spec_path)?;
        let entries = directory_entry_names(&spec_directory, &spec_path)?;
        let allowed = [
            "spec.md",
            "design.md",
            "plan.json",
            "decisions.md",
            "review.md",
        ];
        if entries
            .iter()
            .any(|entry| !allowed.contains(&entry.as_str()))
        {
            return Err(ArtifactError::InvalidSpec(format!(
                "unknown artifact in {directory_name}"
            )));
        }
        if !entries.iter().any(|entry| entry == "spec.md") {
            return Err(ArtifactError::InvalidSpec(format!(
                "{directory_name} has no mandatory spec.md"
            )));
        }
        let spec_file_path = spec_path.join("spec.md");
        let bytes = read_file_at(&spec_directory, "spec.md", &spec_file_path)?;
        let content_hash = sha256(&bytes);
        let content = String::from_utf8(bytes)
            .map_err(|error| ArtifactError::InvalidText(error.to_string()))?;
        let (header, _) = parse_spec(&content)?;
        let expected_directory = header.id.directory_name(&header.title);
        if expected_directory != directory_name {
            return Err(ArtifactError::InvalidSpec(format!(
                "directory {directory_name} does not match canonical identity/title {expected_directory}"
            )));
        }
        if !seen_ids.insert(header.id.to_string()) {
            return Err(ArtifactError::InvalidSpec(format!(
                "duplicate specification identity {}",
                header.id
            )));
        }
        let mut later_artifacts = Vec::new();
        for (optional, kind) in [
            ("design.md", "design"),
            ("plan.json", "plan"),
            ("decisions.md", "decisions"),
            ("review.md", "review"),
        ] {
            if !entries.iter().any(|entry| entry == optional) {
                continue;
            }
            let optional_path = spec_path.join(optional);
            let optional_bytes = read_file_at(&spec_directory, optional, &optional_path)?;
            let optional_hash = sha256(&optional_bytes);
            let optional_text = String::from_utf8(optional_bytes)
                .map_err(|error| ArtifactError::InvalidText(error.to_string()))?;
            if optional_text.trim().is_empty() {
                return Err(ArtifactError::InvalidSpec(format!(
                    "optional artifact {directory_name}/{optional} contains no information"
                )));
            }
            if optional == "plan.json" {
                validate_discovered_plan(&optional_text, &header)?;
            }
            later_artifacts.push(DiscoveredLaterArtifact {
                kind: kind.into(),
                file_name: optional.into(),
                relative_path: format!(".agentum/specs/{directory_name}/{optional}"),
                content: optional_text,
                content_hash: optional_hash,
            });
        }
        specs.push(DiscoveredSpecArtifact {
            relative_path: format!(".agentum/specs/{directory_name}/spec.md"),
            directory_name,
            header,
            content,
            content_hash,
            later_artifacts,
        });
    }
    Ok(Some(DiscoveredArtifactSet { manifest, specs }))
}

fn directory_entry_names(
    directory: &File,
    display_path: &Path,
) -> Result<Vec<String>, ArtifactError> {
    Ok(directory_entries(directory, display_path)?
        .into_iter()
        .map(|entry| entry.name)
        .collect())
}

fn directory_entries(
    directory: &File,
    display_path: &Path,
) -> Result<Vec<AnchoredEntry>, ArtifactError> {
    let mut entries_out = Vec::new();
    let entries = cap_primitives::fs::read_base_dir(directory).map_err(|error| {
        ArtifactError::UnsafeRoot(format!("{} ({error})", display_path.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            ArtifactError::UnsafeRoot(format!("{} ({error})", display_path.display()))
        })?;
        let file_type = entry.file_type().map_err(|error| {
            ArtifactError::UnsafeRoot(format!("{} ({error})", display_path.display()))
        })?;
        if file_type.is_symlink() {
            return Err(ArtifactError::UnsafeRoot(format!(
                "{} contains a link",
                display_path.display()
            )));
        }
        let kind = if file_type.is_file() {
            AnchoredEntryKind::File
        } else if file_type.is_dir() {
            AnchoredEntryKind::Directory
        } else {
            return Err(ArtifactError::UnsafeRoot(format!(
                "{} contains an unsupported entry",
                display_path.display()
            )));
        };
        let name = entry.file_name().into_string().map_err(|_| {
            ArtifactError::UnsafeRoot(format!(
                "{} contains a non-UTF-8 name",
                display_path.display()
            ))
        })?;
        if name.is_empty() || matches!(name.as_str(), "." | "..") {
            return Err(ArtifactError::UnsafeRoot(
                display_path.display().to_string(),
            ));
        }
        entries_out.push(AnchoredEntry { name, kind });
    }
    Ok(entries_out)
}

fn valid_spec_directory_name(name: &str) -> bool {
    name.starts_with("spc-")
        && name.len() <= 4 + 26 + 1 + 48
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn validate_discovered_plan(content: &str, header: &ParsedSpecHeader) -> Result<(), ArtifactError> {
    use agentum_core::sdd::PlanArtifact;

    let plan: PlanArtifact = serde_json::from_str(content)?;
    if plan.schema_version != SCHEMA_VERSION
        || plan.spec_id != header.id
        || plan.spec_revision != header.revision
        || plan.tasks.is_empty()
        || plan.tasks.len() > 256
    {
        return Err(ArtifactError::InvalidSpec(
            "plan identity or task count is invalid".into(),
        ));
    }
    let ids: HashSet<_> = plan.tasks.iter().map(|task| task.id.as_str()).collect();
    if ids.len() != plan.tasks.len() {
        return Err(ArtifactError::InvalidSpec(
            "plan task identities are not unique".into(),
        ));
    }
    let check_count = plan
        .tasks
        .iter()
        .map(|task| task.browser_checks.len())
        .sum::<usize>();
    let check_ids = plan
        .tasks
        .iter()
        .flat_map(|task| task.browser_checks.iter().map(|check| check.id.as_str()))
        .collect::<HashSet<_>>();
    if check_ids.len() != check_count {
        return Err(ArtifactError::InvalidSpec(
            "browser check identities are not unique".into(),
        ));
    }
    for task in &plan.tasks {
        if task.id.trim().is_empty()
            || task.objective.trim().is_empty()
            || task.dependencies.len() > 256
            || task.read_scopes.len() > 256
            || task.write_scopes.len() > 256
            || task.acceptance_criteria.len() > 256
            || task.verification.len() > 32
            || task.browser_checks.len() > 32
            || task
                .browser_checks
                .iter()
                .any(|check| super::lifecycle::validate_browser_check(check).is_err())
            || task
                .dependencies
                .iter()
                .any(|dependency| dependency == &task.id || !ids.contains(dependency.as_str()))
            || task
                .read_scopes
                .iter()
                .chain(task.write_scopes.iter())
                .any(|path| agentum_core::sdd::validate_relative_path(path).is_err())
        {
            return Err(ArtifactError::InvalidSpec(format!(
                "plan task {} is malformed",
                task.id
            )));
        }
    }
    let dependencies: std::collections::HashMap<_, _> = plan
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task.dependencies.as_slice()))
        .collect();
    fn visit<'a>(
        id: &'a str,
        dependencies: &std::collections::HashMap<&'a str, &'a [String]>,
        visiting: &mut HashSet<&'a str>,
        visited: &mut HashSet<&'a str>,
    ) -> bool {
        if visited.contains(id) {
            return true;
        }
        if !visiting.insert(id) {
            return false;
        }
        if dependencies[id]
            .iter()
            .any(|dependency| !visit(dependency, dependencies, visiting, visited))
        {
            return false;
        }
        visiting.remove(id);
        visited.insert(id);
        true
    }
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    if dependencies
        .keys()
        .any(|id| !visit(id, &dependencies, &mut visiting, &mut visited))
    {
        return Err(ArtifactError::InvalidSpec(
            "plan dependency graph contains a cycle".into(),
        ));
    }
    Ok(())
}

fn validate_manifest(manifest: ArtifactManifest) -> Result<ArtifactManifest, ArtifactError> {
    if !manifest.validate() {
        return Err(ArtifactError::UnownedRoot(
            "manifest format or schema version is not recognized".into(),
        ));
    }
    Ok(manifest)
}

/// Validate an existing artifact root without creating or repairing anything.
/// Directory and file resolution stays relative to held no-follow handles.
pub fn validate_existing_root(
    worktree: &Path,
    spec_id: &SpecId,
    directory_name: &str,
) -> Result<ArtifactManifest, ArtifactError> {
    let expected_prefix = format!("spc-{}-", spec_id.ulid().to_ascii_lowercase());
    if !directory_name.starts_with(&expected_prefix)
        || directory_name
            .chars()
            .any(|value| !(value.is_ascii_lowercase() || value.is_ascii_digit() || value == '-'))
    {
        return Err(ArtifactError::InvalidSpec(
            "spec directory does not match its canonical identity".into(),
        ));
    }
    let worktree_directory = open_directory_chain_portable(worktree)?;
    let root_path = worktree.join(".agentum");
    let root_directory = open_child_directory(&worktree_directory, ".agentum", &root_path)?;
    let manifest_path = root_path.join("manifest.json");
    let manifest = validate_manifest(serde_json::from_slice(&read_file_at(
        &root_directory,
        "manifest.json",
        &manifest_path,
    )?)?)?;
    let specs_path = root_path.join("specs");
    let specs_directory = open_child_directory(&root_directory, "specs", &specs_path)?;
    open_child_directory(
        &specs_directory,
        directory_name,
        &specs_path.join(directory_name),
    )?;
    Ok(manifest)
}

/// Create the mandatory root without following any existing link. An existing
/// `.agentum` is accepted only when its manifest proves Agentum ownership.
pub fn initialize(
    worktree: &Path,
    spec_id: &SpecId,
    title: &str,
    artifact_set_id: Ulid,
) -> Result<ArtifactRoot, ArtifactError> {
    let worktree_directory = open_directory_chain_portable(worktree)?;
    let root = worktree.join(".agentum");
    let manifest_path = root.join("manifest.json");
    let (root_directory, created_root) =
        ensure_child_directory_at(&worktree_directory, ".agentum", &root)?;
    let expected_manifest = ArtifactManifest {
        format: agentum_core::sdd::ARTIFACT_FORMAT.into(),
        schema_version: SCHEMA_VERSION,
        artifact_set_id,
    };
    let manifest = if created_root {
        atomic_write_in_directory(
            &root_directory,
            "manifest.json",
            &manifest_path,
            &pretty_json(&expected_manifest)?,
            Some(MISSING_HASH),
        )?;
        expected_manifest
    } else {
        let bytes =
            read_file_at(&root_directory, "manifest.json", &manifest_path).map_err(|error| {
                match error {
                    ArtifactError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        ArtifactError::UnownedRoot("existing .agentum has no manifest.json".into())
                    }
                    other => other,
                }
            })?;
        validate_manifest(serde_json::from_slice(&bytes)?)?
    };
    if manifest.artifact_set_id != artifact_set_id {
        return Err(ArtifactError::Collision(format!(
            "repository artifact set is {}, expected {}",
            manifest.artifact_set_id, artifact_set_id
        )));
    }

    let specs = root.join("specs");
    let (specs_directory, _) = ensure_child_directory_at(&root_directory, "specs", &specs)?;
    let directory_name = spec_id.directory_name(title);
    let spec_dir = specs.join(&directory_name);
    create_child_directory_exclusive(&specs_directory, &directory_name, &spec_dir)?;
    Ok(ArtifactRoot {
        root,
        spec_dir,
        spec_relative_path: format!(".agentum/specs/{directory_name}/spec.md"),
        manifest,
    })
}

fn pretty_json<T: Serialize>(value: &T) -> Result<Vec<u8>, ArtifactError> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn content_hash(path: &Path) -> Result<String, ArtifactError> {
    match read_bytes(path) {
        Ok((_, hash)) => Ok(hash),
        Err(ArtifactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(MISSING_HASH.into())
        }
        Err(error) => Err(error),
    }
}

/// Read one artifact without following its file or any parent link. Returning
/// the bytes and their hash from the same open handle prevents a provider from
/// swapping a staging link between a validation read and the content read.
pub fn read_bytes(path: &Path) -> Result<(Vec<u8>, String), ArtifactError> {
    let parent = path
        .parent()
        .ok_or_else(|| ArtifactError::UnsafeRoot(path.display().to_string()))?;
    let directory = open_directory_chain_portable(parent)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ArtifactError::UnsafeRoot(path.display().to_string()))?;
    let bytes = read_file_at(&directory, name, path)?;
    let hash = sha256(&bytes);
    Ok((bytes, hash))
}

pub fn read_text(path: &Path) -> Result<(String, String), ArtifactError> {
    let (bytes, hash) = read_bytes(path)?;
    let text =
        String::from_utf8(bytes).map_err(|error| ArtifactError::InvalidText(error.to_string()))?;
    Ok((text, hash))
}

/// Remove one exact regular file relative to a held no-follow parent handle.
/// Missing files are an idempotent success; links and directories are refused.
pub fn remove_file_nofollow(path: &Path) -> Result<(), ArtifactError> {
    let parent = path
        .parent()
        .ok_or_else(|| ArtifactError::UnsafeRoot(path.display().to_string()))?;
    let directory = open_directory_chain_portable(parent)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ArtifactError::UnsafeRoot(path.display().to_string()))?;
    match read_file_at(&directory, name, path) {
        Ok(_) => {}
        Err(ArtifactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(error) => return Err(error),
    }
    cap_primitives::fs::remove_file(&directory, Path::new(name))?;
    #[cfg(unix)]
    {
        directory.sync_all()?;
        Ok(())
    }
    #[cfg(windows)]
    {
        // Windows refuses FlushFileBuffers on directory handles. The unlink
        // has already completed relative to the held no-follow parent handle;
        // this matches `atomic_remove`'s Windows durability boundary.
        Ok(())
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        Err(ArtifactError::UnsafeRoot(format!(
            "safe artifact removal is unsupported on this operating system: {}",
            path.display()
        )))
    }
}

#[cfg(unix)]
fn open_directory_chain_portable(path: &Path) -> Result<File, ArtifactError> {
    open_directory_chain(path)
}

#[cfg(windows)]
fn open_directory_chain_portable(path: &Path) -> Result<File, ArtifactError> {
    open_cap_directory_chain(path)
}

#[cfg(all(not(unix), not(windows)))]
fn open_directory_chain_portable(path: &Path) -> Result<File, ArtifactError> {
    Err(ArtifactError::UnsafeRoot(format!(
        "safe artifact access is unsupported on this operating system: {}",
        path.display()
    )))
}

#[cfg(unix)]
fn open_child_directory_optional(
    parent: &File,
    name: &str,
    display_path: &Path,
) -> Result<Option<File>, ArtifactError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};

    let name = CString::new(name)
        .map_err(|_| ArtifactError::UnsafeRoot(display_path.display().to_string()))?;
    // SAFETY: `parent` and `name` remain live for the call. O_NOFOLLOW and
    // O_DIRECTORY reject both symlinks and non-directory entries.
    let raw = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if raw < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(ArtifactError::UnsafeRoot(format!(
            "{} ({error})",
            display_path.display()
        )));
    }
    // SAFETY: openat returned a fresh owned descriptor.
    Ok(Some(unsafe { File::from_raw_fd(raw) }))
}

#[cfg(windows)]
fn open_child_directory_optional(
    parent: &File,
    name: &str,
    display_path: &Path,
) -> Result<Option<File>, ArtifactError> {
    match cap_primitives::fs::open_dir_nofollow(parent, Path::new(name)) {
        Ok(directory) => Ok(Some(directory)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ArtifactError::UnsafeRoot(format!(
            "{} ({error})",
            display_path.display()
        ))),
    }
}

#[cfg(all(not(unix), not(windows)))]
fn open_child_directory_optional(
    _parent: &File,
    _name: &str,
    display_path: &Path,
) -> Result<Option<File>, ArtifactError> {
    Err(ArtifactError::UnsafeRoot(format!(
        "safe artifact access is unsupported on this operating system: {}",
        display_path.display()
    )))
}

fn open_child_directory(
    parent: &File,
    name: &str,
    display_path: &Path,
) -> Result<File, ArtifactError> {
    open_child_directory_optional(parent, name, display_path)?.ok_or_else(|| {
        ArtifactError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            display_path.display().to_string(),
        ))
    })
}

#[cfg(unix)]
fn ensure_child_directory_at(
    parent: &File,
    name: &str,
    display_path: &Path,
) -> Result<(File, bool), ArtifactError> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;

    if let Some(directory) = open_child_directory_optional(parent, name, display_path)? {
        return Ok((directory, false));
    }
    let name_c = CString::new(name)
        .map_err(|_| ArtifactError::UnsafeRoot(display_path.display().to_string()))?;
    // SAFETY: parent and name remain live; mkdirat resolves only beneath the
    // held directory. A racing entry is reopened with O_NOFOLLOW below.
    let created = unsafe { libc::mkdirat(parent.as_raw_fd(), name_c.as_ptr(), 0o755) } == 0;
    if !created {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(error.into());
        }
    } else {
        parent.sync_all()?;
    }
    Ok((open_child_directory(parent, name, display_path)?, created))
}

#[cfg(windows)]
fn ensure_child_directory_at(
    parent: &File,
    name: &str,
    display_path: &Path,
) -> Result<(File, bool), ArtifactError> {
    if let Some(directory) = open_child_directory_optional(parent, name, display_path)? {
        return Ok((directory, false));
    }
    let created = match cap_primitives::fs::create_dir(
        parent,
        Path::new(name),
        &cap_primitives::fs::DirOptions::new(),
    ) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error.into()),
    };
    Ok((open_child_directory(parent, name, display_path)?, created))
}

#[cfg(all(not(unix), not(windows)))]
fn ensure_child_directory_at(
    _parent: &File,
    _name: &str,
    display_path: &Path,
) -> Result<(File, bool), ArtifactError> {
    Err(ArtifactError::UnsafeRoot(format!(
        "safe artifact creation is unsupported on this operating system: {}",
        display_path.display()
    )))
}

fn create_child_directory_exclusive(
    parent: &File,
    name: &str,
    display_path: &Path,
) -> Result<File, ArtifactError> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        let name_c = CString::new(name)
            .map_err(|_| ArtifactError::UnsafeRoot(display_path.display().to_string()))?;
        // SAFETY: parent and name remain live and mkdirat does not follow the
        // final entry. Existing entries are treated as identity collisions.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name_c.as_ptr(), 0o755) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                return Err(ArtifactError::Collision(display_path.display().to_string()));
            }
            return Err(error.into());
        }
        parent.sync_all()?;
        open_child_directory(parent, name, display_path)
    }
    #[cfg(windows)]
    {
        match cap_primitives::fs::create_dir(
            parent,
            Path::new(name),
            &cap_primitives::fs::DirOptions::new(),
        ) {
            Ok(()) => open_child_directory(parent, name, display_path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(ArtifactError::Collision(display_path.display().to_string()))
            }
            Err(error) => Err(error.into()),
        }
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let _ = (parent, name);
        Err(ArtifactError::UnsafeRoot(format!(
            "safe artifact creation is unsupported on this operating system: {}",
            display_path.display()
        )))
    }
}

#[cfg(unix)]
fn same_directory_identity(left: &File, right: &File) -> Result<bool, ArtifactError> {
    use std::os::unix::fs::MetadataExt;

    let left = left.metadata()?;
    let right = right.metadata()?;
    Ok(left.is_dir() && right.is_dir() && left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(windows)]
fn same_directory_identity(left: &File, right: &File) -> Result<bool, ArtifactError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    fn identity(file: &File) -> std::io::Result<(u32, u64)> {
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the file handle and output allocation remain live for this
        // synchronous call.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let index =
            (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
        Ok((information.dwVolumeSerialNumber, index))
    }

    Ok(left.metadata()?.is_dir()
        && right.metadata()?.is_dir()
        && identity(left)? == identity(right)?)
}

#[cfg(all(not(unix), not(windows)))]
fn same_directory_identity(_left: &File, _right: &File) -> Result<bool, ArtifactError> {
    Err(ArtifactError::UnsafeRoot(
        "safe directory identity checks are unsupported on this operating system".into(),
    ))
}

#[cfg(target_os = "linux")]
fn rename_child_directory_noreplace(
    parent: &File,
    from: &str,
    to: &str,
    display_path: &Path,
) -> Result<(), ArtifactError> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;

    let from = CString::new(from)
        .map_err(|_| ArtifactError::UnsafeRoot(display_path.display().to_string()))?;
    let to = CString::new(to)
        .map_err(|_| ArtifactError::UnsafeRoot(display_path.display().to_string()))?;
    // SAFETY: both names are bare owned C strings and both resolutions are
    // rooted at the same held no-follow directory descriptor.
    if unsafe {
        libc::renameat2(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0
    {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(ArtifactError::Collision(
                display_path
                    .join(to.to_string_lossy().as_ref())
                    .display()
                    .to_string(),
            ));
        }
        return Err(error.into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn rename_child_directory_noreplace(
    parent: &File,
    from: &str,
    to: &str,
    display_path: &Path,
) -> Result<(), ArtifactError> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;

    let from = CString::new(from)
        .map_err(|_| ArtifactError::UnsafeRoot(display_path.display().to_string()))?;
    let to = CString::new(to)
        .map_err(|_| ArtifactError::UnsafeRoot(display_path.display().to_string()))?;
    // SAFETY: renameatx_np resolves both bare names relative to the same held
    // parent, and RENAME_EXCL refuses an existing destination atomically.
    if unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    } != 0
    {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(ArtifactError::Collision(
                display_path
                    .join(to.to_string_lossy().as_ref())
                    .display()
                    .to_string(),
            ));
        }
        return Err(error.into());
    }
    Ok(())
}

#[cfg(windows)]
fn rename_child_directory_noreplace(
    parent: &File,
    from: &str,
    to: &str,
    display_path: &Path,
) -> Result<(), ArtifactError> {
    use cap_primitives::fs::{FollowSymlinks, OpenOptions, OpenOptionsExt};
    use windows_sys::Win32::Foundation::GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        READ_CONTROL,
    };

    let mut options = OpenOptions::new();
    options
        ._cap_fs_ext_follow(FollowSymlinks::No)
        .access_mode(GENERIC_READ | DELETE | READ_CONTROL)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    let staging = cap_primitives::fs::open(parent, Path::new(from), &options).map_err(|error| {
        ArtifactError::UnsafeRoot(format!("{} ({error})", display_path.join(from).display()))
    })?;
    if !staging.metadata()?.is_dir() {
        return Err(ArtifactError::UnsafeRoot(
            display_path.join(from).display().to_string(),
        ));
    }
    if let Err(error) = rename_file_handle(&staging, parent, Path::new(to), false) {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(ArtifactError::Collision(
                display_path.join(to).display().to_string(),
            ));
        }
        return Err(error.into());
    }
    Ok(())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn rename_child_directory_noreplace(
    _parent: &File,
    _from: &str,
    _to: &str,
    display_path: &Path,
) -> Result<(), ArtifactError> {
    Err(ArtifactError::UnsafeRoot(format!(
        "atomic no-replace directory publication is unsupported on this operating system: {}",
        display_path.display()
    )))
}

#[cfg(all(not(unix), not(windows)))]
fn rename_child_directory_noreplace(
    _parent: &File,
    _from: &str,
    _to: &str,
    display_path: &Path,
) -> Result<(), ArtifactError> {
    Err(ArtifactError::UnsafeRoot(format!(
        "safe directory publication is unsupported on this operating system: {}",
        display_path.display()
    )))
}

#[cfg(unix)]
fn read_file_at(parent: &File, name: &str, display_path: &Path) -> Result<Vec<u8>, ArtifactError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};

    let name = CString::new(name)
        .map_err(|_| ArtifactError::UnsafeRoot(display_path.display().to_string()))?;
    // SAFETY: parent and name remain live. O_NOFOLLOW prevents opening a
    // provider-created symlink as an artifact.
    let raw = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if raw < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ELOOP) {
            return Err(ArtifactError::UnsafeRoot(
                display_path.display().to_string(),
            ));
        }
        return Err(error.into());
    }
    // SAFETY: openat returned a fresh owned descriptor.
    let mut file = unsafe { File::from_raw_fd(raw) };
    if !file.metadata()?.is_file() {
        return Err(ArtifactError::UnsafeRoot(
            display_path.display().to_string(),
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(windows)]
fn read_file_at(parent: &File, name: &str, display_path: &Path) -> Result<Vec<u8>, ArtifactError> {
    use cap_primitives::fs::{FollowSymlinks, OpenOptions};

    let mut options = OpenOptions::new();
    options.read(true)._cap_fs_ext_follow(FollowSymlinks::No);
    let mut file =
        cap_primitives::fs::open(parent, Path::new(name), &options).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ArtifactError::Io(error)
            } else {
                ArtifactError::UnsafeRoot(format!("{} ({error})", display_path.display()))
            }
        })?;
    if !file.metadata()?.is_file() {
        return Err(ArtifactError::UnsafeRoot(
            display_path.display().to_string(),
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(all(not(unix), not(windows)))]
fn read_file_at(
    _parent: &File,
    _name: &str,
    display_path: &Path,
) -> Result<Vec<u8>, ArtifactError> {
    Err(ArtifactError::UnsafeRoot(format!(
        "safe artifact access is unsupported on this operating system: {}",
        display_path.display()
    )))
}

fn atomic_write_in_directory(
    directory: &File,
    name: &str,
    display_path: &Path,
    content: &[u8],
    expected_hash: Option<&str>,
) -> Result<String, ArtifactError> {
    #[cfg(unix)]
    {
        atomic_write_unix_in(directory, name, display_path, content, expected_hash)
    }
    #[cfg(windows)]
    {
        atomic_write_windows_in(directory, name, display_path, content, expected_hash)
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let _ = (directory, name, content, expected_hash);
        Err(ArtifactError::UnsafeRoot(format!(
            "safe artifact publication is unsupported on this operating system: {}",
            display_path.display()
        )))
    }
}

/// Publish bytes with an expected-content CAS. The temporary file is opened
/// create-new/no-follow in the destination directory, synced, renamed, and the
/// directory is synced so a successful response survives power loss.
pub fn atomic_write(
    path: &Path,
    content: &[u8],
    expected_hash: Option<&str>,
) -> Result<String, ArtifactError> {
    #[cfg(unix)]
    {
        atomic_write_at(path, content, expected_hash)
    }
    #[cfg(windows)]
    {
        atomic_write_windows(path, content, expected_hash)
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let _ = (content, expected_hash);
        Err(ArtifactError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "safe artifact publication is not implemented for this operating system",
        )))
    }
}

/// Remove a regular file only when the bytes still match the expected hash.
/// Parent resolution and deletion stay relative to one held directory handle;
/// links and non-files are rejected.
pub fn atomic_remove(path: &Path, expected_hash: &str) -> Result<(), ArtifactError> {
    let parent = path
        .parent()
        .ok_or_else(|| ArtifactError::UnsafeRoot(path.display().to_string()))?;
    let directory = open_directory_chain_portable(parent)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ArtifactError::UnsafeRoot(path.display().to_string()))?;
    let current = sha256(&read_file_at(&directory, name, path)?);
    if current != expected_hash {
        return Err(ArtifactError::ContentChanged {
            expected: expected_hash.into(),
            current,
        });
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        let name = CString::new(name)
            .map_err(|_| ArtifactError::UnsafeRoot(path.display().to_string()))?;
        // SAFETY: directory and name stay live and unlinkat resolves only the
        // final entry beneath the held no-follow parent descriptor.
        if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        directory.sync_all()?;
        Ok(())
    }
    #[cfg(windows)]
    {
        cap_primitives::fs::remove_file(&directory, Path::new(name))?;
        Ok(())
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let _ = (directory, name);
        Err(ArtifactError::UnsafeRoot(format!(
            "safe artifact removal is unsupported on this operating system: {}",
            path.display()
        )))
    }
}

/// Windows publication uses capability-relative opens and renames. This keeps
/// the operation bound to held directory handles even if an attacker swaps a
/// checked path component for a junction or another reparse point.
#[cfg(windows)]
fn atomic_write_windows(
    path: &Path,
    content: &[u8],
    expected_hash: Option<&str>,
) -> Result<String, ArtifactError> {
    let parent = path
        .parent()
        .ok_or_else(|| ArtifactError::UnsafeRoot(path.display().to_string()))?;
    let directory = open_cap_directory_chain(parent)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ArtifactError::UnsafeRoot(path.display().to_string()))?;
    atomic_write_windows_in(&directory, name, path, content, expected_hash)
}

#[cfg(windows)]
fn atomic_write_windows_in(
    directory: &File,
    name: &str,
    display_path: &Path,
    content: &[u8],
    expected_hash: Option<&str>,
) -> Result<String, ArtifactError> {
    use cap_primitives::fs::{FollowSymlinks, OpenOptions, OpenOptionsExt};
    use windows_sys::Win32::Foundation::GENERIC_WRITE;
    use windows_sys::Win32::Storage::FileSystem::{DELETE, READ_CONTROL, WRITE_DAC};

    let name = Path::new(name);
    let current = cap_hash_at(directory, name, display_path)?;
    if let Some(expected) = expected_hash {
        if current != expected {
            return Err(ArtifactError::ContentChanged {
                expected: expected.into(),
                current,
            });
        }
    }

    let temporary_name = format!(
        ".{}.{}.tmp",
        name.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("artifact"),
        uuid::Uuid::new_v4()
    );
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        ._cap_fs_ext_follow(FollowSymlinks::No);
    options.access_mode(GENERIC_WRITE | DELETE | READ_CONTROL | WRITE_DAC);
    let mut temporary = cap_primitives::fs::open(directory, Path::new(&temporary_name), &options)?;
    restrict_file_to_owner(&temporary)?;
    temporary.write_all(content)?;
    temporary.sync_all()?;

    let publish = (|| -> Result<(), ArtifactError> {
        let latest = cap_hash_at(directory, name, display_path)?;
        if let Some(expected) = expected_hash {
            if latest != expected {
                return Err(ArtifactError::ContentChanged {
                    expected: expected.into(),
                    current: latest,
                });
            }
        }
        if let Err(error) = rename_file_handle(
            &temporary,
            directory,
            name,
            expected_hash != Some(MISSING_HASH),
        ) {
            if expected_hash == Some(MISSING_HASH)
                && error.kind() == std::io::ErrorKind::AlreadyExists
            {
                return Err(ArtifactError::ContentChanged {
                    expected: MISSING_HASH.into(),
                    current: cap_hash_at(directory, name, display_path)?,
                });
            }
            return Err(error.into());
        }
        Ok(())
    })();
    if publish.is_err() {
        let _ = cap_primitives::fs::remove_file(directory, Path::new(&temporary_name));
    }
    publish?;
    Ok(sha256(content))
}

/// Rename an already-open Windows file relative to an already-open directory.
/// `NtSetInformationFile` supports an actual root-directory handle for a
/// relative target. This avoids converting either handle back into an ambient
/// path, which would reintroduce a junction-swap race. The Win32
/// `SetFileInformationByHandle` wrapper rejects this documented native form
/// with `ERROR_INVALID_PARAMETER` on current Windows runners.
#[cfg(windows)]
pub(crate) fn rename_file_handle(
    file: &File,
    directory: &File,
    name: &Path,
    replace_existing: bool,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_RENAME_INFORMATION, FileRenameInformation, NtSetInformationFile,
    };
    use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let wide: Vec<u16> = name.as_os_str().encode_wide().collect();
    if wide.is_empty() || wide.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "artifact name is empty or contains NUL",
        ));
    }
    let file_name_bytes = wide
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "artifact name is too long",
            )
        })?;
    // Windows requires at least sizeof(FILE_RENAME_INFORMATION) plus the complete
    // variable filename. Using only offset_of(FileName) produced an undersized
    // buffer and ERROR_INVALID_PARAMETER on real Windows runners.
    let byte_len = std::mem::size_of::<FILE_RENAME_INFORMATION>()
        .checked_add(file_name_bytes)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "artifact name is too long",
            )
        })?;
    let word = std::mem::size_of::<usize>();
    let mut storage = vec![0usize; byte_len.div_ceil(word)];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let file_name_length = u32::try_from(file_name_bytes).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "artifact name is too long",
        )
    })?;
    let buffer_length = u32::try_from(byte_len).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "artifact rename buffer is too large",
        )
    })?;
    // SAFETY: `storage` is pointer-aligned and sized for the fixed header plus
    // every UTF-16 code unit copied into its trailing filename buffer.
    unsafe {
        (*info).Anonymous.ReplaceIfExists = replace_existing;
        (*info).RootDirectory = directory.as_raw_handle();
        (*info).FileNameLength = file_name_length;
        std::ptr::copy_nonoverlapping(
            wide.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            wide.len(),
        );
    }
    // SAFETY: both handles remain live and `storage` remains allocated for the
    // duration of the synchronous Windows call.
    let mut io_status = IO_STATUS_BLOCK::default();
    let status = unsafe {
        NtSetInformationFile(
            file.as_raw_handle(),
            &raw mut io_status,
            info.cast_const().cast(),
            buffer_length,
            FileRenameInformation,
        )
    };
    if status < 0 {
        // SAFETY: conversion has no preconditions and maps an NTSTATUS returned
        // by the immediately preceding native call to its Win32 error code.
        let code = unsafe { RtlNtStatusToDosError(status) };
        Err(std::io::Error::from_raw_os_error(
            i32::try_from(code).unwrap_or(i32::MAX),
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn open_cap_directory_chain(path: &Path) -> Result<File, ArtifactError> {
    use cap_primitives::ambient_authority;

    let mut anchor = PathBuf::new();
    let mut children = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) if children.is_empty() => anchor.push(prefix.as_os_str()),
            Component::RootDir if children.is_empty() => {
                anchor.push(Path::new(std::path::MAIN_SEPARATOR_STR));
            }
            Component::Normal(part) => children.push(part.to_owned()),
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => {
                return Err(ArtifactError::UnsafeRoot(path.display().to_string()));
            }
        }
    }
    if path.is_absolute() {
        if anchor.as_os_str().is_empty() {
            return Err(ArtifactError::UnsafeRoot(path.display().to_string()));
        }
    } else {
        if !anchor.as_os_str().is_empty() {
            return Err(ArtifactError::UnsafeRoot(path.display().to_string()));
        }
        anchor.push(".");
    }
    let mut directory = cap_primitives::fs::open_ambient_dir(&anchor, ambient_authority())?;
    for child in children {
        directory = cap_primitives::fs::open_dir_nofollow(&directory, Path::new(&child))?;
    }
    Ok(directory)
}

#[cfg(windows)]
fn cap_hash_at(
    directory: &File,
    name: &Path,
    display_path: &Path,
) -> Result<String, ArtifactError> {
    use cap_primitives::fs::{FollowSymlinks, OpenOptions};

    let mut options = OpenOptions::new();
    options.read(true)._cap_fs_ext_follow(FollowSymlinks::No);
    let mut file = match cap_primitives::fs::open(directory, name, &options) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MISSING_HASH.into());
        }
        Err(error) => {
            return Err(ArtifactError::UnsafeRoot(format!(
                "{} ({error})",
                display_path.display()
            )));
        }
    };
    if !file.metadata()?.is_file() {
        return Err(ArtifactError::UnsafeRoot(
            display_path.display().to_string(),
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(sha256(bytes))
}

#[cfg(windows)]
fn restrict_file_to_owner(file: &File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, GetSecurityInfo, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS,
        SetEntriesInAclW, SetSecurityInfo, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, NO_INHERITANCE, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

    let handle = file.as_raw_handle();
    let mut owner: PSID = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: the file handle is live; Windows allocates `descriptor` and the
    // owner SID points into it until LocalFree below.
    let owner_result = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if owner_result != ERROR_SUCCESS || owner.is_null() {
        if !descriptor.is_null() {
            // SAFETY: allocated by GetSecurityInfo.
            unsafe { LocalFree(descriptor) };
        }
        return Err(std::io::Error::from_raw_os_error(owner_result as i32));
    }

    let trustee = TRUSTEE_W {
        pMultipleTrustee: std::ptr::null_mut(),
        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_USER,
        ptstrName: owner.cast(),
    };
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: SET_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: trustee,
    };
    let mut acl: *mut ACL = std::ptr::null_mut();
    // SAFETY: `access` and its owner SID remain live through the call.
    let acl_result = unsafe { SetEntriesInAclW(1, &access, std::ptr::null(), &mut acl) };
    let set_result = if acl_result == ERROR_SUCCESS {
        // SAFETY: the handle is live and `acl` was allocated by Windows.
        unsafe {
            SetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null(),
            )
        }
    } else {
        acl_result
    };
    if !acl.is_null() {
        // SAFETY: allocated by SetEntriesInAclW.
        unsafe { LocalFree(acl.cast()) };
    }
    // SAFETY: allocated by GetSecurityInfo.
    unsafe { LocalFree(descriptor) };
    if set_result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(set_result as i32))
    }
}

/// Descriptor-relative publication prevents an attacker from swapping any
/// checked parent directory for a symlink between validation and creation.
#[cfg(unix)]
fn atomic_write_at(
    path: &Path,
    content: &[u8],
    expected_hash: Option<&str>,
) -> Result<String, ArtifactError> {
    let parent = path
        .parent()
        .ok_or_else(|| ArtifactError::UnsafeRoot(path.display().to_string()))?;
    let directory = open_directory_chain(parent)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ArtifactError::UnsafeRoot(path.display().to_string()))?;
    atomic_write_unix_in(&directory, name, path, content, expected_hash)
}

#[cfg(unix)]
fn atomic_write_unix_in(
    directory: &File,
    name: &str,
    display_path: &Path,
    content: &[u8],
    expected_hash: Option<&str>,
) -> Result<String, ArtifactError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};

    let name = CString::new(name)
        .map_err(|_| ArtifactError::UnsafeRoot(display_path.display().to_string()))?;
    let current = hash_at(directory.as_raw_fd(), &name, display_path)?;
    if let Some(expected) = expected_hash {
        if current != expected {
            return Err(ArtifactError::ContentChanged {
                expected: expected.into(),
                current,
            });
        }
    }

    let temporary_name = CString::new(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        uuid::Uuid::new_v4()
    ))
    .expect("generated temporary name contains no NUL");
    // SAFETY: both C strings are owned for the duration of the calls and the
    // descriptor is an O_NOFOLLOW-opened directory owned by this function.
    let raw = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            temporary_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: openat returned a new owned descriptor.
    let mut temporary = unsafe { File::from_raw_fd(raw) };
    let publish = (|| -> Result<(), ArtifactError> {
        temporary.write_all(content)?;
        temporary.sync_all()?;
        // Recheck immediately before publication to preserve the expected-hash
        // contract while all path resolution remains descriptor-relative.
        let latest = hash_at(directory.as_raw_fd(), &name, display_path)?;
        if let Some(expected) = expected_hash {
            if latest != expected {
                return Err(ArtifactError::ContentChanged {
                    expected: expected.into(),
                    current: latest,
                });
            }
        }
        let published = if expected_hash == Some(MISSING_HASH) {
            // linkat provides an atomic create-without-replacement publication
            // for a destination that must still be absent.
            unsafe {
                libc::linkat(
                    directory.as_raw_fd(),
                    temporary_name.as_ptr(),
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    0,
                )
            }
        } else {
            // SAFETY: renameat operates only within the held directory
            // descriptor; replacing a link replaces the entry, never its target.
            unsafe {
                libc::renameat(
                    directory.as_raw_fd(),
                    temporary_name.as_ptr(),
                    directory.as_raw_fd(),
                    name.as_ptr(),
                )
            }
        };
        if published != 0 {
            let error = std::io::Error::last_os_error();
            if expected_hash == Some(MISSING_HASH)
                && error.kind() == std::io::ErrorKind::AlreadyExists
            {
                return Err(ArtifactError::ContentChanged {
                    expected: MISSING_HASH.into(),
                    current: hash_at(directory.as_raw_fd(), &name, display_path)?,
                });
            }
            return Err(error.into());
        }
        if expected_hash == Some(MISSING_HASH) {
            // SAFETY: after linkat succeeds both names refer to the same synced
            // file; removing the private temporary name completes publication.
            if unsafe { libc::unlinkat(directory.as_raw_fd(), temporary_name.as_ptr(), 0) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        directory.sync_all()?;
        Ok(())
    })();
    if publish.is_err() {
        // SAFETY: unlinkat targets the generated entry in the held directory.
        unsafe {
            libc::unlinkat(directory.as_raw_fd(), temporary_name.as_ptr(), 0);
        }
    }
    publish?;
    Ok(sha256(content))
}

#[cfg(unix)]
fn open_directory_chain(path: &Path) -> Result<File, ArtifactError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let start = if path.is_absolute() { "/" } else { "." };
    let start = CString::new(start).expect("static path has no NUL");
    // SAFETY: static C string; returned descriptor is checked below.
    let raw = unsafe {
        libc::open(
            start.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: open returned a new owned descriptor.
    let mut directory = unsafe { File::from_raw_fd(raw) };
    for component in path.components() {
        let Component::Normal(part) = component else {
            if matches!(component, Component::RootDir) {
                continue;
            }
            return Err(ArtifactError::UnsafeRoot(path.display().to_string()));
        };
        let part = CString::new(part.as_bytes())
            .map_err(|_| ArtifactError::UnsafeRoot(path.display().to_string()))?;
        // SAFETY: part is owned and directory is a valid held descriptor.
        let child = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                part.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if child < 0 {
            let error = std::io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(code) if code == libc::ELOOP || code == libc::ENOTDIR
            ) {
                return Err(ArtifactError::UnsafeRoot(format!(
                    "{} ({error})",
                    path.display()
                )));
            }
            return Err(error.into());
        }
        // SAFETY: openat returned a new owned descriptor.
        directory = unsafe { File::from_raw_fd(child) };
    }
    Ok(directory)
}

#[cfg(unix)]
fn hash_at(
    directory: std::os::fd::RawFd,
    name: &std::ffi::CStr,
    display_path: &Path,
) -> Result<String, ArtifactError> {
    use std::os::fd::FromRawFd;
    // SAFETY: name is owned by the caller and directory is live for this call.
    let raw = unsafe {
        libc::openat(
            directory,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if raw < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(MISSING_HASH.into());
        }
        return Err(ArtifactError::UnsafeRoot(format!(
            "{} ({error})",
            display_path.display()
        )));
    }
    // SAFETY: openat returned a new owned descriptor.
    let mut file = unsafe { File::from_raw_fd(raw) };
    if !file.metadata()?.is_file() {
        return Err(ArtifactError::UnsafeRoot(
            display_path.display().to_string(),
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(sha256(bytes))
}

pub fn render_spec(
    spec_id: &SpecId,
    revision: i64,
    title: &str,
    source: Option<&str>,
    body: &str,
) -> Result<String, ArtifactError> {
    if body.trim_start().starts_with("---") {
        return Err(ArtifactError::InvalidSpec(
            "submitted body must not contain frontmatter".into(),
        ));
    }
    validate_spec_body(body)?;
    if title.contains(['\n', '\r']) || title.trim().is_empty() {
        return Err(ArtifactError::InvalidSpec(
            "title must be one non-empty line".into(),
        ));
    }
    let mut output = format!(
        "---\nschema: {SCHEMA_VERSION}\nid: {spec_id}\nrevision: {revision}\ntitle: {}\n",
        serde_json::to_string(title.trim())?
    );
    if let Some(source) = source.filter(|source| !source.trim().is_empty()) {
        if source.contains(['\n', '\r']) {
            return Err(ArtifactError::InvalidSpec("source must be one line".into()));
        }
        output.push_str(&format!(
            "source: {}\n",
            serde_json::to_string(source.trim())?
        ));
    }
    output.push_str("---\n\n");
    output.push_str(body.trim());
    output.push('\n');
    Ok(output)
}

pub fn parse_spec(content: &str) -> Result<(ParsedSpecHeader, &str), ArtifactError> {
    let normalized = content
        .strip_prefix("---\n")
        .ok_or_else(|| ArtifactError::InvalidSpec("spec.md must start with frontmatter".into()))?;
    let (header, body) = normalized.split_once("\n---\n").ok_or_else(|| {
        ArtifactError::InvalidSpec("spec.md frontmatter is not terminated".into())
    })?;
    let mut schema = None;
    let mut id = None;
    let mut revision = None;
    let mut title = None;
    let mut source = None;
    let mut seen = HashSet::new();
    for line in header.lines() {
        let (key, value) = line.split_once(':').ok_or_else(|| {
            ArtifactError::InvalidSpec(format!("malformed frontmatter line: {line}"))
        })?;
        let key = key.trim();
        let value = value.trim();
        if !seen.insert(key) {
            return Err(ArtifactError::InvalidSpec(format!(
                "duplicate field: {key}"
            )));
        }
        match key {
            "schema" => schema = value.parse().ok(),
            "id" => id = value.parse().ok(),
            "revision" => revision = value.parse().ok(),
            "title" if !value.is_empty() => title = Some(parse_frontmatter_scalar(value)?),
            "source" if !value.is_empty() => source = Some(parse_frontmatter_scalar(value)?),
            _ => {
                return Err(ArtifactError::InvalidSpec(format!(
                    "unknown or invalid field: {key}"
                )));
            }
        }
    }
    let parsed = ParsedSpecHeader {
        schema: schema.ok_or_else(|| ArtifactError::InvalidSpec("missing schema".into()))?,
        id: id.ok_or_else(|| ArtifactError::InvalidSpec("missing canonical id".into()))?,
        revision: revision
            .filter(|value| *value > 0)
            .ok_or_else(|| ArtifactError::InvalidSpec("revision must be positive".into()))?,
        title: title.ok_or_else(|| ArtifactError::InvalidSpec("missing title".into()))?,
        source,
    };
    if parsed.schema != SCHEMA_VERSION {
        return Err(ArtifactError::InvalidSpec("unsupported schema".into()));
    }
    validate_spec_body(body)?;
    Ok((parsed, body))
}

fn validate_spec_body(body: &str) -> Result<(), ArtifactError> {
    let requirement_scope =
        if let Some((owned, historical)) = body.split_once("\n## Imported historical source\n") {
            if historical
                .lines()
                .any(|line| !line.trim().is_empty() && !line.starts_with('>'))
            {
                return Err(ArtifactError::InvalidSpec(
                    "imported historical source must remain a Markdown blockquote".into(),
                ));
            }
            owned
        } else {
            body
        };
    validate_requirement_ids(requirement_scope)
}

fn validate_requirement_ids(body: &str) -> Result<(), ArtifactError> {
    let requirement_ids = collect_ids(body, "RQ-")?;
    let acceptance_ids = collect_ids(body, "AC-")?;
    if requirement_ids.is_empty() || acceptance_ids.is_empty() {
        return Err(ArtifactError::InvalidSpec(
            "specification requires stable RQ-* and AC-* identifiers".into(),
        ));
    }
    for (label, ids) in [
        ("requirement", requirement_ids),
        ("acceptance criterion", acceptance_ids),
    ] {
        let mut unique = HashSet::new();
        for id in ids {
            if !unique.insert(id.clone()) {
                return Err(ArtifactError::InvalidSpec(format!(
                    "duplicate {label} id: {id}"
                )));
            }
        }
    }
    Ok(())
}

fn parse_frontmatter_scalar(value: &str) -> Result<String, ArtifactError> {
    if value.starts_with('"') {
        serde_json::from_str(value).map_err(|error| {
            ArtifactError::InvalidSpec(format!("invalid quoted frontmatter scalar: {error}"))
        })
    } else {
        Ok(value.to_owned())
    }
}

fn collect_ids(body: &str, prefix: &str) -> Result<Vec<String>, ArtifactError> {
    let mut ids = Vec::new();
    for (offset, _) in body.match_indices(prefix) {
        if body[..offset]
            .chars()
            .next_back()
            .is_some_and(|value| value.is_alphanumeric() || value == '_')
        {
            continue;
        }
        let suffix = &body[offset + prefix.len()..];
        let digits: String = suffix.chars().take_while(char::is_ascii_digit).collect();
        let next = suffix[digits.len()..].chars().next();
        if digits.len() < 3
            || next.is_some_and(|value| value.is_alphanumeric() || matches!(value, '_' | '-'))
        {
            let sample: String = body[offset..].chars().take(24).collect();
            return Err(ArtifactError::InvalidSpec(format!(
                "malformed stable identifier near {sample:?}"
            )));
        }
        ids.push(format!("{prefix}{digits}"));
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    fn body() -> &'static str {
        "# Refresh tokens\n\n## Requirements\n\n- RQ-001 Refresh without interruption.\n\n## Acceptance criteria\n\n- AC-001 Existing sessions remain active."
    }

    #[test]
    fn mandatory_root_is_static_and_spec_is_canonical() {
        let dir = tempfile::tempdir().unwrap();
        let id = SpecId::new();
        let artifact_set_id = Ulid::new();
        let root = initialize(dir.path(), &id, "Refresh tokens", artifact_set_id).unwrap();
        let rendered = render_spec(&id, 1, "Refresh tokens", None, body()).unwrap();
        let hash = atomic_write(
            &root.spec_dir.join("spec.md"),
            rendered.as_bytes(),
            Some(MISSING_HASH),
        )
        .unwrap();
        assert_eq!(hash, content_hash(&root.spec_dir.join("spec.md")).unwrap());
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.root.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest.as_object().unwrap().len(), 3);
        assert_eq!(parse_spec(&rendered).unwrap().0.id, id);
    }

    #[test]
    fn discovery_scans_multiple_canonical_specs_without_mutating_them() {
        let dir = tempfile::tempdir().unwrap();
        let artifact_set_id = Ulid::new();
        let mut expected = Vec::new();
        for title in ["Refresh tokens", "Rotate sessions"] {
            let id = SpecId::new();
            let root = initialize(dir.path(), &id, title, artifact_set_id).unwrap();
            let rendered = render_spec(&id, 1, title, Some("fixture:source"), body()).unwrap();
            atomic_write(
                &root.spec_dir.join("spec.md"),
                rendered.as_bytes(),
                Some(MISSING_HASH),
            )
            .unwrap();
            expected.push(id.to_string());
        }
        expected.sort();
        let discovered = discover_specs(dir.path()).unwrap().unwrap();
        assert_eq!(discovered.manifest.artifact_set_id, artifact_set_id);
        assert_eq!(discovered.specs.len(), 2);
        let mut actual: Vec<_> = discovered
            .specs
            .iter()
            .map(|spec| spec.header.id.to_string())
            .collect();
        actual.sort();
        assert_eq!(actual, expected);
        assert!(discovered.specs.iter().all(|spec| {
            spec.content_hash == content_hash(&dir.path().join(&spec.relative_path)).unwrap()
        }));
    }

    #[test]
    fn discovery_fails_closed_for_identity_mismatch_unknown_files_and_malformed_plan() {
        let dir = tempfile::tempdir().unwrap();
        let id = SpecId::new();
        let root = initialize(dir.path(), &id, "Original title", Ulid::new()).unwrap();
        let rendered = render_spec(&id, 1, "Original title", None, body()).unwrap();
        atomic_write(
            &root.spec_dir.join("spec.md"),
            rendered.as_bytes(),
            Some(MISSING_HASH),
        )
        .unwrap();
        std::fs::write(root.spec_dir.join("unexpected.txt"), "data").unwrap();
        assert!(matches!(
            discover_specs(dir.path()),
            Err(ArtifactError::InvalidSpec(_))
        ));
        std::fs::remove_file(root.spec_dir.join("unexpected.txt")).unwrap();
        std::fs::write(root.spec_dir.join("plan.json"), "{not-json").unwrap();
        assert!(matches!(
            discover_specs(dir.path()),
            Err(ArtifactError::Json(_))
        ));
        std::fs::remove_file(root.spec_dir.join("plan.json")).unwrap();
        let changed = rendered.replace("title: \"Original title\"", "title: \"Changed title\"");
        std::fs::write(root.spec_dir.join("spec.md"), changed).unwrap();
        assert!(matches!(
            discover_specs(dir.path()),
            Err(ArtifactError::InvalidSpec(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn discovery_never_follows_entries_beneath_specs() {
        let dir = tempfile::tempdir().unwrap();
        let id = SpecId::new();
        let root = initialize(dir.path(), &id, "Safe", Ulid::new()).unwrap();
        let rendered = render_spec(&id, 1, "Safe", None, body()).unwrap();
        atomic_write(
            &root.spec_dir.join("spec.md"),
            rendered.as_bytes(),
            Some(MISSING_HASH),
        )
        .unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        symlink(outside.path(), root.spec_dir.join("design.md")).unwrap();
        assert!(matches!(
            discover_specs(dir.path()),
            Err(ArtifactError::UnsafeRoot(_))
        ));
    }

    #[test]
    #[ignore = "release gate for the repository migration fixture"]
    fn release_gate_repository_migration_discovers_all_65_specs() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let discovered = discover_specs(&repository).unwrap().unwrap();
        assert_eq!(discovered.specs.len(), 65);
    }

    #[test]
    fn expected_hash_prevents_lost_update() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("value");
        std::fs::write(&path, "newer").unwrap();
        let error = atomic_write(&path, b"stale", Some("wrong")).unwrap_err();
        assert!(matches!(error, ArtifactError::ContentChanged { .. }));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "newer");
    }

    #[test]
    fn missing_hash_never_replaces_a_racing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("value");
        atomic_write(&path, b"winner", Some(MISSING_HASH)).unwrap();
        let error = atomic_write(&path, b"loser", Some(MISSING_HASH)).unwrap_err();
        assert!(matches!(error, ArtifactError::ContentChanged { .. }));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "winner");
    }

    #[test]
    fn replacing_an_existing_artifact_is_atomic_and_hash_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("value");
        std::fs::write(&path, "old").unwrap();
        let old_hash = content_hash(&path).unwrap();
        let new_hash = atomic_write(&path, b"new", Some(&old_hash)).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "new");
        assert_eq!(new_hash, sha256(b"new"));
    }

    #[cfg(windows)]
    #[test]
    fn openspec_windows_atomic_publication_is_handle_relative() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact.md");

        let initial_hash = atomic_write(&path, b"initial", Some(MISSING_HASH)).unwrap();
        assert_eq!(initial_hash, sha256(b"initial"));
        assert_eq!(std::fs::read(&path).unwrap(), b"initial");

        let replacement_hash = atomic_write(&path, b"replacement", Some(&initial_hash)).unwrap();
        assert_eq!(replacement_hash, sha256(b"replacement"));
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");

        let error = atomic_write(&path, b"must-not-win", Some(MISSING_HASH)).unwrap_err();
        assert!(matches!(error, ArtifactError::ContentChanged { .. }));
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_control_root_is_refused() {
        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), worktree.path().join(".agentum")).unwrap();
        assert!(matches!(
            initialize(worktree.path(), &SpecId::new(), "Unsafe", Ulid::new()),
            Err(ArtifactError::UnsafeRoot(_))
        ));
        assert!(outside.path().read_dir().unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn artifact_reads_never_follow_a_provider_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), "outside-secret").unwrap();
        let path = dir.path().join("spec-output.md");
        symlink(outside.path(), &path).unwrap();
        assert!(matches!(
            read_text(&path),
            Err(ArtifactError::UnsafeRoot(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn artifact_reads_never_follow_a_parent_directory_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("spec-output.md"), "outside-secret").unwrap();
        symlink(outside.path(), dir.path().join("staging")).unwrap();
        assert!(matches!(
            read_text(&dir.path().join("staging/spec-output.md")),
            Err(ArtifactError::UnsafeRoot(_))
        ));
    }

    #[test]
    fn repository_artifact_set_identity_is_stable_across_specs() {
        let dir = tempfile::tempdir().unwrap();
        let artifact_set_id = Ulid::new();
        let first = initialize(dir.path(), &SpecId::new(), "First", artifact_set_id).unwrap();
        assert_eq!(first.manifest.artifact_set_id, artifact_set_id);
        let second = initialize(dir.path(), &SpecId::new(), "Second", artifact_set_id).unwrap();
        assert_eq!(second.manifest.artifact_set_id, artifact_set_id);
        assert_eq!(
            discover_manifest(dir.path())
                .unwrap()
                .unwrap()
                .artifact_set_id,
            artifact_set_id
        );
    }

    #[test]
    fn unowned_or_conflicting_roots_are_never_claimed() {
        let unowned = tempfile::tempdir().unwrap();
        std::fs::create_dir(unowned.path().join(".agentum")).unwrap();
        assert!(matches!(
            initialize(unowned.path(), &SpecId::new(), "Unsafe", Ulid::new()),
            Err(ArtifactError::UnownedRoot(_))
        ));
        assert!(
            unowned
                .path()
                .join(".agentum/specs")
                .symlink_metadata()
                .is_err()
        );

        let conflicting = tempfile::tempdir().unwrap();
        let registered = Ulid::new();
        initialize(conflicting.path(), &SpecId::new(), "First", registered).unwrap();
        assert!(matches!(
            initialize(conflicting.path(), &SpecId::new(), "Second", Ulid::new()),
            Err(ArtifactError::Collision(_))
        ));
    }

    #[test]
    fn malformed_or_identity_changing_frontmatter_is_detectable() {
        let id = SpecId::new();
        let rendered = render_spec(&id, 1, "T", None, body()).unwrap();
        let (header, _) = parse_spec(&rendered).unwrap();
        assert_eq!(header.id, id);
        assert!(parse_spec(&rendered.replace("schema: 1", "schema: 99")).is_err());
        assert!(render_spec(&id, 1, "T", None, "- RQ-001 only").is_err());
    }

    #[test]
    fn frontmatter_scalars_are_deterministically_quoted_and_round_trip() {
        let id = SpecId::new();
        let title = "Refresh: tokens # safely {now} \"please\"";
        let source = "https://example.invalid/item?q={token}#part";
        let rendered = render_spec(&id, 1, title, Some(source), body()).unwrap();
        assert!(rendered.contains(&format!("title: {}", serde_json::to_string(title).unwrap())));
        assert!(rendered.contains(&format!(
            "source: {}",
            serde_json::to_string(source).unwrap()
        )));
        let parsed = parse_spec(&rendered).unwrap().0;
        assert_eq!(parsed.title, title);
        assert_eq!(parsed.source.as_deref(), Some(source));
    }

    #[test]
    fn stable_ids_require_three_digits_a_token_boundary_and_global_uniqueness() {
        let id = SpecId::new();
        let valid = "A table cell | RQ-001 | works\n\nParagraph criterion AC-001.";
        assert!(render_spec(&id, 1, "T", None, valid).is_ok());
        for malformed in [
            "RQ-1 junk and AC-001 ok",
            "RQ-001suffix and AC-001 ok",
            "RQ-001é and AC-001 ok",
            "RQ-001 ok and AC-001-more",
            "RQ-001 twice RQ-001 and AC-001 ok",
        ] {
            assert!(
                render_spec(&id, 1, "T", None, malformed).is_err(),
                "accepted malformed body: {malformed}"
            );
        }
    }

    #[test]
    fn migrated_blockquote_does_not_turn_legacy_labels_into_native_ids() {
        let id = SpecId::new();
        let body = "## Requirements\n\n- RQ-001 Preserve history.\n\n## Acceptance criteria\n\n- AC-001 Provenance is retained.\n\n## Imported historical source\n> Old references such as AC-4 and RQ-2 remain exact archival text.";
        assert!(render_spec(&id, 1, "T", None, body).is_ok());
        let unquoted = body.replace(
            "> Old references",
            "This is mutable native prose. Old references",
        );
        assert!(render_spec(&id, 1, "T", None, &unquoted).is_err());
    }

    #[test]
    fn existing_root_validation_rejects_a_changed_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let id = SpecId::new();
        let artifact_set_id = Ulid::new();
        let root = initialize(dir.path(), &id, "Refresh tokens", artifact_set_id).unwrap();
        assert!(
            validate_existing_root(
                dir.path(),
                &id,
                root.spec_dir.file_name().unwrap().to_str().unwrap(),
            )
            .is_ok()
        );
        let manifest_path = root.root.join("manifest.json");
        let current = content_hash(&manifest_path).unwrap();
        atomic_write(
            &manifest_path,
            br#"{"format":"other","schemaVersion":1,"artifactSetId":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}"#,
            Some(&current),
        )
        .unwrap();
        assert!(matches!(
            validate_existing_root(
                dir.path(),
                &id,
                root.spec_dir.file_name().unwrap().to_str().unwrap(),
            ),
            Err(ArtifactError::UnownedRoot(_))
        ));
    }
}
